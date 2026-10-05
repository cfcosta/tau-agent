//! Signing in against a local authorization server and an MCP server
//! behind a bearer check (`docs/reference/mcp.md`, "Signing in"): both
//! served by a small tokio HTTP responder, the MCP side bridged to the
//! in-process rmcp server of `common`. The browser is a plain GET of the
//! authorization URL, whose redirect goes to the loopback.
//!
//! - a 401 leaves the server waiting for a sign-in, with no browser;
//!   signing in discovers, registers, exchanges the code with PKCE and
//!   connects;
//! - a token about to expire is refreshed before the request, and one
//!   the server turns down is refreshed and the request sent again;
//! - signing out, or a sign-in in "another process", connects again on
//!   the next use;
//! - `insufficient_scope` asks for the granted scopes and the new one;
//! - a configured client skips registration, sends its secret, and uses
//!   `authServerMetadataUrl`;
//! - the callback ignores answers for another state;
//! - a signed-in connection gets a new session when the server forgets
//!   its own, whatever its token is doing, and runs each call once;
//! - the connection's watch sees it wait for a sign-in and connect;
//! - the host's page shows the sign-in, and its actions sign in and out.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

mod common;

use std::{
    collections::{BTreeSet, HashMap},
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_mcp::{
    config::{HttpConfig, OAuthConfig, Origin, ServerConfig, Transport},
    ui::NEEDS_AUTH,
};
use tau_mcp_host::{
    Host,
    auth::{self, Grant, TokenStore, pkce_challenge, valid_verifier},
    connection::{CallFailure, Connection, Environment, Progress, State},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::oneshot,
};
use tokio_util::sync::CancellationToken;

/// One HTTP request, as the responder reads it.
struct Request {
    method: String,
    target: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl Request {
    fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or("")
    }

    fn query(&self) -> HashMap<String, String> {
        let query = self.target.split_once('?').map_or("", |(_, q)| q);
        url::form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect()
    }

    fn form(&self) -> HashMap<String, String> {
        url::form_urlencoded::parse(&self.body)
            .into_owned()
            .collect()
    }

    fn bearer(&self) -> Option<&str> {
        self.headers.get("authorization")?.strip_prefix("Bearer ")
    }
}

struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Response {
    fn json(status: u16, value: Value) -> Self {
        Self {
            status,
            headers: vec![("content-type".into(), "application/json".into())],
            body: value.to_string().into_bytes(),
        }
    }

    fn empty(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    fn header(mut self, name: &str, value: String) -> Self {
        self.headers.push((name.into(), value));
        self
    }
}

async fn read_request(socket: &mut TcpStream) -> Option<Request> {
    let mut seen = Vec::new();
    let mut buffer = [0u8; 8192];
    let end = loop {
        if let Some(end) = seen.windows(4).position(|w| w == b"\r\n\r\n") {
            break end;
        }
        let read = socket.read(&mut buffer).await.ok()?;
        if read == 0 {
            return None;
        }
        seen.extend_from_slice(&buffer[..read]);
    };
    let head = String::from_utf8_lossy(&seen[..end]).into_owned();
    let mut lines = head.lines();
    let mut first = lines.next()?.split_whitespace();
    let (method, target) = (first.next()?.to_owned(), first.next()?.to_owned());
    let headers: HashMap<String, String> = lines
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        })
        .collect();
    let length: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = seen[end + 4..].to_vec();
    while body.len() < length {
        let read = socket.read(&mut buffer).await.ok()?;
        if read == 0 {
            return None;
        }
        body.extend_from_slice(&buffer[..read]);
    }
    Some(Request {
        method,
        target,
        headers,
        body,
    })
}

async fn write_response(socket: &mut TcpStream, response: Response) {
    let mut head = format!("HTTP/1.1 {} Status\r\n", response.status);
    for (name, value) in &response.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str(&format!(
        "content-length: {}\r\nconnection: close\r\n\r\n",
        response.body.len()
    ));
    let _ = socket.write_all(head.as_bytes()).await;
    let _ = socket.write_all(&response.body).await;
    let _ = socket.shutdown().await;
}

/// A code the authorization endpoint gave, waiting for its exchange.
struct Code {
    client_id: String,
    redirect_uri: String,
    challenge: String,
    scope: Vec<String>,
}

/// One MCP session's stream to the in-process server.
struct Bridge {
    write: tokio::io::WriteHalf<Box<dyn tau_mcp::config::ServerStream>>,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>>,
}

#[derive(Default)]
struct Books {
    registrations: Vec<Value>,
    authorizations: Vec<HashMap<String, String>>,
    codes: HashMap<String, Code>,
    /// Valid access tokens, with their scopes.
    access: HashMap<String, Vec<String>>,
    refresh: HashMap<String, Vec<String>>,
    /// (verifier, challenge) of each exchange.
    pkce: Vec<(String, String)>,
    /// The `Authorization` header of each token request.
    token_auth: Vec<Option<String>>,
    refreshes: usize,
    expires_in: u64,
    /// A scope every MCP request needs.
    needs_scope: Option<String>,
    mcp_requests: usize,
    /// The sessions the MCP server knows, when it keeps sessions as the
    /// reference servers do; none when it keeps none.
    sessions: Option<Vec<String>>,
    /// The sessions it began.
    initializes: usize,
    /// The `tools/call` requests that reached the server.
    tool_calls: usize,
    next: usize,
    /// Every code, token and verifier that went by: none may be logged.
    secrets: Vec<String>,
}

/// The authorization server and the MCP server, on one port.
struct Fake {
    base: String,
    mcp: Arc<common::State>,
    books: Mutex<Books>,
    bridge: tokio::sync::Mutex<Option<Bridge>>,
}

impl Fake {
    async fn start() -> Arc<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fake = Arc::new(Self {
            base: format!("http://{}", listener.local_addr().unwrap()),
            mcp: common::State::new(false),
            books: Mutex::new(Books {
                expires_in: 3600,
                ..Books::default()
            }),
            bridge: tokio::sync::Mutex::default(),
        });
        let serving = fake.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let fake = serving.clone();
                tokio::spawn(async move {
                    if let Some(request) = read_request(&mut socket).await {
                        let response = fake.answer(request).await;
                        write_response(&mut socket, response).await;
                    }
                });
            }
        });
        fake
    }

    fn url(&self) -> String {
        format!("{}/mcp", self.base)
    }

    fn metadata_url(&self) -> String {
        format!("{}/.well-known/oauth-authorization-server", self.base)
    }

    fn books<T>(&self, f: impl FnOnce(&mut Books) -> T) -> T {
        f(&mut self.books.lock().unwrap())
    }

    fn next(books: &mut Books, what: &str) -> String {
        books.next += 1;
        format!("{what}-{}", books.next)
    }

    fn challenge(&self) -> String {
        format!(
            "Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource/mcp\"",
            self.base
        )
    }

    async fn answer(&self, request: Request) -> Response {
        match (request.method.as_str(), request.path()) {
            ("GET", "/.well-known/oauth-protected-resource/mcp")
            | ("GET", "/.well-known/oauth-protected-resource") => {
                Response::json(
                    200,
                    json!({
                        "resource": self.url(),
                        "authorization_servers": [self.base],
                        "scopes_supported": ["read"],
                    }),
                )
            }
            ("GET", "/.well-known/oauth-authorization-server") => {
                Response::json(
                    200,
                    json!({
                        "issuer": self.base,
                        "authorization_endpoint": format!("{}/authorize", self.base),
                        "token_endpoint": format!("{}/token", self.base),
                        "registration_endpoint": format!("{}/register", self.base),
                        "response_types_supported": ["code"],
                        "code_challenge_methods_supported": ["S256"],
                        "scopes_supported": ["read", "write"],
                    }),
                )
            }
            ("POST", "/register") => self.register(&request),
            ("GET", "/authorize") => self.authorize(&request),
            ("POST", "/token") => self.token(&request),
            ("POST", "/mcp") => self.mcp(request).await,
            (_, "/mcp") => Response::empty(405),
            _ => Response::empty(404),
        }
    }

    fn register(&self, request: &Request) -> Response {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        self.books(|books| {
            let client_id = Self::next(books, "client");
            books.registrations.push(body.clone());
            Response::json(
                201,
                json!({
                    "client_id": client_id,
                    "redirect_uris": body["redirect_uris"],
                }),
            )
        })
    }

    fn authorize(&self, request: &Request) -> Response {
        let query = request.query();
        assert_eq!(query["response_type"], "code");
        assert_eq!(query["code_challenge_method"], "S256");
        self.books(|books| {
            let code = Self::next(books, "code");
            books.secrets.push(code.clone());
            books.codes.insert(
                code.clone(),
                Code {
                    client_id: query["client_id"].clone(),
                    redirect_uri: query["redirect_uri"].clone(),
                    challenge: query["code_challenge"].clone(),
                    scope: query
                        .get("scope")
                        .map(|s| s.split(' ').map(str::to_owned).collect())
                        .unwrap_or_default(),
                },
            );
            books.authorizations.push(query.clone());
            let mut location = url::Url::parse(&query["redirect_uri"]).unwrap();
            location
                .query_pairs_mut()
                .append_pair("code", &code)
                .append_pair("state", &query["state"])
                .append_pair("iss", &self.base);
            Response::empty(302).header("location", location.to_string())
        })
    }

    fn token(&self, request: &Request) -> Response {
        let form = request.form();
        self.books(|books| {
            books
                .token_auth
                .push(request.headers.get("authorization").cloned());
            let scopes = match form["grant_type"].as_str() {
                "authorization_code" => {
                    let Some(code) = books.codes.remove(&form["code"]) else {
                        return Response::json(
                            400,
                            json!({"error": "invalid_grant"}),
                        );
                    };
                    let verifier = form["code_verifier"].clone();
                    books.pkce.push((verifier.clone(), code.challenge.clone()));
                    let client = form.get("client_id").cloned().or_else(|| {
                        let basic = request.headers.get("authorization")?;
                        let decoded = STANDARD
                            .decode(basic.strip_prefix("Basic ")?)
                            .ok()?;
                        let text = String::from_utf8(decoded).ok()?;
                        Some(text.split(':').next()?.to_owned())
                    });
                    if pkce_challenge(&verifier) != code.challenge
                        || !valid_verifier(&verifier)
                        || form["redirect_uri"] != code.redirect_uri
                        || client.as_deref() != Some(code.client_id.as_str())
                    {
                        return Response::json(
                            400,
                            json!({"error": "invalid_grant"}),
                        );
                    }
                    code.scope
                }
                "refresh_token" => {
                    let Some(scopes) =
                        books.refresh.get(&form["refresh_token"])
                    else {
                        return Response::json(
                            400,
                            json!({"error": "invalid_grant"}),
                        );
                    };
                    books.refreshes += 1;
                    scopes.clone()
                }
                _ => {
                    return Response::json(
                        400,
                        json!({"error": "unsupported_grant_type"}),
                    );
                }
            };
            let access = Self::next(books, "access");
            books.access.insert(access.clone(), scopes.clone());
            let claims =
                URL_SAFE_NO_PAD.encode(r#"{"email":"ada@example.com"}"#);
            let id_token =
                format!("e30.{claims}.{}", Self::next(books, "idsig"));
            books.secrets.push(id_token.clone());
            books.secrets.push(access.clone());
            books.secrets.extend(form.get("code_verifier").cloned());
            let mut answer = json!({
                "access_token": access,
                "token_type": "Bearer",
                "expires_in": books.expires_in,
                "scope": scopes.join(" "),
                "id_token": id_token,
            });
            // A refresh keeps the refresh token it was given.
            if form["grant_type"] == "authorization_code" {
                let refresh = Self::next(books, "refresh");
                books.secrets.push(refresh.clone());
                books.refresh.insert(refresh.clone(), scopes);
                answer["refresh_token"] = json!(refresh);
            }
            Response::json(200, answer)
        })
    }

    async fn mcp(&self, request: Request) -> Response {
        let scopes = self.books(|books| {
            books.mcp_requests += 1;
            request
                .bearer()
                .and_then(|token| books.access.get(token).cloned())
        });
        let Some(scopes) = scopes else {
            return Response::empty(401)
                .header("www-authenticate", self.challenge());
        };
        if let Some(needed) = self.books(|b| b.needs_scope.clone())
            && !scopes.contains(&needed)
        {
            return Response::empty(403).header(
                "www-authenticate",
                format!(
                    "Bearer error=\"insufficient_scope\", scope=\"{needed}\", \
                     resource_metadata=\"{}/.well-known/oauth-protected-resource/mcp\"",
                    self.base
                ),
            );
        }
        let message: Value = serde_json::from_slice(&request.body).unwrap();
        let method = message["method"].as_str().unwrap_or_default().to_owned();
        let mut session = None;
        if let Some(refused) = self.books(|books| {
            Self::session(books, &request, &message, &mut session)
        }) {
            return refused;
        }
        if method == "tools/call" {
            self.books(|books| books.tool_calls += 1);
        }
        let mut bridge = self.bridge.lock().await;
        if bridge.is_none()
            || method == "server/discover"
            || method == "initialize"
        {
            *bridge = Some(self.open_bridge().await);
        }
        let bridge_ref = bridge.as_mut().unwrap();
        let waiting = match (message.get("id"), message.get("method")) {
            (Some(id), Some(_)) => {
                let (sender, answer) = oneshot::channel();
                bridge_ref
                    .pending
                    .lock()
                    .unwrap()
                    .insert(id.to_string(), sender);
                Some(answer)
            }
            _ => None,
        };
        let mut line = message.to_string();
        line.push('\n');
        bridge_ref.write.write_all(line.as_bytes()).await.unwrap();
        drop(bridge);
        let response = match waiting {
            None => Response::empty(202),
            Some(answer) => match answer.await {
                Ok(value) => Response::json(200, value),
                Err(_) => Response::empty(500),
            },
        };
        match session {
            Some(session) => response.header("mcp-session-id", session),
            None => response,
        }
    }

    /// The session check of a server that keeps sessions, after the
    /// bearer check, as the TypeScript SDK's servers do: `initialize`
    /// begins one (into `begun`), and a request in a session it does not
    /// know is answered 400 with an error that names no request. `None`
    /// lets the request through.
    fn session(
        books: &mut Books,
        request: &Request,
        message: &Value,
        begun: &mut Option<String>,
    ) -> Option<Response> {
        let sessions = books.sessions.as_mut()?;
        let given = request.headers.get("mcp-session-id");
        match message["method"].as_str() {
            // No 2026-07-28 discovery: the legacy handshake, with sessions.
            Some("server/discover") => Some(Response::json(
                200,
                json!({
                    "jsonrpc": "2.0",
                    "id": message["id"],
                    "error": {"code": -32601, "message": "Method not found"},
                }),
            )),
            Some("initialize") if given.is_none() => {
                books.initializes += 1;
                let session = format!("session-{}", books.initializes);
                sessions.push(session.clone());
                *begun = Some(session);
                None
            }
            _ if given.is_some_and(|given| sessions.contains(given)) => None,
            _ => Some(Response::json(
                400,
                json!({
                    "jsonrpc": "2.0",
                    "error": {
                        "code": -32000,
                        "message": "Bad Request: No valid session ID provided",
                    },
                }),
            )),
        }
    }

    /// Keeps sessions from now on.
    fn keep_sessions(&self) {
        self.books(|books| books.sessions = Some(Vec::new()));
    }

    /// Forgets every session, as a restart does.
    fn forget_sessions(&self) {
        self.books(|books| {
            if let Some(sessions) = &mut books.sessions {
                sessions.clear();
            }
        });
    }

    async fn open_bridge(&self) -> Bridge {
        let stream = self.mcp.dial().open().await.unwrap();
        let (read, write) = tokio::io::split(stream);
        let pending: Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>> =
            Arc::default();
        let answers = pending.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(read).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(value) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                // Answers only: the server's own requests and
                // notifications have no stream to go to here.
                if value.get("method").is_some() {
                    continue;
                }
                if let Some(id) = value.get("id")
                    && let Some(sender) =
                        answers.lock().unwrap().remove(&id.to_string())
                {
                    let _ = sender.send(value);
                }
            }
        });
        Bridge { write, pending }
    }

    /// Makes every access token given so far invalid at the MCP server.
    fn revoke_access(&self) {
        self.books(|books| books.access.clear());
    }
}

/// A GET over plain TCP: the status, the `location` header, the body.
async fn get(url: &str) -> (u16, Option<String>, String) {
    let parsed = url::Url::parse(url).unwrap();
    let host = parsed
        .host_str()
        .unwrap()
        .trim_matches(['[', ']'])
        .to_owned();
    let mut stream =
        TcpStream::connect((host.as_str(), parsed.port().unwrap()))
            .await
            .unwrap();
    let target = match parsed.query() {
        Some(query) => format!("{}?{query}", parsed.path()),
        None => parsed.path().to_owned(),
    };
    let request = format!("GET {target} HTTP/1.1\r\nHost: {host}\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).await.unwrap();
    let status = answer[9..12].parse().unwrap();
    let location = answer.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("location")
            .then(|| value.trim().to_owned())
    });
    let body = answer
        .split_once("\r\n\r\n")
        .map_or("", |(_, b)| b)
        .to_owned();
    (status, location, body)
}

/// The browser: opens the authorization URL and follows its redirect to
/// the loopback. The status the loopback answered.
async fn browse(url: &str) -> u16 {
    let (status, location, _) = get(url).await;
    assert_eq!(status, 302, "the authorization endpoint redirects");
    get(&location.unwrap()).await.0
}

fn environment(dir: &Path) -> Environment {
    Environment {
        env: Arc::new(|name| (name == "SECRET").then(|| "s3cret".to_owned())),
        home: None,
        repo: None,
        auth: Some(TokenStore::in_dir(dir)),
        launcher: Default::default(),
    }
}

fn server(fake: &Fake, oauth: Option<OAuthConfig>) -> ServerConfig {
    ServerConfig::new(
        "remote",
        Transport::Http(HttpConfig {
            url: fake.url(),
            headers: Vec::new(),
            oauth,
        }),
    )
}

async fn settle(connection: &Arc<Connection>) {
    tokio::time::timeout(
        Duration::from_secs(20),
        connection.settled(&CancellationToken::new()),
    )
    .await
    .expect("the connect settles");
}

async fn echo(connection: &Arc<Connection>) -> Result<Value, CallFailure> {
    let progress = |_: Progress| {};
    connection
        .call(
            "echo",
            json!({"text": "hi"}),
            &progress,
            &CancellationToken::new(),
        )
        .await
}

/// Signs `connection`'s server in as a browser would, and returns the
/// grant saved.
async fn sign_in(connection: &Arc<Connection>) -> Grant {
    let (store, request) = connection.sign_in_request().unwrap();
    let sign_in = auth::begin(&store, request).await.unwrap();
    let url = sign_in.url().to_owned();
    let finish = tokio::spawn(sign_in.finish());
    assert_eq!(browse(&url).await, 200);
    tokio::time::timeout(Duration::from_secs(10), finish)
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}

/// Every exchange's verifier is a PKCE verifier whose S256 challenge
/// is the one the authorization URL sent.
fn assert_pkce(fake: &Fake) {
    let pkce = fake.books(|books| books.pkce.clone());
    assert!(!pkce.is_empty());
    for (verifier, challenge) in pkce {
        assert!(valid_verifier(&verifier), "{verifier}");
        assert_eq!(pkce_challenge(&verifier), challenge);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_401_waits_for_a_sign_in_that_then_connects() {
    let fake = Fake::start().await;
    let dir = tempfile::tempdir().unwrap();
    let connection = Connection::new(
        server(&fake, None),
        Origin::User,
        environment(dir.path()),
    );
    connection.connect();
    settle(&connection).await;
    let status = connection.status();
    assert_eq!(status.state, State::NeedsAuth, "{status:?}");
    let need = connection.auth_need().unwrap();
    assert!(need.challenge.unwrap().contains("resource_metadata="));
    assert_eq!(need.scope, None);

    // Calls fail at once, and nothing goes to the authorization server.
    let requests = fake.books(|b| b.mcp_requests);
    let error = echo(&connection).await.unwrap_err();
    assert!(error.to_string().contains("sign in"), "{error}");
    assert_eq!(fake.books(|b| b.mcp_requests), requests);
    assert!(fake.books(|b| b.authorizations.is_empty()));

    let grant = sign_in(&connection).await;
    assert_eq!(grant.client_id, "client-1");
    assert_eq!(grant.client, None);
    assert_eq!(grant.scopes, vec!["read".to_owned()]);
    assert_eq!(grant.issuer.as_deref(), Some(fake.base.as_str()));
    assert!(grant.can_refresh());
    // Who signed in, from the ID token, for the page.
    assert_eq!(grant.account.as_deref(), Some("ada@example.com"));
    // Registered once, as a public client with tau's name, for the
    // loopback it listens on.
    let registration = fake.books(|b| b.registrations[0].clone());
    assert_eq!(registration["client_name"], "tau");
    assert_eq!(registration["token_endpoint_auth_method"], "none");
    assert_eq!(
        registration["redirect_uris"][0].as_str(),
        Some(grant.redirect_uri.as_str())
    );
    assert!(grant.redirect_uri.starts_with("http://127.0.0.1:"));
    // The resource indicator names the server (RFC 8707).
    let authorization = fake.books(|b| b.authorizations[0].clone());
    assert_eq!(authorization["resource"], fake.url());
    assert_pkce(&fake);

    // The grants file is the owner's alone, and Debug shows no token.
    let path = dir.path().join(auth::AUTH_FILE);
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("access-"));
    assert!(!format!("{grant:?}").contains("access-"));
    assert!(!format!("{grant:?}").contains("refresh-"));

    // The next use sees the new grant and connects.
    let result = echo(&connection).await.unwrap();
    assert_eq!(result["structuredContent"]["echo"], "hi");
    assert_eq!(connection.status().state, State::Connected);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_token_about_to_expire_is_refreshed_first() {
    let fake = Fake::start().await;
    fake.books(|b| b.expires_in = 5);
    let dir = tempfile::tempdir().unwrap();
    let connection = Connection::new(
        server(&fake, None),
        Origin::User,
        environment(dir.path()),
    );
    let first = sign_in(&connection).await;
    connection.connect();
    settle(&connection).await;
    assert_eq!(connection.status().state, State::Connected);
    assert!(fake.books(|b| b.refreshes) >= 1);
    echo(&connection).await.unwrap();
    // The refresh saved new tokens under the same sign-in.
    let store = TokenStore::in_dir(dir.path());
    let now = store.get(&first.key()).unwrap().unwrap();
    assert_ne!(now.tokens, first.tokens);
    assert_eq!(now.signed_in, first.signed_in);
    assert!(now.can_refresh(), "the refresh token is kept");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turned_down_token_is_refreshed_and_the_call_sent_again() {
    let fake = Fake::start().await;
    let dir = tempfile::tempdir().unwrap();
    let connection = Connection::new(
        server(&fake, None),
        Origin::User,
        environment(dir.path()),
    );
    sign_in(&connection).await;
    echo(&connection).await.unwrap();
    assert_eq!(fake.books(|b| b.refreshes), 0);
    fake.revoke_access();
    let result = echo(&connection).await.unwrap();
    assert_eq!(result["structuredContent"]["echo"], "hi");
    assert_eq!(fake.books(|b| b.refreshes), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn signing_out_waits_for_a_sign_in_again() {
    let fake = Fake::start().await;
    let dir = tempfile::tempdir().unwrap();
    let connection = Connection::new(
        server(&fake, None),
        Origin::User,
        environment(dir.path()),
    );
    let grant = sign_in(&connection).await;
    echo(&connection).await.unwrap();

    // Another process signs out: the next use connects again, and the
    // server asks for a sign-in.
    let store = TokenStore::in_dir(dir.path());
    assert!(store.sign_out(&grant.key()).unwrap());
    fake.revoke_access();
    let error = echo(&connection).await.unwrap_err();
    assert!(error.to_string().contains("sign in"), "{error}");
    assert_eq!(connection.status().state, State::NeedsAuth);
    let kept = store.get(&grant.key()).unwrap().unwrap();
    assert!(!kept.is_signed_in());
    assert_eq!(kept.client_id, grant.client_id, "the client is kept");

    // Signing in again reuses the client on its port.
    let again = sign_in(&connection).await;
    assert_eq!(again.client_id, grant.client_id);
    assert_eq!(fake.books(|b| b.registrations.len()), 1);
    assert_ne!(again.signed_in, grant.signed_in);
    echo(&connection).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn insufficient_scope_asks_for_more() {
    let fake = Fake::start().await;
    let dir = tempfile::tempdir().unwrap();
    let connection = Connection::new(
        server(&fake, None),
        Origin::User,
        environment(dir.path()),
    );
    sign_in(&connection).await;
    echo(&connection).await.unwrap();

    fake.books(|b| b.needs_scope = Some("write".into()));
    connection.restart();
    settle(&connection).await;
    assert_eq!(connection.status().state, State::NeedsAuth);
    assert_eq!(
        connection.auth_need().unwrap().scope.as_deref(),
        Some("write")
    );

    let grant = sign_in(&connection).await;
    let asked =
        fake.books(|b| b.authorizations.last().unwrap()["scope"].clone());
    let asked: BTreeSet<&str> = asked.split(' ').collect();
    assert_eq!(asked, BTreeSet::from(["read", "write"]));
    assert!(grant.scopes.contains(&"write".to_owned()));
    echo(&connection).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_configured_client_skips_registration() {
    let fake = Fake::start().await;
    let dir = tempfile::tempdir().unwrap();
    let port = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let oauth = OAuthConfig {
        client_id: Some("pre".into()),
        client_secret: Some("${SECRET}".into()),
        callback_url: Some(format!("http://localhost:{port}/back")),
        scope: Some("read write".into()),
        auth_server_metadata_url: Some(fake.metadata_url()),
        ..OAuthConfig::default()
    };
    let connection = Connection::new(
        server(&fake, Some(oauth)),
        Origin::User,
        environment(dir.path()),
    );
    let grant = sign_in(&connection).await;
    assert_eq!(grant.client.as_deref(), Some("pre"));
    assert_eq!(grant.client_id, "pre");
    assert_eq!(grant.redirect_uri, format!("http://localhost:{port}/back"));
    assert_eq!(
        grant.client_secret, None,
        "a configured secret is not saved"
    );
    assert!(fake.books(|b| b.registrations.is_empty()));
    let authorization = fake.books(|b| b.authorizations[0].clone());
    assert_eq!(authorization["scope"], "read write");
    // The secret went with the exchange, expanded.
    let sent = fake.books(|b| b.token_auth[0].clone()).unwrap();
    let expected = format!("Basic {}", STANDARD.encode("pre:s3cret"));
    assert_eq!(sent, expected);
    assert!(
        !std::fs::read_to_string(dir.path().join(auth::AUTH_FILE))
            .unwrap()
            .contains("s3cret")
    );
    echo(&connection).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_callback_waits_for_its_own_state() {
    let fake = Fake::start().await;
    let dir = tempfile::tempdir().unwrap();
    let connection = Connection::new(
        server(&fake, None),
        Origin::User,
        environment(dir.path()),
    );
    let (store, request) = connection.sign_in_request().unwrap();
    let sign_in = auth::begin(&store, request).await.unwrap();
    let redirect = sign_in.redirect_uri();
    let url = sign_in.url().to_owned();
    let finish = tokio::spawn(sign_in.finish());
    let (status, _, _) =
        get(&format!("{redirect}?code=forged&state=other")).await;
    assert_eq!(status, 400);
    let (status, _, _) =
        get(&redirect.replace("/callback", "/favicon.ico")).await;
    assert_eq!(status, 404);
    assert!(!finish.is_finished());
    assert_eq!(browse(&url).await, 200);
    let grant = finish.await.unwrap().unwrap();
    assert!(grant.is_signed_in());
}

/// The page shows a server waiting for a sign-in, the host's sign-in
/// connects it and the row says who and what, and signing out waits
/// again.
#[tokio::test(flavor = "multi_thread")]
async fn the_host_signs_in_and_out() {
    let fake = Fake::start().await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("mcp.json"),
        json!({ "mcpServers": { "remote": { "url": fake.url() } } })
            .to_string(),
    )
    .unwrap();
    let host = Arc::new(Host::new(
        tokio::runtime::Handle::current(),
        Some(dir.path().to_owned()),
    ));
    let settings = tau_mcp::config::Settings::default();
    let plugin = host.plugin(None, &settings);
    let connection = plugin.connections()[0].clone();
    settle(&connection).await;
    let row = host.servers(None, &settings).servers[0].clone();
    assert_eq!(row.state.as_deref(), Some(NEEDS_AUTH));
    assert!(!row.auth.as_ref().unwrap().signed_in);
    assert_eq!(
        host.servers(None, &settings).summary(),
        "1 server · 0 connected · 1 needs sign-in"
    );

    let (sender, done) = oneshot::channel();
    let signing = host.clone();
    let url = tokio::task::spawn_blocking(move || {
        signing.sign_in(
            None,
            &tau_mcp::config::Settings::default(),
            "remote",
            move |done| {
                let _ = sender.send(done.map(|grant| grant.client_id));
            },
        )
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(browse(&url).await, 200);
    let done = tokio::time::timeout(Duration::from_secs(10), done)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(done.unwrap(), "client-1");
    settle(&connection).await;
    assert_eq!(connection.status().state, State::Connected);
    let row = host.servers(None, &settings).servers[0].clone();
    let auth = row.auth.unwrap();
    assert!(auth.signed_in);
    assert_eq!(auth.scopes, vec!["read".to_owned()]);
    assert_eq!(auth.issuer.as_deref(), Some(fake.base.as_str()));

    let out = host.clone();
    let was = tokio::task::spawn_blocking(move || {
        out.sign_out(None, &tau_mcp::config::Settings::default(), "remote")
    })
    .await
    .unwrap()
    .unwrap();
    assert!(was);
    fake.revoke_access();
    settle(&connection).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    settle(&connection).await;
    assert_eq!(connection.status().state, State::NeedsAuth);
    assert!(
        !host.servers(None, &settings).servers[0]
            .auth
            .as_ref()
            .unwrap()
            .signed_in
    );
}

/// What a subscriber was given: each event's and span's target, level
/// and fields, as text.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<(String, tracing::Level, String)>>>);

struct Fields(String);

impl tracing::field::Visit for Fields {
    fn record_debug(
        &mut self,
        field: &tracing::field::Field,
        value: &dyn std::fmt::Debug,
    ) {
        self.0.push_str(&format!(" {}={value:?}", field.name()));
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Captured {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut fields = Fields(String::new());
        event.record(&mut fields);
        let meta = event.metadata();
        self.0.lock().unwrap().push((
            meta.target().to_owned(),
            *meta.level(),
            fields.0,
        ));
    }

    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        _: &tracing::span::Id,
        _: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut fields = Fields(String::new());
        attrs.record(&mut fields);
        let meta = attrs.metadata();
        self.0.lock().unwrap().push((
            meta.target().to_owned(),
            *meta.level(),
            fields.0,
        ));
    }
}

/// A sign-in, a refresh and calls, under a subscriber that takes every
/// level of every target: rmcp's sign-in debug lines reach it (so it
/// is listening), yet no code, token, ID token, verifier or secret is
/// in anything it was given. A layer behind `secrets_filter` gets none
/// of rmcp's sign-in lines below info.
#[tokio::test(flavor = "multi_thread")]
async fn no_subscriber_sees_a_code_or_a_token() {
    use tracing_subscriber::{Layer as _, layer::SubscriberExt as _};
    let (all, filtered) = (Captured::default(), Captured::default());
    let subscriber = tracing_subscriber::registry()
        .with(all.clone())
        .with(filtered.clone().with_filter(auth::secrets_filter()));
    // Global, so the tasks rmcp spawns are seen too. The other tests in
    // this binary may log into it as well; their lines are checked the
    // same way.
    tracing::subscriber::set_global_default(subscriber).unwrap();

    let fake = Fake::start().await;
    fake.books(|b| b.expires_in = 5);
    let dir = tempfile::tempdir().unwrap();
    let oauth = OAuthConfig {
        client_id: Some("pre".into()),
        client_secret: Some("${SECRET}".into()),
        ..OAuthConfig::default()
    };
    let connection = Connection::new(
        server(&fake, Some(oauth)),
        Origin::User,
        environment(dir.path()),
    );
    sign_in(&connection).await;
    echo(&connection).await.unwrap();
    fake.revoke_access();
    echo(&connection).await.unwrap();
    assert!(fake.books(|b| b.refreshes) >= 1);

    let mut secrets = fake.books(|b| b.secrets.clone());
    secrets.push("s3cret".into());
    let all = all.0.lock().unwrap().clone();
    assert!(
        all.iter().any(|(target, level, _)| target
            .starts_with("rmcp::transport::auth")
            && *level == tracing::Level::DEBUG),
        "the subscriber hears rmcp's sign-in debug lines"
    );
    for (target, level, fields) in &all {
        for secret in &secrets {
            assert!(
                !fields.contains(secret.as_str()),
                "{target} at {level} logged a secret: {fields}"
            );
        }
    }
    for (target, level, _) in filtered.0.lock().unwrap().iter() {
        let secret_target = auth::SECRET_TARGETS
            .iter()
            .any(|t| target == t || target.starts_with(&format!("{t}::")));
        assert!(
            !(secret_target && *level > tracing::Level::INFO),
            "{target} at {level} passed the filter"
        );
    }
}

/// A signed-in connection to `fake`, which keeps sessions, connected.
async fn signed_in_with_sessions(fake: &Fake, dir: &Path) -> Arc<Connection> {
    fake.keep_sessions();
    let mut config = server(fake, None);
    config.timeout = 5.0;
    let connection = Connection::new(config, Origin::User, environment(dir));
    sign_in(&connection).await;
    connection.connect();
    settle(&connection).await;
    assert_eq!(connection.status().state, State::Connected);
    connection
}

/// A call that answers at once: a forgotten session used to make it
/// wait out the server's timeout.
async fn quick_echo(connection: &Arc<Connection>) {
    let started = Instant::now();
    let result = echo(connection).await.unwrap();
    assert_eq!(result["structuredContent"]["echo"], "hi");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
}

/// A server behind a sign-in that restarted, forgetting the session,
/// and answers it with 400 and an error naming no request: the signed-in
/// connection begins a new session, with its token, and the call runs
/// once. Through a turned-down token too: the refresh comes first, then
/// the new session.
#[tokio::test(flavor = "multi_thread")]
async fn a_signed_in_connection_gets_a_new_session_after_a_restart() {
    let fake = Fake::start().await;
    let dir = tempfile::tempdir().unwrap();
    let connection = signed_in_with_sessions(&fake, dir.path()).await;
    quick_echo(&connection).await;
    assert_eq!(fake.books(|b| b.initializes), 1);

    fake.forget_sessions();
    quick_echo(&connection).await;
    assert_eq!(fake.books(|b| (b.initializes, b.tool_calls)), (2, 2));

    fake.forget_sessions();
    fake.revoke_access();
    quick_echo(&connection).await;
    assert_eq!(fake.books(|b| (b.initializes, b.tool_calls)), (3, 3));
    assert_eq!(fake.books(|b| b.refreshes), 1);
    assert_eq!(connection.status().state, State::Connected);
    connection.shutdown().await;
}

/// What happens to a signed-in connection between two calls.
#[derive(
    Debug, Clone, Copy, hegel::PrettyPrintable, hegel::DefaultGenerator,
)]
enum Between {
    /// Another call.
    Call,
    /// The server restarts and forgets its sessions.
    Restart,
    /// The server turns down every token it gave.
    Revoke,
}

/// Whatever mix of calls, restarts that forget the session and
/// turned-down tokens a signed-in connection meets, every call answers
/// at once and runs on the server exactly once: a session or a token
/// that went bad never makes a call fail, wait, or run twice.
#[hegel::test(test_cases = 30)]
fn a_signed_in_connection_outlives_restarts_and_revocations(tc: TestCase) {
    let steps: Vec<Between> =
        tc.draw(gs::vecs(gs::default::<Between>()).max_size(8));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async move {
        let fake = Fake::start().await;
        let dir = tempfile::tempdir().unwrap();
        let connection = signed_in_with_sessions(&fake, dir.path()).await;
        let mut calls = 0;
        for step in steps.into_iter().chain([Between::Call]) {
            match step {
                Between::Call => {
                    quick_echo(&connection).await;
                    calls += 1;
                }
                Between::Restart => fake.forget_sessions(),
                Between::Revoke => fake.revoke_access(),
            }
        }
        assert_eq!(fake.books(|b| b.tool_calls), calls);
        assert_eq!(connection.status().state, State::Connected);
        connection.shutdown().await;
    });
}

/// The connection's watch, which the host redraws its page on, sees a
/// server wait for a sign-in, and then connect once signed in.
#[tokio::test(flavor = "multi_thread")]
async fn the_watch_sees_a_sign_in_wait_and_connect() {
    let fake = Fake::start().await;
    let dir = tempfile::tempdir().unwrap();
    let connection = Connection::new(
        server(&fake, None),
        Origin::User,
        environment(dir.path()),
    );
    let mut changes = connection.watch();
    connection.connect();
    settle(&connection).await;
    assert_eq!(connection.status().state, State::NeedsAuth);
    assert!(changes.has_changed().unwrap());
    changes.borrow_and_update();
    assert!(connection.auth_need().is_some());

    sign_in(&connection).await;
    connection.restart();
    tokio::time::timeout(Duration::from_secs(20), async {
        while connection.status().state != State::Connected {
            changes.changed().await.unwrap();
        }
    })
    .await
    .expect("the watch sees the connection connect");
    assert_eq!(connection.auth_need(), None);
}
