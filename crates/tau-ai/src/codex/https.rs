//! The token requests of the Codex sign-in, over [`crate::http`].

use std::io;

use crate::http::{Request, Tls, send};
pub use crate::http::{Response, parse};

/// POSTs `body` to `https://{host}{path}`.
pub async fn post(
    host: &str,
    path: &str,
    content_type: &str,
    body: &[u8],
) -> io::Result<Response> {
    let url = url::Url::parse(&format!("https://{host}{path}"))
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let request = Request {
        method: "POST",
        url,
        headers: vec![
            ("Accept".into(), "application/json".into()),
            ("Content-Type".into(), content_type.into()),
        ],
        body: body.to_vec(),
    };
    send(&Tls, &request).await
}
