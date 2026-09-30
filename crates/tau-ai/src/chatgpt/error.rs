//! What can go wrong, and what to do about it.
//!
//! Every failure carries a [`Recovery`] (in [`crate::retry`]): the action
//! OpenAI's "Errors and recovery" page asks for. Plan-usage errors stop
//! inference; OpenAI never moves a request to another billing path, and
//! neither does tau.

use std::{fmt, io, path::PathBuf};

use serde_json::Value;

use crate::http::Response;
pub use crate::retry::Recovery;

/// The body of a failed API response, by shape. OpenAI asks to keep the
/// shape: direct-route admission answers `{"detail": …}` before a
/// Responses request starts; the Responses layer answers an error object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorBody {
    /// `{"error": {"code", "message", "param", "type"}}`.
    Structured {
        code: Option<String>,
        message: Option<String>,
        param: Option<String>,
        kind: Option<String>,
    },
    /// `{"detail": "…"}`: diagnostic text, not a stable code.
    Detail(String),
    /// Anything else, kept verbatim in [`ApiError::body`].
    Other,
}

impl ErrorBody {
    pub fn parse(body: &[u8]) -> Self {
        let Ok(value) = serde_json::from_slice::<Value>(body) else {
            return Self::Other;
        };
        let text = |value: &Value, name: &str| {
            value.get(name).and_then(Value::as_str).map(str::to_owned)
        };
        if let Some(error) = value.get("error").filter(|e| e.is_object()) {
            return Self::Structured {
                code: text(error, "code"),
                message: text(error, "message"),
                param: text(error, "param"),
                kind: text(error, "type"),
            };
        }
        match value.get("detail") {
            Some(Value::String(detail)) => Self::Detail(detail.clone()),
            Some(other) => Self::Detail(other.to_string()),
            None => Self::Other,
        }
    }

    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Structured { code, .. } => code.as_deref(),
            _ => None,
        }
    }
}

/// A failed request to `api.openai.com`, kept as it came: status, body,
/// request id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub status: u16,
    pub request_id: Option<String>,
    pub body: String,
    pub parsed: ErrorBody,
}

impl ApiError {
    pub fn from_response(response: &Response) -> Self {
        Self::new(
            response.status,
            response.request_id().map(str::to_owned),
            &response.body,
        )
    }

    pub fn new(status: u16, request_id: Option<String>, body: &[u8]) -> Self {
        Self {
            status,
            request_id,
            body: String::from_utf8_lossy(body).into_owned(),
            parsed: ErrorBody::parse(body),
        }
    }

    pub fn code(&self) -> Option<&str> {
        self.parsed.code()
    }

    /// Documented codes first, then the admission statuses, then the
    /// status alone.
    pub fn recovery(&self) -> Recovery {
        if let Some(recovery) = self.code().and_then(Recovery::of_code) {
            return recovery;
        }
        match (&self.parsed, self.status) {
            (ErrorBody::Detail(_), 401) => Recovery::SignInAgain,
            (ErrorBody::Detail(_), 403) => Recovery::Restricted,
            (ErrorBody::Detail(_), 503) => Recovery::RetryLater,
            (_, status) => Recovery::of_status(status),
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "HTTP {}", self.status)?;
        if let Some(code) = self.code() {
            write!(f, " {code}")?;
        }
        match &self.parsed {
            ErrorBody::Structured {
                message: Some(message),
                ..
            } => write!(f, ": {message}")?,
            ErrorBody::Detail(detail) => write!(f, ": {detail}")?,
            _ if !self.body.is_empty() => write!(f, ": {}", self.body)?,
            _ => {}
        }
        if let Some(id) = &self.request_id {
            write!(f, " (request {id})")?;
        }
        Ok(())
    }
}

/// An error from the OAuth token or revocation endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthError {
    pub status: u16,
    /// `error`, or `error.code` when `error` is an object.
    pub code: Option<String>,
    pub description: Option<String>,
}

impl OAuthError {
    pub fn from_response(response: &Response) -> Self {
        let value: Value =
            serde_json::from_slice(&response.body).unwrap_or(Value::Null);
        let (code, description) = match value.get("error") {
            Some(Value::String(code)) => (
                Some(code.clone()),
                value
                    .get("error_description")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            ),
            Some(error @ Value::Object(_)) => (
                error.get("code").and_then(Value::as_str).map(str::to_owned),
                error
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            ),
            _ => (None, (!response.body.is_empty()).then(|| response.text())),
        };
        Self {
            status: response.status,
            code,
            description,
        }
    }

    /// Whether the refresh token can never work again: sign in again.
    pub fn is_unusable_refresh_token(&self) -> bool {
        matches!(
            self.code.as_deref(),
            Some(
                "invalid_grant"
                    | "invalid_refresh_token"
                    | "token_expired"
                    | "refresh_token_expired"
                    | "refresh_token_invalidated"
                    | "refresh_token_reused"
            )
        )
    }
}

impl fmt::Display for OAuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "HTTP {}", self.status)?;
        if let Some(code) = &self.code {
            write!(f, " {code}")?;
        }
        if let Some(description) = &self.description {
            write!(f, ": {description}")?;
        }
        Ok(())
    }
}

/// Why an ID token was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdTokenError {
    #[error("the ID token is not a JWT")]
    Malformed,
    #[error("the ID token is signed with {0}, which tau does not accept")]
    Algorithm(String),
    #[error("no key in OpenAI's JWKS matches the ID token")]
    UnknownKey,
    #[error("the ID token's signature does not verify")]
    Signature,
    #[error("the ID token was issued by {0:?}, not OpenAI")]
    Issuer(Option<String>),
    #[error("the ID token is not for this client")]
    Audience,
    #[error("the ID token has expired")]
    Expired,
    #[error("the ID token's nonce is not this sign-in's")]
    Nonce,
    #[error("the ID token names no subject")]
    Subject,
}

/// Everything signing in, refreshing, signing out and listing models can
/// fail with.
#[derive(Debug, thiserror::Error)]
pub enum ChatGptError {
    /// A network failure: nothing is known to be wrong with the
    /// credentials. Not a `#[source]`: the message already carries it.
    #[error("ChatGPT: network: {0}")]
    Network(io::Error),
    #[error("ChatGPT credentials at {path}: {error}")]
    Storage { path: PathBuf, error: io::Error },
    #[error("ChatGPT credentials at {path} are unreadable: {message}")]
    Corrupt { path: PathBuf, message: String },
    #[error("no home directory to keep ChatGPT credentials in")]
    NoConfigDir,
    #[error("no saved ChatGPT account {0}")]
    UnknownAccount(String),
    #[error("no ChatGPT account is active")]
    NoActiveAccount,
    /// The callback's `state` is not this attempt's.
    #[error("the sign-in came back with another attempt's state")]
    StateMismatch,
    #[error("the sign-in was declined")]
    ConsentDeclined,
    #[error("the sign-in failed: {error}{}", .description.as_deref().map(|d| format!(": {d}")).unwrap_or_default())]
    Authorization {
        error: String,
        description: Option<String>,
    },
    #[error("the callback has no authorization code")]
    NoCode,
    /// A new registration came back without an issued client id.
    #[error("the registration is incomplete: no client id came back")]
    RegistrationIncomplete,
    /// A returning sign-in came back with another registration's client.
    #[error("the sign-in came back for another client ({got})")]
    ClientMismatch { expected: String, got: String },
    /// `invalid_grant` on the code exchange: start a fresh sign-in.
    #[error("the authorization code was refused; sign in again")]
    CodeRejected,
    #[error("the ID token was refused: {0}")]
    IdToken(#[from] IdTokenError),
    /// A returning sign-in validated as another account.
    #[error("the sign-in is for another ChatGPT account")]
    IdentityMismatch,
    /// Signed in, but plan usage was not granted.
    #[error("this sign-in does not allow using the ChatGPT plan")]
    PlanUsageDisabled,
    /// No usable tokens: signed out, or the refresh token died (and was
    /// cleared). Sign in again with the saved client id.
    #[error("sign in to ChatGPT again{}", .reason.as_deref().map(|r| format!(" ({r})")).unwrap_or_default())]
    SignInRequired { reason: Option<String> },
    /// `invalid_client` from the token endpoint.
    #[error("OpenAI refused the client: {0}")]
    InvalidClient(OAuthError),
    /// Any other token-endpoint error.
    #[error("OpenAI's token endpoint: {0}")]
    OAuth(OAuthError),
    /// An error from `api.openai.com`.
    #[error("OpenAI API: {0}")]
    Api(Box<ApiError>),
    /// A response that does not follow the protocol.
    #[error("OpenAI answered unexpectedly: {0}")]
    Protocol(String),
}

impl ChatGptError {
    pub fn recovery(&self) -> Recovery {
        match self {
            Self::Network(_) => Recovery::RetryLater,
            Self::Storage { .. }
            | Self::Corrupt { .. }
            | Self::NoConfigDir
            | Self::InvalidClient(_) => Recovery::FixClient,
            Self::UnknownAccount(_)
            | Self::NoActiveAccount
            | Self::StateMismatch
            | Self::ConsentDeclined
            | Self::Authorization { .. }
            | Self::NoCode
            | Self::RegistrationIncomplete
            | Self::ClientMismatch { .. }
            | Self::CodeRejected
            | Self::IdToken(_)
            | Self::IdentityMismatch
            | Self::SignInRequired { .. } => Recovery::SignInAgain,
            Self::PlanUsageDisabled => Recovery::EnablePlanUsage,
            Self::OAuth(error) if error.status >= 500 => Recovery::RetryLater,
            Self::OAuth(_) => Recovery::FixClient,
            Self::Api(error) => error.recovery(),
            Self::Protocol(_) => Recovery::RetryLater,
        }
    }
}

/// A network failure stays the `io::Error` it was; anything else is
/// wrapped whole, so a connection can recover the [`ChatGptError`] with
/// `get_ref` and `downcast_ref`, and with it the refusal.
impl From<ChatGptError> for io::Error {
    fn from(error: ChatGptError) -> Self {
        match error {
            ChatGptError::Network(error) => error,
            other => io::Error::other(other),
        }
    }
}
