//! Sign in with ChatGPT, for ChatGPT plan usage in an open-source app:
//! OpenAI's documented flow ("ChatGPT plan usage for open-source apps",
//! <https://developers.openai.com/siwc/token-sharing-open-source>, read
//! on 2026-09-29).
//!
//! - **Host:** a stable, opaque `ext_agent_host_id`, made once per host
//!   and kept in the [`Store`].
//! - **Sign-in:** [`ChatGpt::start_sign_in`] builds the authorization URL
//!   (`dynamic_agent_client` and `agent_name_hint=tau` the first time,
//!   the issued client id and hints after that) for a [`Loopback`]
//!   listener on `127.0.0.1`. [`Loopback::wait`] or a pasted redirect URL
//!   ([`SignIn::callback`]) gives the checked [`Callback`], and
//!   [`ChatGpt::finish_sign_in`] exchanges the code, validates the ID
//!   token against OpenAI's JWKS, reads the granted scopes and saves the
//!   record.
//! - **Tokens:** [`ChatGpt::access_token`] refreshes an access token near
//!   expiry. Refreshes of one account take turns across threads and
//!   processes (a lock file), since each one rotates the refresh token.
//! - **Sign-out:** [`ChatGpt::sign_out`] revokes the refresh token, then
//!   clears the tokens but keeps the registration.
//! - **Models:** [`ChatGpt::models`] lists the account's models.
//!
//! Inference goes to `wss://api.openai.com/v1/responses` with the access
//! token as a bearer token ([`crate::client::OpenAi::chatgpt`]), without
//! the [`UNSUPPORTED_FIELDS`]. Failures carry a [`Recovery`].

mod authorize;
mod connector;
mod error;
pub mod id_token;
mod store;

use std::{fmt, sync::Arc, time::Duration};

pub use authorize::{
    AuthorizeParams,
    CALLBACK_PATH,
    Callback,
    Loopback,
    PREFERRED_PORT,
    Pkce,
    RedirectUri,
    Registration,
    SignIn,
    query_pairs,
    read_callback,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
pub use connector::{ChatGptConnector, websocket_request};
pub use error::{
    ApiError,
    ChatGptError,
    ErrorBody,
    IdTokenError,
    OAuthError,
    Recovery,
};
use id_token::{Expected, Identity, Jwks, Jwt, check_claims};
use serde::Deserialize;
use serde_json::Value;
pub use store::{
    AccountId,
    AccountStatus,
    Credentials,
    HostId,
    PlanUsage,
    REFRESH_MARGIN,
    Store,
    rfc3339,
};
use tokio::sync::Mutex;
use url::Url;

use crate::http::{self, Dialer, Request, Tls};

/// OpenAI's issuer.
pub const ISSUER: &str = "https://auth.openai.com";
pub const AUTHORIZE_URL: &str =
    "https://auth.openai.com/api/accounts/authorize";
pub const TOKEN_URL: &str = "https://auth.openai.com/api/accounts/oauth/token";
pub const DISCOVERY_URL: &str =
    "https://auth.openai.com/.well-known/openid-configuration";
/// The `resource` of every authorization, exchange and refresh.
pub const RESOURCE: &str = "https://api.openai.com/v1";
/// The API base, with the slash URL joins need.
pub const API_BASE: &str = "https://api.openai.com/v1/";
/// The Responses WebSocket endpoint.
pub const WEBSOCKET_URL: &str = "wss://api.openai.com/v1/responses";
/// Identity scopes, then the plan-usage ones.
pub const SCOPES: &str = "openid profile email offline_access resource.invoke \
                          chatgpt.tokens.use.direct";
/// The scope that allows using the ChatGPT plan.
pub const PLAN_USAGE_SCOPE: &str = "chatgpt.tokens.use.direct";
/// The client id of a first registration; never saved.
pub const DYNAMIC_CLIENT_ID: &str = "dynamic_agent_client";
/// `agent_name_hint`: tau's name, the same on every installation.
pub const AGENT_NAME: &str = "tau";
/// Request fields the plan route does not take ("Preview limitations").
/// A plan client strips them from every request.
pub const UNSUPPORTED_FIELDS: [&str; 15] = [
    "background",
    "conversation",
    "max_output_tokens",
    "max_tool_calls",
    "metadata",
    "moderation",
    "multi_agent",
    "prompt",
    "prompt_cache_retention",
    "safety_identifier",
    "temperature",
    "top_logprobs",
    "top_p",
    "truncation",
    "user",
];
/// Where users review and limit app usage of their plan (ChatGPT
/// Settings → Usage). The docs name the page but not its URL; this is
/// ChatGPT's settings route for it.
pub const USAGE_SETTINGS_URL: &str = "https://chatgpt.com/#settings/Usage";

pub(crate) fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).expect("the OS has a random source");
    bytes
}

/// 32 random bytes, base64url: for `state` and `nonce`.
pub(crate) fn random_token() -> String {
    URL_SAFE_NO_PAD.encode(random_bytes::<32>())
}

/// Unix seconds, from the system clock.
pub fn system_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Endpoints, clock and retry settings. [`Config::default`] is OpenAI's.
#[derive(Clone)]
pub struct Config {
    pub issuer: String,
    pub authorize_url: Url,
    pub token_url: Url,
    pub discovery_url: Url,
    pub api_base: Url,
    pub websocket_url: String,
    /// Unix seconds now.
    pub clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    /// Revocation attempts on a network failure or 5xx.
    pub revoke_attempts: u32,
    /// The wait before the second attempt; it doubles after each.
    pub revoke_backoff: Duration,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("issuer", &self.issuer)
            .field("api_base", &self.api_base.as_str())
            .finish_non_exhaustive()
    }
}

impl Default for Config {
    fn default() -> Self {
        let url = |text: &str| Url::parse(text).expect("a static, valid URL");
        Self {
            issuer: ISSUER.to_owned(),
            authorize_url: url(AUTHORIZE_URL),
            token_url: url(TOKEN_URL),
            discovery_url: url(DISCOVERY_URL),
            api_base: url(API_BASE),
            websocket_url: WEBSOCKET_URL.to_owned(),
            clock: Arc::new(system_now),
            revoke_attempts: 4,
            revoke_backoff: Duration::from_secs(1),
        }
    }
}

/// What OpenAI's discovery document says, of what tau uses.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Discovery {
    pub issuer: String,
    pub jwks_uri: Url,
    #[serde(default)]
    pub revocation_endpoint: Option<Url>,
}

/// A finished sign-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedIn {
    pub account: AccountId,
    pub label: String,
    pub email: Option<String>,
    /// [`PlanUsage::Disabled`] means signed in, but inference must not
    /// start: offer to enable plan usage or another way to pay.
    pub plan_usage: PlanUsage,
}

/// Whether OpenAI confirmed the end of the session on sign-out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Revocation {
    Confirmed,
    /// There was no refresh token to revoke.
    NothingToRevoke,
    /// Tokens were cleared locally, but OpenAI did not confirm: tell the
    /// user they can disconnect tau in ChatGPT Settings.
    Unconfirmed {
        reason: String,
    },
}

/// A model the account may use, in the server's order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    /// What requests send as `model`.
    pub slug: String,
    /// What a picker shows.
    pub display_name: String,
}

/// The models a `GET /v1/models` body lists for display: those with
/// `visibility: "list"`, in order.
pub fn listed_models(body: &Value) -> Vec<ModelInfo> {
    body.get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|model| model["visibility"] == "list")
        .filter_map(|model| {
            let slug = model.get("slug")?.as_str()?.to_owned();
            let display_name = model
                .get("display_name")
                .and_then(Value::as_str)
                .unwrap_or(&slug)
                .to_owned();
            Some(ModelInfo { slug, display_name })
        })
        .collect()
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    token_type: Option<String>,
    expires_in: Option<u64>,
    scope: Option<String>,
    earliest_refresh_at: Option<Value>,
}

/// Access tokens last an hour when the response does not say.
const DEFAULT_EXPIRES_IN: u64 = 3600;

fn sorted_scopes(scope: &str) -> Vec<String> {
    let mut scopes: Vec<String> =
        scope.split_whitespace().map(str::to_owned).collect();
    scopes.sort();
    scopes.dedup();
    scopes
}

/// Signing in, keeping tokens fresh, signing out and listing models, for
/// the accounts in one [`Store`]. Clones share caches and the refresh
/// turn.
pub struct ChatGpt<D: Dialer = Tls> {
    inner: Arc<Inner<D>>,
}

impl<D: Dialer> Clone for ChatGpt<D> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<D: Dialer> fmt::Debug for ChatGpt<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatGpt")
            .field("store", &self.inner.store.dir())
            .finish_non_exhaustive()
    }
}

struct Inner<D> {
    dialer: D,
    config: Config,
    store: Store,
    discovery: Mutex<Option<Discovery>>,
    jwks: Mutex<Option<Jwks>>,
    /// Refreshes and sign-outs in this process take turns; the lock file
    /// does the same across processes.
    turn: Mutex<()>,
}

impl ChatGpt<Tls> {
    /// OpenAI's endpoints over TLS.
    pub fn new(store: Store) -> Self {
        Self::with_dialer(store, Tls, Config::default())
    }
}

impl<D: Dialer> ChatGpt<D> {
    pub fn with_dialer(store: Store, dialer: D, config: Config) -> Self {
        Self {
            inner: Arc::new(Inner {
                dialer,
                config,
                store,
                discovery: Mutex::new(None),
                jwks: Mutex::new(None),
                turn: Mutex::new(()),
            }),
        }
    }

    pub fn store(&self) -> &Store {
        &self.inner.store
    }

    pub fn config(&self) -> &Config {
        &self.inner.config
    }

    pub fn dialer(&self) -> &D {
        &self.inner.dialer
    }

    fn now(&self) -> u64 {
        (self.inner.config.clock)()
    }

    async fn send(
        &self,
        request: &Request,
    ) -> Result<http::Response, ChatGptError> {
        http::send(&self.inner.dialer, request)
            .await
            .map_err(ChatGptError::Network)
    }

    async fn get_json(&self, url: Url) -> Result<Value, ChatGptError> {
        let response = self.send(&Request::get(url.clone())).await?;
        if !response.is_success() {
            return Err(ChatGptError::Protocol(format!(
                "GET {url}: HTTP {}: {}",
                response.status,
                response.text()
            )));
        }
        serde_json::from_slice(&response.body).map_err(|error| {
            ChatGptError::Protocol(format!("GET {url}: {error}"))
        })
    }

    /// OpenAI's discovery document, fetched once.
    pub async fn discovery(&self) -> Result<Discovery, ChatGptError> {
        let mut cached = self.inner.discovery.lock().await;
        if let Some(discovery) = &*cached {
            return Ok(discovery.clone());
        }
        let value = self
            .get_json(self.inner.config.discovery_url.clone())
            .await?;
        let discovery: Discovery =
            serde_json::from_value(value).map_err(|error| {
                ChatGptError::Protocol(format!("discovery: {error}"))
            })?;
        *cached = Some(discovery.clone());
        Ok(discovery)
    }

    async fn jwks(&self, fresh: bool) -> Result<Jwks, ChatGptError> {
        let mut cached = self.inner.jwks.lock().await;
        if !fresh && let Some(jwks) = &*cached {
            return Ok(jwks.clone());
        }
        let discovery = self.discovery().await?;
        let value = self.get_json(discovery.jwks_uri).await?;
        let jwks: Jwks = serde_json::from_value(value).map_err(|error| {
            ChatGptError::Protocol(format!("JWKS: {error}"))
        })?;
        *cached = Some(jwks.clone());
        Ok(jwks)
    }

    /// Validates an ID token: signature against OpenAI's JWKS (fetched
    /// again once for an unknown key id), then issuer, audience, expiry
    /// and, on a sign-in, the nonce.
    pub async fn validate_id_token(
        &self,
        token: &str,
        client_id: &str,
        nonce: Option<&str>,
    ) -> Result<Identity, ChatGptError> {
        let jwt = Jwt::parse(token)?;
        jwt.check_algorithm()?;
        let mut jwks = self.jwks(false).await?;
        if jwt.key(&jwks).is_none() && jwt.has_kid() {
            jwks = self.jwks(true).await?;
        }
        let key = jwt.key(&jwks).ok_or(IdTokenError::UnknownKey)?;
        jwt.verify(key)?;
        Ok(check_claims(
            &jwt.claims,
            &Expected {
                issuer: &self.inner.config.issuer,
                client_id,
                nonce,
                now: self.now(),
            },
        )?)
    }

    /// Starts a sign-in to `redirect_uri`. `account` signs a saved
    /// registration in again with its client id and hints; `None`
    /// registers a new one. `ask_consent` asks again for plan usage after
    /// it was declined. The host id is made on the first call.
    pub fn start_sign_in(
        &self,
        account: Option<&AccountId>,
        redirect_uri: RedirectUri,
        ask_consent: bool,
    ) -> Result<SignIn, ChatGptError> {
        let host_id = self.inner.store.host_id()?;
        let (registration, hints) = match account {
            None => (Registration::New, (None, None)),
            Some(id) => {
                let saved = self.inner.store.load(id)?;
                (
                    Registration::Returning {
                        account: id.clone(),
                        client_id: saved.client_id,
                        subject: saved.subject,
                    },
                    (saved.id_token, saved.email),
                )
            }
        };
        Ok(SignIn::new(
            &self.inner.config.authorize_url,
            registration,
            host_id,
            redirect_uri,
            hints,
            ask_consent,
        ))
    }

    /// Exchanges the callback's code, validates the ID token, saves the
    /// record and makes it the active account. A returning sign-in must
    /// validate as the same account. Nothing is saved on failure.
    pub async fn finish_sign_in(
        &self,
        sign_in: &SignIn,
        callback: &Callback,
    ) -> Result<SignedIn, ChatGptError> {
        let redirect = sign_in.redirect_uri.to_string();
        let request = Request::post_form(
            self.inner.config.token_url.clone(),
            &[
                ("grant_type", "authorization_code"),
                ("client_id", &callback.client_id),
                ("code", &callback.code),
                ("code_verifier", &sign_in.pkce.verifier),
                ("redirect_uri", &redirect),
                ("resource", RESOURCE),
            ],
        );
        let response = self.send(&request).await?;
        if !response.is_success() {
            let error = OAuthError::from_response(&response);
            return Err(match error.code.as_deref() {
                Some("invalid_grant") => ChatGptError::CodeRejected,
                Some("invalid_client") => ChatGptError::InvalidClient(error),
                _ => ChatGptError::OAuth(error),
            });
        }
        let tokens: TokenResponse = serde_json::from_slice(&response.body)
            .map_err(|error| {
                ChatGptError::Protocol(format!("token response: {error}"))
            })?;
        let (Some(access_token), Some(id_token)) =
            (tokens.access_token, tokens.id_token)
        else {
            return Err(ChatGptError::Protocol(
                "the token response has no access or ID token".into(),
            ));
        };
        let identity = self
            .validate_id_token(
                &id_token,
                &callback.client_id,
                Some(&sign_in.nonce),
            )
            .await?;
        let store = &self.inner.store;
        let previous = match &sign_in.registration {
            Registration::New => None,
            Registration::Returning {
                account, subject, ..
            } => {
                if identity.subject != *subject {
                    return Err(ChatGptError::IdentityMismatch);
                }
                store.load(account).ok()
            }
        };
        let now = self.now();
        let expires_in = tokens.expires_in.unwrap_or(DEFAULT_EXPIRES_IN);
        let email = identity
            .email
            .clone()
            .or_else(|| previous.as_ref().and_then(|p| p.email.clone()));
        let scope = tokens.scope.or_else(|| callback.scope.clone());
        let credentials = Credentials {
            label: store.label_for(
                email.as_deref(),
                &callback.client_id,
                &identity.subject,
            )?,
            email: email.clone(),
            issuer: self.inner.config.issuer.clone(),
            subject: identity.subject,
            client_id: callback.client_id.clone(),
            ext_agent_host_id: store.host_id()?,
            id_token: Some(id_token),
            access_token: Some(access_token),
            refresh_token: tokens.refresh_token,
            token_type: tokens.token_type,
            expires_in: Some(expires_in),
            expires_at: Some(now + expires_in),
            earliest_refresh_at: tokens.earliest_refresh_at,
            scopes: sorted_scopes(scope.as_deref().unwrap_or("")),
            saved_at: rfc3339(now),
        };
        let account = credentials.id();
        {
            let _turn = self.inner.turn.lock().await;
            let _lock = store.lock(&account).await?;
            store.save(&credentials)?;
        }
        store.set_active(&account)?;
        Ok(SignedIn {
            account,
            label: credentials.label.clone(),
            email,
            plan_usage: credentials.plan_usage(),
        })
    }

    /// The account's access token, refreshed first when it is within
    /// [`REFRESH_MARGIN`] of expiring. Only one refresh runs for all
    /// callers, in this process and others.
    pub async fn access_token(
        &self,
        account: &AccountId,
    ) -> Result<String, ChatGptError> {
        let saved = self.inner.store.load(account)?;
        let credentials = if saved.needs_refresh(self.now()) {
            self.refresh_as_needed(account, false).await?
        } else {
            saved
        };
        credentials
            .access_token
            .ok_or(ChatGptError::SignInRequired { reason: None })
    }

    /// [`Self::access_token`], for inference: fails with
    /// [`ChatGptError::PlanUsageDisabled`] when plan usage was not
    /// granted.
    pub async fn inference_token(
        &self,
        account: &AccountId,
    ) -> Result<String, ChatGptError> {
        let token = self.access_token(account).await?;
        match self.inner.store.load(account)?.plan_usage() {
            PlanUsage::Enabled => Ok(token),
            PlanUsage::Disabled => Err(ChatGptError::PlanUsageDisabled),
        }
    }

    /// Refreshes the account's tokens now.
    pub async fn refresh(
        &self,
        account: &AccountId,
    ) -> Result<Credentials, ChatGptError> {
        self.refresh_as_needed(account, true).await
    }

    async fn refresh_as_needed(
        &self,
        account: &AccountId,
        force: bool,
    ) -> Result<Credentials, ChatGptError> {
        let store = &self.inner.store;
        let _turn = self.inner.turn.lock().await;
        let _lock = store.lock(account).await?;
        // Read again under the lock: another refresh may have rotated the
        // tokens meanwhile.
        let mut credentials = store.load(account)?;
        if !force && !credentials.needs_refresh(self.now()) {
            return Ok(credentials);
        }
        let refresh_token = credentials
            .refresh_token
            .clone()
            .ok_or(ChatGptError::SignInRequired { reason: None })?;
        let request = Request::post_form(
            self.inner.config.token_url.clone(),
            &[
                ("grant_type", "refresh_token"),
                ("client_id", &credentials.client_id),
                ("refresh_token", &refresh_token),
                ("resource", RESOURCE),
            ],
        );
        let response = self.send(&request).await?;
        if !response.is_success() {
            let error = OAuthError::from_response(&response);
            if error.is_unusable_refresh_token() {
                credentials.clear_session();
                store.save(&credentials)?;
                return Err(ChatGptError::SignInRequired {
                    reason: error.code,
                });
            }
            if error.code.as_deref() == Some("invalid_client") {
                return Err(ChatGptError::InvalidClient(error));
            }
            return Err(ChatGptError::OAuth(error));
        }
        let tokens: TokenResponse = serde_json::from_slice(&response.body)
            .map_err(|error| {
                ChatGptError::Protocol(format!("token response: {error}"))
            })?;
        let access_token = tokens.access_token.ok_or_else(|| {
            ChatGptError::Protocol(
                "the refresh response has no access token".into(),
            )
        })?;
        // A new ID token replaces the hint only if it validates as the
        // same account.
        if let Some(id_token) = tokens.id_token
            && self
                .validate_id_token(&id_token, &credentials.client_id, None)
                .await
                .is_ok_and(|identity| identity.subject == credentials.subject)
        {
            credentials.id_token = Some(id_token);
        }
        let now = self.now();
        let expires_in = tokens.expires_in.unwrap_or(DEFAULT_EXPIRES_IN);
        credentials.access_token = Some(access_token);
        if let Some(refresh) = tokens.refresh_token {
            credentials.refresh_token = Some(refresh);
        }
        if let Some(scope) = tokens.scope {
            credentials.scopes = sorted_scopes(&scope);
        }
        if tokens.token_type.is_some() {
            credentials.token_type = tokens.token_type;
        }
        credentials.expires_in = Some(expires_in);
        credentials.expires_at = Some(now + expires_in);
        credentials.earliest_refresh_at = tokens.earliest_refresh_at;
        credentials.saved_at = rfc3339(now);
        store.save(&credentials)?;
        Ok(credentials)
    }

    /// Signs the account out: revokes its refresh token (retrying network
    /// failures and 5xx with backoff), then clears its tokens. The
    /// registration and the host id stay for the next sign-in.
    pub async fn sign_out(
        &self,
        account: &AccountId,
    ) -> Result<Revocation, ChatGptError> {
        let store = &self.inner.store;
        let _turn = self.inner.turn.lock().await;
        let _lock = store.lock(account).await?;
        let mut credentials = store.load(account)?;
        let revocation = match credentials.refresh_token.clone() {
            None => Revocation::NothingToRevoke,
            Some(token) => self.revoke(&credentials.client_id, &token).await,
        };
        credentials.clear_tokens();
        store.save(&credentials)?;
        Ok(revocation)
    }

    async fn revoke(&self, client_id: &str, token: &str) -> Revocation {
        let config = &self.inner.config;
        let mut backoff = config.revoke_backoff;
        let mut reason = String::from("not attempted");
        for attempt in 0..config.revoke_attempts.max(1) {
            if attempt > 0 {
                tokio::time::sleep(backoff).await;
                backoff *= 2;
            }
            let endpoint = match self.discovery().await {
                Ok(Discovery {
                    revocation_endpoint: Some(endpoint),
                    ..
                }) => endpoint,
                Ok(_) => {
                    return Revocation::Unconfirmed {
                        reason: "OpenAI publishes no revocation endpoint"
                            .into(),
                    };
                }
                Err(error) => {
                    reason = error.to_string();
                    continue;
                }
            };
            let request = Request::post_form(
                endpoint,
                &[
                    ("token", token),
                    ("token_type_hint", "refresh_token"),
                    ("client_id", client_id),
                ],
            );
            match self.send(&request).await {
                Ok(response) if response.is_success() => {
                    return Revocation::Confirmed;
                }
                Ok(response) if response.status >= 500 => {
                    reason = OAuthError::from_response(&response).to_string();
                }
                Ok(response) => {
                    return Revocation::Unconfirmed {
                        reason: OAuthError::from_response(&response)
                            .to_string(),
                    };
                }
                Err(error) => reason = error.to_string(),
            }
        }
        Revocation::Unconfirmed { reason }
    }

    /// The account's models for a picker, from `GET /v1/models`.
    pub async fn models(
        &self,
        account: &AccountId,
    ) -> Result<Vec<ModelInfo>, ChatGptError> {
        let token = self.access_token(account).await?;
        let url = self
            .inner
            .config
            .api_base
            .join("models")
            .expect("a relative path joins");
        let response = self.send(&Request::get(url).bearer(&token)).await?;
        if !response.is_success() {
            return Err(ChatGptError::Api(Box::new(ApiError::from_response(
                &response,
            ))));
        }
        let body: Value =
            serde_json::from_slice(&response.body).map_err(|error| {
                ChatGptError::Protocol(format!("models: {error}"))
            })?;
        Ok(listed_models(&body))
    }

    /// The active account, if one is set.
    pub fn active(&self) -> Result<Option<AccountId>, ChatGptError> {
        self.inner.store.active()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn only_listed_models_stay_in_order() {
        let body = json!({"models": [
            {"slug": "b", "display_name": "B", "visibility": "list"},
            {"slug": "hidden", "display_name": "H", "visibility": "hide"},
            {"slug": "a", "visibility": "list"},
            {"display_name": "no slug", "visibility": "list"},
        ]});
        assert_eq!(
            listed_models(&body),
            vec![
                ModelInfo {
                    slug: "b".into(),
                    display_name: "B".into()
                },
                ModelInfo {
                    slug: "a".into(),
                    display_name: "a".into()
                },
            ]
        );
    }

    #[test]
    fn scopes_are_split_and_sorted() {
        assert_eq!(
            sorted_scopes("openid chatgpt.tokens.use.direct  email openid"),
            vec!["chatgpt.tokens.use.direct", "email", "openid"]
        );
    }
}
