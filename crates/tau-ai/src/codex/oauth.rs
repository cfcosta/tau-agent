//! Signing in with a ChatGPT account, ported from pi's
//! `packages/ai/src/auth/oauth/openai-codex.ts` (commit `2b0a123`).
//!
//! Two flows end in the same token exchange:
//!
//! - **Browser:** PKCE. The caller opens [`BrowserLogin::url`]; the
//!   browser comes back to a server on `localhost:1455`, or the user
//!   pastes the redirect URL.
//! - **Device code:** for machines without a browser. The user enters a
//!   code at [`DEVICE_VERIFICATION_URL`]; we poll until they have.

use std::time::Duration;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

use super::{CodexCredentials, CodexError, https, now_ms};

pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const AUTH_HOST: &str = "auth.openai.com";
pub const REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
pub const CALLBACK_PORT: u16 = 1455;
pub const DEVICE_VERIFICATION_URL: &str =
    "https://auth.openai.com/codex/device";
const DEVICE_REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";
const SCOPE: &str = "openid profile email offline_access";
/// How long a device code stays valid.
const DEVICE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// A PKCE verifier and its S256 challenge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
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

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).expect("the OS has a random source");
    bytes
}

/// A random hex id, for OAuth states and request ids.
pub fn random_hex() -> String {
    random_bytes::<16>()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A browser sign-in in progress.
#[derive(Debug, Clone)]
pub struct BrowserLogin {
    /// The page the user opens.
    pub url: String,
    pub state: String,
    pkce: Pkce,
}

impl BrowserLogin {
    /// Starts a sign-in. `originator` names the app to OpenAI.
    pub fn start(originator: &str) -> Self {
        let pkce = Pkce::new();
        let state = random_hex();
        let mut url =
            url::Url::parse("https://auth.openai.com/oauth/authorize")
                .expect("a static, valid URL");
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", CLIENT_ID)
            .append_pair("redirect_uri", REDIRECT_URI)
            .append_pair("scope", SCOPE)
            .append_pair("code_challenge", &pkce.challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &state)
            .append_pair("id_token_add_organizations", "true")
            .append_pair("codex_cli_simplified_flow", "true")
            .append_pair("originator", originator);
        Self {
            url: url.into(),
            state,
            pkce,
        }
    }

    /// Waits for the browser to come back to `localhost:1455`, then
    /// exchanges the code. Fails if the port is taken; use
    /// [`Self::finish_with`] with a pasted URL then.
    pub async fn wait(&self) -> Result<CodexCredentials, CodexError> {
        let listener = TcpListener::bind(("127.0.0.1", CALLBACK_PORT))
            .await
            .map_err(CodexError::Io)?;
        loop {
            let (mut socket, _) =
                listener.accept().await.map_err(CodexError::Io)?;
            let mut buffer = vec![0; 8192];
            let read =
                socket.read(&mut buffer).await.map_err(CodexError::Io)?;
            let request = String::from_utf8_lossy(&buffer[..read]);
            let target = request.split_whitespace().nth(1).unwrap_or("/");
            let outcome = callback(target, &self.state);
            let (status, page) = match &outcome {
                Ok(_) => (
                    "200 OK",
                    "Signed in to OpenAI. You can close this window.",
                ),
                Err(Callback::Ignore) => ("404 Not Found", "Not found."),
                Err(Callback::Invalid(why)) => {
                    ("400 Bad Request", why.as_str())
                }
            };
            let body = format!(
                "<!doctype html><meta charset=utf-8><title>tau</title><p>{page}</p>"
            );
            let reply = format!(
                "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(reply.as_bytes()).await;
            match outcome {
                Ok(code) => return self.exchange(&code, REDIRECT_URI).await,
                Err(Callback::Ignore) => continue,
                Err(Callback::Invalid(why)) => {
                    return Err(CodexError::Login(why));
                }
            }
        }
    }

    /// Finishes with what the user pasted: the redirect URL, `code#state`,
    /// a query string, or the bare code.
    pub async fn finish_with(
        &self,
        pasted: &str,
    ) -> Result<CodexCredentials, CodexError> {
        let (code, state) = parse_pasted(pasted);
        if state.as_deref().is_some_and(|state| state != self.state) {
            return Err(CodexError::Login("state mismatch".into()));
        }
        let code = code
            .ok_or_else(|| CodexError::Login("no authorization code".into()))?;
        self.exchange(&code, REDIRECT_URI).await
    }

    async fn exchange(
        &self,
        code: &str,
        redirect: &str,
    ) -> Result<CodexCredentials, CodexError> {
        exchange_code(code, &self.pkce.verifier, redirect).await
    }
}

enum Callback {
    /// Not the callback path: a favicon request, say.
    Ignore,
    Invalid(String),
}

/// Reads the code out of a callback request target.
fn callback(target: &str, state: &str) -> Result<String, Callback> {
    let url = url::Url::parse(&format!("http://localhost{target}"))
        .map_err(|_| Callback::Ignore)?;
    if url.path() != "/auth/callback" {
        return Err(Callback::Ignore);
    }
    let param = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    };
    if param("state").as_deref() != Some(state) {
        return Err(Callback::Invalid("State mismatch.".into()));
    }
    param("code")
        .ok_or_else(|| Callback::Invalid("Missing authorization code.".into()))
}

fn parse_pasted(input: &str) -> (Option<String>, Option<String>) {
    let value = input.trim();
    if value.is_empty() {
        return (None, None);
    }
    let from_query = |query: &str| {
        let pairs: Vec<(String, String)> =
            url::form_urlencoded::parse(query.as_bytes())
                .into_owned()
                .collect();
        let get = |name: &str| {
            pairs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };
        (get("code"), get("state"))
    };
    if let Ok(url) = url::Url::parse(value) {
        return from_query(url.query().unwrap_or(""));
    }
    if let Some((code, state)) = value.split_once('#') {
        return (Some(code.to_owned()), Some(state.to_owned()));
    }
    if value.contains("code=") {
        return from_query(value);
    }
    (Some(value.to_owned()), None)
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
}

async fn token_request(
    form: &[(&str, &str)],
    what: &str,
) -> Result<CodexCredentials, CodexError> {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(form)
        .finish();
    let response = https::post(
        AUTH_HOST,
        "/oauth/token",
        "application/x-www-form-urlencoded",
        body.as_bytes(),
    )
    .await
    .map_err(CodexError::Io)?;
    if !response.is_success() {
        return Err(CodexError::Login(format!(
            "token {what} failed ({}): {}",
            response.status,
            response.text()
        )));
    }
    let token: TokenResponse = serde_json::from_slice(&response.body)
        .map_err(|error| CodexError::Login(format!("token {what}: {error}")))?;
    let (Some(access), Some(refresh), Some(expires_in)) =
        (token.access_token, token.refresh_token, token.expires_in)
    else {
        return Err(CodexError::Login(format!(
            "token {what} response is missing fields"
        )));
    };
    CodexCredentials::from_tokens(access, refresh, now_ms() + expires_in * 1000)
}

async fn exchange_code(
    code: &str,
    verifier: &str,
    redirect: &str,
) -> Result<CodexCredentials, CodexError> {
    token_request(
        &[
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT_ID),
            ("code", code),
            ("code_verifier", verifier),
            ("redirect_uri", redirect),
        ],
        "exchange",
    )
    .await
}

/// Trades a refresh token for new credentials.
pub async fn refresh(
    refresh_token: &str,
) -> Result<CodexCredentials, CodexError> {
    token_request(
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", CLIENT_ID),
        ],
        "refresh",
    )
    .await
}

/// A device-code sign-in: show [`Self::user_code`] and
/// [`DEVICE_VERIFICATION_URL`], then [`Self::wait`].
#[derive(Debug, Clone)]
pub struct DeviceLogin {
    pub user_code: String,
    device_auth_id: String,
    interval: Duration,
}

#[derive(Deserialize)]
struct DeviceStart {
    device_auth_id: Option<String>,
    user_code: Option<String>,
    interval: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct DeviceDone {
    authorization_code: Option<String>,
    code_verifier: Option<String>,
}

impl DeviceLogin {
    pub async fn start() -> Result<Self, CodexError> {
        let body = json!({ "client_id": CLIENT_ID }).to_string();
        let response = https::post(
            AUTH_HOST,
            "/api/accounts/deviceauth/usercode",
            "application/json",
            body.as_bytes(),
        )
        .await
        .map_err(CodexError::Io)?;
        if !response.is_success() {
            return Err(CodexError::Login(format!(
                "device code request failed ({}): {}",
                response.status,
                response.text()
            )));
        }
        let start: DeviceStart = serde_json::from_slice(&response.body)
            .map_err(|error| CodexError::Login(error.to_string()))?;
        let interval = match start.interval {
            Some(serde_json::Value::Number(n)) => n.as_f64(),
            Some(serde_json::Value::String(s)) => s.trim().parse().ok(),
            _ => None,
        };
        match (start.device_auth_id, start.user_code, interval) {
            (Some(device_auth_id), Some(user_code), Some(interval))
                if interval >= 0.0 =>
            {
                Ok(Self {
                    user_code,
                    device_auth_id,
                    interval: Duration::from_secs_f64(interval.max(1.0)),
                })
            }
            _ => Err(CodexError::Login("invalid device code response".into())),
        }
    }

    /// Polls until the user approves, then exchanges the code.
    pub async fn wait(&self) -> Result<CodexCredentials, CodexError> {
        let started = tokio::time::Instant::now();
        let mut interval = self.interval;
        let body = json!({
            "device_auth_id": self.device_auth_id,
            "user_code": self.user_code,
        })
        .to_string();
        while started.elapsed() < DEVICE_TIMEOUT {
            tokio::time::sleep(interval).await;
            let response = https::post(
                AUTH_HOST,
                "/api/accounts/deviceauth/token",
                "application/json",
                body.as_bytes(),
            )
            .await
            .map_err(CodexError::Io)?;
            if response.is_success() {
                let done: DeviceDone = serde_json::from_slice(&response.body)
                    .map_err(|error| {
                    CodexError::Login(error.to_string())
                })?;
                let (Some(code), Some(verifier)) =
                    (done.authorization_code, done.code_verifier)
                else {
                    return Err(CodexError::Login(
                        "invalid device token response".into(),
                    ));
                };
                return exchange_code(&code, &verifier, DEVICE_REDIRECT_URI)
                    .await;
            }
            match (response.status, device_error(&response.body).as_deref()) {
                (403 | 404, _)
                | (_, Some("deviceauth_authorization_pending")) => {}
                (_, Some("slow_down")) => interval += Duration::from_secs(5),
                _ => {
                    return Err(CodexError::Login(format!(
                        "device sign-in failed ({}): {}",
                        response.status,
                        response.text()
                    )));
                }
            }
        }
        Err(CodexError::Login("the device code expired".into()))
    }
}

/// The error code of a device-auth error body, a string or `{code}`.
fn device_error(body: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    match value.get("error")? {
        serde_json::Value::String(code) => Some(code.clone()),
        other => other.get("code")?.as_str().map(str::to_owned),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_challenge_is_the_rfc_7636_example() {
        // RFC 7636, appendix B.
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
    fn the_authorize_url_carries_the_flow() {
        let login = BrowserLogin::start("tau");
        let url = url::Url::parse(&login.url).unwrap();
        let get = |name: &str| {
            url.query_pairs()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned())
        };
        assert_eq!(url.host_str(), Some("auth.openai.com"));
        assert_eq!(get("client_id").as_deref(), Some(CLIENT_ID));
        assert_eq!(get("redirect_uri").as_deref(), Some(REDIRECT_URI));
        assert_eq!(get("state"), Some(login.state.clone()));
        assert_eq!(
            get("code_challenge").as_deref(),
            Some(login.pkce.challenge.as_str())
        );
        assert_eq!(get("originator").as_deref(), Some("tau"));
    }

    #[test]
    fn the_callback_checks_path_and_state() {
        assert_eq!(
            callback("/auth/callback?code=abc&state=s1", "s1")
                .ok()
                .as_deref(),
            Some("abc")
        );
        assert!(matches!(
            callback("/favicon.ico", "s1"),
            Err(Callback::Ignore)
        ));
        assert!(matches!(
            callback("/auth/callback?code=abc&state=other", "s1"),
            Err(Callback::Invalid(_))
        ));
        assert!(matches!(
            callback("/auth/callback?state=s1", "s1"),
            Err(Callback::Invalid(_))
        ));
    }

    #[test]
    fn pasted_input_takes_every_shape() {
        let both = (Some("c".into()), Some("s".into()));
        assert_eq!(
            parse_pasted("http://localhost:1455/auth/callback?code=c&state=s"),
            both
        );
        assert_eq!(parse_pasted("c#s"), both);
        assert_eq!(parse_pasted("code=c&state=s"), both);
        assert_eq!(parse_pasted("  c  "), (Some("c".into()), None));
        assert_eq!(parse_pasted(""), (None, None));
    }

    #[test]
    fn device_errors_are_strings_or_objects() {
        assert_eq!(
            device_error(br#"{"error":"slow_down"}"#).as_deref(),
            Some("slow_down")
        );
        assert_eq!(
            device_error(
                br#"{"error":{"code":"deviceauth_authorization_pending"}}"#
            )
            .as_deref(),
            Some("deviceauth_authorization_pending")
        );
        assert_eq!(device_error(b"nope"), None);
    }
}
