//! The HTTP client for TypeSafe's System One endpoint.

use std::{fmt, sync::Arc, time::Duration};

use async_trait::async_trait;
use serde_json::json;
use tau_ai::retry::{RetryPolicy, jitter};
use tokio_rustls::rustls::{ClientConfig, RootCertStore, crypto::ring};

use crate::{DEFAULT_MODEL, Jev, JevError, Request, Response, SYSTEM_ONE_URL};

/// The environment variable [`TypeSafe::from_env`] reads.
pub const API_KEY_VAR: &str = "TYPESAFE_API_KEY";

/// How long one attempt may take, connecting included. Jev answers in a
/// fraction of a second, so a request this slow is not coming back.
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(30);

/// The key in `value`, trimmed, unless it is missing or blank.
pub fn api_key(value: Option<String>) -> Result<String, MissingApiKey> {
    value
        .map(|key| key.trim().to_owned())
        .filter(|key| !key.is_empty())
        .ok_or(MissingApiKey)
}

/// `TYPESAFE_API_KEY` is not set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MissingApiKey;

impl fmt::Display for MissingApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{API_KEY_VAR} is not set")
    }
}

impl std::error::Error for MissingApiKey {}

/// Jev over HTTPS. Clones share one connection pool.
///
/// Status 429, 503 and 529 are retried under the client's
/// [`RetryPolicy`], honoring `retry-after`; other failures are not.
/// TLS is rustls with the `ring` provider and Mozilla's roots, as for
/// OpenAI (`tau_ai::ws::io::tls`): no system certificate store.
#[derive(Clone)]
pub struct TypeSafe {
    http: reqwest::Client,
    api_key: Arc<str>,
    url: String,
    model: String,
    retry: RetryPolicy,
}

impl fmt::Debug for TypeSafe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never print the key.
        f.debug_struct("TypeSafe")
            .field("url", &self.url)
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl TypeSafe {
    pub fn new(api_key: impl Into<String>) -> Self {
        let roots = RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        let tls = ClientConfig::builder_with_provider(Arc::new(
            ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("ring supports the default protocol versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
        let http = reqwest::Client::builder()
            .tls_backend_preconfigured(tls)
            .timeout(ATTEMPT_TIMEOUT)
            .build()
            .expect("a static client configuration builds");
        Self {
            http,
            api_key: api_key.into().into(),
            url: SYSTEM_ONE_URL.to_owned(),
            model: DEFAULT_MODEL.to_owned(),
            retry: RetryPolicy::default(),
        }
    }

    /// A client that reads its key from `TYPESAFE_API_KEY`.
    pub fn from_env() -> Result<Self, MissingApiKey> {
        api_key(std::env::var(API_KEY_VAR).ok()).map(Self::new)
    }

    /// The model to ask; [`DEFAULT_MODEL`] by default.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// The endpoint; [`SYSTEM_ONE_URL`] by default. For a proxy, or a
    /// local server in tests.
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    pub fn retry(mut self, policy: RetryPolicy) -> Self {
        self.retry = policy;
        self
    }

    /// One attempt. An error with a delay may be retried, after that
    /// delay at the earliest (the server's `retry-after`). Transport
    /// errors leave the URL out, since it could be private.
    async fn attempt(
        &self,
        body: &str,
    ) -> Result<Response, (JevError, Option<Duration>)> {
        let response = self
            .http
            .post(&self.url)
            .bearer_auth(&*self.api_key)
            .header("content-type", "application/json")
            .body(body.to_owned())
            .send()
            .await
            .map_err(|error| {
                (JevError::Transport(error.without_url().to_string()), None)
            })?;
        let status = response.status().as_u16();
        if !response.status().is_success() {
            let retry_after = matches!(status, 429 | 503 | 529).then(|| {
                response
                    .headers()
                    .get("retry-after")
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.trim().parse::<u64>().ok())
                    .map_or(Duration::ZERO, Duration::from_secs)
            });
            return Err((JevError::Status(status), retry_after));
        }
        let text = response.text().await.map_err(|error| {
            (JevError::Transport(error.without_url().to_string()), None)
        })?;
        serde_json::from_str(&text)
            .map_err(|error| (JevError::Malformed(error.to_string()), None))
    }
}

#[async_trait]
impl Jev for TypeSafe {
    async fn ask(&self, request: &Request) -> Result<Response, JevError> {
        let body = json!({
            "model": self.model,
            "state": request.state,
            "questions": request.questions,
        })
        .to_string();
        let mut attempts = 1;
        loop {
            match self.attempt(&body).await {
                Ok(response) => return Ok(response),
                Err((_, Some(retry_after))) if self.retry.allows(attempts) => {
                    let delay =
                        self.retry.delay(attempts, jitter()).max(retry_after);
                    attempts += 1;
                    tokio::time::sleep(delay).await;
                }
                Err((error, _)) => return Err(error),
            }
        }
    }
}
