//! An agent's streams (ADR 0037): server-sent events or a websocket
//! that the host keeps open while the agent's live version declares it.
//! Each frame goes to the trigger's `parse`, in Luau, which says what
//! event, if any, it is.
//!
//! The connection is the host's, never Luau's. It opens with the
//! trigger's headers, their secrets put in, and opens again whenever it
//! drops, waiting longer each time it fails ([`wait`]: 1 s, doubling, at
//! most 5 min, with jitter). Server-sent events send `Last-Event-ID` as
//! they open again, once the server gave an id. A frame longer than
//! [`MAX_FRAME`] is dropped; an event whose id `parse` gave among the
//! last [`RECENT`] does not fire again.

use std::{
    collections::{BTreeMap, VecDeque},
    time::Duration,
};

use base64::Engine;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tau_agents::{StreamVia, TriggerKind};
use tau_ai::http::Dialer;
use tau_tools_host::http;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{
        Message,
        client::IntoClientRequest,
        protocol::WebSocketConfig,
    },
};

/// The longest frame `parse` reads; a longer one is dropped.
pub const MAX_FRAME: usize = 256 * 1024;

/// How many of the ids `parse` gave a stream keeps, against repeats.
pub const RECENT: usize = 1000;

/// How long a stream waits to open again after its first failure.
pub const FIRST_WAIT: Duration = Duration::from_secs(1);

/// The longest a stream waits to open again.
pub const LONGEST_WAIT: Duration = Duration::from_secs(300);

/// How long opening a stream may take.
const OPEN_LIMIT: Duration = Duration::from_secs(30);

/// The longest websocket message read at all: past it, the connection
/// fails and opens again.
const MAX_MESSAGE: usize = 4 * MAX_FRAME;

/// What tau keeps of one stream across restarts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Streamed {
    /// What it reads: a stream that changes starts over.
    pub kind: TriggerKind,
    /// The id the server last gave, sent as `Last-Event-ID` when it
    /// opens again.
    #[serde(default)]
    pub last_id: Option<String>,
    /// The ids of the events it fired last, oldest first.
    #[serde(default)]
    pub recent: VecDeque<String>,
    /// The state `parse` keeps, from frame to frame.
    #[serde(default = "empty")]
    pub state: Value,
    /// Why it last failed: its connection, or `parse`.
    #[serde(default)]
    pub error: Option<String>,
}

fn empty() -> Value {
    json!({})
}

impl Streamed {
    /// A stream of `kind` not opened yet.
    pub fn new(kind: TriggerKind) -> Self {
        Self {
            kind,
            last_id: None,
            recent: VecDeque::new(),
            state: empty(),
            error: None,
        }
    }

    /// Whether `id` is new to it: if so, it is noted.
    pub fn fresh(&mut self, id: &str) -> bool {
        if self.recent.iter().any(|seen| seen == id) {
            return false;
        }
        self.recent.push_back(id.to_owned());
        while self.recent.len() > RECENT {
            self.recent.pop_front();
        }
        true
    }
}

/// How long a stream that failed `failures` times in a row waits before
/// it opens again: `first`, doubling with each failure, at most
/// `longest`, give or take a fifth by `jitter` (from 0 to 1).
pub fn wait(
    failures: u32,
    first: Duration,
    longest: Duration,
    jitter: f64,
) -> Duration {
    let doublings = failures.saturating_sub(1).min(30);
    let base = (first.as_secs_f64() * f64::from(1u32 << doublings))
        .min(longest.as_secs_f64());
    Duration::from_secs_f64(base * (0.8 + 0.4 * jitter.clamp(0.0, 1.0)))
        .min(longest)
}

/// A number from 0 to 1 that differs from one call to the next: enough
/// to keep streams that fail together from opening again together.
pub fn jitter() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.subsec_nanos());
    f64::from(nanos % 1000) / 1000.0
}

/// One frame of a stream, as `parse` reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// `{ event, data, id }` for server-sent events, `{ data }` (and
    /// `binary`, its data in base64) for a websocket message.
    pub value: Value,
    /// For server-sent events, the last id the server gave, as
    /// `Last-Event-ID` would send it.
    pub id: Option<String>,
}

/// What reading a stream gave.
#[derive(Debug, Clone, PartialEq)]
pub enum Read {
    Frame(Frame),
    /// A frame longer than [`MAX_FRAME`], dropped.
    TooLarge,
}

/// Reads `text/event-stream` as it comes, chunk by chunk, into frames:
/// the event's name (`message` when the server named none), its data
/// (its `data:` lines, joined) and the last id. Comments, a keep-alive
/// among them, and events with no data are no frame. Lines end in `\n`,
/// `\r\n` or `\r`. A `retry:` field sets how long to wait before
/// opening again ([`Self::retry`]).
#[derive(Debug, Default)]
pub struct Sse {
    last_id: Option<String>,
    /// How long the server asked to wait before opening again.
    retry: Option<Duration>,
    /// The last byte ended a line with `\r`: a `\n` right after it ends
    /// no other.
    after_cr: bool,
    line: Vec<u8>,
    /// The line going is longer than a frame may be: it is dropped up
    /// to its end.
    skipping: bool,
    data: String,
    event: String,
    /// The event going is longer than a frame may be.
    too_large: bool,
}

impl Sse {
    /// A reader whose last id, before the server gives one, is
    /// `last_id`.
    pub fn new(last_id: Option<String>) -> Self {
        Self {
            last_id,
            ..Self::default()
        }
    }

    /// How long the server last asked to wait before the stream opens
    /// again, in a `retry:` field.
    pub fn retry(&self) -> Option<Duration> {
        self.retry
    }

    /// The frames `bytes` finish.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Read> {
        let mut out = Vec::new();
        for &byte in bytes {
            let after_cr = std::mem::take(&mut self.after_cr);
            if byte == b'\n' && after_cr {
                continue;
            }
            if byte == b'\n' || byte == b'\r' {
                self.after_cr = byte == b'\r';
                let line = std::mem::take(&mut self.line);
                if std::mem::take(&mut self.skipping) {
                    self.too_large = true;
                } else {
                    self.line(&String::from_utf8_lossy(&line), &mut out);
                }
            } else if !self.skipping {
                self.line.push(byte);
                if self.line.len() > MAX_FRAME {
                    self.line.clear();
                    self.skipping = true;
                }
            }
        }
        out
    }

    fn line(&mut self, line: &str, out: &mut Vec<Read>) {
        if line.is_empty() {
            self.dispatch(out);
            return;
        }
        if line.starts_with(':') {
            return;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => {
                (field, value.strip_prefix(' ').unwrap_or(value))
            }
            None => (line, ""),
        };
        match field {
            "event" => value.clone_into(&mut self.event),
            "data" => {
                if self.data.len() + value.len() + 1 > MAX_FRAME {
                    self.too_large = true;
                } else {
                    self.data.push_str(value);
                    self.data.push('\n');
                }
            }
            "id" if !value.contains('\0') => {
                self.last_id = Some(value.to_owned());
            }
            "retry"
                if !value.is_empty()
                    && value.bytes().all(|b| b.is_ascii_digit()) =>
            {
                if let Ok(millis) = value.parse::<u64>() {
                    self.retry = Some(Duration::from_millis(millis));
                }
            }
            _ => {}
        }
    }

    fn dispatch(&mut self, out: &mut Vec<Read>) {
        let mut data = std::mem::take(&mut self.data);
        let event = std::mem::take(&mut self.event);
        if std::mem::take(&mut self.too_large) {
            out.push(Read::TooLarge);
            return;
        }
        if data.is_empty() {
            return;
        }
        data.pop();
        let name = if event.is_empty() {
            "message".to_owned()
        } else {
            event
        };
        let mut value = json!({ "event": name, "data": data });
        if let Some(id) = &self.last_id {
            value["id"] = json!(id);
        }
        out.push(Read::Frame(Frame {
            value,
            id: self.last_id.clone(),
        }));
    }
}

/// The byte stream a websocket runs over: TCP, or TLS over it.
trait Io: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

/// An open stream.
pub struct Connection(Opened);

enum Opened {
    Sse(Box<SseOpen>),
    Websocket(Box<WebSocketStream<Box<dyn Io>>>),
}

struct SseOpen {
    response: reqwest::Response,
    reader: Sse,
    read: VecDeque<Read>,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self.0 {
            Opened::Sse(_) => "Connection::Sse",
            Opened::Websocket(_) => "Connection::Websocket",
        })
    }
}

impl Connection {
    /// Opens the stream `kind` names, its headers' secrets from
    /// `secrets`; server-sent events send `last_id` as `Last-Event-ID`.
    /// Errors are not scrubbed: the caller scrubs them.
    pub async fn open(
        kind: &TriggerKind,
        secrets: &BTreeMap<String, String>,
        last_id: Option<String>,
    ) -> Result<Self, String> {
        let TriggerKind::Stream { url, via, headers } = kind else {
            return Err("this trigger is not a stream".into());
        };
        let headers = http::stream_headers(headers.as_ref(), secrets)?;
        let url = reqwest::Url::parse(url)
            .map_err(|error| format!("`{url}` is not a URL: {error}"))?;
        let opened = match via {
            StreamVia::Sse => {
                tokio::time::timeout(
                    OPEN_LIMIT,
                    open_sse(url, headers, last_id),
                )
                .await
            }
            StreamVia::Websocket => {
                tokio::time::timeout(OPEN_LIMIT, open_websocket(url, headers))
                    .await
            }
        };
        opened.map_err(|_| "the stream did not answer in time".to_owned())?
    }

    /// How long the server asked to wait before opening the stream again,
    /// if it did (server-sent events' `retry:`).
    pub fn retry(&self) -> Option<Duration> {
        match &self.0 {
            Opened::Sse(open) => open.reader.retry(),
            Opened::Websocket(_) => None,
        }
    }

    /// What comes next: a frame, one dropped, why the stream failed, or
    /// nothing when it closed.
    pub async fn next(&mut self) -> Option<Result<Read, String>> {
        match &mut self.0 {
            Opened::Sse(open) => loop {
                if let Some(next) = open.read.pop_front() {
                    return Some(Ok(next));
                }
                match open.response.chunk().await {
                    Ok(Some(bytes)) => {
                        let read = open.reader.feed(&bytes);
                        open.read.extend(read);
                    }
                    Ok(None) => return None,
                    Err(error) => return Some(Err(http::error_text(error))),
                }
            },
            Opened::Websocket(socket) => loop {
                let message = match socket.next().await? {
                    Ok(message) => message,
                    Err(error) => {
                        return Some(Err(format!(
                            "the websocket failed: {error}"
                        )));
                    }
                };
                let value = match message {
                    Message::Text(text) => {
                        if text.len() > MAX_FRAME {
                            return Some(Ok(Read::TooLarge));
                        }
                        json!({ "data": text.as_str() })
                    }
                    Message::Binary(bytes) => {
                        if bytes.len() > MAX_FRAME {
                            return Some(Ok(Read::TooLarge));
                        }
                        json!({
                            "data": base64::engine::general_purpose::STANDARD
                                .encode(&bytes),
                            "binary": true,
                        })
                    }
                    Message::Close(_) => return None,
                    // Pings are answered as the socket reads.
                    Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {
                        continue;
                    }
                };
                return Some(Ok(Read::Frame(Frame { value, id: None })));
            },
        }
    }
}

async fn open_sse(
    url: reqwest::Url,
    mut headers: reqwest::header::HeaderMap,
    last_id: Option<String>,
) -> Result<Connection, String> {
    use reqwest::header::{ACCEPT, CACHE_CONTROL, HeaderValue};
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!(
            "server-sent events come from an http or https URL, not `{}`",
            url.scheme()
        ));
    }
    headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    if let Some(id) = &last_id
        && let Ok(id) = HeaderValue::from_str(id)
    {
        headers.insert("last-event-id", id);
    }
    let response = http::client()
        .get(url)
        .headers(headers)
        .send()
        .await
        .map_err(http::error_text)?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("the stream answered {status}"));
    }
    Ok(Connection(Opened::Sse(Box::new(SseOpen {
        response,
        reader: Sse::new(last_id),
        read: VecDeque::new(),
    }))))
}

async fn open_websocket(
    url: reqwest::Url,
    headers: reqwest::header::HeaderMap,
) -> Result<Connection, String> {
    let secure = match url.scheme() {
        "wss" => true,
        "ws" => false,
        other => {
            return Err(format!(
                "a websocket is a ws or wss URL, not `{other}`"
            ));
        }
    };
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|error| format!("`{url}` cannot open a websocket: {error}"))?;
    for (name, value) in &headers {
        request.headers_mut().insert(name, value.clone());
    }
    let io: Box<dyn Io> = if secure {
        Box::new(
            tau_ai::http::Tls
                .dial(&url)
                .await
                .map_err(|error| format!("could not connect: {error}"))?,
        )
    } else {
        let host = url.host_str().ok_or("the URL names no host")?;
        let port = url.port_or_known_default().unwrap_or(80);
        Box::new(
            tokio::net::TcpStream::connect((host, port))
                .await
                .map_err(|error| format!("could not connect: {error}"))?,
        )
    };
    let config = WebSocketConfig::default().max_message_size(Some(MAX_MESSAGE));
    let (socket, _) =
        tokio_tungstenite::client_async_with_config(request, io, Some(config))
            .await
            .map_err(|error| format!("the websocket did not open: {error}"))?;
    Ok(Connection(Opened::Websocket(Box::new(socket))))
}

#[cfg(test)]
mod tests {
    use hegel::generators::{self as gs, Generator as _};

    use super::*;

    fn frames(reads: Vec<Read>) -> Vec<Value> {
        reads
            .into_iter()
            .map(|read| match read {
                Read::Frame(frame) => frame.value,
                Read::TooLarge => json!("too large"),
            })
            .collect()
    }

    /// One event as a server writes it: maybe a comment, a name, data
    /// lines (with or without the space after `data:`), an id and a
    /// `retry:` field, a number of milliseconds or not, each line ending
    /// in `\n`, `\r\n` or `\r`, then a blank line.
    #[derive(Debug, Clone)]
    struct Written {
        comment: bool,
        name: Option<String>,
        data: Vec<String>,
        spaced: bool,
        id: Option<String>,
        retry: Option<Retry>,
        ends: Vec<&'static str>,
    }

    /// A `retry:` field's value: milliseconds, or what is not.
    #[derive(Debug, Clone)]
    enum Retry {
        Millis(u64),
        Not(&'static str),
    }

    #[hegel::composite]
    fn written(tc: &hegel::TestCase) -> Written {
        let word = |pattern: &str| gs::from_regex(pattern).fullmatch(true);
        Written {
            comment: tc.draw(gs::booleans()),
            name: tc.draw(gs::optional(word("[a-z]{1,5}"))),
            data: tc.draw(gs::vecs(word("[a-zé日]{0,6}")).max_size(3)),
            spaced: tc.draw(gs::booleans()),
            id: tc.draw(gs::optional(word("[a-z0-9]{1,4}"))),
            retry: match tc.draw(gs::integers::<u8>().max_value(2)) {
                0 => None,
                1 => Some(Retry::Millis(
                    tc.draw(gs::integers::<u64>().max_value(9_999)),
                )),
                _ => Some(Retry::Not(tc.draw(gs::sampled_from(vec![
                    "soon", "-1", "", "1.5", "2s",
                ])))),
            },
            ends: tc.draw(
                gs::vecs(gs::sampled_from(vec!["\n", "\r\n", "\r"]))
                    .min_size(8)
                    .max_size(8),
            ),
        }
    }

    /// A stream of events, read in chunks cut anywhere (inside a `\r\n`
    /// or a character included), gives the frames its events say:
    /// each event with data is one frame, named `message` when the server
    /// named none, its data lines joined by newlines, with the last id
    /// given so far, or the one the stream opened with; comments and
    /// events with no data are none. The last id and the last `retry:`
    /// that is a number of milliseconds are the stream's.
    #[hegel::test(test_cases = 300)]
    fn events_come_whole_however_the_stream_is_cut(tc: hegel::TestCase) {
        let events: Vec<Written> =
            tc.draw(gs::vecs(written()).max_size(5).print_as_debug());
        let opened_with: Option<String> = tc.draw(gs::optional(
            gs::from_regex("[a-z0-9]{1,4}").fullmatch(true),
        ));
        let mut bytes = String::new();
        let mut expected = Vec::new();
        let (mut last_id, mut retry) = (opened_with.clone(), None);
        for event in &events {
            let mut lines = Vec::new();
            if event.comment {
                lines.push(": ping".to_owned());
            }
            if let Some(name) = &event.name {
                lines.push(format!("event: {name}"));
            }
            let gap = if event.spaced { " " } else { "" };
            lines.extend(
                event.data.iter().map(|data| format!("data:{gap}{data}")),
            );
            if let Some(id) = &event.id {
                lines.push(format!("id: {id}"));
                last_id = Some(id.clone());
            }
            match &event.retry {
                Some(Retry::Millis(millis)) => {
                    lines.push(format!("retry: {millis}"));
                    retry = Some(Duration::from_millis(*millis));
                }
                Some(Retry::Not(value)) => {
                    tc.event("a retry that is not a number");
                    lines.push(format!("retry: {value}"));
                }
                None => {}
            }
            lines.push(String::new());
            for (line, end) in lines.iter().zip(&event.ends) {
                // A `\n` right after a `\r` ends no other line: a blank
                // line after one ends in `\r\n`.
                let end =
                    if line.is_empty() && *end == "\n" && bytes.ends_with('\r')
                    {
                        "\r\n"
                    } else {
                        end
                    };
                bytes.push_str(line);
                bytes.push_str(end);
            }
            if !event.data.is_empty() {
                let mut frame = json!({
                    "event": event.name.as_deref().unwrap_or("message"),
                    "data": event.data.join("\n"),
                });
                if let Some(id) = &last_id {
                    frame["id"] = json!(id);
                }
                expected.push(frame);
            }
        }
        let bytes = bytes.into_bytes();
        let mut cuts: Vec<usize> = tc.draw(
            gs::vecs(gs::integers::<usize>().max_value(bytes.len()))
                .max_size(6),
        );
        cuts.extend([0, bytes.len()]);
        cuts.sort_unstable();
        cuts.dedup();
        let mut sse = Sse::new(opened_with);
        let mut got = Vec::new();
        for pair in cuts.windows(2) {
            got.extend(frames(sse.feed(&bytes[pair[0]..pair[1]])));
        }
        assert_eq!(got, expected, "{:?}", String::from_utf8_lossy(&bytes));
        assert_eq!(sse.last_id, last_id);
        assert_eq!(sse.retry(), retry);
    }

    #[test]
    fn a_frame_too_long_is_dropped_and_the_next_comes() {
        let mut sse = Sse::new(None);
        let mut long = b"data: ".to_vec();
        long.extend(std::iter::repeat_n(b'x', MAX_FRAME + 10));
        long.extend(b"\n\ndata: next\n\n");
        let got = frames(sse.feed(&long));
        assert_eq!(
            got,
            [
                json!("too large"),
                json!({ "event": "message", "data": "next" })
            ]
        );
        // Many lines that add up past a frame are dropped too.
        let mut many = Vec::new();
        for _ in 0..3 {
            many.extend(b"data: ");
            many.extend(std::iter::repeat_n(b'y', MAX_FRAME / 2));
            many.push(b'\n');
        }
        many.push(b'\n');
        assert_eq!(frames(sse.feed(&many)), [json!("too large")]);
    }

    /// A stream that failed waits [`FIRST_WAIT`], doubling with each
    /// failure after the first, at most [`LONGEST_WAIT`], give or take a
    /// fifth by its jitter: none at jitter 0 takes a fifth off, jitter
    /// one half is the doubled wait itself, and it never passes the
    /// longest. More failures, or more jitter, never wait less.
    #[hegel::test(test_cases = 500)]
    fn a_failed_stream_waits_double_each_time_within_a_fifth(
        tc: hegel::TestCase,
    ) {
        let failures = tc.draw(hegel::one_of!(
            gs::sampled_from(vec![0u32, 1, 2, 9, 10, 31, 32]),
            gs::integers::<u32>().max_value(64),
        ));
        let jitter = tc.draw(hegel::one_of!(
            gs::sampled_from(vec![0.0, 0.5, 1.0]),
            gs::floats::<f64>().min_value(0.0).max_value(1.0),
        ));
        let at = |failures: u32, jitter: f64| {
            wait(failures, FIRST_WAIT, LONGEST_WAIT, jitter).as_secs_f64()
        };
        let (first, longest) =
            (FIRST_WAIT.as_secs_f64(), LONGEST_WAIT.as_secs_f64());
        let doubled =
            (first * 2f64.powi(failures.max(1) as i32 - 1)).min(longest);
        let waited = at(failures, jitter);
        let close = |a: f64, b: f64| (a - b).abs() < 1e-6;
        assert!(waited <= longest + 1e-9, "{waited}");
        assert!(waited >= 0.8 * doubled - 1e-6, "{waited} of {doubled}");
        assert!(waited <= 1.2 * doubled + 1e-6, "{waited} of {doubled}");
        assert!(close(at(failures, 0.0), 0.8 * doubled));
        assert!(close(at(failures, 0.5), doubled));
        assert!(at(failures + 1, jitter) >= waited - 1e-9);
        let more =
            tc.draw(gs::floats::<f64>().min_value(jitter).max_value(1.0));
        assert!(at(failures, more) >= waited - 1e-9);
    }

    /// A stream fires an event only when its id is not among the last
    /// [`RECENT`] it fired, so a repeat fires nothing and an id it let go
    /// fires again. Model: the ids fired, as a queue, oldest first. The
    /// stream may start with the queue nearly full of earlier ids, the
    /// oldest of which (`f0`, `f1`) a few new ids push out.
    #[hegel::test(test_cases = 100)]
    fn a_stream_fires_an_id_it_has_not_fired_lately(tc: hegel::TestCase) {
        let fill = tc.draw(hegel::one_of!(
            gs::integers::<usize>().max_value(8),
            gs::integers::<usize>()
                .min_value(RECENT - 8)
                .max_value(RECENT),
        ));
        let ids: Vec<&str> = tc.draw(
            gs::vecs(gs::sampled_from(vec!["a", "b", "c", "f0", "f1"]))
                .max_size(20),
        );
        let mut streamed = Streamed::new(TriggerKind::Message);
        let mut fired: VecDeque<String> = VecDeque::new();
        let earlier = (0..fill).map(|n| format!("f{n}"));
        let ids = earlier.chain(ids.into_iter().map(str::to_owned));
        for (at, id) in ids.enumerate() {
            let fresh = !fired.contains(&id);
            if fresh {
                fired.push_back(id.clone());
                if fired.len() > RECENT {
                    fired.pop_front();
                }
                if at >= fill && id.starts_with('f') && fill > 8 {
                    tc.event("an id let go fires again");
                }
            }
            assert_eq!(streamed.fresh(&id), fresh, "{id}");
            assert_eq!(streamed.recent, fired);
        }
    }
}
