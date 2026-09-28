//! Connects to the Codex endpoint with a ChatGPT sign-in.

use std::io;

use tokio::net::TcpStream;
use tokio_rustls::{
    TlsConnector,
    client::TlsStream,
    rustls::pki_types::ServerName,
};
use tokio_tungstenite::tungstenite::{
    client::IntoClientRequest,
    http::{self, HeaderName, HeaderValue, header},
};

use super::{
    CODEX_HOST,
    CODEX_URL,
    CodexAuth,
    ORIGINATOR,
    WEBSOCKETS_BETA,
    oauth,
    user_agent,
};
use crate::ws::io::{connection::Connector, tls::tls_connector};

/// Opens Codex connections, refreshing the sign-in first when it is
/// about to expire.
#[derive(Clone)]
pub struct CodexConnector {
    auth: CodexAuth,
    tls: TlsConnector,
}

impl std::fmt::Debug for CodexConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodexConnector").finish_non_exhaustive()
    }
}

impl CodexConnector {
    pub fn new(auth: CodexAuth) -> Self {
        Self {
            auth,
            tls: tls_connector(),
        }
    }
}

impl Connector for CodexConnector {
    type Stream = TlsStream<TcpStream>;

    async fn connect(&self) -> io::Result<Self::Stream> {
        // `request` runs right after this, with the refreshed token.
        self.auth.ensure_fresh().await?;
        let tcp = TcpStream::connect((CODEX_HOST, 443)).await?;
        tcp.set_nodelay(true)?;
        let name = ServerName::try_from(CODEX_HOST)
            .expect("a static, valid host name");
        self.tls.connect(name, tcp).await
    }

    fn request(&self) -> http::Request<()> {
        let (access, account) = self.auth.current();
        codex_request(&access, &account, &oauth::random_hex())
    }
}

/// The upgrade request for one connection. `request_id` names the
/// connection to the server, as pi's `session-id` does.
pub(crate) fn codex_request(
    access: &str,
    account: &str,
    request_id: &str,
) -> http::Request<()> {
    let mut request = CODEX_URL
        .into_client_request()
        .expect("a static, valid URL");
    let headers = request.headers_mut();
    let value = |text: &str| {
        HeaderValue::from_str(text)
            .unwrap_or_else(|_| HeaderValue::from_static("invalid"))
    };
    let mut bearer = value(&format!("Bearer {access}"));
    bearer.set_sensitive(true);
    headers.insert(header::AUTHORIZATION, bearer);
    let named = |name: &'static str| HeaderName::from_static(name);
    headers.insert(named("chatgpt-account-id"), value(account));
    headers.insert(named("originator"), HeaderValue::from_static(ORIGINATOR));
    headers.insert(header::USER_AGENT, value(&user_agent()));
    headers.insert(
        named("openai-beta"),
        HeaderValue::from_static(WEBSOCKETS_BETA),
    );
    headers.insert(named("session-id"), value(request_id));
    headers.insert(named("x-client-request-id"), value(request_id));
    request
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_upgrade_request_carries_the_codex_headers() {
        let request = codex_request("tok", "acct-1", "req-1");
        assert_eq!(request.uri(), CODEX_URL);
        let header = |name: &str| {
            request.headers().get(name).and_then(|v| v.to_str().ok())
        };
        assert_eq!(header("authorization"), Some("Bearer tok"));
        assert!(request.headers()["authorization"].is_sensitive());
        assert_eq!(header("chatgpt-account-id"), Some("acct-1"));
        assert_eq!(header("originator"), Some("tau"));
        assert_eq!(header("openai-beta"), Some(WEBSOCKETS_BETA));
        assert_eq!(header("session-id"), Some("req-1"));
        assert_eq!(header("x-client-request-id"), Some("req-1"));
    }
}
