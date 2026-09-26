//! The connection pool: which lane lives on which connection, and when a
//! request may be sent.
//!
//! OpenAI's WebSocket mode limits each connection to 32 named lanes and
//! 16 in-flight responses across them (`docs/reference/openai-websocket.md`,
//! "Limits and lanes"). The pool keeps both:
//!
//! - A new lane goes on the oldest usable connection that has fewer than
//!   `max_lanes` lanes and fewer than `max_in_flight` requests in flight.
//!   If there is none, the pool opens a connection.
//! - A lane stays on its connection, because its continuation lives
//!   there. A request submitted while its connection already has
//!   `max_in_flight` requests in flight waits, in submission order, until
//!   one finishes.
//! - When a lane must reconnect (see the recovery ladder in
//!   [`lane`](super::lane)), it moves to another connection by the same
//!   placement rule. A connection that answered
//!   `websocket_connection_limit_reached` takes no new lanes.
//! - From `rotate_after` (55 minutes, against OpenAI's 60-minute cap) a
//!   connection drains: it takes no new lanes, its idle and waiting lanes
//!   move at once, and a busy lane moves when its request finishes. An
//!   empty draining connection is closed. A lane that moves loses its
//!   continuation, so its next request is a full resend.
//! - When a connection is lost, every lane on it moves. A request that
//!   had produced no output is resent in full on the new connection; one
//!   that had fails.
//!
//! Like [`lane`](super::lane), the pool does no I/O: it returns
//! [`PoolAction`]s for the driver to carry out, in order. The driver may
//! queue a `Send` for a connection it is still opening. Time comes in
//! through [`Pool::tick`]; the pool never reads a clock.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    time::Duration,
};

use super::{
    continuation::Body,
    lane::{self, Event, Lane, LaneStats},
};

pub type ConnectionId = u64;
pub type LaneId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_lanes: usize,
    pub max_in_flight: usize,
    /// Age at which a connection starts draining.
    pub rotate_after: Duration,
}

impl Default for Limits {
    /// OpenAI's limits: 32 named lanes and 16 in-flight responses per
    /// connection, and rotation 5 minutes before the 60-minute cap, the
    /// margin pi uses.
    fn default() -> Self {
        Self {
            max_lanes: 32,
            max_in_flight: 16,
            rotate_after: Duration::from_secs(55 * 60),
        }
    }
}

/// What the driver must do.
#[derive(Debug, Clone, PartialEq)]
pub enum PoolAction {
    /// Open a new connection with this id.
    Open(ConnectionId),
    /// Close this connection; no lane uses it any more.
    Close(ConnectionId),
    /// Send a request for `lane` on `connection`.
    Send {
        connection: ConnectionId,
        lane: LaneId,
        body: Body,
    },
    /// Give up on `lane`'s current request.
    Fail {
        lane: LaneId,
        failure: lane::Failure,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolError {
    UnknownLane(LaneId),
    /// The lane already has a request submitted or in flight.
    Busy(LaneId),
}

impl std::fmt::Display for PoolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownLane(lane) => write!(f, "unknown lane {lane}"),
            Self::Busy(lane) => write!(f, "lane {lane} already has a request"),
        }
    }
}

impl std::error::Error for PoolError {}

/// Counters for the whole pool. Lane counters are summed over every lane
/// the pool has had, including closed ones; `last_delta_items` is the
/// size of the pool's last delta request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PoolStats {
    pub lanes: LaneStats,
    pub connections_opened: u64,
    /// Lanes placed on a connection that was already open.
    pub connections_reused: u64,
}

#[derive(Debug, Default)]
struct Connection {
    opened_at: Duration,
    /// Takes no new lanes: it reached `rotate_after`, or the server
    /// reported its age limit.
    draining: bool,
    lanes: BTreeSet<LaneId>,
    in_flight: usize,
    /// Lanes whose submitted request waits for an in-flight slot.
    waiting: VecDeque<LaneId>,
}

#[derive(Debug)]
struct Slot {
    connection: ConnectionId,
    lane: Lane,
    /// A submitted request waiting for an in-flight slot.
    queued: Option<Body>,
}

#[derive(Debug, Default)]
pub struct Pool {
    limits: Limits,
    connections: BTreeMap<ConnectionId, Connection>,
    lanes: BTreeMap<LaneId, Slot>,
    next_connection: ConnectionId,
    next_lane: LaneId,
    /// Counters of lanes that have been closed.
    closed_lane_stats: LaneStats,
    connections_opened: u64,
    connections_reused: u64,
    last_delta_items: u64,
    now: Duration,
}

impl Pool {
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            ..Self::default()
        }
    }

    /// Opens a lane for a new run and places it on a connection.
    pub fn open_lane(&mut self) -> (LaneId, Vec<PoolAction>) {
        let lane = self.next_lane;
        self.next_lane += 1;
        let (connection, actions) = self.place(lane);
        self.lanes.insert(
            lane,
            Slot {
                connection,
                lane: Lane::new(),
                queued: None,
            },
        );
        (lane, actions)
    }

    /// Closes a lane when its run ends. A request still in flight or
    /// waiting is dropped, as on cancel.
    pub fn close_lane(
        &mut self,
        lane: LaneId,
    ) -> Result<Vec<PoolAction>, PoolError> {
        let mut actions = self.cancel(lane)?;
        let slot = self.lanes.remove(&lane).expect("cancel checked the lane");
        add_stats(&mut self.closed_lane_stats, slot.lane.stats());
        if let Some(connection) = self.connections.get_mut(&slot.connection) {
            connection.lanes.remove(&lane);
        }
        actions.extend(self.drain_waiting(slot.connection));
        actions.extend(self.retire_if_empty(slot.connection));
        Ok(actions)
    }

    /// Submits `lane`'s next request, given as it would be sent in full.
    pub fn submit(
        &mut self,
        lane: LaneId,
        full_body: Body,
    ) -> Result<Vec<PoolAction>, PoolError> {
        let slot = self
            .lanes
            .get_mut(&lane)
            .ok_or(PoolError::UnknownLane(lane))?;
        if slot.queued.is_some() || slot.lane.is_busy() {
            return Err(PoolError::Busy(lane));
        }
        let connection = self
            .connections
            .get_mut(&slot.connection)
            .expect("a lane's connection exists");
        if connection.in_flight >= self.limits.max_in_flight {
            slot.queued = Some(full_body);
            connection.waiting.push_back(lane);
            return Ok(Vec::new());
        }
        Ok(self.start(lane, full_body))
    }

    /// Cancels `lane`'s request, whether it is in flight or still waiting.
    pub fn cancel(
        &mut self,
        lane: LaneId,
    ) -> Result<Vec<PoolAction>, PoolError> {
        let slot = self
            .lanes
            .get_mut(&lane)
            .ok_or(PoolError::UnknownLane(lane))?;
        let connection = slot.connection;
        if slot.queued.take().is_some() {
            if let Some(c) = self.connections.get_mut(&connection) {
                c.waiting.retain(|&l| l != lane);
            }
            return Ok(Vec::new());
        }
        Ok(self.apply(lane, Event::Cancel))
    }

    /// Applies an event for `lane`'s current request, such as a frame the
    /// driver routed to it by `stream_id`.
    pub fn handle(
        &mut self,
        lane: LaneId,
        event: Event,
    ) -> Result<Vec<PoolAction>, PoolError> {
        if !self.lanes.contains_key(&lane) {
            return Err(PoolError::UnknownLane(lane));
        }
        Ok(self.apply(lane, event))
    }

    /// Reports that `connection` closed or failed. Every lane on it moves
    /// to another connection.
    pub fn connection_lost(
        &mut self,
        connection: ConnectionId,
    ) -> Vec<PoolAction> {
        let Some(lost) = self.connections.remove(&connection) else {
            return Vec::new();
        };
        let mut actions = Vec::new();
        for lane in lost.lanes {
            let slot = self.lanes.get_mut(&lane).expect("a connection's lane");
            let queued = slot.queued.take();
            let action = slot.lane.handle(Event::ConnectionLost);
            let (moved_to, open) = self.place(lane);
            actions.extend(open);
            self.lanes.get_mut(&lane).expect("known lane").connection =
                moved_to;
            match action {
                Some(lane::Action::Reconnect) => {
                    // The lane already sits on its new connection, so the
                    // reconnect is done: resend there.
                    self.connections
                        .get_mut(&moved_to)
                        .expect("just placed")
                        .in_flight += 1;
                    let resend = self
                        .lanes
                        .get_mut(&lane)
                        .expect("known lane")
                        .lane
                        .handle(Event::Reconnected);
                    actions.extend(self.follow(lane, resend));
                }
                other => actions.extend(self.follow(lane, other)),
            }
            if let Some(body) = queued {
                actions.extend(
                    self.submit(lane, body).expect("a waiting lane is idle"),
                );
            }
        }
        actions
    }

    /// Advances the pool's clock to `now`, the time since an arbitrary
    /// origin, and rotates connections that reached `rotate_after`.
    pub fn tick(&mut self, now: Duration) -> Vec<PoolAction> {
        self.now = self.now.max(now);
        let aged: Vec<ConnectionId> = self
            .connections
            .iter()
            .filter(|(_, c)| {
                self.now.saturating_sub(c.opened_at) >= self.limits.rotate_after
            })
            .map(|(id, _)| *id)
            .collect();
        let mut actions = Vec::new();
        for connection in aged {
            if let Some(c) = self.connections.get_mut(&connection) {
                c.draining = true;
            }
            actions.extend(self.evacuate(connection));
        }
        actions
    }

    /// Whether `connection` is open and draining.
    pub fn is_draining(&self, connection: ConnectionId) -> bool {
        self.connections
            .get(&connection)
            .is_some_and(|c| c.draining)
    }

    /// Counters over every lane the pool has had.
    pub fn stats(&self) -> PoolStats {
        let mut lanes = self.closed_lane_stats.clone();
        for slot in self.lanes.values() {
            add_stats(&mut lanes, slot.lane.stats());
        }
        lanes.last_delta_items = self.last_delta_items;
        PoolStats {
            lanes,
            connections_opened: self.connections_opened,
            connections_reused: self.connections_reused,
        }
    }

    /// Requests in flight on `connection`.
    pub fn in_flight(&self, connection: ConnectionId) -> usize {
        self.connections.get(&connection).map_or(0, |c| c.in_flight)
    }

    /// Lanes placed on `connection`.
    pub fn lane_count(&self, connection: ConnectionId) -> usize {
        self.connections
            .get(&connection)
            .map_or(0, |c| c.lanes.len())
    }

    /// The connection `lane` lives on.
    pub fn connection_of(&self, lane: LaneId) -> Option<ConnectionId> {
        self.lanes.get(&lane).map(|slot| slot.connection)
    }

    /// Whether `lane` has a request in flight or waiting.
    pub fn is_busy(&self, lane: LaneId) -> bool {
        self.lanes
            .get(&lane)
            .is_some_and(|slot| slot.lane.is_busy() || slot.queued.is_some())
    }

    /// Sends a request for `lane`, which must be idle, and counts it in
    /// flight.
    fn start(&mut self, lane: LaneId, full_body: Body) -> Vec<PoolAction> {
        let slot = self.lanes.get_mut(&lane).expect("start on a known lane");
        let action =
            slot.lane.submit(full_body).expect("start on an idle lane");
        let connection = slot.connection;
        self.connections
            .get_mut(&connection)
            .expect("a lane's connection exists")
            .in_flight += 1;
        self.follow(lane, Some(action))
    }

    /// Feeds an event to a lane, keeps the in-flight count in step with
    /// the lane, and starts waiting requests when a slot frees up.
    fn apply(&mut self, lane: LaneId, event: Event) -> Vec<PoolAction> {
        let slot = self.lanes.get_mut(&lane).expect("apply on a known lane");
        let was_busy = slot.lane.is_busy();
        let action = slot.lane.handle(event);
        let connection = slot.connection;
        let mut actions = Vec::new();
        if was_busy && !self.lanes[&lane].lane.is_busy() {
            self.connections
                .get_mut(&connection)
                .expect("a lane's connection exists")
                .in_flight -= 1;
            actions.extend(self.follow(lane, action));
            actions.extend(self.drain_waiting(connection));
            if self.is_draining(connection) {
                actions.extend(self.evacuate(connection));
            }
        } else {
            actions.extend(self.follow(lane, action));
        }
        actions
    }

    /// Carries a lane action into pool actions. A reconnect moves the lane,
    /// with its in-flight request, to another connection and resends.
    fn follow(
        &mut self,
        lane: LaneId,
        action: Option<lane::Action>,
    ) -> Vec<PoolAction> {
        let connection = self.lanes[&lane].connection;
        match action {
            None => Vec::new(),
            Some(lane::Action::Send(body)) => {
                if body.contains_key("previous_response_id") {
                    self.last_delta_items = body["input"]
                        .as_array()
                        .map_or(0, |items| items.len() as u64);
                }
                vec![PoolAction::Send {
                    connection,
                    lane,
                    body,
                }]
            }
            Some(lane::Action::Fail(failure)) => {
                vec![PoolAction::Fail { lane, failure }]
            }
            Some(lane::Action::Reconnect) => {
                let mut actions = Vec::new();
                if let Some(old) = self.connections.get_mut(&connection) {
                    // A live connection only sends a lane away when the
                    // server reported its age limit.
                    old.draining = true;
                    old.lanes.remove(&lane);
                    old.in_flight -= 1;
                    actions.extend(self.evacuate(connection));
                }
                let (moved_to, open) = self.place(lane);
                actions.extend(open);
                self.lanes.get_mut(&lane).expect("known lane").connection =
                    moved_to;
                self.connections
                    .get_mut(&moved_to)
                    .expect("just placed")
                    .in_flight += 1;
                let resend = self
                    .lanes
                    .get_mut(&lane)
                    .expect("known lane")
                    .lane
                    .handle(Event::Reconnected);
                actions.extend(self.follow(lane, resend));
                actions
            }
        }
    }

    /// Picks a connection for `lane`, opening one if none has room, and
    /// records the lane on it.
    fn place(&mut self, lane: LaneId) -> (ConnectionId, Vec<PoolAction>) {
        let limits = self.limits;
        let found = self
            .connections
            .iter()
            .find(|(_, c)| {
                !c.draining
                    && c.lanes.len() < limits.max_lanes
                    && c.in_flight < limits.max_in_flight
            })
            .map(|(id, _)| *id);
        let mut actions = Vec::new();
        let connection = match found {
            Some(id) => {
                self.connections_reused += 1;
                id
            }
            None => {
                let id = self.next_connection;
                self.next_connection += 1;
                self.connections.insert(
                    id,
                    Connection {
                        opened_at: self.now,
                        ..Connection::default()
                    },
                );
                self.connections_opened += 1;
                actions.push(PoolAction::Open(id));
                id
            }
        };
        self.connections
            .get_mut(&connection)
            .expect("just placed")
            .lanes
            .insert(lane);
        (connection, actions)
    }

    /// Moves every lane without a request in flight off a draining
    /// `connection`, resubmitting waiting requests on the new connection,
    /// and closes the connection once it is empty.
    fn evacuate(&mut self, connection: ConnectionId) -> Vec<PoolAction> {
        let movable: Vec<LaneId> = self
            .connections
            .get(&connection)
            .map(|c| {
                c.lanes
                    .iter()
                    .copied()
                    .filter(|lane| !self.lanes[lane].lane.is_busy())
                    .collect()
            })
            .unwrap_or_default();
        let mut actions = Vec::new();
        for lane in movable {
            if let Some(c) = self.connections.get_mut(&connection) {
                c.lanes.remove(&lane);
                c.waiting.retain(|&l| l != lane);
            }
            let (moved_to, open) = self.place(lane);
            actions.extend(open);
            let slot = self.lanes.get_mut(&lane).expect("a connection's lane");
            slot.connection = moved_to;
            // The new connection holds nothing for this lane.
            slot.lane.handle(Event::Reconnected);
            if let Some(body) = slot.queued.take() {
                actions.extend(
                    self.submit(lane, body).expect("a waiting lane is idle"),
                );
            }
        }
        actions.extend(self.retire_if_empty(connection));
        actions
    }

    /// Closes a draining connection that no lane uses any more.
    fn retire_if_empty(&mut self, connection: ConnectionId) -> Vec<PoolAction> {
        match self.connections.get(&connection) {
            Some(c) if c.draining && c.lanes.is_empty() => {
                self.connections.remove(&connection);
                vec![PoolAction::Close(connection)]
            }
            _ => Vec::new(),
        }
    }

    /// Starts waiting requests on `connection` while it has free slots.
    fn drain_waiting(&mut self, connection: ConnectionId) -> Vec<PoolAction> {
        let mut actions = Vec::new();
        while let Some(c) = self.connections.get_mut(&connection) {
            if c.in_flight >= self.limits.max_in_flight {
                break;
            }
            let Some(lane) = c.waiting.pop_front() else {
                break;
            };
            let body = self
                .lanes
                .get_mut(&lane)
                .and_then(|slot| slot.queued.take())
                .expect("a waiting lane has a queued request");
            actions.extend(self.start(lane, body));
        }
        actions
    }
}

fn add_stats(total: &mut LaneStats, stats: &LaneStats) {
    total.full_requests += stats.full_requests;
    total.delta_requests += stats.delta_requests;
    total.previous_response_not_found += stats.previous_response_not_found;
    total.connection_limit_reached += stats.connection_limit_reached;
    total.connection_lost += stats.connection_lost;
}
