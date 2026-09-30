//! Opens Responses WebSockets on `api.openai.com` with a ChatGPT sign-in:
//! the account's access token is the bearer token of the upgrade,
//! refreshed before each connection. Clones of the
//! [`ChatGpt`] share one refresh at a time, and its lock file makes other
//! processes wait their turn too.

use std::{
    fmt,
    io,
    sync::{Arc, RwLock},
};

use tokio_tungstenite::tungstenite::{
    client::IntoClientRequest,
    http::{self, HeaderValue, header},
};
use url::Url;

use super::{AccountId, ChatGpt};
use crate::{
    http::{Dialer, Tls, user_agent},
    ws::io::connection::Connector,
};

/// The upgrade request to `url` with `access` as the bearer token.
pub fn websocket_request(url: &str, access: &str) -> http::Request<()> {
    let mut request = url
        .into_client_request()
        .unwrap_or_else(|_| http::Request::new(()));
    let headers = request.headers_mut();
    let mut bearer = HeaderValue::from_str(&format!("Bearer {access}"))
        .unwrap_or_else(|_| HeaderValue::from_static("Bearer invalid"));
    bearer.set_sensitive(true);
    headers.insert(header::AUTHORIZATION, bearer);
    if let Ok(agent) = HeaderValue::from_str(&user_agent()) {
        headers.insert(header::USER_AGENT, agent);
    }
    request
}

/// Connects one account's Responses WebSockets. Inference needs plan
/// usage: a sign-in without it fails to connect with
/// [`super::ChatGptError::PlanUsageDisabled`].
pub struct ChatGptConnector<D: Dialer = Tls> {
    chatgpt: ChatGpt<D>,
    account: AccountId,
    /// The token the next upgrade request sends.
    access: Arc<RwLock<String>>,
}

impl<D: Dialer> fmt::Debug for ChatGptConnector<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatGptConnector")
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

impl<D: Dialer> ChatGptConnector<D> {
    pub fn new(chatgpt: ChatGpt<D>, account: AccountId) -> Self {
        Self {
            chatgpt,
            account,
            access: Arc::new(RwLock::new(String::new())),
        }
    }
}

impl<D: Dialer> Connector for ChatGptConnector<D> {
    type Stream = D::Stream;

    async fn connect(&self) -> io::Result<Self::Stream> {
        let token = self.chatgpt.inference_token(&self.account).await?;
        *self.access.write().expect("not poisoned") = token;
        let url = Url::parse(&self.chatgpt.config().websocket_url).map_err(
            |error| io::Error::new(io::ErrorKind::InvalidInput, error),
        )?;
        self.chatgpt.dialer().dial(&url).await
    }

    fn request(&self) -> http::Request<()> {
        let access = self.access.read().expect("not poisoned").clone();
        websocket_request(&self.chatgpt.config().websocket_url, &access)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_upgrade_carries_the_bearer_token() {
        let request =
            websocket_request("wss://api.openai.com/v1/responses", "tok");
        assert_eq!(request.uri(), "wss://api.openai.com/v1/responses");
        assert_eq!(request.headers()["authorization"], "Bearer tok");
        assert!(request.headers()["authorization"].is_sensitive());
    }
}
