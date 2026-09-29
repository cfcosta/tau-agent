//! OpenAI Codex: the Responses API behind a ChatGPT Plus or Pro
//! subscription, as pi's `openai-codex` provider uses it
//! (`packages/ai/src/api/openai-codex-responses.ts` and
//! `auth/oauth/openai-codex.ts`, commit `2b0a123`).
//!
//! It is the same WebSocket protocol as the API-key endpoint, so the
//! pool, lanes and continuation logic are shared. What differs:
//!
//! - the endpoint, `wss://chatgpt.com/backend-api/codex/responses`;
//! - OAuth credentials instead of an API key, refreshed before they
//!   expire, with the account id from the token sent as
//!   `chatgpt-account-id`;
//! - `originator` and `OpenAI-Beta: responses_websockets=…` headers;
//! - `instructions` is required, and `store` must stay `false` (it
//!   already does for every request).
//!
//! Sign in with [`BrowserLogin`] or [`DeviceLogin`], or load what the
//! Codex CLI saved with [`CodexCredentials::from_codex_cli`]. Then build
//! a client with [`crate::client::OpenAi::codex`].

mod connector;
pub mod https;
pub mod oauth;

use std::{
    fmt,
    io,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
pub use connector::CodexConnector;
pub use oauth::{BrowserLogin, DeviceLogin};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;

/// The Codex host.
pub const CODEX_HOST: &str = "chatgpt.com";
/// The Codex Responses WebSocket endpoint.
pub const CODEX_URL: &str = "wss://chatgpt.com/backend-api/codex/responses";
/// The beta the Codex endpoint needs for WebSocket mode.
pub const WEBSOCKETS_BETA: &str = "responses_websockets=2026-02-06";
/// What tau calls itself in `originator`.
pub const ORIGINATOR: &str = "tau";
/// Instructions Codex gets when a run has none; it rejects requests
/// without them.
pub const DEFAULT_INSTRUCTIONS: &str = "You are a helpful assistant.";

/// Models the Codex endpoint serves, as pi lists them, less
/// `gpt-5.3-codex-spark`: Codex refuses it to ChatGPT accounts. They are
/// priced like their API versions, as pi prices them; the subscription
/// bills differently.
pub const MODELS: [&str; 7] = [
    "gpt-5.5",
    "gpt-5.6-luna",
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-6-astra",
    "gpt-6-sol",
    "gpt-6-luna",
];

/// Refresh this long before the access token expires.
const REFRESH_MARGIN_MS: u64 = 5 * 60 * 1000;
/// Where the account id sits in the access token's claims.
const JWT_CLAIM_PATH: &str = "https://api.openai.com/auth";

pub(crate) fn user_agent() -> String {
    format!("tau/{}", env!("CARGO_PKG_VERSION"))
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

#[derive(Debug)]
pub enum CodexError {
    Io(io::Error),
    /// Signing in or refreshing failed; the message says why.
    Login(String),
    /// The access token has no ChatGPT account id.
    NoAccount,
    Parse(String),
}

impl fmt::Display for CodexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "codex: {error}"),
            Self::Login(why) => write!(f, "codex sign-in: {why}"),
            Self::NoAccount => {
                write!(f, "codex: the access token has no ChatGPT account id")
            }
            Self::Parse(why) => write!(f, "codex credentials: {why}"),
        }
    }
}

impl std::error::Error for CodexError {}

impl From<CodexError> for io::Error {
    fn from(error: CodexError) -> Self {
        match error {
            CodexError::Io(error) => error,
            other => io::Error::other(other.to_string()),
        }
    }
}

/// A ChatGPT sign-in: tokens, when the access token expires, and the
/// account they belong to. The serde shape is pi's.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexCredentials {
    pub access: String,
    pub refresh: String,
    /// Milliseconds since the Unix epoch.
    pub expires: u64,
    pub account_id: String,
}

impl fmt::Debug for CodexCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never print tokens.
        f.debug_struct("CodexCredentials")
            .field("expires", &self.expires)
            .field("account_id", &self.account_id)
            .finish_non_exhaustive()
    }
}

impl CodexCredentials {
    /// Credentials from fresh tokens, reading the account id from the
    /// access token.
    pub fn from_tokens(
        access: String,
        refresh: String,
        expires: u64,
    ) -> Result<Self, CodexError> {
        let account_id = account_id(&access).ok_or(CodexError::NoAccount)?;
        Ok(Self {
            access,
            refresh,
            expires,
            account_id,
        })
    }

    /// Whether the access token expires within the refresh margin.
    pub fn needs_refresh(&self, now_ms: u64) -> bool {
        now_ms + REFRESH_MARGIN_MS >= self.expires
    }

    /// Where tau keeps credentials: `$XDG_CONFIG_HOME/tau/codex.json`.
    pub fn default_path() -> Option<PathBuf> {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|home| PathBuf::from(home).join(".config"))
            })?;
        Some(config.join("tau").join("codex.json"))
    }

    pub fn load(path: &Path) -> Result<Self, CodexError> {
        let text = std::fs::read_to_string(path).map_err(CodexError::Io)?;
        serde_json::from_str(&text)
            .map_err(|error| CodexError::Parse(error.to_string()))
    }

    /// Saves to `path`, readable only by the user.
    pub fn save(&self, path: &Path) -> Result<(), CodexError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(CodexError::Io)?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|error| CodexError::Parse(error.to_string()))?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        use std::io::Write;
        options
            .open(path)
            .and_then(|mut file| file.write_all(text.as_bytes()))
            .map_err(CodexError::Io)
    }

    /// Reads what the Codex CLI saved (`~/.codex/auth.json`), taking the
    /// expiry from the access token.
    pub fn from_codex_cli(path: &Path) -> Result<Self, CodexError> {
        let text = std::fs::read_to_string(path).map_err(CodexError::Io)?;
        Self::parse_codex_cli(&text)
    }

    fn parse_codex_cli(text: &str) -> Result<Self, CodexError> {
        let value: Value = serde_json::from_str(text)
            .map_err(|error| CodexError::Parse(error.to_string()))?;
        let tokens = value.get("tokens").ok_or_else(|| {
            CodexError::Parse("no `tokens` (signed in with an API key?)".into())
        })?;
        let field = |name: &str| {
            tokens
                .get(name)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| CodexError::Parse(format!("no `tokens.{name}`")))
        };
        let access = field("access_token")?;
        let refresh = field("refresh_token")?;
        let expires = claims(&access)
            .and_then(|claims| claims.get("exp")?.as_u64())
            .map_or(0, |exp| exp * 1000);
        let account_id = field("account_id")
            .ok()
            .or_else(|| account_id(&access))
            .ok_or(CodexError::NoAccount)?;
        Ok(Self {
            access,
            refresh,
            expires,
            account_id,
        })
    }
}

/// The claims of a JWT, without checking its signature: the server
/// checks it; we only read the account id and expiry.
fn claims(token: &str) -> Option<Value> {
    let mut parts = token.split('.');
    let (_, payload, _) = (parts.next()?, parts.next()?, parts.next()?);
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// The ChatGPT account id in an access token.
pub fn account_id(access_token: &str) -> Option<String> {
    claims(access_token)?
        .get(JWT_CLAIM_PATH)?
        .get("chatgpt_account_id")?
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

type OnRefresh = Arc<dyn Fn(&CodexCredentials) + Send + Sync>;

/// Credentials shared by every connection of a client, refreshed when
/// they are about to expire. Clones share the same credentials.
#[derive(Clone)]
pub struct CodexAuth {
    credentials: Arc<Mutex<CodexCredentials>>,
    /// What the next upgrade request sends, as `(access, account id)`.
    current: Arc<RwLock<(String, String)>>,
    on_refresh: Option<OnRefresh>,
}

impl fmt::Debug for CodexAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CodexAuth").finish_non_exhaustive()
    }
}

impl CodexAuth {
    pub fn new(credentials: CodexCredentials) -> Self {
        let current =
            (credentials.access.clone(), credentials.account_id.clone());
        Self {
            credentials: Arc::new(Mutex::new(credentials)),
            current: Arc::new(RwLock::new(current)),
            on_refresh: None,
        }
    }

    /// Calls `save` with new credentials after each refresh, so they can
    /// be persisted.
    pub fn on_refresh(
        mut self,
        save: impl Fn(&CodexCredentials) + Send + Sync + 'static,
    ) -> Self {
        self.on_refresh = Some(Arc::new(save));
        self
    }

    /// Loads credentials from `path` and saves refreshed ones back to it.
    pub fn from_file(path: impl Into<PathBuf>) -> Result<Self, CodexError> {
        let path = path.into();
        let credentials = CodexCredentials::load(&path)?;
        Ok(Self::new(credentials).on_refresh(move |fresh| {
            let _ = fresh.save(&path);
        }))
    }

    /// Makes sure the access token is valid for a while, refreshing it
    /// if not. Concurrent callers wait for one refresh.
    pub async fn ensure_fresh(&self) -> Result<(), CodexError> {
        let mut credentials = self.credentials.lock().await;
        if !credentials.needs_refresh(now_ms()) {
            return Ok(());
        }
        let fresh = oauth::refresh(&credentials.refresh).await?;
        *self.current.write().expect("not poisoned") =
            (fresh.access.clone(), fresh.account_id.clone());
        if let Some(save) = &self.on_refresh {
            save(&fresh);
        }
        *credentials = fresh;
        Ok(())
    }

    pub(crate) fn current(&self) -> (String, String) {
        self.current.read().expect("not poisoned").clone()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// An unsigned JWT with `claims`.
    pub(crate) fn jwt(claims: Value) -> String {
        let encode = |value: &Value| URL_SAFE_NO_PAD.encode(value.to_string());
        format!(
            "{}.{}.sig",
            encode(&serde_json::json!({ "alg": "none" })),
            encode(&claims)
        )
    }

    pub(crate) fn token(account: &str, exp: u64) -> String {
        jwt(serde_json::json!({
            "exp": exp,
            JWT_CLAIM_PATH: { "chatgpt_account_id": account },
        }))
    }

    #[test]
    fn the_account_id_comes_from_the_token() {
        assert_eq!(account_id(&token("acct-1", 1)).as_deref(), Some("acct-1"));
        assert_eq!(account_id("not.a.jwt"), None);
        assert_eq!(account_id(&jwt(serde_json::json!({ "exp": 1 }))), None);
        assert!(matches!(
            CodexCredentials::from_tokens(
                jwt(serde_json::json!({})),
                "r".into(),
                0
            ),
            Err(CodexError::NoAccount)
        ));
    }

    #[test]
    fn refresh_starts_five_minutes_early() {
        let credentials = CodexCredentials::from_tokens(
            token("a", 0),
            "r".into(),
            10 * 60 * 1000,
        )
        .unwrap();
        assert!(!credentials.needs_refresh(0));
        assert!(!credentials.needs_refresh(4 * 60 * 1000));
        assert!(credentials.needs_refresh(5 * 60 * 1000));
    }

    #[test]
    fn codex_cli_credentials_are_read() {
        let text = serde_json::json!({
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": "x",
                "access_token": token("acct-9", 1_900_000_000),
                "refresh_token": "refresh-1",
                "account_id": "acct-9",
            },
        })
        .to_string();
        let credentials = CodexCredentials::parse_codex_cli(&text).unwrap();
        assert_eq!(credentials.account_id, "acct-9");
        assert_eq!(credentials.refresh, "refresh-1");
        assert_eq!(credentials.expires, 1_900_000_000_000);
        assert!(
            CodexCredentials::parse_codex_cli(r#"{"OPENAI_API_KEY":"sk"}"#)
                .is_err()
        );
    }

    #[test]
    fn credentials_round_trip_and_stay_private() {
        let dir = std::env::temp_dir()
            .join(format!("tau-codex-{}", oauth::random_hex()));
        let path = dir.join("codex.json");
        let credentials =
            CodexCredentials::from_tokens(token("acct", 5), "r".into(), 5000)
                .unwrap();
        credentials.save(&path).unwrap();
        assert_eq!(CodexCredentials::load(&path).unwrap(), credentials);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"accountId\""), "pi's field names");
        assert!(
            !format!("{credentials:?}").contains("r\""),
            "no tokens in Debug"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn fresh_credentials_are_not_refreshed() {
        let credentials = CodexCredentials::from_tokens(
            token("acct", 0),
            "r".into(),
            now_ms() + 3_600_000,
        )
        .unwrap();
        let auth = CodexAuth::new(credentials.clone());
        auth.ensure_fresh().await.unwrap();
        assert_eq!(auth.current(), (credentials.access, "acct".into()));
    }
}
