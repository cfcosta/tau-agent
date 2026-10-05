//! Signing in, from the server's challenge to saved tokens, and the
//! authorization a connection sends its requests with.
//!
//! The protocol work is rmcp's (`rmcp::transport::auth`): discovery
//! (protected resource metadata, then authorization server metadata),
//! registration (RFC 7591), the authorization URL with PKCE S256 and a
//! fresh `state`, the code exchange, refreshing, and the scopes to ask
//! for. tau adds the loopback the browser comes back to, the checks on
//! what comes back, where grants are kept, and its own HTTP.

use std::{fmt, sync::Arc, time::Duration};

use rmcp::transport::auth::{
    AuthError,
    AuthorizationManager,
    AuthorizationMetadata,
    AuthorizationRequest,
    AuthorizationSession,
    OAuthClientConfig,
};
use tracing::{instrument::WithSubscriber as _, subscriber::NoSubscriber};
use url::Url;

use super::{
    callback::Loopback,
    http::OAuthHttp,
    store::{Grant, GrantKey, Pending, TokenStore},
};
use crate::config::{CallbackAddress, OAuthConfig, callback_address};

/// How long the browser has to come back.
pub const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);

/// The name a registered client goes by when `oauth.clientName` does not
/// give one.
pub const CLIENT_NAME: &str = "tau";

/// What a sign-in needs.
#[derive(Debug, Clone)]
pub struct SignInRequest {
    /// The server's URL.
    pub url: String,
    /// Its `oauth` block, `clientSecret` expanded; the default for a
    /// server without one.
    pub oauth: OAuthConfig,
    /// The server's `WWW-Authenticate` challenge, when it gave one: it
    /// may name its metadata and the scopes it wants.
    pub challenge: Option<String>,
    /// Scopes the server said it needs beyond those granted
    /// (`insufficient_scope`): asked for with them.
    pub scope: Option<String>,
}

impl SignInRequest {
    /// The grant this sign-in makes.
    pub fn key(&self) -> GrantKey {
        GrantKey {
            url: self.url.clone(),
            client: self.oauth.client_id.clone(),
        }
    }
}

/// One sign-in, waiting for the browser.
pub struct SignIn {
    url: String,
    state: String,
    loopback: Loopback,
    session: AuthorizationSession,
    store: TokenStore,
    key: GrantKey,
}

impl fmt::Debug for SignIn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignIn")
            .field("key", &self.key)
            .field("redirect_uri", &self.loopback.redirect_uri())
            .finish_non_exhaustive()
    }
}

fn auth(error: AuthError) -> String {
    error.to_string()
}

/// Starts signing in to `request.url`: listens for the browser, finds
/// the authorization server, registers a client when none is configured
/// (or reuses the one registered before, on its port), and makes the
/// authorization URL. Nothing is saved until [`SignIn::finish`].
pub async fn begin(
    store: &TokenStore,
    request: SignInRequest,
) -> Result<SignIn, String> {
    let http = Arc::new(OAuthHttp::new());
    let oauth = &request.oauth;
    let address =
        callback_address(oauth.callback_url.as_deref(), oauth.callback_port)?;
    let key = request.key();
    let previous = store.get(&key).map_err(|error| {
        format!("cannot read {}: {error}", store.path().display())
    })?;
    let loopback = bind(&address, previous.as_ref(), oauth).await?;
    let redirect = loopback.redirect_uri();

    let mut manager = AuthorizationManager::new_with_oauth_http_client(
        request.url.as_str(),
        http.clone(),
    )
    .await
    .map_err(auth)?;
    let metadata: AuthorizationMetadata = match &oauth.auth_server_metadata_url
    {
        Some(url) => serde_json::from_value(http.get_json(url).await?)
            .map_err(|error| {
                format!("{url} is not authorization server metadata: {error}")
            })?,
        None => {
            manager
                .resolve_metadata_from_challenge(request.challenge.as_deref())
                .await
                .map_err(auth)?
                .metadata
        }
    };
    manager.set_metadata(metadata.clone());

    let mut scopes: Vec<String> = match &oauth.scope {
        Some(scope) => scope.split_whitespace().map(str::to_owned).collect(),
        None => manager.select_scopes(None, &[]),
    };
    if let Some(more) = &request.scope {
        // Step-up: what was granted, and what the server asked for.
        let granted = previous.iter().flat_map(|grant| grant.scopes.clone());
        for scope in granted.chain(more.split_whitespace().map(str::to_owned)) {
            if !scopes.contains(&scope) {
                scopes.push(scope);
            }
        }
    }

    let reused = previous.filter(|grant| {
        oauth.client_id.is_none()
            && !grant.client_id.is_empty()
            && grant.redirect_uri == redirect
            && grant.issuer == metadata.issuer
    });
    let (client_id, registered_secret) = match (&oauth.client_id, reused) {
        (Some(id), _) => (id.clone(), None),
        (None, Some(grant)) => (grant.client_id, grant.client_secret),
        (None, None) => {
            let refs: Vec<&str> = scopes.iter().map(String::as_str).collect();
            let name = oauth.client_name.as_deref().unwrap_or(CLIENT_NAME);
            let config: OAuthClientConfig = manager
                .register_client(name, &redirect, &refs)
                .await
                .map_err(auth)?;
            (config.client_id, config.client_secret)
        }
    };
    let pending: Pending = Arc::new(std::sync::Mutex::new(Some(Grant {
        url: key.url.clone(),
        client: key.client.clone(),
        client_id: client_id.clone(),
        client_secret: registered_secret.clone(),
        redirect_uri: redirect.clone(),
        metadata: serde_json::to_value(&metadata).unwrap_or_default(),
        issuer: metadata.issuer.clone(),
        ..Grant::default()
    })));
    manager.set_credential_store(
        store.credentials(key.clone(), Some(pending.clone())),
    );

    let mut authorization = AuthorizationRequest::new(redirect)
        .with_preregistered_client(client_id)
        .with_scopes(scopes);
    if let Some(secret) = oauth.client_secret.clone().or(registered_secret) {
        authorization = authorization.with_client_secret(secret);
    }
    let session = AuthorizationSession::new(manager, authorization)
        .await
        .map_err(|(_, error)| auth(error))?;
    let url = session.get_authorization_url().to_owned();
    let parsed = Url::parse(&url).map_err(|error| {
        format!("the authorization URL is not valid: {error}")
    })?;
    let param = |name: &str| {
        parsed
            .query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    };
    let state =
        param("state").ok_or("the authorization URL carries no state")?;
    let canonical = Url::parse(&request.url).map(String::from).ok();
    if let Some(grant) = pending.lock().expect("not poisoned").as_mut() {
        grant.resource =
            param("resource").filter(|r| Some(r) != canonical.as_ref());
    }
    Ok(SignIn {
        url,
        state,
        loopback,
        session,
        store: store.clone(),
        key,
    })
}

/// Listens where the configuration says. A client registered before, on
/// a port the configuration leaves free, gets that port again when it is
/// free, so the client is reused.
async fn bind(
    address: &CallbackAddress,
    previous: Option<&Grant>,
    oauth: &OAuthConfig,
) -> Result<Loopback, String> {
    let registered = previous
        .filter(|_| oauth.client_id.is_none() && address.port == 0)
        .and_then(|grant| Url::parse(&grant.redirect_uri).ok())
        .filter(|url| {
            url.path() == address.path
                && url.host_str().map(|h| h.trim_matches(['[', ']']))
                    == Some(address.host.trim_matches(['[', ']']))
        })
        .and_then(|url| url.port());
    if let Some(port) = registered {
        let again = CallbackAddress {
            port,
            ..address.clone()
        };
        if let Ok(loopback) = Loopback::bind(&again).await {
            return Ok(loopback);
        }
    }
    Loopback::bind(address).await.map_err(|error| {
        format!(
            "cannot listen on {}:{} for the browser: {error}",
            address.host, address.port
        )
    })
}

impl SignIn {
    /// The page to open in the browser.
    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn redirect_uri(&self) -> String {
        self.loopback.redirect_uri()
    }

    pub fn key(&self) -> &GrantKey {
        &self.key
    }

    /// Waits for the browser to come back, exchanges the code and saves
    /// the grant. Put a time limit on it ([`SIGN_IN_TIMEOUT`]).
    pub async fn finish(self) -> Result<Grant, String> {
        let callback = self
            .loopback
            .wait(&self.state)
            .await
            .map_err(|error| error.to_string())?;
        // rmcp logs the code and the token response at debug level: the
        // exchange runs with no subscriber, so no host's can see them.
        self.session
            .handle_callback_with_issuer(
                &callback.code,
                &self.state,
                callback.issuer.as_deref(),
            )
            .with_subscriber(NoSubscriber::default())
            .await
            .map_err(auth)?;
        self.store
            .get(&self.key)
            .map_err(|error| error.to_string())?
            .filter(Grant::is_signed_in)
            .ok_or_else(|| "the tokens were not saved".to_owned())
    }
}

/// The authorization a connection to `url` sends its requests with,
/// when `key` is signed in: rmcp's manager over the grant, which adds
/// the access token, refreshes it when it is about to expire or the
/// server turns it down, and saves what the refresh gives.
/// `client_secret` is the configured client's, expanded.
pub(crate) async fn authorization(
    url: &str,
    store: &TokenStore,
    key: &GrantKey,
    client_secret: Option<&str>,
) -> Result<Option<AuthorizationManager>, String> {
    let Some(grant) = store
        .get(key)
        .map_err(|error| {
            format!("cannot read {}: {error}", store.path().display())
        })?
        .filter(Grant::is_signed_in)
    else {
        return Ok(None);
    };
    let http = Arc::new(OAuthHttp::new());
    let mut manager =
        AuthorizationManager::new_with_oauth_http_client(url, http)
            .await
            .map_err(auth)?;
    if grant.resource.is_some() {
        // rmcp takes a resource other than the URL only from discovery.
        let _ = manager.resolve_metadata().await;
    }
    let metadata: AuthorizationMetadata =
        serde_json::from_value(grant.metadata.clone()).map_err(|error| {
            format!("the saved authorization server is not valid: {error}")
        })?;
    manager.set_metadata(metadata);
    manager.set_credential_store(store.credentials(key.clone(), None));
    let mut client = OAuthClientConfig::new(
        grant.client_id.clone(),
        grant.redirect_uri.clone(),
    );
    if let Some(secret) = client_secret
        .map(str::to_owned)
        .or(grant.client_secret.clone())
    {
        client = client.with_client_secret(secret);
    }
    manager.configure_client(client).map_err(auth)?;
    Ok(Some(manager))
}
