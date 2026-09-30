//! `FakeChatGpt`: OpenAI's auth server and the model list of
//! `api.openai.com`, as turmoil hosts, for the Sign in with ChatGPT
//! tests (`tau_ai::chatgpt`).
//!
//! It keeps OpenAI's rules as the docs state them: a first sign-in with
//! `dynamic_agent_client` registers a client and returns its issued id;
//! codes are single-use and bound to their client, redirect URI, resource
//! and PKCE challenge; every refresh rotates the refresh token, and
//! using a rotated one fails with `refresh_token_reused`; ID tokens are
//! RS256 JWTs signed with a test key its JWKS publishes. What the client
//! got wrong (a missing parameter, a hint where none belongs) is kept in
//! [`FakeChatGpt::violations`].
//!
//! The browser is not simulated: [`FakeChatGpt::approve`] takes the
//! authorization URL and returns the redirect the browser would follow.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet, VecDeque},
    io,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::{
    rand::SystemRandom,
    rsa::{KeyPair, PublicKeyComponents},
    signature::RSA_PKCS1_SHA256,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tau_ai::{
    chatgpt::{
        AGENT_NAME,
        CALLBACK_PATH,
        Config,
        DYNAMIC_CLIENT_ID,
        ISSUER,
        PLAN_USAGE_SCOPE,
        RESOURCE,
        SCOPES,
    },
    http::Dialer,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::Url;

/// The hosts the fake answers as.
pub const AUTH_HOST: &str = "auth.openai.com";
pub const API_HOST: &str = "api.openai.com";
/// The port both listen on; plain HTTP.
pub const PORT: u16 = 80;
/// The key id of the signing key.
pub const KEY_ID: &str = "tau-test-1";
/// The simulated clock starts here (2026-09-29).
pub const START: u64 = 1_790_000_000;

const SIGNING_KEY: &[u8] = include_bytes!("../data/chatgpt-test-key-1.der");
/// A key the JWKS does not publish, for bad signatures.
const OTHER_KEY: &[u8] = include_bytes!("../data/chatgpt-test-key-2.der");

/// Dials the fake's hosts on the simulated network, whatever the URL's
/// scheme and port.
#[derive(Debug, Clone, Copy, Default)]
pub struct SimDialer;

impl Dialer for SimDialer {
    type Stream = turmoil::net::TcpStream;

    async fn dial(&self, url: &Url) -> io::Result<Self::Stream> {
        let host = url.host_str().unwrap_or_default().to_owned();
        turmoil::net::TcpStream::connect((host, PORT)).await
    }
}

/// What the user does on the consent page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Consent {
    /// Allows everything asked for.
    Grant,
    /// Signs in but does not allow plan usage.
    GrantWithoutPlanUsage,
    /// Declines: `error=access_denied`.
    Deny,
    /// A new registration whose callback lacks the issued client id.
    OmitClientId,
    /// A callback naming a client other than the one asked for.
    OtherClientId,
}

/// Something wrong with the next ID token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdTokenFault {
    WrongNonce,
    WrongAudience,
    WrongIssuer,
    Expired,
    /// Signed with a key the JWKS does not publish.
    WrongKey,
}

#[derive(Debug, Clone)]
struct CodeGrant {
    client_id: String,
    redirect_uri: String,
    challenge: String,
    nonce: String,
    resource: String,
    scopes: Vec<String>,
}

#[derive(Debug, Clone)]
struct RefreshGrant {
    client_id: String,
    scopes: Vec<String>,
    /// False once rotated or revoked.
    live: bool,
}

#[derive(Debug)]
struct State {
    subject: String,
    email: String,
    consents: VecDeque<Consent>,
    id_token_faults: VecDeque<IdTokenFault>,
    refresh_replies: VecDeque<(u16, Value)>,
    revoke_statuses: VecDeque<u16>,
    models_reply: Option<(u16, Value)>,
    models: Value,
    clients: Vec<String>,
    counter: u64,
    codes: HashMap<String, CodeGrant>,
    refresh_tokens: HashMap<String, RefreshGrant>,
    access_tokens: HashSet<String>,
    authorizations: Vec<Vec<(String, String)>>,
    refresh_grants: u32,
    revocations: Vec<Vec<(String, String)>>,
    violations: Vec<String>,
}

/// A fake auth server and API. Clones share state.
#[derive(Debug, Clone)]
pub struct FakeChatGpt {
    state: Rc<RefCell<State>>,
    clock: Arc<AtomicU64>,
}

impl Default for FakeChatGpt {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeChatGpt {
    pub fn new() -> Self {
        Self {
            state: Rc::new(RefCell::new(State {
                subject: "user-sub-1".into(),
                email: "user@example.com".into(),
                consents: VecDeque::new(),
                id_token_faults: VecDeque::new(),
                refresh_replies: VecDeque::new(),
                revoke_statuses: VecDeque::new(),
                models_reply: None,
                models: json!([]),
                clients: Vec::new(),
                counter: 0,
                codes: HashMap::new(),
                refresh_tokens: HashMap::new(),
                access_tokens: HashSet::new(),
                authorizations: Vec::new(),
                refresh_grants: 0,
                revocations: Vec::new(),
                violations: Vec::new(),
            })),
            clock: Arc::new(AtomicU64::new(START)),
        }
    }

    /// The client configuration for this fake: OpenAI's URLs (the
    /// [`SimDialer`] routes them here), its clock, and a short revocation
    /// backoff.
    pub fn config(&self) -> Config {
        let clock = self.clock.clone();
        Config {
            clock: Arc::new(move || clock.load(Ordering::SeqCst)),
            revoke_backoff: Duration::from_millis(100),
            ..Config::default()
        }
    }

    pub fn now(&self) -> u64 {
        self.clock.load(Ordering::SeqCst)
    }

    /// Moves the shared clock forward.
    pub fn advance(&self, seconds: u64) {
        self.clock.fetch_add(seconds, Ordering::SeqCst);
    }

    /// The user who signs in.
    pub fn set_user(&self, subject: &str, email: &str) {
        let mut state = self.state.borrow_mut();
        state.subject = subject.into();
        state.email = email.into();
    }

    /// What the user does on the next consent pages, in order; then
    /// [`Consent::Grant`].
    pub fn consent(&self, consent: Consent) {
        self.state.borrow_mut().consents.push_back(consent);
    }

    /// Faults for the next ID tokens, in order.
    pub fn fault_id_token(&self, fault: IdTokenFault) {
        self.state.borrow_mut().id_token_faults.push_back(fault);
    }

    /// Answers the next refresh with `status` and `body`, touching no
    /// token.
    pub fn fail_refresh(&self, status: u16, body: Value) {
        self.state
            .borrow_mut()
            .refresh_replies
            .push_back((status, body));
    }

    /// Statuses for the next revocations, in order; then 200.
    pub fn revoke_statuses(&self, statuses: &[u16]) {
        self.state.borrow_mut().revoke_statuses.extend(statuses);
    }

    /// The `models` array `GET /v1/models` returns.
    pub fn set_models(&self, models: Value) {
        self.state.borrow_mut().models = models;
    }

    /// Answers `GET /v1/models` with `status` and `body` instead.
    pub fn fail_models(&self, status: u16, body: Value) {
        self.state.borrow_mut().models_reply = Some((status, body));
    }

    /// Every authorization request's query, in order.
    pub fn authorizations(&self) -> Vec<Vec<(String, String)>> {
        self.state.borrow().authorizations.clone()
    }

    /// Client ids issued so far.
    pub fn clients(&self) -> Vec<String> {
        self.state.borrow().clients.clone()
    }

    /// Successful refresh grants so far.
    pub fn refresh_grants(&self) -> u32 {
        self.state.borrow().refresh_grants
    }

    /// Every revocation request's form, in order.
    pub fn revocations(&self) -> Vec<Vec<(String, String)>> {
        self.state.borrow().revocations.clone()
    }

    /// Whether `token` is a refresh token that still works.
    pub fn refresh_token_is_live(&self, token: &str) -> bool {
        self.state
            .borrow()
            .refresh_tokens
            .get(token)
            .is_some_and(|grant| grant.live)
    }

    /// Protocol rules the client broke.
    pub fn violations(&self) -> Vec<String> {
        self.state.borrow().violations.clone()
    }

    /// The browser: follows `authorize_url` through sign-in and consent,
    /// and returns the redirect back to the client.
    pub fn approve(&self, authorize_url: &str) -> String {
        let url = Url::parse(authorize_url).expect("a valid authorize URL");
        let pairs: Vec<(String, String)> =
            url.query_pairs().into_owned().collect();
        let mut state = self.state.borrow_mut();
        state.authorizations.push(pairs.clone());
        let get = |name: &str| {
            pairs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };
        let mut violations = Vec::new();
        if url.path() != "/api/accounts/authorize" {
            violations.push(format!("authorize path {}", url.path()));
        }
        for (name, expected) in [
            ("response_type", "code"),
            ("scope", SCOPES),
            ("resource", RESOURCE),
            ("code_challenge_method", "S256"),
        ] {
            if get(name).as_deref() != Some(expected) {
                violations.push(format!("{name} is {:?}", get(name)));
            }
        }
        for name in ["ext_agent_host_id", "state", "nonce", "code_challenge"] {
            if get(name).is_none_or(|value| value.is_empty()) {
                violations.push(format!("no {name}"));
            }
        }
        let redirect_uri = get("redirect_uri").unwrap_or_default();
        match Url::parse(&redirect_uri) {
            Ok(redirect)
                if redirect.scheme() == "http"
                    && redirect.host_str() == Some("127.0.0.1")
                    && redirect.path() == CALLBACK_PATH => {}
            _ => violations.push(format!("redirect_uri {redirect_uri}")),
        }
        let asked = get("client_id").unwrap_or_default();
        let new = asked == DYNAMIC_CLIENT_ID;
        match (new, get("agent_name_hint")) {
            (true, Some(hint)) if hint == AGENT_NAME => {}
            (true, other) => {
                violations.push(format!("agent_name_hint {other:?}"))
            }
            (false, Some(_)) => {
                violations.push("agent_name_hint on reauthorization".into());
            }
            (false, None) => {}
        }
        if new
            && (get("id_token_hint").is_some() || get("login_hint").is_some())
        {
            violations.push("hints on a new registration".into());
        }
        state.violations.extend(violations);
        let consent = state.consents.pop_front().unwrap_or(Consent::Grant);
        let state_param = get("state").unwrap_or_default();
        let mut redirect = Url::parse(&redirect_uri)
            .unwrap_or_else(|_| Url::parse("http://127.0.0.1/").unwrap());
        if consent == Consent::Deny {
            redirect
                .query_pairs_mut()
                .append_pair("error", "access_denied")
                .append_pair("state", &state_param);
            return redirect.into();
        }
        let client_id = if new {
            state.counter += 1;
            let issued = format!("oaiapp_test{}", state.counter);
            state.clients.push(issued.clone());
            issued
        } else if state.clients.contains(&asked) {
            asked
        } else {
            redirect
                .query_pairs_mut()
                .append_pair("error", "invalid_client")
                .append_pair("state", &state_param);
            return redirect.into();
        };
        let scopes: Vec<String> = SCOPES
            .split(' ')
            .filter(|scope| {
                consent != Consent::GrantWithoutPlanUsage
                    || *scope != PLAN_USAGE_SCOPE
            })
            .map(str::to_owned)
            .collect();
        state.counter += 1;
        let code = format!("code-{}", state.counter);
        state.codes.insert(
            code.clone(),
            CodeGrant {
                client_id: client_id.clone(),
                redirect_uri: redirect_uri.clone(),
                challenge: get("code_challenge").unwrap_or_default(),
                nonce: get("nonce").unwrap_or_default(),
                resource: get("resource").unwrap_or_default(),
                scopes: scopes.clone(),
            },
        );
        {
            let mut query = redirect.query_pairs_mut();
            query
                .append_pair("code", &code)
                .append_pair("scope", &scopes.join(" "))
                .append_pair("state", &state_param);
            match consent {
                Consent::OmitClientId => {}
                Consent::OtherClientId => {
                    query.append_pair("client_id", "oaiapp_someone_else");
                }
                _ => {
                    query.append_pair("client_id", &client_id);
                }
            }
        }
        redirect.into()
    }

    /// Registers the fake as `auth.openai.com` and `api.openai.com`.
    pub fn install(&self, sim: &mut turmoil::Sim<'_>) {
        for host in [AUTH_HOST, API_HOST] {
            let fake = self.clone();
            sim.host(host, move || {
                let fake = fake.clone();
                async move {
                    let listener =
                        turmoil::net::TcpListener::bind(("0.0.0.0", PORT))
                            .await?;
                    loop {
                        let (stream, _) = listener.accept().await?;
                        let fake = fake.clone();
                        tokio::task::spawn_local(async move {
                            let _ = fake.serve(host, stream).await;
                        });
                    }
                }
            });
        }
    }

    async fn serve(
        &self,
        host: &str,
        mut stream: turmoil::net::TcpStream,
    ) -> io::Result<()> {
        let Some(request) = read_request(&mut stream).await? else {
            return Ok(());
        };
        let (status, body) = self.answer(host, &request);
        let text = body.map(|body| body.to_string()).unwrap_or_default();
        let request_id = {
            let mut state = self.state.borrow_mut();
            state.counter += 1;
            format!("req_fake_{}", state.counter)
        };
        let response = format!(
            "HTTP/1.1 {status} Fake\r\nContent-Type: application/json\r\n\
             x-request-id: {request_id}\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{text}",
            text.len()
        );
        stream.write_all(response.as_bytes()).await?;
        stream.shutdown().await
    }

    fn answer(
        &self,
        host: &str,
        request: &HttpRequest,
    ) -> (u16, Option<Value>) {
        let route = (host, request.method.as_str(), request.path.as_str());
        match route {
            (AUTH_HOST, "GET", "/.well-known/openid-configuration") => (
                200,
                Some(json!({
                    "issuer": ISSUER,
                    "authorization_endpoint": format!("{ISSUER}/api/accounts/authorize"),
                    "token_endpoint": format!("{ISSUER}/api/accounts/oauth/token"),
                    "jwks_uri": format!("{ISSUER}/.well-known/jwks.json"),
                    "revocation_endpoint": format!("{ISSUER}/oauth/revoke"),
                })),
            ),
            (AUTH_HOST, "GET", "/.well-known/jwks.json") => {
                let key = KeyPair::from_der(SIGNING_KEY).expect("a test key");
                let public = PublicKeyComponents::<Vec<u8>>::from(key.public());
                (
                    200,
                    Some(json!({"keys": [{
                        "kty": "RSA",
                        "kid": KEY_ID,
                        "alg": "RS256",
                        "use": "sig",
                        "n": URL_SAFE_NO_PAD.encode(&public.n),
                        "e": URL_SAFE_NO_PAD.encode(&public.e),
                    }]})),
                )
            }
            (AUTH_HOST, "POST", "/api/accounts/oauth/token") => {
                self.token(&request.form())
            }
            (AUTH_HOST, "POST", "/oauth/revoke") => self.revoke(request.form()),
            (API_HOST, "GET", "/v1/models") => self.models(request),
            _ => (404, Some(json!({"error": "not_found"}))),
        }
    }

    fn token(&self, form: &HashMap<String, String>) -> (u16, Option<Value>) {
        let field = |name: &str| form.get(name).cloned().unwrap_or_default();
        let error = |status: u16, code: &str| {
            (
                status,
                Some(json!({"error": code, "error_description": code})),
            )
        };
        let mut state = self.state.borrow_mut();
        if form.contains_key("client_secret") {
            state.violations.push("a client secret".into());
        }
        if field("client_id") == DYNAMIC_CLIENT_ID {
            state
                .violations
                .push("dynamic_agent_client at the token endpoint".into());
        }
        match field("grant_type").as_str() {
            "authorization_code" => {
                let Some(grant) = state.codes.remove(&field("code")) else {
                    return error(400, "invalid_grant");
                };
                let verifier = field("code_verifier");
                let challenge =
                    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
                if grant.client_id != field("client_id")
                    || grant.redirect_uri != field("redirect_uri")
                    || grant.resource != field("resource")
                    || grant.challenge != challenge
                {
                    state.violations.push(
                        "a code exchange that does not match its authorization"
                            .into(),
                    );
                    return error(400, "invalid_grant");
                }
                let fault = state.id_token_faults.pop_front();
                let id_token = sign_id_token(
                    &state,
                    &grant.client_id,
                    &grant.nonce,
                    self.now(),
                    fault,
                );
                let reply = issue(
                    &mut state,
                    &grant.client_id,
                    &grant.scopes,
                    Some(id_token),
                );
                (200, Some(reply))
            }
            "refresh_token" => {
                if let Some((status, body)) = state.refresh_replies.pop_front()
                {
                    return (status, Some(body));
                }
                if field("resource") != RESOURCE {
                    state
                        .violations
                        .push("a refresh without the resource".into());
                }
                let token = field("refresh_token");
                let Some(grant) = state.refresh_tokens.get(&token).cloned()
                else {
                    return error(400, "invalid_grant");
                };
                if !grant.live {
                    return error(400, "refresh_token_reused");
                }
                if grant.client_id != field("client_id") {
                    return error(401, "invalid_client");
                }
                if let Some(old) = state.refresh_tokens.get_mut(&token) {
                    old.live = false;
                }
                state.refresh_grants += 1;
                let reply =
                    issue(&mut state, &grant.client_id, &grant.scopes, None);
                (200, Some(reply))
            }
            _ => error(400, "unsupported_grant_type"),
        }
    }

    fn revoke(&self, form: HashMap<String, String>) -> (u16, Option<Value>) {
        let mut state = self.state.borrow_mut();
        let mut recorded: Vec<(String, String)> =
            form.clone().into_iter().collect();
        recorded.sort();
        state.revocations.push(recorded);
        let status = state.revoke_statuses.pop_front().unwrap_or(200);
        if status == 200 {
            if let Some(grant) = form
                .get("token")
                .and_then(|token| state.refresh_tokens.get_mut(token))
            {
                grant.live = false;
            }
            return (200, None);
        }
        (status, Some(json!({"error": "server_error"})))
    }

    fn models(&self, request: &HttpRequest) -> (u16, Option<Value>) {
        let state = self.state.borrow();
        if let Some((status, body)) = &state.models_reply {
            return (*status, Some(body.clone()));
        }
        let token = request
            .headers
            .get("authorization")
            .and_then(|value| value.strip_prefix("Bearer "));
        if !token.is_some_and(|token| state.access_tokens.contains(token)) {
            return (401, Some(json!({"detail": "Unauthorized"})));
        }
        (200, Some(json!({"models": state.models})))
    }
}

/// A token response, with fresh access and refresh tokens.
fn issue(
    state: &mut State,
    client_id: &str,
    scopes: &[String],
    id_token: Option<String>,
) -> Value {
    state.counter += 1;
    let access = format!("at-{}", state.counter);
    let refresh = format!("rt-{}", state.counter);
    state.access_tokens.insert(access.clone());
    state.refresh_tokens.insert(
        refresh.clone(),
        RefreshGrant {
            client_id: client_id.to_owned(),
            scopes: scopes.to_vec(),
            live: true,
        },
    );
    let mut reply = json!({
        "access_token": access,
        "refresh_token": refresh,
        "token_type": "Bearer",
        "expires_in": 3600,
        "scope": scopes.join(" "),
    });
    if let Some(id_token) = id_token {
        reply["id_token"] = id_token.into();
    }
    reply
}

fn sign_id_token(
    state: &State,
    client_id: &str,
    nonce: &str,
    now: u64,
    fault: Option<IdTokenFault>,
) -> String {
    let mut claims = json!({
        "iss": ISSUER,
        "aud": client_id,
        "sub": state.subject,
        "email": state.email,
        "nonce": nonce,
        "iat": now,
        "exp": now + 3600,
    });
    let mut key = SIGNING_KEY;
    match fault {
        Some(IdTokenFault::WrongNonce) => claims["nonce"] = "another".into(),
        Some(IdTokenFault::WrongAudience) => {
            claims["aud"] = "oaiapp_someone_else".into();
        }
        Some(IdTokenFault::WrongIssuer) => {
            claims["iss"] = "https://auth.example".into();
        }
        Some(IdTokenFault::Expired) => claims["exp"] = (now - 3600).into(),
        Some(IdTokenFault::WrongKey) => key = OTHER_KEY,
        None => {}
    }
    sign(&claims, key)
}

/// An RS256 JWT of `claims`, signed with `key` under [`KEY_ID`].
pub fn sign(claims: &Value, key: &[u8]) -> String {
    let header = json!({"alg": "RS256", "kid": KEY_ID, "typ": "JWT"});
    let signed = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(header.to_string()),
        URL_SAFE_NO_PAD.encode(claims.to_string())
    );
    let key = KeyPair::from_der(key).expect("a test key");
    let mut signature = vec![0; key.public().modulus_len()];
    key.sign(
        &RSA_PKCS1_SHA256,
        &SystemRandom::new(),
        signed.as_bytes(),
        &mut signature,
    )
    .expect("signing works");
    format!("{signed}.{}", URL_SAFE_NO_PAD.encode(signature))
}

#[derive(Debug)]
struct HttpRequest {
    method: String,
    path: String,
    /// Names lowercased.
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl HttpRequest {
    fn form(&self) -> HashMap<String, String> {
        url::form_urlencoded::parse(&self.body)
            .into_owned()
            .collect()
    }
}

async fn read_request(
    stream: &mut turmoil::net::TcpStream,
) -> io::Result<Option<HttpRequest>> {
    let mut buffer = Vec::new();
    let mut piece = [0; 4096];
    loop {
        let mut headers = [httparse::EMPTY_HEADER; 64];
        let mut parsed = httparse::Request::new(&mut headers);
        if let Ok(httparse::Status::Complete(start)) = parsed.parse(&buffer) {
            let headers: HashMap<String, String> = parsed
                .headers
                .iter()
                .map(|header| {
                    (
                        header.name.to_ascii_lowercase(),
                        String::from_utf8_lossy(header.value).into_owned(),
                    )
                })
                .collect();
            let length: usize = headers
                .get("content-length")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            let method = parsed.method.unwrap_or_default().to_owned();
            let path = parsed.path.unwrap_or_default().to_owned();
            while buffer.len() < start + length {
                let read = stream.read(&mut piece).await?;
                if read == 0 {
                    return Ok(None);
                }
                buffer.extend_from_slice(&piece[..read]);
            }
            let path = path.split('?').next().unwrap_or_default().to_owned();
            return Ok(Some(HttpRequest {
                method,
                path,
                headers,
                body: buffer[start..start + length].to_vec(),
            }));
        }
        let read = stream.read(&mut piece).await?;
        if read == 0 {
            return Ok(None);
        }
        buffer.extend_from_slice(&piece[..read]);
    }
}
