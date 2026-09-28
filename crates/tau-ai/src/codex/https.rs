//! Just enough HTTPS for the OAuth token endpoints: one POST per
//! connection, the whole response read, chunked bodies decoded.
//!
//! The client already speaks TLS for its WebSocket; a general HTTP client
//! would bring a second stack for three small requests.

use std::io;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::rustls::pki_types::ServerName;

use crate::ws::io::tls::tls_connector;

/// A response: its status and body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Response {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// POSTs `body` to `https://{host}{path}`.
pub async fn post(
    host: &str,
    path: &str,
    content_type: &str,
    body: &[u8],
) -> io::Result<Response> {
    let tcp = tokio::net::TcpStream::connect((host, 443)).await?;
    let name = ServerName::try_from(host.to_owned())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let mut stream = tls_connector().connect(name, tcp).await?;
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: {}\r\n\
         Accept: application/json\r\nContent-Type: {content_type}\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        super::user_agent(),
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await?;
    let mut raw = Vec::new();
    // Servers may close without a TLS close_notify; what arrived counts.
    match stream.read_to_end(&mut raw).await {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {}
        Err(error) => return Err(error),
    }
    parse(&raw)
}

/// Parses a whole HTTP/1.1 response.
pub fn parse(raw: &[u8]) -> io::Result<Response> {
    let invalid =
        |why: &str| io::Error::new(io::ErrorKind::InvalidData, why.to_owned());
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut response = httparse::Response::new(&mut headers);
    let httparse::Status::Complete(start) = response
        .parse(raw)
        .map_err(|error| invalid(&error.to_string()))?
    else {
        return Err(invalid("incomplete response head"));
    };
    let status = response.code.ok_or_else(|| invalid("no status code"))?;
    let chunked = response.headers.iter().any(|header| {
        header.name.eq_ignore_ascii_case("transfer-encoding")
            && String::from_utf8_lossy(header.value)
                .to_ascii_lowercase()
                .contains("chunked")
    });
    let rest = &raw[start..];
    let body = if chunked {
        dechunk(rest).ok_or_else(|| invalid("malformed chunked body"))?
    } else {
        rest.to_vec()
    };
    Ok(Response { status, body })
}

fn dechunk(mut raw: &[u8]) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    loop {
        let line_end = raw.windows(2).position(|w| w == b"\r\n")?;
        let size_text = std::str::from_utf8(&raw[..line_end]).ok()?;
        // Chunk extensions follow a `;`.
        let size_text = size_text.split(';').next()?.trim();
        let size = usize::from_str_radix(size_text, 16).ok()?;
        raw = &raw[line_end + 2..];
        if size == 0 {
            return Some(body);
        }
        body.extend_from_slice(raw.get(..size)?);
        raw = raw.get(size + 2..)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_body_is_read_as_is() {
        let response =
            parse(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}").unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"{}");
    }

    #[test]
    fn a_chunked_body_is_joined() {
        let raw =
            b"HTTP/1.1 400 Bad Request\r\nTransfer-Encoding: chunked\r\n\r\n\
                    3\r\n{\"a\r\n5;x=y\r\n\":1}\n\r\n0\r\n\r\n";
        let response = parse(raw).unwrap();
        assert_eq!(response.status, 400);
        assert!(!response.is_success());
        assert_eq!(response.text(), "{\"a\":1}\n");
    }

    #[test]
    fn a_cut_response_is_an_error() {
        assert!(parse(b"HTTP/1.1 200").is_err());
        assert!(
            parse(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nZZ\r\n"
            )
            .is_err()
        );
    }
}
