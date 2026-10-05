//! The browser half of signing in: the authorization URL, the loopback
//! listener it redirects to, and the checks on what comes back.

use std::{fmt, io};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use url::Url;

use super::{
    AGENT_NAME,
    AccountId,
    ChatGptError,
    DYNAMIC_CLIENT_ID,
    HostId,
    RESOURCE,
    SCOPES,
    random_bytes,
    random_token,
};

/// The callback path OpenAI redirects to. Only the port may vary.
pub const CALLBACK_PATH: &str = "/auth/callback";
/// The port tried first for the loopback listener.
pub const PREFERRED_PORT: u16 = 1455;

/// `http://127.0.0.1:<port>/auth/callback`: never `localhost`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RedirectUri {
    pub port: u16,
}

impl fmt::Display for RedirectUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "http://127.0.0.1:{}{CALLBACK_PATH}", self.port)
    }
}

/// A PKCE verifier and its S256 challenge.
#[derive(Clone, PartialEq, Eq)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl fmt::Debug for Pkce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pkce")
            .field("challenge", &self.challenge)
            .finish_non_exhaustive()
    }
}

impl Pkce {
    pub fn new() -> Self {
        Self::from_verifier(URL_SAFE_NO_PAD.encode(random_bytes::<32>()))
    }

    pub fn from_verifier(verifier: String) -> Self {
        let challenge =
            URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Self {
            verifier,
            challenge,
        }
    }
}

impl Default for Pkce {
    fn default() -> Self {
        Self::new()
    }
}

/// Whose sign-in this is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Registration {
    /// A first sign-in: registers a client with `dynamic_agent_client`.
    New,
    /// A saved registration signing in again with its issued client id.
    Returning {
        account: AccountId,
        client_id: String,
        subject: String,
    },
}

/// The parameters of one authorization request.
#[derive(Clone, PartialEq, Eq)]
pub struct AuthorizeParams {
    /// The issued client id; `None` registers a new client.
    pub client_id: Option<String>,
    pub host_id: HostId,
    pub redirect_uri: RedirectUri,
    pub state: String,
    pub nonce: String,
    pub code_challenge: String,
    pub id_token_hint: Option<String>,
    pub login_hint: Option<String>,
    /// Asks for consent again (`prompt=consent`), to enable plan usage
    /// after it was declined. Never on an ordinary sign-in.
    pub ask_consent: bool,
}

impl fmt::Debug for AuthorizeParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthorizeParams")
            .field("client_id", &self.client_id)
            .field("redirect_uri", &self.redirect_uri)
            .field("id_token_hint", &self.id_token_hint.as_ref().map(|_| "…"))
            .finish_non_exhaustive()
    }
}

impl AuthorizeParams {
    /// The URL to open in the system browser.
    pub fn url(&self, authorize_url: &Url) -> Url {
        let mut url = authorize_url.clone();
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("response_type", "code").append_pair(
                "client_id",
                self.client_id.as_deref().unwrap_or(DYNAMIC_CLIENT_ID),
            );
            if self.client_id.is_none() {
                query.append_pair("agent_name_hint", AGENT_NAME);
            }
            query
                .append_pair("ext_agent_host_id", self.host_id.as_str())
                .append_pair("redirect_uri", &self.redirect_uri.to_string())
                .append_pair("scope", SCOPES)
                .append_pair("resource", RESOURCE)
                .append_pair("state", &self.state)
                .append_pair("nonce", &self.nonce)
                .append_pair("code_challenge", &self.code_challenge)
                .append_pair("code_challenge_method", "S256");
            if let Some(hint) = &self.id_token_hint {
                query.append_pair("id_token_hint", hint);
            }
            if let Some(hint) = &self.login_hint {
                query.append_pair("login_hint", hint);
            }
            if self.ask_consent {
                query.append_pair("prompt", "consent");
            }
        }
        url
    }
}

/// One sign-in attempt, from the URL the user opens to the callback.
/// Keep it until the callback completes; start a new one to retry.
#[derive(Clone)]
pub struct SignIn {
    url: Url,
    pub(crate) state: String,
    pub(crate) nonce: String,
    pub(crate) pkce: Pkce,
    pub(crate) redirect_uri: RedirectUri,
    pub(crate) registration: Registration,
}

impl fmt::Debug for SignIn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The URL may carry `id_token_hint`: never print it.
        f.debug_struct("SignIn")
            .field("redirect_uri", &self.redirect_uri)
            .field("registration", &self.registration)
            .finish_non_exhaustive()
    }
}

/// What a valid callback gave: the code, and the client to exchange it
/// with.
#[derive(Clone, PartialEq, Eq)]
pub struct Callback {
    pub code: String,
    /// The issued client id: from the callback on a new registration,
    /// the saved one on a returning sign-in.
    pub client_id: String,
    /// The callback's `scope`, if any. The token response's scopes win.
    pub scope: Option<String>,
}

impl fmt::Debug for Callback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Callback")
            .field("client_id", &self.client_id)
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

impl SignIn {
    pub(crate) fn new(
        authorize_url: &Url,
        registration: Registration,
        host_id: HostId,
        redirect_uri: RedirectUri,
        hints: (Option<String>, Option<String>),
        ask_consent: bool,
    ) -> Self {
        let pkce = Pkce::new();
        let (state, nonce) = (random_token(), random_token());
        let client_id = match &registration {
            Registration::New => None,
            Registration::Returning { client_id, .. } => {
                Some(client_id.clone())
            }
        };
        let (id_token_hint, login_hint) = match registration {
            Registration::New => (None, None),
            Registration::Returning { .. } => hints,
        };
        let params = AuthorizeParams {
            client_id,
            host_id,
            redirect_uri,
            state: state.clone(),
            nonce: nonce.clone(),
            code_challenge: pkce.challenge.clone(),
            id_token_hint,
            login_hint,
            ask_consent,
        };
        Self {
            url: params.url(authorize_url),
            state,
            nonce,
            pkce,
            redirect_uri,
            registration,
        }
    }

    /// The page to open. It may carry `id_token_hint`: show it to the
    /// user, but never log it.
    pub fn url(&self) -> &str {
        self.url.as_str()
    }

    pub fn redirect_uri(&self) -> RedirectUri {
        self.redirect_uri
    }

    pub fn registration(&self) -> &Registration {
        &self.registration
    }

    /// Checks what came back: the redirect URL, a request target
    /// (`/auth/callback?…`) or a bare query string, as the user may paste
    /// it.
    pub fn callback(&self, returned: &str) -> Result<Callback, ChatGptError> {
        let pairs = query_pairs(returned);
        read_callback(&pairs, &self.state, &self.registration)
    }
}

/// The query pairs of a redirect URL, a request target or a query string.
pub fn query_pairs(returned: &str) -> Vec<(String, String)> {
    let returned = returned.trim();
    let query = if let Ok(url) = Url::parse(returned) {
        url.query().unwrap_or("").to_owned()
    } else if let Some((_, query)) = returned.split_once('?') {
        query.to_owned()
    } else {
        returned.to_owned()
    };
    let query = query.split('#').next().unwrap_or("");
    url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect()
}

/// The checks OpenAI asks for, in order: the state, an OAuth error, the
/// code, then the client id.
pub fn read_callback(
    pairs: &[(String, String)],
    state: &str,
    registration: &Registration,
) -> Result<Callback, ChatGptError> {
    let get = |name: &str| {
        pairs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    if get("state") != Some(state) {
        return Err(ChatGptError::StateMismatch);
    }
    if let Some(error) = get("error") {
        if error == "access_denied" {
            return Err(ChatGptError::ConsentDeclined);
        }
        return Err(ChatGptError::Authorization {
            error: error.to_owned(),
            description: get("error_description").map(str::to_owned),
        });
    }
    let code = get("code")
        .filter(|code| !code.is_empty())
        .ok_or(ChatGptError::NoCode)?;
    let returned = get("client_id").filter(|id| !id.is_empty());
    let client_id = match registration {
        Registration::New => returned
            .filter(|id| *id != DYNAMIC_CLIENT_ID)
            .ok_or(ChatGptError::RegistrationIncomplete)?,
        Registration::Returning { client_id, .. } => match returned {
            None => client_id.as_str(),
            Some(id) if id == client_id => id,
            Some(other) => {
                return Err(ChatGptError::ClientMismatch {
                    expected: client_id.clone(),
                    got: other.to_owned(),
                });
            }
        },
    };
    Ok(Callback {
        code: code.to_owned(),
        client_id: client_id.to_owned(),
        scope: get("scope").map(str::to_owned),
    })
}

/// The loopback listener the browser returns to. Bind it before opening
/// the browser.
#[derive(Debug)]
pub struct Loopback {
    listener: TcpListener,
    port: u16,
}

impl Loopback {
    /// Listens on `127.0.0.1:1455`, or on any free port if that is taken.
    pub async fn bind() -> io::Result<Self> {
        match Self::bind_port(PREFERRED_PORT).await {
            Ok(loopback) => Ok(loopback),
            Err(_) => Self::bind_port(0).await,
        }
    }

    /// Listens on `127.0.0.1:port` (0 for any).
    pub async fn bind_port(port: u16) -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", port)).await?;
        let port = listener.local_addr()?.port();
        Ok(Self { listener, port })
    }

    pub fn redirect_uri(&self) -> RedirectUri {
        RedirectUri { port: self.port }
    }

    /// Waits for the browser to come back for `sign_in`, answers it with
    /// a page, and returns the checked callback. Requests for other paths
    /// get a 404 and the wait goes on.
    pub async fn wait(
        &self,
        sign_in: &SignIn,
    ) -> Result<Callback, ChatGptError> {
        loop {
            let (mut socket, _) = self
                .listener
                .accept()
                .await
                .map_err(ChatGptError::Network)?;
            let Some(target) = read_target(&mut socket).await else {
                continue;
            };
            let path = target.split('?').next().unwrap_or("");
            if path != CALLBACK_PATH {
                let _ = reply(&mut socket, "404 Not Found", "Not found.").await;
                continue;
            }
            let outcome = sign_in.callback(&target);
            let (status, page) = match &outcome {
                Ok(_) => (
                    "200 OK",
                    "Signed in with ChatGPT. You can close this window."
                        .to_owned(),
                ),
                Err(error) => ("400 Bad Request", error.to_string()),
            };
            let _ = reply(&mut socket, status, &page).await;
            return outcome;
        }
    }
}

async fn read_target(socket: &mut tokio::net::TcpStream) -> Option<String> {
    let mut buffer = Vec::new();
    let mut piece = [0; 4096];
    while !buffer.windows(4).any(|w| w == b"\r\n\r\n") && buffer.len() < 16384 {
        let read = socket.read(&mut piece).await.ok()?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&piece[..read]);
    }
    let line = buffer.split(|&b| b == b'\n').next()?;
    let line = String::from_utf8_lossy(line);
    let mut parts = line.split_whitespace();
    (parts.next()? == "GET").then_some(())?;
    parts.next().map(str::to_owned)
}

async fn reply(
    socket: &mut tokio::net::TcpStream,
    status: &str,
    page: &str,
) -> io::Result<()> {
    let escaped = page
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>tau</title><p>{escaped}</p>"
    );
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    socket.write_all(response.as_bytes()).await?;
    socket.shutdown().await
}

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0027)"
)]
mod tests {
    use super::*;

    #[test]
    fn the_challenge_is_the_rfc_7636_example() {
        let pkce = Pkce::from_verifier(
            "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into(),
        );
        assert_eq!(
            pkce.challenge,
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        assert_eq!(Pkce::new().verifier.len(), 43);
    }

    #[test]
    fn the_redirect_is_on_127_0_0_1() {
        assert_eq!(
            RedirectUri { port: 1455 }.to_string(),
            "http://127.0.0.1:1455/auth/callback"
        );
    }

    #[test]
    fn pasted_shapes_give_the_same_pairs() {
        let expected = vec![
            ("code".to_owned(), "c 1".to_owned()),
            ("state".to_owned(), "s".to_owned()),
        ];
        for pasted in [
            "http://127.0.0.1:1455/auth/callback?code=c+1&state=s",
            "  /auth/callback?code=c%201&state=s ",
            "code=c+1&state=s",
            "?code=c+1&state=s",
        ] {
            assert_eq!(query_pairs(pasted), expected, "{pasted}");
        }
    }

    #[tokio::test]
    async fn the_loopback_answers_the_browser() {
        let loopback = Loopback::bind_port(0).await.unwrap();
        let sign_in = SignIn::new(
            &Url::parse("https://auth.example/authorize").unwrap(),
            Registration::New,
            HostId::new_uuid(),
            loopback.redirect_uri(),
            (None, None),
            false,
        );
        let port = loopback.redirect_uri().port;
        let state = sign_in.state.clone();
        let browser = tokio::spawn(async move {
            let get = |target: String| async move {
                let mut stream =
                    tokio::net::TcpStream::connect(("127.0.0.1", port))
                        .await
                        .unwrap();
                let request =
                    format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
                stream.write_all(request.as_bytes()).await.unwrap();
                let mut answer = String::new();
                stream.read_to_string(&mut answer).await.unwrap();
                answer
            };
            let favicon = get("/favicon.ico".into()).await;
            let callback = get(format!(
                "{CALLBACK_PATH}?code=c1&state={state}&client_id=oaiapp_9"
            ))
            .await;
            (favicon, callback)
        });
        let callback = loopback.wait(&sign_in).await.unwrap();
        let (favicon, page) = browser.await.unwrap();
        assert!(favicon.starts_with("HTTP/1.1 404"));
        assert!(page.starts_with("HTTP/1.1 200"));
        assert_eq!(callback.code, "c1");
        assert_eq!(callback.client_id, "oaiapp_9");
    }
}
