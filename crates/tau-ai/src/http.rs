//! Just enough HTTP/1.1 for OAuth and a few JSON endpoints: one request
//! per connection, `Connection: close`, the body read whole or streamed
//! (for server-sent events), chunked bodies decoded.
//!
//! The WebSocket already needs TLS; a general HTTP client would bring a
//! second stack for a handful of small requests. The byte stream comes
//! from a [`Dialer`]: [`Tls`] reaches the real hosts (the WebSocket
//! dials through it too), tests dial a simulated network.

use std::{fmt, future::Future, io, sync::Arc};

use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
};
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
use url::Url;

/// What tau sends as `User-Agent`.
pub fn user_agent() -> String {
    format!("tau/{}", env!("CARGO_PKG_VERSION"))
}

/// Opens the byte stream a request to `url` runs over.
pub trait Dialer: Send + Sync + 'static {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    fn dial(
        &self,
        url: &Url,
    ) -> impl Future<Output = io::Result<Self::Stream>> + Send;
}

/// The real network: TCP, then TLS with rustls and Mozilla's roots. Only
/// `https` URLs.
#[derive(Debug, Clone, Copy, Default)]
pub struct Tls;

impl Dialer for Tls {
    type Stream = TlsStream<TcpStream>;

    async fn dial(&self, url: &Url) -> io::Result<Self::Stream> {
        let invalid = |why: &str| {
            io::Error::new(io::ErrorKind::InvalidInput, format!("{why}: {url}"))
        };
        if url.scheme() != "https" && url.scheme() != "wss" {
            return Err(invalid("not an https URL"));
        }
        let host = url.host_str().ok_or_else(|| invalid("no host"))?;
        let port = url.port_or_known_default().unwrap_or(443);
        let tcp = TcpStream::connect((host, port)).await?;
        tcp.set_nodelay(true)?;
        let name = ServerName::try_from(host.to_owned()).map_err(|error| {
            io::Error::new(io::ErrorKind::InvalidInput, error)
        })?;
        tls_connector().connect(name, tcp).await
    }
}

/// tau's TLS for every client it opens: rustls, the `ring` provider and
/// Mozilla's roots from `webpki-roots`, with no system certificate
/// store and no native crypto build.
pub fn tls_config() -> ClientConfig {
    let roots = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    ClientConfig::builder_with_provider(Arc::new(ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("ring supports the default protocol versions")
        .with_root_certificates(roots)
        .with_no_client_auth()
}

fn tls_connector() -> TlsConnector {
    TlsConnector::from(Arc::new(tls_config()))
}

/// A request to send.
#[derive(Clone)]
pub struct Request {
    pub method: &'static str,
    pub url: Url,
    /// Beyond `Host`, `User-Agent`, `Content-Length` and `Connection`,
    /// which are always sent.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Headers and bodies carry tokens: only the target.
        f.debug_struct("Request")
            .field("method", &self.method)
            .field("url", &self.url.as_str())
            .finish_non_exhaustive()
    }
}

impl Request {
    pub fn get(url: Url) -> Self {
        Self {
            method: "GET",
            url,
            headers: vec![("Accept".into(), "application/json".into())],
            body: Vec::new(),
        }
    }

    /// A form-encoded POST of `pairs`.
    pub fn post_form(url: Url, pairs: &[(&str, &str)]) -> Self {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(pairs)
            .finish();
        Self {
            method: "POST",
            url,
            headers: vec![
                ("Accept".into(), "application/json".into()),
                (
                    "Content-Type".into(),
                    "application/x-www-form-urlencoded".into(),
                ),
            ],
            body: body.into_bytes(),
        }
    }

    /// A JSON POST.
    pub fn post_json(url: Url, body: &serde_json::Value) -> Self {
        Self {
            method: "POST",
            url,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: body.to_string().into_bytes(),
        }
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    pub fn bearer(self, token: &str) -> Self {
        self.header("Authorization", &format!("Bearer {token}"))
    }

    fn head(&self) -> String {
        let host = match (self.url.host_str(), self.url.port()) {
            (Some(host), Some(port)) => format!("{host}:{port}"),
            (Some(host), None) => host.to_owned(),
            (None, _) => String::new(),
        };
        let target = match self.url.query() {
            Some(query) => format!("{}?{query}", self.url.path()),
            None => self.url.path().to_owned(),
        };
        let mut head = format!(
            "{} {target} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: {}\r\n",
            self.method,
            user_agent()
        );
        for (name, value) in &self.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        if self.method != "GET" || !self.body.is_empty() {
            head.push_str(&format!("Content-Length: {}\r\n", self.body.len()));
        }
        head.push_str("Connection: close\r\n\r\n");
        head
    }
}

/// A whole response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    /// In arrival order, names as sent.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// The first header named `name`, ignoring case.
    pub fn header(&self, name: &str) -> Option<&str> {
        header(&self.headers, name)
    }

    /// OpenAI's `x-request-id`, which support asks for.
    pub fn request_id(&self) -> Option<&str> {
        self.header("x-request-id")
    }
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

/// Sends `request` and reads the whole response.
pub async fn send<D: Dialer>(
    dialer: &D,
    request: &Request,
) -> io::Result<Response> {
    let mut streaming = open(dialer, request).await?;
    let mut body = Vec::new();
    while let Some(chunk) = streaming.chunk().await? {
        body.extend_from_slice(&chunk);
    }
    Ok(Response {
        status: streaming.status,
        headers: streaming.headers,
        body,
    })
}

/// Sends `request` and returns once the response head has arrived; the
/// body is read with [`Streaming::chunk`].
pub async fn open<D: Dialer>(
    dialer: &D,
    request: &Request,
) -> io::Result<Streaming<D::Stream>> {
    let mut stream = dialer.dial(&request.url).await?;
    stream.write_all(request.head().as_bytes()).await?;
    stream.write_all(&request.body).await?;
    stream.flush().await?;
    let mut buffer = Vec::new();
    let (status, headers, start) = loop {
        if let Some(head) = parse_head(&buffer)? {
            break head;
        }
        if read_more(&mut stream, &mut buffer).await? == 0 {
            return Err(invalid("the connection closed before the head"));
        }
    };
    buffer.drain(..start);
    let framing = framing(&headers);
    Ok(Streaming {
        status,
        headers,
        stream,
        buffer,
        framing,
    })
}

/// A response whose body is still arriving.
pub struct Streaming<S> {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    stream: S,
    /// Bytes read but not yet returned.
    buffer: Vec<u8>,
    framing: Framing,
}

impl<S> fmt::Debug for Streaming<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Streaming")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Framing {
    Length(usize),
    /// In a chunk with this many bytes left, or between chunks.
    Chunked {
        left: usize,
        done: bool,
    },
    UntilClose {
        done: bool,
    },
}

fn framing(headers: &[(String, String)]) -> Framing {
    if header(headers, "transfer-encoding")
        .is_some_and(|value| value.to_ascii_lowercase().contains("chunked"))
    {
        return Framing::Chunked {
            left: 0,
            done: false,
        };
    }
    match header(headers, "content-length").and_then(|v| v.trim().parse().ok())
    {
        Some(length) => Framing::Length(length),
        None => Framing::UntilClose { done: false },
    }
}

impl<S: AsyncRead + Unpin> Streaming<S> {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        header(&self.headers, name)
    }

    pub fn request_id(&self) -> Option<&str> {
        self.header("x-request-id")
    }

    /// The next piece of the body, `None` at its end.
    pub async fn chunk(&mut self) -> io::Result<Option<Vec<u8>>> {
        loop {
            match self.framing {
                Framing::Length(0)
                | Framing::Chunked { done: true, .. }
                | Framing::UntilClose { done: true } => return Ok(None),
                Framing::Length(left) => {
                    if self.buffer.is_empty()
                        && read_more(&mut self.stream, &mut self.buffer).await?
                            == 0
                    {
                        return Err(invalid("the body was cut short"));
                    }
                    let take = left.min(self.buffer.len());
                    self.framing = Framing::Length(left - take);
                    return Ok(Some(self.buffer.drain(..take).collect()));
                }
                Framing::UntilClose { .. } => {
                    if !self.buffer.is_empty() {
                        return Ok(Some(std::mem::take(&mut self.buffer)));
                    }
                    if read_more(&mut self.stream, &mut self.buffer).await? == 0
                    {
                        self.framing = Framing::UntilClose { done: true };
                    }
                }
                Framing::Chunked { left: 0, .. } => {
                    // A size line.
                    if let Some(end) =
                        self.buffer.windows(2).position(|w| w == b"\r\n")
                    {
                        let line = &self.buffer[..end];
                        let size = std::str::from_utf8(line)
                            .ok()
                            .and_then(|line| line.split(';').next())
                            .and_then(|size| {
                                usize::from_str_radix(size.trim(), 16).ok()
                            })
                            .ok_or_else(|| invalid("malformed chunked body"))?;
                        self.buffer.drain(..end + 2);
                        self.framing = Framing::Chunked {
                            left: size,
                            done: size == 0,
                        };
                        continue;
                    }
                    if read_more(&mut self.stream, &mut self.buffer).await? == 0
                    {
                        return Err(invalid("malformed chunked body"));
                    }
                }
                Framing::Chunked { left, .. } => {
                    if self.buffer.is_empty()
                        && read_more(&mut self.stream, &mut self.buffer).await?
                            == 0
                    {
                        return Err(invalid("malformed chunked body"));
                    }
                    let take = left.min(self.buffer.len());
                    self.framing = Framing::Chunked {
                        left: left - take,
                        done: false,
                    };
                    let piece: Vec<u8> = self.buffer.drain(..take).collect();
                    if left - take == 0 {
                        // The CRLF after the data must come next.
                        while self.buffer.len() < 2 {
                            if read_more(&mut self.stream, &mut self.buffer)
                                .await?
                                == 0
                            {
                                return Err(invalid("malformed chunked body"));
                            }
                        }
                        if !self.buffer.starts_with(b"\r\n") {
                            return Err(invalid("malformed chunked body"));
                        }
                        self.buffer.drain(..2);
                    }
                    return Ok(Some(piece));
                }
            }
        }
    }
}

/// Reads what is available into `buffer`; 0 at the end of the stream.
/// A TLS peer that closes without `close_notify` ends the stream too.
async fn read_more<S: AsyncRead + Unpin>(
    stream: &mut S,
    buffer: &mut Vec<u8>,
) -> io::Result<usize> {
    let mut piece = [0; 8192];
    match stream.read(&mut piece).await {
        Ok(read) => {
            buffer.extend_from_slice(&piece[..read]);
            Ok(read)
        }
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(0),
        Err(error) => Err(error),
    }
}

fn invalid(why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why.to_owned())
}

type Head = (u16, Vec<(String, String)>, usize);

fn parse_head(raw: &[u8]) -> io::Result<Option<Head>> {
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut response = httparse::Response::new(&mut headers);
    match response
        .parse(raw)
        .map_err(|error| invalid(&error.to_string()))?
    {
        httparse::Status::Partial => Ok(None),
        httparse::Status::Complete(start) => {
            let status =
                response.code.ok_or_else(|| invalid("no status code"))?;
            let headers = response
                .headers
                .iter()
                .map(|header| {
                    (
                        header.name.to_owned(),
                        String::from_utf8_lossy(header.value).into_owned(),
                    )
                })
                .collect();
            Ok(Some((status, headers, start)))
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]
mod tests {
    use std::{
        pin::Pin,
        task::{Context, Poll},
    };

    use tokio::io::ReadBuf;

    use super::*;

    /// Parses a whole HTTP/1.1 response: the oracle for [`Streaming`].
    fn parse(raw: &[u8]) -> io::Result<Response> {
        let (status, headers, start) = parse_head(raw)?
            .ok_or_else(|| invalid("incomplete response head"))?;
        let rest = &raw[start..];
        let body = match framing(&headers) {
            Framing::Chunked { .. } => dechunk(rest)
                .ok_or_else(|| invalid("malformed chunked body"))?,
            Framing::Length(length) => rest[..length.min(rest.len())].to_vec(),
            Framing::UntilClose { .. } => rest.to_vec(),
        };
        Ok(Response {
            status,
            headers,
            body,
        })
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
    fn headers_are_kept_and_found_in_any_case() {
        let response =
            parse(b"HTTP/1.1 200 OK\r\nX-Request-Id: req_1\r\n\r\n").unwrap();
        assert_eq!(response.request_id(), Some("req_1"));
    }

    /// A stream that hands out its bytes `step` at a time.
    struct Trickle {
        data: Vec<u8>,
        at: usize,
        step: usize,
    }

    impl AsyncRead for Trickle {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            let end = (self.at + self.step).min(self.data.len());
            let piece = self.data[self.at..end].to_vec();
            buf.put_slice(&piece);
            self.at = end;
            Poll::Ready(Ok(()))
        }
    }

    async fn stream_all(raw: &[u8], step: usize) -> io::Result<Vec<u8>> {
        let mut stream = Trickle {
            data: raw.to_vec(),
            at: 0,
            step,
        };
        let mut buffer = Vec::new();
        let (status, headers, start) = loop {
            if let Some(head) = parse_head(&buffer)? {
                break head;
            }
            if read_more(&mut stream, &mut buffer).await? == 0 {
                return Err(invalid("cut"));
            }
        };
        buffer.drain(..start);
        let framing = framing(&headers);
        let mut streaming = Streaming {
            status,
            headers,
            stream,
            buffer,
            framing,
        };
        let mut body = Vec::new();
        while let Some(chunk) = streaming.chunk().await? {
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }

    /// A response as a server frames its body: by length, in chunks of
    /// the given sizes (some with an extension), or until the close.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Frame {
        Length,
        Chunked,
        UntilClose,
    }

    hegel::pretty_print_as_debug!(Frame);

    /// `(framing, raw response, body)`.
    #[hegel::composite]
    fn response(tc: &hegel::TestCase) -> (Frame, Vec<u8>, Vec<u8>) {
        use hegel::generators as gs;
        let body: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).max_size(60));
        let frame = tc.draw(gs::sampled_from(vec![
            Frame::Length,
            Frame::Chunked,
            Frame::UntilClose,
        ]));
        let mut raw = b"HTTP/1.1 200 OK\r\nX-Request-Id: req_1\r\n".to_vec();
        match frame {
            Frame::Length => {
                raw.extend(
                    format!("Content-Length: {}\r\n\r\n", body.len()).bytes(),
                );
                raw.extend(&body);
            }
            Frame::Chunked => {
                raw.extend(b"Transfer-Encoding: chunked\r\n\r\n");
                let mut rest = body.as_slice();
                while !rest.is_empty() {
                    let take = tc.draw(
                        gs::integers::<usize>()
                            .min_value(1)
                            .max_value(rest.len()),
                    );
                    let extension =
                        if tc.draw(gs::booleans()) { ";x=y" } else { "" };
                    raw.extend(format!("{:x}{extension}\r\n", take).bytes());
                    raw.extend(&rest[..take]);
                    raw.extend(b"\r\n");
                    rest = &rest[take..];
                }
                raw.extend(b"0\r\n\r\n");
            }
            Frame::UntilClose => {
                raw.extend(b"\r\n");
                raw.extend(&body);
            }
        }
        (frame, raw, body)
    }

    fn run<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(future)
    }

    /// However the bytes arrive, one at a time or a few, the stream
    /// yields the body the whole-response oracle finds.
    #[hegel::test(test_cases = 200)]
    fn streaming_matches_the_whole_parse_at_any_read_size(tc: hegel::TestCase) {
        use hegel::generators as gs;
        let (_, raw, body) = tc.draw(response());
        let step =
            tc.draw(gs::integers::<usize>().min_value(1).max_value(raw.len()));
        assert_eq!(parse(&raw).unwrap().body, body);
        assert_eq!(run(stream_all(&raw, step)).unwrap(), body);
    }

    /// A response cut short is an error, not a shorter body: anywhere
    /// inside a head, a length-framed body or a chunked one. (A body
    /// that ends when the connection does cannot be told from a whole
    /// one, so it is left out.)
    #[hegel::test(test_cases = 300)]
    fn a_cut_response_is_an_error(tc: hegel::TestCase) {
        use hegel::generators as gs;
        let (frame, raw, body) = tc.draw(response());
        tc.assume(frame != Frame::UntilClose);
        // A cut inside the final `0\r\n\r\n` of a chunked body may leave
        // a body that is already whole; every other cut loses data.
        let tail = if frame == Frame::Chunked { 5 } else { 0 };
        let keep =
            tc.draw(gs::integers::<usize>().max_value(raw.len() - tail - 1));
        let step =
            tc.draw(gs::integers::<usize>().min_value(1).max_value(raw.len()));
        match run(stream_all(&raw[..keep], step)) {
            Err(_) => {}
            Ok(got) => panic!(
                "{keep} of {} bytes gave {got:?}, not an error (body {body:?})",
                raw.len()
            ),
        }
    }
}
