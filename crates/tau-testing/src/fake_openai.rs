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
    time::Duration,
};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::time::{Instant, sleep_until};
use tokio_tungstenite::tungstenite::{
    Message,
    handshake::server::{ErrorResponse, Request, Response},
    http::{HeaderValue, StatusCode},
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
                        let upgrade = fake.clone();
                        // tungstenite's callback type, not ours.
                        #[allow(clippy::result_large_err)]
                        let answer = move |request: &Request, response| {
                            upgrade.upgrade(request, response)
                        };
                        if let Ok(socket) =
                            tokio_tungstenite::accept_hdr_async(stream, answer)
                                .await
                        {
                            fake.serve(connection, socket).await;
                        }
                    });
                }
            }
        });
    }

    /// Answers the next upgrade with `refusal` instead of accepting it.
    /// Refusals queue up.
    pub fn refuse_upgrade(&self, refusal: Refusal) {
        self.state.borrow_mut().refusals.push_back(refusal);
    }

    /// The `Authorization` header of every upgrade, refused or not, in
    /// order.
    pub fn authorizations(&self) -> Vec<Option<String>> {
        self.state.borrow().authorizations.clone()
    }

    // tungstenite's callback type, not ours.
    #[allow(clippy::result_large_err)]
    fn upgrade(
        &self,
        request: &Request,
        response: Response,
    ) -> Result<Response, ErrorResponse> {
        let mut state = self.state.borrow_mut();
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
        self.state.borrow().received.clone()
    }

    /// Limits the client broke: more than [`MAX_IN_FLIGHT`] requests in
    /// flight on one connection. A correct client never does.
    pub fn violations(&self) -> Vec<String> {
        self.state.borrow().violations.clone()
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
                        if !answer(&mut socket, &mut held, p.reply, p.rebuilt).await {
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
                self.state.borrow_mut().violations.push(format!(
                    "connection {connection}: a request beyond {MAX_IN_FLIGHT} in flight"
                ));
                let frame = error_frame("fake_in_flight_limit_exceeded");
                if send(&mut socket, frame).await.is_err() {
                    return;
                }
                continue;
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
                let frame = error_frame("previous_response_not_found");
                if send(&mut socket, frame).await.is_err() {
                    return;
                }
                continue;
            };

            match reply {
                Some(Reply::Delay(delay, reply)) => pending.push(Pending {
                    due: Instant::now() + delay,
                    reply: Some(*reply),
                    rebuilt,
                }),
                reply => {
                    if !answer(&mut socket, &mut held, reply, rebuilt).await {
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
}

/// Sends `reply` to a request whose rebuilt input is `rebuilt`. Returns
/// false once the connection is done.
async fn answer(
    socket: &mut tokio_tungstenite::WebSocketStream<turmoil::net::TcpStream>,
    held: &mut HashMap<String, Vec<Value>>,
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
                if send(socket, frame).await.is_err() {
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
            Box::pin(answer(socket, held, Some(*reply), rebuilt)).await
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

async fn send(
    socket: &mut tokio_tungstenite::WebSocketStream<turmoil::net::TcpStream>,
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
