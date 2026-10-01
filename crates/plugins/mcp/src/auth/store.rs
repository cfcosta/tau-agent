//! Where sign-ins live: `~/.config/tau/mcp-auth.json`, one owner-only
//! file of grants, each for one server URL and the client it signs in
//! with.
//!
//! ```json
//! {
//!   "grants": [
//!     {
//!       "url": "https://mcp.example.com/mcp",
//!       "clientId": "issued-or-configured",
//!       "redirectUri": "http://127.0.0.1:53682/callback",
//!       "metadata": { "issuer": "…", "token_endpoint": "…" },
//!       "tokens": { "access_token": "…", "token_type": "bearer", … },
//!       "receivedAt": 1790000000,
//!       "scopes": ["read"],
//!       "signedIn": "…"
//!     }
//!   ]
//! }
//! ```
//!
//! Every write replaces the file whole through an owner-only temporary
//! file, under a lock file, so a reader never sees half of one and two
//! processes do not lose each other's grants. A refresh holds a second
//! lock from reading the refresh token to saving the new one. `Debug`
//! never prints a token or a secret, and nothing logs one.

use std::{
    fmt,
    fs,
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rmcp::transport::auth::{
    AuthError,
    CredentialRefreshGuard,
    CredentialStore,
    OAuthTokenResponse,
    StoredCredentials,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_ai::files::{create_private_dir, private_options, write_private};

/// The file, in tau's configuration directory.
pub const AUTH_FILE: &str = "mcp-auth.json";

/// Which grant: a server's URL and the client configured for it, if
/// any (`oauth.clientId`). A registered client is the grant's, under
/// `client: None`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GrantKey {
    pub url: String,
    pub client: Option<String>,
}

/// One sign-in to one server: the client it uses, the authorization
/// server it signed in at, and the tokens, when signed in.
#[derive(Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Grant {
    pub url: String,
    /// The configured client id this grant is for; `None` for a client
    /// sign-in registered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    /// The client it signs in with: the configured one, or the one
    /// registration issued.
    pub client_id: String,
    /// The secret registration issued, if any. A configured client's
    /// secret stays in the configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    /// The redirect URI the client was registered with.
    pub redirect_uri: String,
    /// The authorization server's metadata (RFC 8414), as discovered.
    pub metadata: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
    /// The resource the tokens are for (RFC 8707), when it is not the
    /// server's URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    /// The last token response, as the authorization server gave it;
    /// `None` when signed out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<Value>,
    /// Unix seconds the tokens came.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub received_at: Option<u64>,
    /// The scopes granted.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
    /// Who signed in, when an ID token says so: shown, never trusted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// A fresh random id for each sign-in, kept by refreshes: when it
    /// changes, another sign-in (or a sign-out) happened, and the
    /// server's connections connect again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signed_in: Option<String>,
}

impl fmt::Debug for Grant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never print a token or a secret.
        f.debug_struct("Grant")
            .field("url", &self.url)
            .field("client", &self.client)
            .field("client_id", &self.client_id)
            .field("redirect_uri", &self.redirect_uri)
            .field("issuer", &self.issuer)
            .field("scopes", &self.scopes)
            .field("account", &self.account)
            .field("signed_in", &self.is_signed_in())
            .field("expires_at", &self.expires_at())
            .finish_non_exhaustive()
    }
}

impl Grant {
    pub fn key(&self) -> GrantKey {
        GrantKey {
            url: self.url.clone(),
            client: self.client.clone(),
        }
    }

    pub fn is_signed_in(&self) -> bool {
        self.tokens.is_some()
    }

    /// Unix seconds the access token expires, when the server said.
    pub fn expires_at(&self) -> Option<u64> {
        let expires_in = self.tokens.as_ref()?.get("expires_in")?.as_u64()?;
        Some(self.received_at? + expires_in)
    }

    /// Whether a refresh token was given.
    pub fn can_refresh(&self) -> bool {
        self.tokens
            .as_ref()
            .and_then(|tokens| tokens.get("refresh_token"))
            .is_some_and(|token| !token.is_null())
    }

    /// Forgets the tokens and who signed in, keeping the client for the
    /// next sign-in.
    pub fn sign_out(&mut self) {
        self.tokens = None;
        self.received_at = None;
        self.scopes.clear();
        self.account = None;
        self.signed_in = None;
    }
}

/// The file's shape.
#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    grants: Vec<Grant>,
}

/// The grants file. Cheap to clone; every call reads or writes the
/// file, so other processes' sign-ins show at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenStore {
    path: PathBuf,
}

impl TokenStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// `<dir>/mcp-auth.json`.
    pub fn in_dir(dir: &Path) -> Self {
        Self::new(dir.join(AUTH_FILE))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every grant. No file is none.
    pub fn grants(&self) -> io::Result<Vec<Grant>> {
        Ok(self.read()?.grants)
    }

    /// The grant for `key`.
    pub fn get(&self, key: &GrantKey) -> io::Result<Option<Grant>> {
        Ok(self.grants()?.into_iter().find(|grant| grant.key() == *key))
    }

    /// Replaces the grant for `key` with what `change` makes of it
    /// (`None` removes it), under the file's lock.
    pub fn update(
        &self,
        key: &GrantKey,
        change: impl FnOnce(Option<Grant>) -> Option<Grant>,
    ) -> io::Result<()> {
        let _lock = self.lock("lock")?;
        let mut file = self.read()?;
        let at = file.grants.iter().position(|grant| grant.key() == *key);
        let old = at.map(|at| file.grants.remove(at));
        if let Some(mut new) = change(old) {
            new.url.clone_from(&key.url);
            new.client.clone_from(&key.client);
            file.grants.insert(at.unwrap_or(file.grants.len()), new);
        }
        self.write(&file)
    }

    /// Saves `grant`, replacing the one of its key.
    pub fn put(&self, grant: Grant) -> io::Result<()> {
        let key = grant.key();
        self.update(&key, |_| Some(grant))
    }

    /// Forgets the tokens of `key`'s grant, keeping its client. Whether
    /// it was signed in.
    pub fn sign_out(&self, key: &GrantKey) -> io::Result<bool> {
        let mut was = false;
        self.update(key, |grant| {
            let mut grant = grant?;
            was = grant.is_signed_in();
            grant.sign_out();
            Some(grant)
        })?;
        Ok(was)
    }

    /// The sign-in `key` is under now, if signed in: it changes with
    /// each sign-in and sign-out, not with a refresh. An unreadable file
    /// reads as signed out.
    pub fn fingerprint(&self, key: &GrantKey) -> Option<String> {
        let grant = self.get(key).ok()??;
        grant.is_signed_in().then_some(grant.signed_in?)
    }

    /// The grant as rmcp's credentials: what an `AuthorizationManager`
    /// loads, refreshes and saves. A sign-in's (`sign_in`) marks the
    /// tokens it saves as a new sign-in, written over the grant it
    /// holds: the client and server it signed in with. A connection's
    /// keeps the mark.
    pub(crate) fn credentials(
        &self,
        key: GrantKey,
        sign_in: Option<Pending>,
    ) -> GrantCredentials {
        GrantCredentials {
            store: self.clone(),
            key,
            sign_in,
        }
    }

    fn read(&self) -> io::Result<File> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(File::default());
            }
            Err(error) => return Err(error),
        };
        serde_json::from_str(&text).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{} is not a grants file: {error}",
                    self.path.display()
                ),
            )
        })
    }

    fn write(&self, file: &File) -> io::Result<()> {
        let text = serde_json::to_string_pretty(file)
            .expect("the grants always print");
        write_private(&self.path, text.as_bytes())
    }

    /// Holds `<file>.<suffix>` locked until the guard drops.
    fn lock(&self, suffix: &str) -> io::Result<fs::File> {
        let mut name = self.path.as_os_str().to_owned();
        name.push(format!(".{suffix}"));
        let path = PathBuf::from(name);
        if let Some(dir) = path.parent() {
            create_private_dir(dir)?;
        }
        let file = private_options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        file.lock()?;
        Ok(file)
    }
}

/// A sign-in's grant before its tokens come: taken by the first save.
pub(crate) type Pending = Arc<Mutex<Option<Grant>>>;

/// A grant as rmcp's [`CredentialStore`].
#[derive(Clone)]
pub(crate) struct GrantCredentials {
    store: TokenStore,
    key: GrantKey,
    /// For a sign-in: the grant its tokens complete.
    sign_in: Option<Pending>,
}

fn store_error(error: impl fmt::Display) -> AuthError {
    AuthError::CredentialStoreError(error.to_string())
}

#[async_trait]
impl CredentialStore for GrantCredentials {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        let Some(grant) = self.store.get(&self.key).map_err(store_error)?
        else {
            return Ok(None);
        };
        let Some(tokens) = grant.tokens else {
            return Ok(None);
        };
        let tokens: OAuthTokenResponse =
            serde_json::from_value(tokens).map_err(store_error)?;
        Ok(Some(
            StoredCredentials::new(
                grant.client_id,
                Some(tokens),
                grant.scopes,
                grant.received_at,
            )
            .with_issuer(grant.issuer),
        ))
    }

    async fn save(
        &self,
        credentials: StoredCredentials,
    ) -> Result<(), AuthError> {
        let tokens = credentials
            .token_response
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(store_error)?;
        let base = self
            .sign_in
            .as_ref()
            .and_then(|base| base.lock().expect("not poisoned").take());
        let fresh = base.is_some();
        self.store
            .update(&self.key, |old| {
                let mut grant = base.or(old).unwrap_or_default();
                grant.client_id = credentials.client_id.clone();
                if credentials.issuer.is_some() {
                    grant.issuer.clone_from(&credentials.issuer);
                }
                grant.account = tokens
                    .as_ref()
                    .and_then(account)
                    .or(if fresh { None } else { grant.account.take() });
                grant.tokens = tokens;
                grant.received_at = credentials.token_received_at;
                grant.scopes.clone_from(&credentials.granted_scopes);
                if fresh {
                    grant.signed_in = Some(random_id());
                }
                Some(grant)
            })
            .map_err(store_error)
    }

    async fn clear(&self) -> Result<(), AuthError> {
        self.store
            .sign_out(&self.key)
            .map(drop)
            .map_err(store_error)
    }

    async fn acquire_refresh_guard(
        &self,
    ) -> Result<Option<CredentialRefreshGuard>, AuthError> {
        let store = self.store.clone();
        let lock = tokio::task::spawn_blocking(move || store.lock("refresh"))
            .await
            .map_err(store_error)?
            .map_err(store_error)?;
        Ok(Some(CredentialRefreshGuard::new(lock)))
    }
}

/// Who an ID token in a token response names: its `email`, else
/// `preferred_username`, `name` or `sub`. Read for showing, not checked:
/// nothing trusts it.
pub fn account(tokens: &Value) -> Option<String> {
    let token = tokens.get("id_token")?.as_str()?;
    let payload = token.split('.').nth(1)?;
    let claims: Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;
    ["email", "preferred_username", "name", "sub"]
        .iter()
        .find_map(|claim| claims.get(claim)?.as_str().map(str::to_owned))
}

/// 16 random bytes in hex.
pub(crate) fn random_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the system has randomness");
    crate::config::hex(&bytes)
}
