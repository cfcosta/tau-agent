//! The browser's way back: the loopback listener the authorization
//! server redirects to, PKCE's S256 challenge, and the checks on what
//! comes back.

use std::{fmt, io, net::SocketAddr};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

use crate::config::CallbackAddress;

/// PKCE's S256 challenge of `verifier` (RFC 7636 §4.2):
/// `BASE64URL(SHA256(verifier))`, unpadded.
pub fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Whether `verifier` is a PKCE code verifier (RFC 7636 §4.1): 43 to 128
/// unreserved characters, `[A-Za-z0-9-._~]`.
pub fn valid_verifier(verifier: &str) -> bool {
    (43..=128).contains(&verifier.len())
        && verifier.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'-' | b'.' | b'_' | b'~')
        })
}

/// What a valid callback gave.
#[derive(Clone, PartialEq, Eq)]
pub struct Callback {
    pub code: String,
    /// The authorization server's `iss` (RFC 9207), when it sent one.
    pub issuer: Option<String>,
}

impl fmt::Debug for Callback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The code is a credential until it is exchanged.
        f.debug_struct("Callback")
            .field("issuer", &self.issuer)
            .finish_non_exhaustive()
    }
}

/// Why a request to the callback is not this sign-in's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallbackError {
    /// Not the callback's path.
    WrongPath,
    /// Its `state` is missing or not this attempt's: not an answer to
    /// this sign-in.
    StateMismatch,
    /// The authorization server said no: `error` and its description.
    Denied {
        error: String,
        description: Option<String>,
    },
    /// No `code`.
    NoCode,
}

impl fmt::Display for CallbackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongPath => f.write_str("not the sign-in's callback"),
            Self::StateMismatch => f.write_str(
                "the answer is not for this sign-in: its state does not match",
            ),
            Self::Denied {
                error,
                description: Some(description),
            } => write!(f, "the sign-in was refused ({error}): {description}"),
            Self::Denied { error, .. } => {
                write!(f, "the sign-in was refused ({error})")
            }
            Self::NoCode => f.write_str("the answer carries no code"),
        }
    }
}

impl std::error::Error for CallbackError {}

/// Checks a request target that came to the callback, in order: the
/// path, the `state` (even on an error), an OAuth error, the code.
pub fn read_callback(
    target: &str,
    path: &str,
    state: &str,
) -> Result<Callback, CallbackError> {
    let (at, query) = target.split_once('?').unwrap_or((target, ""));
    if at != path {
        return Err(CallbackError::WrongPath);
    }
    let query = query.split('#').next().unwrap_or("");
    let pairs: Vec<(String, String)> =
        url::form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();
    let get = |name: &str| {
        let mut found = pairs.iter().filter(|(key, _)| key == name);
        match (found.next(), found.next()) {
            // A parameter given twice is not trusted.
            (Some((_, value)), None) => Some(value.as_str()),
            _ => None,
        }
    };
    if get("state") != Some(state) || state.is_empty() {
        return Err(CallbackError::StateMismatch);
    }
    if let Some(error) = get("error") {
        return Err(CallbackError::Denied {
            error: error.to_owned(),
            description: get("error_description").map(str::to_owned),
        });
    }
    let code = get("code")
        .filter(|code| !code.is_empty())
        .ok_or(CallbackError::NoCode)?;
    Ok(Callback {
        code: code.to_owned(),
        issuer: get("iss").map(str::to_owned),
    })
}

/// The loopback listener the browser comes back to. Bind it before the
/// authorization URL is made: the URL names its port.
#[derive(Debug)]
pub struct Loopback {
    listener: TcpListener,
    address: CallbackAddress,
    port: u16,
}

impl Loopback {
    /// Listens where `address` says; port 0 is any free port.
    pub async fn bind(address: &CallbackAddress) -> io::Result<Self> {
        let listener =
            TcpListener::bind(SocketAddr::new(address.ip, address.port))
                .await?;
        let port = listener.local_addr()?.port();
        Ok(Self {
            listener,
            address: address.clone(),
            port,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn redirect_uri(&self) -> String {
        self.address.redirect_uri(self.port)
    }

    /// Waits for the browser to come back with `state`'s answer, tells
    /// the browser how it went, and returns the checked callback.
    /// Requests for other paths, and answers for another state, get an
    /// error page and the wait goes on: they are not this sign-in's.
    pub async fn wait(&self, state: &str) -> Result<Callback, CallbackError> {
        loop {
            let Ok((mut socket, _)) = self.listener.accept().await else {
                continue;
            };
            let Some(target) = read_target(&mut socket).await else {
                continue;
            };
            match read_callback(&target, &self.address.path, state) {
                Err(CallbackError::WrongPath) => {
                    let _ =
                        reply(&mut socket, "404 Not Found", "Not found.").await;
                }
                Err(error @ CallbackError::StateMismatch) => {
                    let _ = reply(
                        &mut socket,
                        "400 Bad Request",
                        &error.to_string(),
                    )
                    .await;
                }
                outcome => {
                    let (status, page) = match &outcome {
                        Ok(_) => (
                            "200 OK",
                            "Signed in. You can close this window and go back \
                             to tau."
                                .to_owned(),
                        ),
                        Err(error) => ("400 Bad Request", error.to_string()),
                    };
                    let _ = reply(&mut socket, status, &page).await;
                    return outcome;
                }
            }
        }
    }
}

/// The target of a `GET` request, once its head is in.
async fn read_target(socket: &mut TcpStream) -> Option<String> {
    let mut buffer = Vec::new();
    let mut piece = [0; 4096];
    while !buffer.windows(4).any(|w| w == b"\r\n\r\n") && buffer.len() < 16384 {
        let read = socket.read(&mut piece).await.ok()?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&piece[..read]);
    }
    let line = buffer.split(|&b| b == b'\n').next()?;
    let line = String::from_utf8_lossy(line);
    let mut parts = line.split_whitespace();
    (parts.next()? == "GET").then_some(())?;
    parts.next().map(str::to_owned)
}

async fn reply(
    socket: &mut TcpStream,
    status: &str,
    page: &str,
) -> io::Result<()> {
    let escaped = page
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>tau</title><p>{escaped}</p>"
    );
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    socket.write_all(response.as_bytes()).await?;
    socket.shutdown().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_challenge_is_the_rfc_7636_example() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
}
