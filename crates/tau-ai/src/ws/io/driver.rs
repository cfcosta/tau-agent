//! The driver: one task that runs the [`Pool`] and its connection tasks,
//! routes server frames to lanes, and turns them into [`AssistantEvent`]s
//! for the runs.
//!
//! A connection carries one lane at a time, so a frame goes to the lane
//! the connection carries. Frames for a lane with no request in flight
//! are ignored.
//!
//! A connection that could not open because it was refused (a
//! [`Refusal`]: an upgrade answered with an HTTP error, or a sign-in with
//! no token to give) fails the requests waiting on it at once when the
//! refusal is not temporary: a usage limit, a sign-in to redo, a
//! restriction. Those are never resent, so a run cannot loop on them. A
//! temporary refusal takes the usual path (one transparent reconnect,
//! then the run's retry policy), and the error the run finally sees is
//! the refusal's. The latest refusal, from a connection or from a failed
//! response with a documented plan-usage code, stays on the transport
//! until a response completes ([`Transport::refusal`]).
//!
//! A cancelled request that was already sent keeps streaming on the
//! server, and the server answers a connection's requests in order, so
//! the next request there would wait for its tail. A Responses Lite
//! request whose response id is known is stopped on the server instead:
//! the driver sends `response.interrupt` (Codex's instant interrupt), the
//! tail ends within milliseconds, and the lane keeps its connection and
//! the cache it holds. If the server refuses
//! (`response.interrupt.failed`: a model that does not support it), or
//! the request was not Lite or had no id yet, the connection closes,
//! which stops the response, and the lane goes on on another. Meanwhile
//! the driver skips the connection's frames up to the cancelled
//! response's terminal frame (`response.completed`, `response.failed`,
//! `response.incomplete` or `error`). The skip ends when that connection
//! closes.
//!
//! Recoveries are invisible to the caller
//! (`docs/reference/openai-websocket.md`, "Retries are invisible to the
//! caller"): when a lane resends a request, the error that caused it is
//! not forwarded, and the resent attempt's `Start` is dropped, so the
//! caller sees exactly one `Start` and no `Error`. An `Error` reaches the
//! caller only when the lane gives up.
//!
//! Events go to each caller over an unbounded channel, so a slow caller
//! never holds up another lane; backpressure within a run is the agent
//! loop's job.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use serde_json::Value;
use tokio::{
    sync::{mpsc, oneshot},
    time::Instant,
};

use super::connection::{
    self,
    ConnectionEvent,
    ConnectionHandle,
    Connector,
    Outgoing,
};
use crate::{
    event::{Accumulator, AssistantEvent, ErrorReason},
    message::{Timestamp, Usage},
    refusal::Refusal,
    responses::{input::response_items, stream::StreamProcessor},
    retry::Class,
    responses::request::LITE_MARKER,
    ws::proto::{
        continuation::Body,
        lane::Event,
        pool::{
            Affinity,
            ConnectionId,
            LaneId,
            Limits,
            Pool,
            PoolAction,
            PoolStats,
        },
    },
};

/// How often the driver advances the pool's clock for rotation.
const TICK: Duration = Duration::from_secs(1);

/// A handle on a running transport. Cloning shares the transport; it
/// stops when every handle and lane is dropped.
#[derive(Debug, Clone)]
pub struct Transport {
    commands: mpsc::UnboundedSender<Command>,
    /// The latest refusal, shared with the driver that records it.
    refusal: Arc<Mutex<Option<Refusal>>>,
}

/// A lane for one run. Dropping it closes the lane.
#[derive(Debug)]
pub struct LaneHandle {
    lane: LaneId,
    commands: mpsc::UnboundedSender<Command>,
}

/// The events of one request. Dropping it before the terminal event
/// cancels the request.
#[derive(Debug)]
pub struct Response {
    lane: LaneId,
    events: mpsc::UnboundedReceiver<AssistantEvent>,
    commands: mpsc::UnboundedSender<Command>,
    finished: bool,
}

/// The transport stopped before answering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the WebSocket transport stopped")]
pub struct Stopped;

#[derive(Debug)]
enum Command {
    OpenLane(Affinity, oneshot::Sender<LaneId>),
    CloseLane(LaneId),
    Request {
        lane: LaneId,
        body: Body,
        model: String,
        timestamp: Timestamp,
        events: mpsc::UnboundedSender<AssistantEvent>,
    },
    Cancel(LaneId),
    Stats(oneshot::Sender<PoolStats>),
}

impl Transport {
    /// Starts a transport on the current tokio runtime.
    pub fn start<C: Connector>(connector: C, limits: Limits) -> Self {
        let (commands, receiver) = mpsc::unbounded_channel();
        let (connection_events, connection_receiver) =
            mpsc::unbounded_channel();
        let refusal = Arc::new(Mutex::new(None));
        let driver = Driver {
            refusal: refusal.clone(),
            refused: HashMap::new(),
            connector: Arc::new(connector),
            pool: Pool::new(limits),
            connections: HashMap::new(),
            connection_events,
            lanes: HashMap::new(),
            skipping: HashMap::new(),
            interrupting: HashMap::new(),
            origin: Instant::now(),
        };
        tokio::spawn(driver.run(receiver, connection_receiver));
        Self { commands, refusal }
    }

    /// The latest refusal, if no response has completed since: a refused
    /// connection, or a response that failed with a documented plan-usage
    /// code, such as a usage limit.
    pub fn refusal(&self) -> Option<Refusal> {
        self.refusal.lock().expect("not poisoned").clone()
    }

    /// Opens a lane for a new run of `affinity`'s conversation.
    pub async fn open_lane(
        &self,
        affinity: Affinity,
    ) -> Result<LaneHandle, Stopped> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(Command::OpenLane(affinity, reply))
            .map_err(|_| Stopped)?;
        let lane = answer.await.map_err(|_| Stopped)?;
        Ok(LaneHandle {
            lane,
            commands: self.commands.clone(),
        })
    }

    /// The pool's counters.
    pub async fn stats(&self) -> Result<PoolStats, Stopped> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(Command::Stats(reply))
            .map_err(|_| Stopped)?;
        answer.await.map_err(|_| Stopped)
    }
}

impl LaneHandle {
    /// Sends the lane's next request, given in full; the lane decides
    /// whether it goes out as a delta. `model` and `timestamp` label the
    /// response's events.
    ///
    /// A lane holds one request at a time: the previous response must
    /// have finished or been dropped.
    pub fn request(
        &self,
        body: Body,
        model: String,
        timestamp: Timestamp,
    ) -> Response {
        let (events, receiver) = mpsc::unbounded_channel();
        let _ = self.commands.send(Command::Request {
            lane: self.lane,
            body,
            model,
            timestamp,
            events,
        });
        Response {
            lane: self.lane,
            events: receiver,
            commands: self.commands.clone(),
            finished: false,
        }
    }
}

impl Drop for LaneHandle {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::CloseLane(self.lane));
    }
}

impl Response {
    /// The next event; `None` after the terminal event.
    pub async fn next(&mut self) -> Option<AssistantEvent> {
        let event = self.events.recv().await;
        if matches!(
            event,
            Some(AssistantEvent::Done { .. } | AssistantEvent::Error { .. })
                | None
        ) {
            self.finished = true;
        }
        event
    }
}

impl Drop for Response {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.commands.send(Command::Cancel(self.lane));
        }
    }
}

struct Driver<C: Connector> {
    connector: Arc<C>,
    pool: Pool,
    connections: HashMap<ConnectionId, ConnectionHandle>,
    connection_events: mpsc::UnboundedSender<ConnectionEvent>,
    lanes: HashMap<LaneId, Active>,
    /// Connections whose cancelled responses are still streaming: how
    /// many terminal frames to skip on each.
    skipping: HashMap<ConnectionId, usize>,
    /// Responses asked to stop with `response.interrupt`, by id, with
    /// their connection: a refusal closes it.
    interrupting: HashMap<String, ConnectionId>,
    origin: Instant,
    refusal: Arc<Mutex<Option<Refusal>>>,
    /// Lanes whose connection was refused for a while: the error their
    /// request fails with if the reconnect fails too.
    refused: HashMap<LaneId, Refusal>,
}

/// A request in flight on a lane.
struct Active {
    events: mpsc::UnboundedSender<AssistantEvent>,
    model: String,
    timestamp: Timestamp,
    processor: StreamProcessor,
    accumulator: Accumulator,
    /// Whether the caller has seen `Start`.
    start_sent: bool,
    /// Whether the lane knows output has started.
    output_reported: bool,
    /// An error event held back while the lane decides whether to retry.
    held_error: Option<AssistantEvent>,
    /// The connection the current attempt was sent on, once sent.
    sent_on: Option<ConnectionId>,
    /// Whether the request went as Responses Lite, whose response the
    /// server can stop.
    lite: bool,
}

impl Active {
    fn new_attempt(&mut self) {
        self.processor =
            StreamProcessor::new(self.model.clone(), self.timestamp);
        self.accumulator = Accumulator::new();
        self.output_reported = false;
        self.held_error = None;
    }

    /// Forwards an event to the caller, dropping repeated `Start`s, and
    /// keeps the accumulator in step. The processor emits `Start` before
    /// anything else, even when the first frame is an error, so every
    /// stream the caller sees begins with one.
    fn forward(&mut self, event: AssistantEvent) {
        if matches!(event, AssistantEvent::Start { .. }) {
            if self.start_sent {
                // The accumulator of a resent attempt still needs it.
                let _ = self.accumulator.push(event);
                return;
            }
            self.start_sent = true;
        }
        let _ = self.accumulator.push(event.clone());
        let _ = self.events.send(event);
    }

    /// Ends the request with `refusal`'s error.
    fn refuse(&mut self, refusal: &Refusal) {
        if !self.start_sent {
            self.forward(AssistantEvent::Start {
                model: self.model.clone(),
                response_id: None,
                timestamp: self.timestamp,
            });
        }
        self.forward(AssistantEvent::Error {
            reason: ErrorReason::Error,
            message: refusal.message.clone(),
            usage: Usage::default(),
            class: refusal.class(),
        });
    }
}

impl<C: Connector> Driver<C> {
    async fn run(
        mut self,
        mut commands: mpsc::UnboundedReceiver<Command>,
        mut connection_events: mpsc::UnboundedReceiver<ConnectionEvent>,
    ) {
        let mut tick = tokio::time::interval(TICK);
        loop {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(command) => self.command(command),
                    // Every handle is gone: stop, closing all connections.
                    None => return,
                },
                Some(event) = connection_events.recv() => self.connection_event(event),
                _ = tick.tick() => {
                    let actions = self.pool.tick(self.origin.elapsed());
                    self.apply(actions);
                }
            }
        }
    }

    fn command(&mut self, command: Command) {
        match command {
            Command::OpenLane(affinity, reply) => {
                let (lane, actions) = self.pool.open_lane(affinity);
                self.apply(actions);
                let _ = reply.send(lane);
            }
            Command::CloseLane(lane) => {
                self.lanes.remove(&lane);
                self.refused.remove(&lane);
                if let Ok(actions) = self.pool.close_lane(lane) {
                    self.apply(actions);
                }
            }
            Command::Request {
                lane,
                body,
                model,
                timestamp,
                events,
            } => {
                let lite = body
                    .fields
                    .get("client_metadata")
                    .and_then(|metadata| metadata.get(LITE_MARKER))
                    .is_some();
                let active = Active {
                    lite,
                    events,
                    processor: StreamProcessor::new(model.clone(), timestamp),
                    model,
                    timestamp,
                    accumulator: Accumulator::new(),
                    start_sent: false,
                    output_reported: false,
                    held_error: None,
                    sent_on: None,
                };
                if self.lanes.contains_key(&lane) {
                    // One request at a time per lane; the caller broke that
                    // rule, so this request is refused.
                    return;
                }
                self.refused.remove(&lane);
                self.lanes.insert(lane, active);
                match self.pool.submit(lane, body) {
                    Ok(actions) => self.apply(actions),
                    Err(_) => {
                        self.lanes.remove(&lane);
                    }
                }
            }
            Command::Cancel(lane) => {
                let Some(active) = self.lanes.remove(&lane) else {
                    return;
                };
                let streaming =
                    active.sent_on.filter(|_| !active.processor.is_finished());
                if let Some(connection) = streaming {
                    *self.skipping.entry(connection).or_default() += 1;
                }
                // A Lite response with an id is stopped on the server;
                // any other goes with its connection.
                let interrupt = streaming
                    .filter(|_| active.lite)
                    .zip(active.processor.response_id())
                    .and_then(|(connection, id)| {
                        let handle = self.connections.get(&connection)?;
                        handle.send(Outgoing::Json(serde_json::json!({
                            "type": "response.interrupt",
                            "response_id": id,
                            "mode": "discard_partial_items",
                        })));
                        Some((id.to_owned(), connection))
                    });
                let actions = match interrupt {
                    Some((id, connection)) => {
                        self.interrupting.insert(id, connection);
                        self.pool.interrupted(lane)
                    }
                    None => self.pool.cancel(lane),
                };
                if let Ok(actions) = actions {
                    self.apply(actions);
                }
            }
            Command::Stats(reply) => {
                let _ = reply.send(self.pool.stats());
            }
        }
    }

    fn connection_event(&mut self, event: ConnectionEvent) {
        match event {
            ConnectionEvent::Opened { .. } => {}
            ConnectionEvent::Closed {
                connection,
                refusal,
            } => {
                self.skipping.remove(&connection);
                self.interrupting.retain(|_, on| *on != connection);
                if let Some(refusal) = &refusal {
                    *self.refusal.lock().expect("not poisoned") =
                        Some(refusal.clone());
                }
                if self.connections.remove(&connection).is_some() {
                    if let Some(refusal) = refusal {
                        self.refused_on(connection, refusal);
                    }
                    let actions = self.pool.connection_lost(connection);
                    self.apply(actions);
                }
            }
            ConnectionEvent::Frame { connection, frame } => {
                // Frames still queued from a connection the pool closed
                // (a stalled one) belong to requests resent elsewhere.
                if self.connections.contains_key(&connection) {
                    self.pool.activity(connection);
                    self.frame(connection, &frame);
                }
            }
        }
    }

    /// `connection` was refused. Requests waiting on it fail now unless
    /// the refusal is temporary; those keep it for their failure.
    fn refused_on(&mut self, connection: ConnectionId, refusal: Refusal) {
        let lanes: Vec<LaneId> = self
            .lanes
            .keys()
            .copied()
            .filter(|lane| self.pool.connection_of(*lane) == Some(connection))
            .collect();
        for lane in lanes {
            if refusal.class() == Class::Retryable {
                self.refused.insert(lane, refusal.clone());
                continue;
            }
            if let Some(mut active) = self.lanes.remove(&lane) {
                active.refuse(&refusal);
            }
            if let Ok(actions) = self.pool.cancel(lane) {
                self.apply(actions);
            }
        }
    }

    fn frame(&mut self, connection: ConnectionId, frame: &Value) {
        // A refusal belongs to the account, not to a lane.
        if let Some(refusal) = Refusal::from_frame(frame) {
            *self.refusal.lock().expect("not poisoned") = Some(refusal);
        }
        let kind = frame.get("type").and_then(Value::as_str);
        let response_id = frame
            .get("response_id")
            .or_else(|| frame.pointer("/response/id"))
            .and_then(Value::as_str);
        match kind {
            Some("response.interrupt.accepted") => {
                if let Some(id) = response_id {
                    self.interrupting.remove(id);
                }
                return;
            }
            // The model cannot stop it: its tail would hold up the
            // connection, so the connection goes, as without Lite.
            Some("response.interrupt.failed") => {
                if let Some(lost) =
                    response_id.and_then(|id| self.interrupting.remove(id))
                {
                    self.lose(lost);
                }
                return;
            }
            _ => {}
        }
        if let Some(remaining) = self.skipping.get_mut(&connection) {
            let terminal = matches!(
                kind,
                Some(
                    "response.completed"
                        | "response.failed"
                        | "response.incomplete"
                        | "error"
                )
            );
            if terminal {
                if let Some(id) = response_id {
                    self.interrupting.remove(id);
                }
                *remaining -= 1;
                if *remaining == 0 {
                    self.skipping.remove(&connection);
                }
            }
            return;
        }
        let Some(lane) = self.pool.lane_on(connection) else {
            return;
        };
        let Some(active) = self.lanes.get_mut(&lane) else {
            return;
        };
        let mut lane_events = Vec::new();
        for event in active.processor.push(frame) {
            match event {
                AssistantEvent::Error { .. } => {
                    active.held_error = Some(event);
                    lane_events.push(Event::ServerError {
                        code: active.processor.error_code().map(str::to_owned),
                    });
                }
                AssistantEvent::Done { .. } => {
                    *self.refusal.lock().expect("not poisoned") = None;
                    active.forward(event);
                    let message =
                        std::mem::take(&mut active.accumulator).finish();
                    if let Ok(message) = message {
                        lane_events.push(Event::Completed {
                            response_id: message
                                .response_id
                                .clone()
                                .unwrap_or_default(),
                            output_items: response_items(&message),
                        });
                    }
                }
                AssistantEvent::Start { .. } => active.forward(event),
                _ => {
                    if !active.output_reported {
                        active.output_reported = true;
                        lane_events.push(Event::Output);
                    }
                    active.forward(event);
                }
            }
        }
        for event in lane_events {
            let completed = matches!(event, Event::Completed { .. });
            if let Ok(actions) = self.pool.handle(lane, event) {
                self.apply(actions);
            }
            if completed {
                self.lanes.remove(&lane);
                self.refused.remove(&lane);
            }
        }
    }

    /// Closes `connection` as if it were lost: the pool sends a request
    /// waiting there elsewhere.
    fn lose(&mut self, connection: ConnectionId) {
        self.skipping.remove(&connection);
        self.interrupting.retain(|_, on| *on != connection);
        if self.connections.remove(&connection).is_some() {
            let actions = self.pool.connection_lost(connection);
            self.apply(actions);
        }
    }

    fn apply(&mut self, actions: Vec<PoolAction>) {
        for action in actions {
            match action {
                PoolAction::Open(connection) => {
                    let handle = connection::spawn(
                        connection,
                        self.connector.clone(),
                        self.connection_events.clone(),
                    );
                    self.connections.insert(connection, handle);
                }
                PoolAction::Close(connection) => {
                    self.connections.remove(&connection);
                    self.interrupting.retain(|_, on| *on != connection);
                }
                PoolAction::Send {
                    connection,
                    lane,
                    body,
                } => {
                    let Some(active) = self.lanes.get_mut(&lane) else {
                        continue;
                    };
                    active.new_attempt();
                    active.sent_on = Some(connection);
                    if let Some(handle) = self.connections.get(&connection) {
                        handle.send(Outgoing::Request(body));
                    }
                }
                PoolAction::Fail { lane, .. } => {
                    let Some(mut active) = self.lanes.remove(&lane) else {
                        continue;
                    };
                    if let Some(refusal) = self.refused.remove(&lane)
                        && active.held_error.is_none()
                    {
                        active.refuse(&refusal);
                        continue;
                    }
                    let events = match active.held_error.take() {
                        Some(error) => vec![error],
                        None => active.processor.close(),
                    };
                    for event in events {
                        active.forward(event);
                    }
                }
            }
        }
    }
}
