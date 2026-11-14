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
use statig::blocking::{IntoStateMachineExt as _, StateMachine};
use tokio::{
    sync::{mpsc, oneshot},
    time::Instant,
};

use self::request::{Effects, Input, Request};
use super::connection::{
    self,
    ConnectionEvent,
    ConnectionHandle,
    Connector,
    Outgoing,
};
use crate::{
    event::AssistantEvent,
    message::Timestamp,
    refusal::Refusal,
    responses::request::LITE_MARKER,
    retry::Class,
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

mod request;

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
    /// Cancels the lane's request, if its caller dropped the response.
    Cancel(LaneId),
    Stats(oneshot::Sender<PoolStats>),
    CacheLapse(Arc<str>, oneshot::Sender<Option<CacheLapse>>),
}

/// When a conversation's prompt cache lapses: see
/// [`Transport::cache_lapse`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheLapse {
    /// When its connection last took a request.
    pub last_used: Instant,
    /// When its cache lapses.
    pub at: Instant,
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

    /// When the prompt cache of `path`'s conversation lapses, while it
    /// is idle: [`Pool::cache_lapse`]. `None` when no free connection
    /// serves it, or one of its lanes holds one.
    pub async fn cache_lapse(
        &self,
        path: &str,
    ) -> Result<Option<CacheLapse>, Stopped> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(Command::CacheLapse(path.into(), reply))
            .map_err(|_| Stopped)?;
        answer.await.map_err(|_| Stopped)
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
    /// Cancels the request, unless it finished. Closed first, so the
    /// driver tells it from the lane's next request, which a response
    /// whose `Done` it never read may outlive.
    fn drop(&mut self) {
        self.events.close();
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
    /// The request in flight on each lane, until it is over.
    lanes: HashMap<LaneId, StateMachine<Request>>,
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
                if self.lanes.contains_key(&lane) {
                    // One request at a time per lane; the caller broke that
                    // rule, so this request is refused.
                    return;
                }
                self.refused.remove(&lane);
                self.lanes.insert(
                    lane,
                    Request::new(events, model, timestamp, lite)
                        .state_machine(),
                );
                match self.pool.submit(lane, body) {
                    Ok(actions) => self.apply(actions),
                    Err(_) => {
                        self.lanes.remove(&lane);
                    }
                }
            }
            Command::Cancel(lane) => {
                // The lane's request now may be a later one than the
                // dropped response's.
                if !self
                    .lanes
                    .get(&lane)
                    .is_some_and(|request| request.inner().abandoned())
                {
                    return;
                }
                let Some(effects) = self.feed(lane, &Input::Cancel) else {
                    return;
                };
                // A Lite response with an id is stopped on the server;
                // any other goes with its connection.
                let interrupt = effects.cut.and_then(|cut| {
                    *self.skipping.entry(cut.on).or_default() += 1;
                    let id = cut.interrupt?;
                    let handle = self.connections.get(&cut.on)?;
                    handle.send(Outgoing::Json(serde_json::json!({
                        "type": "response.interrupt",
                        "response_id": id,
                        "mode": "discard_partial_items",
                    })));
                    Some((id, cut.on))
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
            Command::CacheLapse(path, reply) => {
                let origin = self.origin;
                let _ = reply.send(self.pool.cache_lapse(&path).map(|lapse| {
                    CacheLapse {
                        last_used: origin + lapse.last_used,
                        at: origin + lapse.at,
                    }
                }));
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
            self.feed(lane, &Input::Refused(&refusal));
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
        let Some(effects) = self.feed(lane, &Input::Frame(frame)) else {
            return;
        };
        self.report(lane, effects.lane);
    }

    /// Gives `input` to the request on `lane`, dropping it once it is
    /// over; `None` when the lane has none.
    fn feed(&mut self, lane: LaneId, input: &Input<'_>) -> Option<Effects> {
        let request = self.lanes.get_mut(&lane)?;
        let mut effects = Effects::default();
        request.handle_with_context(input, &mut effects);
        if matches!(request.state(), request::State::Over {}) {
            self.lanes.remove(&lane);
            self.refused.remove(&lane);
        }
        Some(effects)
    }

    /// Tells the pool what happened on `lane`, in order.
    fn report(&mut self, lane: LaneId, events: Vec<Event>) {
        for event in events {
            if matches!(event, Event::Completed { .. }) {
                *self.refusal.lock().expect("not poisoned") = None;
            }
            if let Ok(actions) = self.pool.handle(lane, event) {
                self.apply(actions);
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
                    if self.feed(lane, &Input::Sent(connection)).is_none() {
                        continue;
                    }
                    if let Some(handle) = self.connections.get(&connection) {
                        handle.send(Outgoing::Request(body));
                    }
                }
                PoolAction::Fail { lane, .. } => {
                    let refusal = self.refused.get(&lane).cloned();
                    self.feed(lane, &Input::Failed(refusal.as_ref()));
                }
            }
        }
    }
}
