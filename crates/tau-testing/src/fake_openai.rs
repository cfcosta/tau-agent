//! `FakeOpenAi`: a server that speaks the Responses WebSocket protocol,
//! for transport tests (`docs/reference/testing.md`, "Fake OpenAI
//! server"): a turmoil host ([`FakeOpenAi::install`]), or a local TCP
//! listener on a thread of its own ([`FakeOpenAi::listen`]) for tests
//! that run tau on a real runtime, such as the host's.
//!
//! It keeps OpenAI's continuation rules: each connection holds the
//! responses it produced, a request whose `previous_response_id` the
//! connection does not hold gets `previous_response_not_found`, and the
//! held responses are dropped after an error and when the connection
//! closes. For every request it accepts, it records the input it rebuilt
//! from its cache plus the request's own input: that is the oracle for
//! the delta rule.
//!
//! Replies are scripted by the test, in order across all requests, and
//! prepared before the simulation runs (frames are drawn with Hegel,
//! which cannot run inside the simulation).
//!
//! Each connection also keeps a prompt cache, as OpenAI's route does
//! (`docs/reference/openai-websocket.md`, "Prompt cache"): a request
//! reads from cache the longest prefix it shares with a prompt that
//! connection saw before, and nothing another connection saw. A prompt
//! is its head (every field but `input`, `instructions`,
//! `previous_response_id`, `prompt_cache_key` and `generate`, so a
//! change of tools or effort reads nothing), then its instructions,
//! which match up to their first difference, then its input items. A
//! token is four bytes of JSON. Each [`Received`] says what it read;
//! with [`FakeOpenAi::report_cache`], completed responses report it in
//! their usage too.

use std::{
    collections::{HashMap, VecDeque},
    io,
    net::SocketAddr,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tau_ai::{
    event::Accumulator,
    message::{AssistantMessage, Timestamp},
    responses::{input::response_items, stream::StreamProcessor},
    ws::io::connection::Connector,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    time::{Instant, sleep_until},
};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{
        Message,
        client::IntoClientRequest,
        handshake::server::{ErrorResponse, Request, Response},
        http::{self, HeaderValue, StatusCode},
    },
};

/// What the server does with one request.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    /// Streams a whole response. The server then holds it, under
    /// `response_id`, as the request's input followed by `output_items`.
    Respond {
        frames: Vec<Value>,
        response_id: String,
        output_items: Vec<Value>,
    },
    /// Answers with an `error` frame carrying `code`.
    Error { code: String },
    /// Sends the first `after` of `frames`, then drops the connection.
    DropAfter { frames: Vec<Value>, after: usize },
    /// Sends the first `after` of `frames`, then goes silent: it keeps
    /// the connection open and ignores everything until the client
    /// closes it.
    StallAfter { frames: Vec<Value>, after: usize },
    /// Forgets every held response on this connection, then applies the
    /// next reply to the same request.
    Evict,
    /// Answers with the inner reply after `delay`, meanwhile reading and
    /// answering other requests, so responses overlap as they do on a
    /// real connection. The request counts as in flight until then.
    Delay(Duration, Box<Reply>),
    /// Sends the first `after` of `frames`, then waits for a
    /// `response.interrupt` of `response_id`, as a Responses Lite
    /// response streams on. If `accept`, the server stops it, as gpt-6
    /// models do: `response.interrupt.accepted`, then `response.incomplete`
    /// with reason `interrupted`, and the connection goes on. Otherwise it
    /// refuses, as gpt-5.6 does: `response.interrupt.failed`, and the rest
    /// never comes, until the client closes the connection.
    Interruptible {
        frames: Vec<Value>,
        after: usize,
        response_id: String,
        accept: bool,
    },
}

impl Reply {
    /// A response whose one output is the message `text`, held under
    /// `response_id` with the output items tau's next request will
    /// carry for it.
    pub fn text(response_id: &str, text: &str) -> Self {
        let frames = vec![
            json!({"type": "response.created", "response": {"id": response_id}}),
            json!({"type": "response.output_item.added", "output_index": 0,
                   "item": {"type": "message", "id": format!("msg_{response_id}"),
                            "role": "assistant", "content": []}}),
            json!({"type": "response.content_part.added", "output_index": 0,
                   "content_index": 0, "part": {"type": "output_text", "text": ""}}),
            json!({"type": "response.output_text.delta", "output_index": 0,
                   "content_index": 0, "delta": text}),
            json!({"type": "response.output_item.done", "output_index": 0,
                   "item": {"type": "message", "id": format!("msg_{response_id}"),
                            "role": "assistant",
                            "content": [{"type": "output_text", "text": text}]}}),
            json!({"type": "response.completed", "response": {
                "id": response_id, "status": "completed",
                "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2},
            }}),
        ];
        let message = streamed(&frames, 0);
        Self::Respond {
            output_items: response_items(&message),
            frames,
            response_id: response_id.to_owned(),
        }
    }
}

/// The message tau builds from `frames`.
fn streamed(frames: &[Value], timestamp: Timestamp) -> AssistantMessage {
    let mut processor = StreamProcessor::new("fake".into(), timestamp);
    let mut accumulator = Accumulator::new();
    for frame in frames {
        for event in processor.push(frame) {
            accumulator.push(event).expect("a well-formed stream");
        }
    }
    accumulator.finish().expect("a finished stream")
}

/// Connects to a fake that [`FakeOpenAi::listen`]s on `address`.
#[derive(Debug, Clone, Copy)]
pub struct LocalConnector(pub SocketAddr);

impl Connector for LocalConnector {
    type Stream = tokio::net::TcpStream;

    async fn connect(&self) -> io::Result<Self::Stream> {
        tokio::net::TcpStream::connect(self.0).await
    }

    fn request(&self) -> http::Request<()> {
        format!("ws://{}/v1/responses", self.0)
            .into_client_request()
            .expect("a valid URL")
    }
}

/// OpenAI's limits per connection (`docs/reference/openai-websocket.md`,
/// "Limits and lanes"), which the fake enforces.
pub const MAX_IN_FLIGHT: usize = 16;
pub const MAX_AGE: Duration = Duration::from_secs(60 * 60);

/// One request the server received.
#[derive(Debug, Clone, PartialEq)]
pub struct Received {
    /// Connections are numbered from 1 in the order they were accepted.
    pub connection: u32,
    pub body: Value,
    /// The input the server rebuilt from its cache and the request, or
    /// `None` if it answered `previous_response_not_found`.
    pub rebuilt_input: Option<Vec<Value>>,
    /// The prompt's size in tokens, and how many of them the
    /// connection's prompt cache held. Both 0 for a request answered
    /// `previous_response_not_found`.
    pub input_tokens: u64,
    pub cached_tokens: u64,
}

/// An HTTP answer to a WebSocket upgrade, instead of `101`.
#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
    pub status: u16,
    pub body: Value,
    /// Sent as `x-request-id`.
    pub request_id: Option<String>,
}

#[derive(Debug, Default)]
struct State {
    replies: VecDeque<Reply>,
    /// Answers for the next upgrades, in order; then they are accepted.
    refusals: VecDeque<Refusal>,
    /// The `Authorization` header of every upgrade, in order.
    authorizations: Vec<Option<String>>,
    received: Vec<Received>,
    connections: u32,
    /// Limits the client broke, which OpenAI would have refused.
    violations: Vec<String>,
    /// Whether completed responses report the prompt cache in their
    /// usage.
    report_cache: bool,
    /// Every `response.interrupt` received, as `(connection, response
    /// id)`.
    interrupts: Vec<(u32, String)>,
}

/// A fake OpenAI endpoint. Clones share state.
#[derive(Debug, Clone, Default)]
pub struct FakeOpenAi {
    state: Arc<Mutex<State>>,
}

/// The port the fake listens on.
pub const PORT: u16 = 80;

impl FakeOpenAi {
    pub fn new(replies: Vec<Reply>) -> Self {
        let fake = Self::default();
        fake.state().replies = replies.into();
        fake
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("not poisoned")
    }

    /// Registers the fake as the host `name` in `sim`.
    pub fn install(&self, sim: &mut turmoil::Sim<'_>, name: &str) {
        let fake = self.clone();
        sim.host(name, move || {
            let fake = fake.clone();
            async move {
                let listener =
                    turmoil::net::TcpListener::bind(("0.0.0.0", PORT)).await?;
                loop {
                    let (stream, _) = listener.accept().await?;
                    let connection = {
                        let mut state = fake.state();
                        state.connections += 1;
                        state.connections
                    };
                    tokio::task::spawn_local(
                        fake.clone().accept(connection, stream),
                    );
                }
            }
        });
    }

    /// Serves on a local TCP port, on a thread of its own, for tests that
    /// run tau on a real runtime. Returns the address; connect to it with
    /// [`LocalConnector`].
    pub fn listen(&self) -> SocketAddr {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("a local port");
        let address = listener.local_addr().expect("a bound address");
        listener
            .set_nonblocking(true)
            .expect("a nonblocking listener");
        let fake = self.clone();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime");
            let local = tokio::task::LocalSet::new();
            local.block_on(&runtime, async move {
                let listener = tokio::net::TcpListener::from_std(listener)
                    .expect("a tokio listener");
                while let Ok((stream, _)) = listener.accept().await {
                    let connection = {
                        let mut state = fake.state();
                        state.connections += 1;
                        state.connections
                    };
                    tokio::task::spawn_local(
                        fake.clone().accept(connection, stream),
                    );
                }
            });
        });
        address
    }

    /// Upgrades one accepted stream and serves it.
    async fn accept<S>(self, connection: u32, stream: S)
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let upgrade = self.clone();
        // tungstenite's callback type, not ours.
        #[allow(clippy::result_large_err)]
        let answer = move |request: &Request, response| {
            upgrade.upgrade(request, response)
        };
        if let Ok(socket) =
            tokio_tungstenite::accept_hdr_async(stream, answer).await
        {
            self.serve(connection, socket).await;
        }
    }

    /// Has completed responses report the prompt's tokens and those read
    /// from the connection's cache in their usage (`input_tokens`,
    /// `input_tokens_details.cached_tokens`), in place of the scripted
    /// counts.
    pub fn report_cache(self) -> Self {
        self.state().report_cache = true;
        self
    }

    /// Answers the next upgrade with `refusal` instead of accepting it.
    /// Refusals queue up.
    pub fn refuse_upgrade(&self, refusal: Refusal) {
        self.state().refusals.push_back(refusal);
    }

    /// The `Authorization` header of every upgrade, refused or not, in
    /// order.
    pub fn authorizations(&self) -> Vec<Option<String>> {
        self.state().authorizations.clone()
    }

    // tungstenite's callback type, not ours.
    #[allow(clippy::result_large_err)]
    fn upgrade(
        &self,
        request: &Request,
        response: Response,
    ) -> Result<Response, ErrorResponse> {
        let mut state = self.state();
        state.authorizations.push(
            request
                .headers()
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned),
        );
        let Some(refusal) = state.refusals.pop_front() else {
            return Ok(response);
        };
        let mut error = ErrorResponse::new(Some(refusal.body.to_string()));
        *error.status_mut() =
            StatusCode::from_u16(refusal.status).expect("a valid HTTP status");
        let headers = error.headers_mut();
        headers.insert(
            "content-type",
            HeaderValue::from_static("application/json"),
        );
        if let Some(id) = refusal.request_id
            && let Ok(value) = HeaderValue::from_str(&id)
        {
            headers.insert("x-request-id", value);
        }
        Err(error)
    }

    /// Every request received so far, in order.
    pub fn received(&self) -> Vec<Received> {
        self.state().received.clone()
    }

    /// Limits the client broke: more than [`MAX_IN_FLIGHT`] requests in
    /// flight on one connection. A correct client never does.
    pub fn violations(&self) -> Vec<String> {
        self.state().violations.clone()
    }

    /// Every `response.interrupt` received so far, as `(connection,
    /// response id)`.
    pub fn interrupts(&self) -> Vec<(u32, String)> {
        self.state().interrupts.clone()
    }

    /// Connections accepted so far.
    pub fn connections(&self) -> u32 {
        self.state().connections
    }

    async fn serve<S>(&self, connection: u32, mut socket: WebSocketStream<S>)
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let accepted = Instant::now();
        // Responses held by this connection: id -> full item list.
        let mut held: HashMap<String, Vec<Value>> = HashMap::new();
        // The prompts this connection saw, for its prompt cache.
        let mut seen: Vec<Prompt> = Vec::new();
        // Delayed replies, still in flight: when each is due.
        let mut pending: Vec<Pending> = Vec::new();
        loop {
            let due = pending.iter().map(|p| p.due).min();
            let message = tokio::select! {
                message = socket.next() => message,
                _ = sleep_until(due.unwrap_or(accepted)), if due.is_some() => {
                    let now = Instant::now();
                    let (ready, later): (Vec<_>, Vec<_>) =
                        pending.drain(..).partition(|p| p.due <= now);
                    pending = later;
                    for p in ready {
                        let served = Served {
                            rebuilt: p.rebuilt,
                            prompt: p.prompt,
                            cached: p.cached,
                            report: self.state().report_cache,
                        };
                        if !answer(self, connection, &mut socket, &mut held, &mut seen, p.reply, served).await {
                            return;
                        }
                    }
                    continue;
                }
                // OpenAI closes a connection at 60 minutes.
                _ = sleep_until(accepted + MAX_AGE) => return,
            };
            let Some(Ok(message)) = message else { return };
            let Message::Text(text) = message else {
                continue;
            };
            let Ok(body) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            if body["type"] != "response.create" {
                continue;
            }
            let input = body["input"].as_array().cloned().unwrap_or_default();

            if pending.len() >= MAX_IN_FLIGHT {
                self.state().violations.push(format!(
                    "connection {connection}: a request beyond {MAX_IN_FLIGHT} in flight"
                ));
                let frame = error_frame("fake_in_flight_limit_exceeded");
                if send(&mut socket, frame).await.is_err() {
                    return;
                }
                continue;
            }

            let mut reply = self.state().replies.pop_front();
            if matches!(reply, Some(Reply::Evict)) {
                held.clear();
                reply = self.state().replies.pop_front();
            }

            let rebuilt = match body.get("previous_response_id") {
                Some(Value::String(id)) => held.get(id).map(|items| {
                    let mut all = items.clone();
                    all.extend(input.iter().cloned());
                    all
                }),
                _ => Some(input.clone()),
            };
            let prompt = rebuilt.as_ref().map(|items| Prompt::of(&body, items));
            let (input_tokens, cached_tokens) = match &prompt {
                Some(prompt) => (prompt.tokens(), prompt.cached(&seen)),
                None => (0, 0),
            };
            self.state().received.push(Received {
                connection,
                body: body.clone(),
                rebuilt_input: rebuilt.clone(),
                input_tokens,
                cached_tokens,
            });

            let Some(rebuilt) = rebuilt else {
                // Not held: the scripted reply stays for the resend.
                if let Some(reply) = reply {
                    self.state().replies.push_front(reply);
                }
                held.clear();
                let frame = error_frame("previous_response_not_found");
                if send(&mut socket, frame).await.is_err() {
                    return;
                }
                continue;
            };

            let prompt = prompt.expect("a rebuilt input has a prompt");
            seen.push(prompt.clone());
            match reply {
                Some(Reply::Delay(delay, reply)) => pending.push(Pending {
                    due: Instant::now() + delay,
                    reply: Some(*reply),
                    rebuilt,
                    prompt,
                    cached: cached_tokens,
                }),
                reply => {
                    let served = Served {
                        rebuilt,
                        prompt,
                        cached: cached_tokens,
                        report: self.state().report_cache,
                    };
                    if !answer(
                        self,
                        connection,
                        &mut socket,
                        &mut held,
                        &mut seen,
                        reply,
                        served,
                    )
                    .await
                    {
                        return;
                    }
                }
            }
        }
    }
}

/// A reply held back by [`Reply::Delay`].
struct Pending {
    due: Instant,
    reply: Option<Reply>,
    rebuilt: Vec<Value>,
    prompt: Prompt,
    cached: u64,
}

/// A request being answered.
struct Served {
    /// The input the server rebuilt.
    rebuilt: Vec<Value>,
    prompt: Prompt,
    /// Its tokens the prompt cache held.
    cached: u64,
    /// Whether the completed response reports the cache in its usage.
    report: bool,
}

/// One part of a prompt, in the order the prompt cache reads them.
#[derive(Debug, Clone, PartialEq)]
enum Part {
    Head(Value),
    Instructions(String),
    Item(Value),
}

impl Part {
    fn tokens(&self) -> u64 {
        let bytes = match self {
            Part::Head(value) | Part::Item(value) => value.to_string().len(),
            Part::Instructions(text) => text.len(),
        };
        (bytes as u64).div_ceil(4)
    }
}

/// A prompt as the prompt cache sees it.
#[derive(Debug, Clone, PartialEq)]
struct Prompt(Vec<Part>);

impl Prompt {
    /// The prompt of `body`, whose whole input is `items`.
    fn of(body: &Value, items: &[Value]) -> Self {
        let mut head = body.as_object().cloned().unwrap_or_default();
        for field in [
            "input",
            "instructions",
            "previous_response_id",
            "prompt_cache_key",
            "generate",
        ] {
            head.remove(field);
        }
        let mut parts = vec![Part::Head(Value::Object(head))];
        if let Some(text) = body["instructions"].as_str() {
            parts.push(Part::Instructions(text.to_owned()));
        }
        parts.extend(items.iter().cloned().map(Part::Item));
        Self(parts)
    }

    /// The prompt followed by a response's output items.
    fn answered(&self, output_items: &[Value]) -> Self {
        let mut parts = self.0.clone();
        parts.extend(output_items.iter().cloned().map(Part::Item));
        Self(parts)
    }

    fn tokens(&self) -> u64 {
        self.0.iter().map(Part::tokens).sum()
    }

    /// The tokens of the longest prefix this prompt shares with one of
    /// `seen`.
    fn cached(&self, seen: &[Prompt]) -> u64 {
        seen.iter()
            .map(|other| self.shared(other))
            .max()
            .unwrap_or(0)
    }

    fn shared(&self, other: &Prompt) -> u64 {
        let mut tokens = 0;
        for (mine, theirs) in self.0.iter().zip(&other.0) {
            if mine == theirs {
                tokens += mine.tokens();
                continue;
            }
            // Instructions match up to their first difference.
            if let (Part::Instructions(mine), Part::Instructions(theirs)) =
                (mine, theirs)
            {
                let common = mine
                    .bytes()
                    .zip(theirs.bytes())
                    .take_while(|(a, b)| a == b)
                    .count();
                tokens += (common as u64) / 4;
            }
            break;
        }
        tokens
    }
}

/// Reports the prompt's tokens and those read from cache in a
/// `response.completed` frame's usage.
fn with_cache_usage(mut frame: Value, served: &Served) -> Value {
    if frame["type"] == "response.completed" {
        let usage = &mut frame["response"]["usage"];
        usage["input_tokens"] = served.prompt.tokens().into();
        usage["input_tokens_details"]["cached_tokens"] = served.cached.into();
    }
    frame
}

/// Sends `reply` to the request `served`. Returns false once the
/// connection is done.
async fn answer<S: AsyncRead + AsyncWrite + Unpin>(
    fake: &FakeOpenAi,
    connection: u32,
    socket: &mut WebSocketStream<S>,
    held: &mut HashMap<String, Vec<Value>>,
    seen: &mut Vec<Prompt>,
    reply: Option<Reply>,
    served: Served,
) -> bool {
    match reply {
        Some(Reply::Respond {
            frames,
            response_id,
            output_items,
        }) => {
            for frame in frames {
                let frame = if served.report {
                    with_cache_usage(frame, &served)
                } else {
                    frame
                };
                if send(socket, frame).await.is_err() {
                    return false;
                }
            }
            seen.push(served.prompt.answered(&output_items));
            let mut items = served.rebuilt;
            items.extend(output_items);
            held.insert(response_id, items);
            true
        }
        Some(Reply::Error { code }) => {
            held.clear();
            send(socket, error_frame(&code)).await.is_ok()
        }
        Some(Reply::DropAfter { frames, after }) => {
            for frame in frames.into_iter().take(after) {
                if send(socket, frame).await.is_err() {
                    return false;
                }
            }
            false
        }
        Some(Reply::StallAfter { frames, after }) => {
            for frame in frames.into_iter().take(after) {
                if send(socket, frame).await.is_err() {
                    return false;
                }
            }
            while let Some(Ok(_)) = socket.next().await {}
            false
        }
        Some(Reply::Delay(_, reply)) => {
            // A delay inside a delay: the outer one already waited.
            Box::pin(answer(
                fake,
                connection,
                socket,
                held,
                seen,
                Some(*reply),
                served,
            ))
            .await
        }
        Some(Reply::Interruptible {
            frames,
            after,
            response_id,
            accept,
        }) => {
            for frame in frames.into_iter().take(after) {
                if send(socket, frame).await.is_err() {
                    return false;
                }
            }
            // The response streams on until the client interrupts it.
            loop {
                let Some(Ok(Message::Text(text))) = socket.next().await else {
                    return false;
                };
                let Ok(frame) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                if frame["type"] == "response.interrupt"
                    && frame["response_id"] == response_id.as_str()
                {
                    fake.state()
                        .interrupts
                        .push((connection, response_id.clone()));
                    break;
                }
                fake.state().violations.push(format!(
                    "connection {connection}: {} before the interrupt of {response_id}",
                    frame["type"]
                ));
            }
            if !accept {
                let failed = json!({
                    "type": "response.interrupt.failed",
                    "response_id": response_id,
                    "error": {
                        "type": "invalid_request_error",
                        "code": "interrupt_not_supported",
                        "message": "This model does not support response.interrupt.",
                        "param": "response_id",
                    },
                });
                if send(socket, failed).await.is_err() {
                    return false;
                }
                // The rest of a long answer: it never ends here.
                while let Some(Ok(_)) = socket.next().await {}
                return false;
            }
            let stopped = [
                json!({"type": "response.interrupt.accepted", "response_id": response_id, "response": null}),
                json!({"type": "response.incomplete", "response": {
                    "id": response_id,
                    "status": "incomplete",
                    "incomplete_details": {"reason": "interrupted"},
                    "output": [],
                    "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2},
                }}),
            ];
            for frame in stopped {
                if send(socket, frame).await.is_err() {
                    return false;
                }
            }
            true
        }
        Some(Reply::Evict) | None => {
            // Out of script: fail loudly in the test's assertions rather
            // than hang.
            send(socket, error_frame("fake_script_exhausted"))
                .await
                .is_ok()
        }
    }
}

async fn send<S: AsyncRead + AsyncWrite + Unpin>(
    socket: &mut WebSocketStream<S>,
    frame: Value,
) -> Result<(), tokio_tungstenite::tungstenite::Error> {
    socket.send(Message::text(frame.to_string())).await
}

/// An error as the server sends it: the details nested in `error`.
fn error_frame(code: &str) -> Value {
    json!({
        "type": "error",
        "status": 400,
        "error": {
            "type": "invalid_request_error",
            "code": code,
            "message": format!("fake error: {code}"),
        },
    })
}
