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
    collections::{HashMap, VecDeque},
    rc::Rc,
};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
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
}

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

    /// Connections accepted so far.
    pub fn connections(&self) -> u32 {
        self.state.borrow().connections
    }

    async fn serve(
        &self,
        connection: u32,
        mut socket: tokio_tungstenite::WebSocketStream<turmoil::net::TcpStream>,
    ) {
        // Responses held by this connection: id -> full item list.
        let mut held: HashMap<String, Vec<Value>> = HashMap::new();
        while let Some(Ok(message)) = socket.next().await {
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
                Some(Reply::Respond {
                    frames,
                    response_id,
                    output_items,
                }) => {
                    for frame in frames {
                        if send(&mut socket, with_stream_id(frame, &stream_id))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    let mut items = rebuilt;
                    items.extend(output_items);
                    held.insert(response_id, items);
                }
                Some(Reply::Error { code }) => {
                    held.clear();
                    let frame = error_frame(&stream_id, &code);
                    if send(&mut socket, frame).await.is_err() {
                        return;
                    }
                }
                Some(Reply::DropAfter { frames, after }) => {
                    for frame in frames.into_iter().take(after) {
                        if send(&mut socket, with_stream_id(frame, &stream_id))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    return;
                }
                Some(Reply::StallAfter { frames, after }) => {
                    for frame in frames.into_iter().take(after) {
                        if send(&mut socket, with_stream_id(frame, &stream_id))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    while let Some(Ok(_)) = socket.next().await {}
                    return;
                }
                Some(Reply::Evict) | None => {
                    // Out of script: fail loudly in the test's assertions
                    // rather than hang.
                    let frame =
                        error_frame(&stream_id, "fake_script_exhausted");
                    if send(&mut socket, frame).await.is_err() {
                        return;
                    }
                }
            }
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
