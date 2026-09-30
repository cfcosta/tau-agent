//! The connection pool: which lane lives on which connection, and when a
//! connection opens, rotates and closes.
//!
//! tau sends no `stream_id`, so a connection carries one lane, and a lane
//! one request at a time (`docs/reference/openai-websocket.md`, "Limits
//! and lanes"). A frame belongs to the lane of the connection it came on.
//!
//! - A new lane goes on the oldest connection that has no lane and is not
//!   draining, such as one a finished run left open. If there is none,
//!   the pool opens a connection.
//! - A lane stays on its connection, because its continuation lives
//!   there.
//! - When a lane must reconnect (see the recovery ladder in
//!   [`lane`](super::lane)), it moves to another connection by the same
//!   placement rule. A connection that answered
//!   `websocket_connection_limit_reached` takes no new lane, and closes.
//! - From `rotate_after` (55 minutes, against OpenAI's 60-minute cap) a
//!   connection drains: it takes no new lane, an idle lane moves at once,
//!   and a busy lane moves when its request finishes. An empty draining
//!   connection is closed. A lane that moves loses its continuation, so
//!   its next request is a full resend.
//! - A connection that has had no lane for `idle_timeout` (5 minutes,
//!   pi's cache lifetime) is closed. One that still has a lane stays: it
//!   holds the lane's continuation, and closing it would only turn the
//!   next request into a full resend. Rotation bounds its age.
//! - A connection with a request in flight that has received nothing for
//!   `stall_timeout` (5 minutes, pi's idle timeout) is treated as lost:
//!   the driver closes it and its lane moves as below. A request that
//!   had produced no output is resent, which is pi's "idle before the
//!   first event" case; one that had fails.
//! - When a connection is lost, its lane moves. A request that had
//!   produced no output is resent in full on the new connection; one
//!   that had fails.
//! - A new lane's connection opens at once, so it is ready by the first
//!   request; so does one a lane moves to for rotation. One a lane moves
//!   to after a lost connection opens only when a request is sent on it:
//!   a connection that keeps failing to open (the network is down, or the
//!   server refuses the upgrade) is tried again when a run asks for
//!   something, never in a loop behind an idle lane. A connection that
//!   was never opened is dropped without a `Close`.
//!
//! Like [`lane`](super::lane), the pool does no I/O: it returns
//! [`PoolAction`]s for the driver to carry out, in order. The driver may
//! queue a `Send` for a connection it is still opening. Time comes in
//! through [`Pool::tick`]; the pool never reads a clock.

use std::{collections::BTreeMap, time::Duration};

use super::{
    continuation::Body,
    lane::{self, Event, Lane, LaneStats},
};

pub type ConnectionId = u64;
pub type LaneId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Age at which a connection starts draining.
    pub rotate_after: Duration,
    /// How long a connection with no lane stays open.
    pub idle_timeout: Duration,
    /// How long a connection with a request in flight may go without
    /// receiving anything before it is presumed dead.
    pub stall_timeout: Duration,
}

impl Default for Limits {
    /// Rotation 5 minutes before OpenAI's 60-minute cap, the margin pi
    /// uses, and pi's 5-minute idle and stall timeouts.
    fn default() -> Self {
        Self {
            rotate_after: Duration::from_secs(55 * 60),
            idle_timeout: Duration::from_secs(5 * 60),
            stall_timeout: Duration::from_secs(5 * 60),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PoolError {
    #[error("unknown lane {0}")]
    UnknownLane(LaneId),
    /// The lane already has a request in flight.
    #[error("lane {0} already has a request")]
    Busy(LaneId),
}

/// Counters for the whole pool. Lane counters are summed over every lane
/// the pool has had, including closed ones; `last_delta_items` is the
/// size of the pool's last delta request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PoolStats {
    pub lanes: LaneStats,
    pub connections_opened: u64,
    /// Lanes placed on a connection that already existed.
    pub connections_reused: u64,
}

#[derive(Debug, Default)]
struct Connection {
    opened_at: Duration,
    /// Whether the driver was told to open it.
    open: bool,
    /// Takes no new lane: it reached `rotate_after`, or the server
    /// reported its age limit.
    draining: bool,
    lane: Option<LaneId>,
    /// When the connection's lane left, if it has none.
    empty_since: Option<Duration>,
    /// When a request was last sent on it or anything last came in.
    /// Only read while a request is in flight, and every request sent
    /// sets it.
    last_activity: Duration,
}

#[derive(Debug)]
struct Slot {
    connection: ConnectionId,
    lane: Lane,
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
        let (connection, actions) = self.place(lane, true);
        self.lanes.insert(
            lane,
            Slot {
                connection,
                lane: Lane::new(),
            },
        );
        (lane, actions)
    }

    /// Closes a lane when its run ends. A request still in flight is
    /// dropped, as on cancel. The connection stays open for the next
    /// lane, up to `idle_timeout`.
    pub fn close_lane(
        &mut self,
        lane: LaneId,
    ) -> Result<Vec<PoolAction>, PoolError> {
        let mut actions = self.cancel(lane)?;
        let slot = self.lanes.remove(&lane).expect("cancel checked the lane");
        add_stats(&mut self.closed_lane_stats, slot.lane.stats());
        if let Some(connection) = self.connections.get_mut(&slot.connection) {
            connection.lane = None;
            connection.empty_since = Some(self.now);
        }
        actions.extend(self.retire_if_empty(slot.connection));
        Ok(actions)
    }

    /// Submits `lane`'s next request, given as it would be sent in full.
    pub fn submit(
        &mut self,
        lane: LaneId,
        full_body: Body,
    ) -> Result<Vec<PoolAction>, PoolError> {
        let slot = self.lanes.get(&lane).ok_or(PoolError::UnknownLane(lane))?;
        if slot.lane.is_busy() {
            return Err(PoolError::Busy(lane));
        }
        Ok(self.start(lane, full_body))
    }

    /// Cancels `lane`'s request, if it has one.
    pub fn cancel(
        &mut self,
        lane: LaneId,
    ) -> Result<Vec<PoolAction>, PoolError> {
        if !self.lanes.contains_key(&lane) {
            return Err(PoolError::UnknownLane(lane));
        }
        Ok(self.apply(lane, Event::Cancel))
    }

    /// Applies an event for `lane`'s current request, such as a frame that
    /// came on its connection. It counts as activity on that connection.
    pub fn handle(
        &mut self,
        lane: LaneId,
        event: Event,
    ) -> Result<Vec<PoolAction>, PoolError> {
        let Some(slot) = self.lanes.get(&lane) else {
            return Err(PoolError::UnknownLane(lane));
        };
        self.activity(slot.connection);
        Ok(self.apply(lane, event))
    }

    /// Records that something arrived on `connection`, as of the last
    /// tick. The driver calls this for every frame.
    pub fn activity(&mut self, connection: ConnectionId) {
        if let Some(c) = self.connections.get_mut(&connection) {
            c.last_activity = self.now;
        }
    }

    /// Reports that `connection` closed or failed. Its lane moves to
    /// another connection.
    pub fn connection_lost(
        &mut self,
        connection: ConnectionId,
    ) -> Vec<PoolAction> {
        let Some(lost) = self.connections.remove(&connection) else {
            return Vec::new();
        };
        let Some(lane) = lost.lane else {
            return Vec::new();
        };
        let action = self.slot(lane).lane.handle(Event::ConnectionLost);
        let (moved_to, mut actions) = self.place(lane, false);
        self.slot(lane).connection = moved_to;
        match action {
            Some(lane::Action::Reconnect) => {
                // The lane already sits on its new connection, so the
                // reconnect is done: resend there.
                self.touch(moved_to);
                let resend = self.slot(lane).lane.handle(Event::Reconnected);
                actions.extend(self.follow(lane, resend));
            }
            other => actions.extend(self.follow(lane, other)),
        }
        actions
    }

    /// Advances the pool's clock to `now`, the time since an arbitrary
    /// origin, closes connections that have had no lane for
    /// `idle_timeout` and stalled ones, and rotates connections that
    /// reached `rotate_after`.
    pub fn tick(&mut self, now: Duration) -> Vec<PoolAction> {
        self.now = self.now.max(now);
        let idle: Vec<ConnectionId> = self
            .connections
            .iter()
            .filter(|(_, c)| {
                c.empty_since.is_some_and(|since| {
                    self.now.saturating_sub(since) >= self.limits.idle_timeout
                })
            })
            .map(|(id, _)| *id)
            .collect();
        let mut actions = Vec::new();
        for connection in idle {
            if self.connections.remove(&connection).is_some_and(|c| c.open) {
                actions.push(PoolAction::Close(connection));
            }
        }
        let stalled: Vec<ConnectionId> = self
            .connections
            .iter()
            .filter(|(_, c)| {
                self.carries_request(c)
                    && self.now.saturating_sub(c.last_activity)
                        >= self.limits.stall_timeout
            })
            .map(|(id, _)| *id)
            .collect();
        for connection in stalled {
            actions.push(PoolAction::Close(connection));
            actions.extend(self.connection_lost(connection));
        }
        let aged: Vec<ConnectionId> = self
            .connections
            .iter()
            .filter(|(_, c)| {
                c.open
                    && self.now.saturating_sub(c.opened_at)
                        >= self.limits.rotate_after
            })
            .map(|(id, _)| *id)
            .collect();
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

    /// The lane on `connection`, if it carries one.
    pub fn lane_on(&self, connection: ConnectionId) -> Option<LaneId> {
        self.connections.get(&connection)?.lane
    }

    /// The connection `lane` lives on.
    pub fn connection_of(&self, lane: LaneId) -> Option<ConnectionId> {
        self.lanes.get(&lane).map(|slot| slot.connection)
    }

    /// Whether `lane` has a request in flight.
    pub fn is_busy(&self, lane: LaneId) -> bool {
        self.lanes
            .get(&lane)
            .is_some_and(|slot| slot.lane.is_busy())
    }

    fn slot(&mut self, lane: LaneId) -> &mut Slot {
        self.lanes.get_mut(&lane).expect("a known lane")
    }

    /// Whether `connection`'s lane has a request in flight.
    fn carries_request(&self, connection: &Connection) -> bool {
        connection
            .lane
            .is_some_and(|lane| self.lanes[&lane].lane.is_busy())
    }

    /// Sends a request for `lane`, which must be idle.
    fn start(&mut self, lane: LaneId, full_body: Body) -> Vec<PoolAction> {
        let slot = self.slot(lane);
        let action =
            slot.lane.submit(full_body).expect("start on an idle lane");
        let connection = slot.connection;
        self.touch(connection);
        self.follow(lane, Some(action))
    }

    /// A request goes out on `connection`: the stall timer starts over.
    fn touch(&mut self, connection: ConnectionId) {
        if let Some(c) = self.connections.get_mut(&connection) {
            c.last_activity = self.now;
        }
    }

    /// Feeds an event to a lane, and moves the lane off a draining
    /// connection once its request finishes.
    fn apply(&mut self, lane: LaneId, event: Event) -> Vec<PoolAction> {
        let slot = self.slot(lane);
        let was_busy = slot.lane.is_busy();
        let action = slot.lane.handle(event);
        let connection = slot.connection;
        let mut actions = self.follow(lane, action);
        if was_busy && !self.is_busy(lane) && self.is_draining(connection) {
            actions.extend(self.evacuate(connection));
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
                if body.previous_response_id.is_some() {
                    self.last_delta_items = body.input.len() as u64;
                }
                let mut actions = self.open(connection);
                actions.push(PoolAction::Send {
                    connection,
                    lane,
                    body,
                });
                actions
            }
            Some(lane::Action::Fail(failure)) => {
                vec![PoolAction::Fail { lane, failure }]
            }
            Some(lane::Action::Reconnect) => {
                let mut actions = Vec::new();
                if let Some(old) = self.connections.get_mut(&connection) {
                    // A live connection only sends its lane away when the
                    // server reported its age limit.
                    old.draining = true;
                    old.lane = None;
                    actions.extend(self.retire_if_empty(connection));
                }
                let (moved_to, open) = self.place(lane, false);
                actions.extend(open);
                self.slot(lane).connection = moved_to;
                self.touch(moved_to);
                let resend = self.slot(lane).lane.handle(Event::Reconnected);
                actions.extend(self.follow(lane, resend));
                actions
            }
        }
    }

    /// Tells the driver to open `connection`, unless it already was.
    fn open(&mut self, connection: ConnectionId) -> Vec<PoolAction> {
        match self.connections.get_mut(&connection) {
            Some(c) if !c.open => {
                c.open = true;
                // Its age, for rotation, starts now.
                c.opened_at = self.now;
                self.connections_opened += 1;
                vec![PoolAction::Open(connection)]
            }
            _ => Vec::new(),
        }
    }

    /// Picks a connection for `lane`, making one if none is free, and
    /// records the lane on it. `eager` opens the connection now; else it
    /// opens with the first request sent on it.
    fn place(
        &mut self,
        lane: LaneId,
        eager: bool,
    ) -> (ConnectionId, Vec<PoolAction>) {
        let found = self
            .connections
            .iter()
            .find(|(_, c)| !c.draining && c.lane.is_none())
            .map(|(id, _)| *id);
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
                id
            }
        };
        let actions = if eager {
            self.open(connection)
        } else {
            Vec::new()
        };
        let placed =
            self.connections.get_mut(&connection).expect("just placed");
        placed.lane = Some(lane);
        placed.empty_since = None;
        (connection, actions)
    }

    /// Moves an idle lane off a draining `connection`, and closes the
    /// connection once it is empty.
    fn evacuate(&mut self, connection: ConnectionId) -> Vec<PoolAction> {
        let mut actions = Vec::new();
        let idle = self
            .connections
            .get(&connection)
            .and_then(|c| c.lane)
            .filter(|lane| !self.is_busy(*lane));
        if let Some(lane) = idle {
            if let Some(c) = self.connections.get_mut(&connection) {
                c.lane = None;
            }
            let (moved_to, open) = self.place(lane, true);
            actions.extend(open);
            let slot = self.slot(lane);
            slot.connection = moved_to;
            // The new connection holds nothing for this lane.
            slot.lane.handle(Event::Reconnected);
        }
        actions.extend(self.retire_if_empty(connection));
        actions
    }

    /// Closes a draining connection that no lane uses any more.
    fn retire_if_empty(&mut self, connection: ConnectionId) -> Vec<PoolAction> {
        match self.connections.get(&connection) {
            Some(c) if c.draining && c.lane.is_none() => {
                let open = c.open;
                self.connections.remove(&connection);
                if open {
                    vec![PoolAction::Close(connection)]
                } else {
                    Vec::new()
                }
            }
            _ => Vec::new(),
        }
    }
}

fn add_stats(total: &mut LaneStats, stats: &LaneStats) {
    total.full_requests += stats.full_requests;
    total.delta_requests += stats.delta_requests;
    total.previous_response_not_found += stats.previous_response_not_found;
    total.connection_limit_reached += stats.connection_limit_reached;
    total.connection_lost += stats.connection_lost;
}
