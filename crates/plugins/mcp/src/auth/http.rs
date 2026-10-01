//! tau's HTTP for MCP: reqwest with rustls, `ring` and the webpki roots,
//! as the rest of tau, for the servers' connections and for every
//! request sign-in makes.

use std::{sync::Arc, time::Duration};

use rmcp::transport::auth::{
    OAuthHttpClient,
    OAuthHttpClientError,
    OAuthHttpClientFuture,
    OAuthHttpRedirectPolicy,
    OAuthHttpRequest,
};
use tokio_rustls::rustls::{
    ClientConfig as TlsConfig,
    RootCertStore,
    crypto::ring,
};

/// The most an OAuth answer may be: metadata, a registration, a token.
const MAX_BODY: usize = 1024 * 1024;

/// How long one OAuth request may take when rmcp does not say.
const OAUTH_TIMEOUT: Duration = Duration::from_secs(30);

fn tls() -> TlsConfig {
    let roots = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    TlsConfig::builder_with_provider(Arc::new(ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("ring supports the default protocol versions")
        .with_root_certificates(roots)
        .with_no_client_auth()
}

/// The client a server's connection uses: as rmcp's default one (no
/// idle pool, no redirects, so headers never reach another host) but
/// with tau's TLS, since reqwest has no provider of its own here.
pub(crate) fn mcp_client() -> reqwest::Client {
    reqwest::Client::builder()
        .tls_backend_preconfigured(tls())
        .pool_max_idle_per_host(0)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("a static client configuration builds")
}

/// What signing in sends its requests through: tau's TLS, rmcp's
/// redirect policy for each request (discovery and token requests
/// follow none; registration up to 5), answers of at most 1 MiB.
pub(crate) struct OAuthHttp {
    follow: reqwest::Client,
    stop: reqwest::Client,
}

impl OAuthHttp {
    pub(crate) fn new() -> Self {
        let client = |policy| {
            reqwest::Client::builder()
                .tls_backend_preconfigured(tls())
                .redirect(policy)
                .timeout(OAUTH_TIMEOUT)
                .build()
                .expect("a static client configuration builds")
        };
        Self {
            follow: client(reqwest::redirect::Policy::limited(5)),
            stop: client(reqwest::redirect::Policy::none()),
        }
    }

    /// Sends `request` and reads the answer whole.
    pub(crate) async fn send(
        &self,
        request: http::Request<Vec<u8>>,
        policy: OAuthHttpRedirectPolicy,
        timeout: Option<Duration>,
    ) -> Result<http::Response<Vec<u8>>, OAuthHttpClientError> {
        let client = match policy {
            OAuthHttpRedirectPolicy::Stop => &self.stop,
            _ => &self.follow,
        };
        let mut request = reqwest::Request::try_from(request)?;
        if let Some(timeout) = timeout {
            *request.timeout_mut() = Some(timeout);
        }
        let mut response = client.execute(request).await?;
        let mut answer = http::Response::builder()
            .status(response.status())
            .version(response.version());
        for (name, value) in response.headers() {
            answer = answer.header(name, value);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if body.len() + chunk.len() > MAX_BODY {
                return Err(format!(
                    "the answer from {} is over {MAX_BODY} bytes",
                    response.url()
                )
                .into());
            }
            body.extend_from_slice(&chunk);
        }
        Ok(answer.body(body)?)
    }

    /// GETs `url` and reads its JSON.
    pub(crate) async fn get_json(
        &self,
        url: &str,
    ) -> Result<serde_json::Value, String> {
        let request = http::Request::get(url)
            .header("accept", "application/json")
            .body(Vec::new())
            .map_err(|error| format!("cannot ask {url}: {error}"))?;
        let response = self
            .send(request, OAuthHttpRedirectPolicy::Stop, None)
            .await
            .map_err(|error| format!("cannot reach {url}: {error}"))?;
        if !response.status().is_success() {
            return Err(format!("{url} answered {}", response.status()));
        }
        serde_json::from_slice(response.body())
            .map_err(|error| format!("{url} did not answer JSON: {error}"))
    }
}

impl OAuthHttpClient for OAuthHttp {
    fn execute(&self, request: OAuthHttpRequest) -> OAuthHttpClientFuture<'_> {
        Box::pin(async move {
            let OAuthHttpRequest {
                request,
                redirect_policy,
                timeout,
                ..
            } = request;
            self.send(request, redirect_policy, timeout).await
        })
    }
}
