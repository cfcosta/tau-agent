//! The real endpoint: `wss://api.openai.com/v1/responses` over TLS.
//!
//! TLS uses rustls with the `ring` provider and Mozilla's root
//! certificates from `webpki-roots`, so it needs no system certificate
//! store and no native crypto build.

use std::{fmt, io, sync::Arc};

use tokio::net::TcpStream;
use tokio_rustls::{
    TlsConnector,
    client::TlsStream,
    rustls::{
        ClientConfig,
        RootCertStore,
        crypto::ring,
        pki_types::ServerName,
    },
};
use tokio_tungstenite::tungstenite::{
    client::IntoClientRequest,
    http::{self, HeaderValue, header::AUTHORIZATION},
};

use super::connection::Connector;

/// The host of OpenAI's API.
pub const OPENAI_HOST: &str = "api.openai.com";

/// The Responses WebSocket endpoint.
pub const OPENAI_URL: &str = "wss://api.openai.com/v1/responses";

/// Connects to OpenAI with an API key.
#[derive(Clone)]
pub struct OpenAiConnector {
    api_key: String,
    tls: TlsConnector,
}

impl fmt::Debug for OpenAiConnector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never print the key.
        f.debug_struct("OpenAiConnector")
            .field("api_key", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl OpenAiConnector {
    pub fn new(api_key: impl Into<String>) -> Self {
        let roots = RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        let config = ClientConfig::builder_with_provider(Arc::new(
            ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("ring supports the default protocol versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
        Self {
            api_key: api_key.into(),
            tls: TlsConnector::from(Arc::new(config)),
        }
    }
}

impl Connector for OpenAiConnector {
    type Stream = TlsStream<TcpStream>;

    async fn connect(&self) -> io::Result<Self::Stream> {
        let tcp = TcpStream::connect((OPENAI_HOST, 443)).await?;
        // Requests are single small writes waiting on an answer: never
        // hold one back for Nagle's algorithm.
        tcp.set_nodelay(true)?;
        let name = ServerName::try_from(OPENAI_HOST)
            .expect("a static, valid host name");
        self.tls.connect(name, tcp).await
    }

    fn request(&self) -> http::Request<()> {
        let mut request = OPENAI_URL
            .into_client_request()
            .expect("a static, valid URL");
        let mut value =
            HeaderValue::from_str(&format!("Bearer {}", self.api_key))
                .unwrap_or_else(|_| HeaderValue::from_static("Bearer invalid"));
        value.set_sensitive(true);
        request.headers_mut().insert(AUTHORIZATION, value);
        request
    }
}
