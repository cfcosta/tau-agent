//! The driver: one task that runs the [`Pool`] and its connection tasks,
//! routes server frames to lanes, and turns them into [`AssistantEvent`]s
//! for the runs.
//!
//! Each lane gets a `stream_id` (`tau-<lane>`), which the driver writes
//! into every request it sends and reads back from every frame it
//! receives. Frames for a lane with no request in flight are ignored.
//!
//! A cancelled request that was already sent keeps streaming on the
//! server. Its tail arrives on the same `stream_id`, before the frames of
//! the lane's next request, because the server answers a lane's requests
//! in order. So after such a cancel, the driver skips the lane's frames
//! until the cancelled response's terminal frame (`response.completed`,
//! `response.failed`, `response.incomplete` or `error`) has gone by. The
//! skip ends early if that connection closes.
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

use std::{collections::HashMap, sync::Arc, time::Duration};

use serde_json::{Value, json};
use tokio::{
    sync::{mpsc, oneshot},
    time::Instant,
};

use super::connection::{self, ConnectionEvent, ConnectionHandle, Connector};
use crate::{
    event::{Accumulator, AssistantEvent},
    message::Timestamp,
    responses::{input::response_items, stream::StreamProcessor},
    ws::proto::{
        continuation::Body,
        lane::Event,
        pool::{ConnectionId, LaneId, Limits, Pool, PoolAction, PoolStats},
    },
};

/// How often the driver advances the pool's clock for rotation.
const TICK: Duration = Duration::from_secs(1);

/// A handle on a running transport. Cloning shares the transport; it
/// stops when every handle and lane is dropped.
#[derive(Debug, Clone)]
pub struct Transport {
    commands: mpsc::UnboundedSender<Command>,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stopped;

impl std::fmt::Display for Stopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the WebSocket transport stopped")
    }
}

impl std::error::Error for Stopped {}

#[derive(Debug)]
enum Command {
    OpenLane(oneshot::Sender<LaneId>),
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
        let driver = Driver {
            connector: Arc::new(connector),
            pool: Pool::new(limits),
            connections: HashMap::new(),
            connection_events,
            lanes: HashMap::new(),
            skipping: HashMap::new(),
            origin: Instant::now(),
        };
        tokio::spawn(driver.run(receiver, connection_receiver));
        Self { commands }
    }

    /// Opens a lane for a new run.
    pub async fn open_lane(&self) -> Result<LaneHandle, Stopped> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(Command::OpenLane(reply))
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
    /// Lanes whose cancelled responses are still streaming: the
    /// connection they stream on, and how many terminal frames to skip.
    skipping: HashMap<LaneId, (ConnectionId, usize)>,
    origin: Instant,
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
}

fn stream_id(lane: LaneId) -> String {
    format!("tau-{lane}")
}

fn lane_of(frame: &Value) -> Option<LaneId> {
    frame
        .get("stream_id")?
        .as_str()?
        .strip_prefix("tau-")?
        .parse()
        .ok()
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
            Command::OpenLane(reply) => {
                let (lane, actions) = self.pool.open_lane();
                self.apply(actions);
                let _ = reply.send(lane);
            }
            Command::CloseLane(lane) => {
                self.lanes.remove(&lane);
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
                let active = Active {
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
                if let Some(connection) = active.sent_on
                    && !active.processor.is_finished()
                {
                    let entry =
                        self.skipping.entry(lane).or_insert((connection, 0));
                    *entry = (connection, entry.1 + 1);
                }
                if let Ok(actions) = self.pool.cancel(lane) {
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
            ConnectionEvent::Closed { connection } => {
                self.skipping.retain(|_, (on, _)| *on != connection);
                if self.connections.remove(&connection).is_some() {
                    let actions = self.pool.connection_lost(connection);
                    self.apply(actions);
                }
            }
            ConnectionEvent::Frame { connection, frame } => {
                // Frames still queued from a connection the pool closed
                // (a stalled one) belong to requests resent elsewhere.
                if self.connections.contains_key(&connection) {
                    self.pool.activity(connection);
                    self.frame(&frame);
                }
            }
        }
    }

    fn frame(&mut self, frame: &Value) {
        let Some(lane) = lane_of(frame) else { return };
        if let Some((_, remaining)) = self.skipping.get_mut(&lane) {
            let terminal = matches!(
                frame.get("type").and_then(Value::as_str),
                Some(
                    "response.completed"
                        | "response.failed"
                        | "response.incomplete"
                        | "error"
                )
            );
            if terminal {
                *remaining -= 1;
                if *remaining == 0 {
                    self.skipping.remove(&lane);
                }
            }
            return;
        }
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
            }
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
                }
                PoolAction::Send {
                    connection,
                    lane,
                    mut body,
                } => {
                    let Some(active) = self.lanes.get_mut(&lane) else {
                        continue;
                    };
                    active.new_attempt();
                    active.sent_on = Some(connection);
                    body.insert("stream_id".into(), json!(stream_id(lane)));
                    if let Some(handle) = self.connections.get(&connection) {
                        handle.send(Value::Object(body));
                    }
                }
                PoolAction::Fail { lane, .. } => {
                    let Some(mut active) = self.lanes.remove(&lane) else {
                        continue;
                    };
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
