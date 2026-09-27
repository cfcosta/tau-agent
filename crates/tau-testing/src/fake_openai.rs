//! `FakeOpenAi`: a turmoil host that speaks the Responses WebSocket
//! protocol, for transport tests (`docs/reference/testing.md`, "Fake
//! OpenAI server").
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

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet, VecDeque},
    rc::Rc,
    time::Duration,
};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::time::{Instant, sleep_until};
use tokio_tungstenite::tungstenite::Message;

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
}

/// OpenAI's limits per connection (`docs/reference/openai-websocket.md`,
/// "Limits and lanes"), which the fake enforces.
pub const MAX_IN_FLIGHT: usize = 16;
pub const MAX_STREAMS: usize = 32;
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
}

#[derive(Debug, Default)]
struct State {
    replies: VecDeque<Reply>,
    received: Vec<Received>,
    connections: u32,
    /// Limits the client broke, which OpenAI would have refused.
    violations: Vec<String>,
    /// Requests refused for a new stream id past `MAX_STREAMS`.
    stream_limit_errors: u64,
}

/// A fake OpenAI endpoint. Clones share state.
#[derive(Debug, Clone, Default)]
pub struct FakeOpenAi {
    state: Rc<RefCell<State>>,
}

/// The port the fake listens on.
pub const PORT: u16 = 80;

impl FakeOpenAi {
    pub fn new(replies: Vec<Reply>) -> Self {
        let fake = Self::default();
        fake.state.borrow_mut().replies = replies.into();
        fake
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
                        let mut state = fake.state.borrow_mut();
                        state.connections += 1;
                        state.connections
                    };
                    let fake = fake.clone();
                    tokio::task::spawn_local(async move {
                        if let Ok(socket) =
                            tokio_tungstenite::accept_async(stream).await
                        {
                            fake.serve(connection, socket).await;
                        }
                    });
                }
            }
        });
    }

    /// Every request received so far, in order.
    pub fn received(&self) -> Vec<Received> {
        self.state.borrow().received.clone()
    }

    /// Limits the client broke: more than [`MAX_IN_FLIGHT`] requests in
    /// flight on one connection. A correct client never does.
    pub fn violations(&self) -> Vec<String> {
        self.state.borrow().violations.clone()
    }

    /// Requests refused with `websocket_stream_limit_reached`, for a new
    /// stream id on a connection that had seen [`MAX_STREAMS`] of them.
    pub fn stream_limit_errors(&self) -> u64 {
        self.state.borrow().stream_limit_errors
    }

    /// Connections accepted so far.
    pub fn connections(&self) -> u32 {
        self.state.borrow().connections
    }

    async fn serve(
        &self,
        connection: u32,
        mut socket: tokio_tungstenite::WebSocketStream<turmoil::net::TcpStream>,
    ) {
        let accepted = Instant::now();
        // Responses held by this connection: id -> full item list.
        let mut held: HashMap<String, Vec<Value>> = HashMap::new();
        // Named stream ids this connection has seen.
        let mut streams: HashSet<Value> = HashSet::new();
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
                        if !answer(&mut socket, &mut held, &p.stream_id, p.reply, p.rebuilt).await {
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
            let stream_id = body.get("stream_id").cloned();
            let input = body["input"].as_array().cloned().unwrap_or_default();

            if pending.len() >= MAX_IN_FLIGHT {
                self.state.borrow_mut().violations.push(format!(
                    "connection {connection}: a request beyond {MAX_IN_FLIGHT} in flight"
                ));
                let frame =
                    error_frame(&stream_id, "fake_in_flight_limit_exceeded");
                if send(&mut socket, frame).await.is_err() {
                    return;
                }
                continue;
            }
            if let Some(id) = &stream_id
                && !streams.contains(id)
            {
                if streams.len() >= MAX_STREAMS {
                    self.state.borrow_mut().stream_limit_errors += 1;
                    let frame = error_frame(
                        &stream_id,
                        "websocket_stream_limit_reached",
                    );
                    if send(&mut socket, frame).await.is_err() {
                        return;
                    }
                    continue;
                }
                streams.insert(id.clone());
            }

            let mut reply = self.state.borrow_mut().replies.pop_front();
            if matches!(reply, Some(Reply::Evict)) {
                held.clear();
                reply = self.state.borrow_mut().replies.pop_front();
            }

            let rebuilt = match body.get("previous_response_id") {
                Some(Value::String(id)) => held.get(id).map(|items| {
                    let mut all = items.clone();
                    all.extend(input.iter().cloned());
                    all
                }),
                _ => Some(input.clone()),
            };
            self.state.borrow_mut().received.push(Received {
                connection,
                body: body.clone(),
                rebuilt_input: rebuilt.clone(),
            });

            let Some(rebuilt) = rebuilt else {
                // Not held: the scripted reply stays for the resend.
                if let Some(reply) = reply {
                    self.state.borrow_mut().replies.push_front(reply);
                }
                held.clear();
                let frame =
                    error_frame(&stream_id, "previous_response_not_found");
                if send(&mut socket, frame).await.is_err() {
                    return;
                }
                continue;
            };

            match reply {
                Some(Reply::Delay(delay, reply)) => pending.push(Pending {
                    due: Instant::now() + delay,
                    stream_id,
                    reply: Some(*reply),
                    rebuilt,
                }),
                reply => {
                    if !answer(
                        &mut socket,
                        &mut held,
                        &stream_id,
                        reply,
                        rebuilt,
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
    stream_id: Option<Value>,
    reply: Option<Reply>,
    rebuilt: Vec<Value>,
}

/// Sends `reply` to a request whose rebuilt input is `rebuilt`. Returns
/// false once the connection is done.
async fn answer(
    socket: &mut tokio_tungstenite::WebSocketStream<turmoil::net::TcpStream>,
    held: &mut HashMap<String, Vec<Value>>,
    stream_id: &Option<Value>,
    reply: Option<Reply>,
    rebuilt: Vec<Value>,
) -> bool {
    match reply {
        Some(Reply::Respond {
            frames,
            response_id,
            output_items,
        }) => {
            for frame in frames {
                if send(socket, with_stream_id(frame, stream_id))
                    .await
                    .is_err()
                {
                    return false;
                }
            }
            let mut items = rebuilt;
            items.extend(output_items);
            held.insert(response_id, items);
            true
        }
        Some(Reply::Error { code }) => {
            held.clear();
            send(socket, error_frame(stream_id, &code)).await.is_ok()
        }
        Some(Reply::DropAfter { frames, after }) => {
            for frame in frames.into_iter().take(after) {
                if send(socket, with_stream_id(frame, stream_id))
                    .await
                    .is_err()
                {
                    return false;
                }
            }
            false
        }
        Some(Reply::StallAfter { frames, after }) => {
            for frame in frames.into_iter().take(after) {
                if send(socket, with_stream_id(frame, stream_id))
                    .await
                    .is_err()
                {
                    return false;
                }
            }
            while let Some(Ok(_)) = socket.next().await {}
            false
        }
        Some(Reply::Delay(_, reply)) => {
            // A delay inside a delay: the outer one already waited.
            Box::pin(answer(socket, held, stream_id, Some(*reply), rebuilt))
                .await
        }
        Some(Reply::Evict) | None => {
            // Out of script: fail loudly in the test's assertions rather
            // than hang.
            send(socket, error_frame(stream_id, "fake_script_exhausted"))
                .await
                .is_ok()
        }
    }
}

async fn send(
    socket: &mut tokio_tungstenite::WebSocketStream<turmoil::net::TcpStream>,
    frame: Value,
) -> Result<(), tokio_tungstenite::tungstenite::Error> {
    socket.send(Message::text(frame.to_string())).await
}

fn with_stream_id(mut frame: Value, stream_id: &Option<Value>) -> Value {
    if let (Some(id), Value::Object(map)) = (stream_id, &mut frame) {
        map.insert("stream_id".into(), id.clone());
    }
    frame
}

fn error_frame(stream_id: &Option<Value>, code: &str) -> Value {
    with_stream_id(
        json!({
            "type": "error",
            "code": code,
            "message": format!("fake error: {code}"),
        }),
        stream_id,
    )
}
