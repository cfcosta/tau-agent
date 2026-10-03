//! The connection pool: which lane uses which connection, and when a
//! connection opens, rotates and closes.
//!
//! tau sends no `stream_id`, so a connection carries one lane at a
//! time, and a lane one request at a time: never two responses in
//! flight on one connection (`docs/reference/openai-websocket.md`,
//! "Limits and lanes"). A frame belongs to the lane of the connection it
//! came on.
//!
//! On OpenAI's route the prompt cache lives on the connection: a
//! connection that served a prefix reads it again from cache, and a new
//! one mostly does not (`docs/reference/openai-websocket.md`, "Prompt
//! cache"). So a lane says which conversation it belongs to, as an
//! [`Affinity`]: its path of work, and for a fork the path it forked
//! from. A connection remembers the path it serves, the one it served
//! before, and the continuation its last lane left.
//!
//! - A lane takes a connection when it opens, and when it has none and
//!   sends a request. In this order, it takes:
//!   1. an idle connection of its own path that no other lane holds;
//!   2. for a fork, until its first response completes: an idle
//!      connection of its parent's path. If the parent's lane holds it,
//!      the parent's lane gives it up and takes another when it next
//!      sends. The connection serves the fork from then on: a handoff;
//!   3. if a connection of 1 or 2 has a response in flight: that
//!      connection once it is idle, waiting at most `affinity_wait`;
//!   4. a free connection (one no lane holds) that served its path
//!      before, such as the one a fork took over, given back when the
//!      fork's run ended; else a free connection that serves no path;
//!      else the free connection used least recently;
//!   5. a new connection.
//!
//!   Ties go to the connection used most recently, except in 4, where
//!   the least recently used goes first; then to the lowest id.
//! - A lane keeps its connection while its run lives, because its
//!   continuation lives there.
//! - When a lane closes, its connection keeps its continuation. The next
//!   lane of the same path to take it continues from there, by delta,
//!   when its input extends the baseline (see
//!   [`continuation`](super::continuation)). A lane of another path
//!   starts it over.
//! - A free connection that serves a path stays open until rotation,
//!   since it holds that path's cache. One that serves no path closes
//!   after `idle_timeout`. Past `max_idle` free connections, the least
//!   recently used close, those that serve no path first.
//! - From `rotate_after` (55 minutes, against OpenAI's 60-minute cap) a
//!   connection drains: it takes no lane, an idle lane leaves it at
//!   once, and a busy lane leaves it when its request finishes. An empty
//!   draining connection is closed. A lane that left takes another
//!   connection, as above, when it next sends; it lost its continuation,
//!   so that request is a full resend. A connection that answered
//!   `websocket_connection_limit_reached` drains the same way, and its
//!   request is resent at once on another connection.
//! - A connection with a request in flight that has received nothing for
//!   `stall_timeout` (5 minutes, pi's idle timeout) is treated as lost.
//! - When a connection is lost, a request that had produced no output
//!   is resent in full on a connection chosen as above, without
//!   waiting; one that had fails. An idle lane is left without a
//!   connection, and takes one when it next sends: a connection that
//!   keeps failing to open is tried again when a run asks for
//!   something, never in a loop behind an idle lane.
//!
//! Like [`lane`](super::lane), the pool does no I/O: it returns
//! [`PoolAction`]s for the driver to carry out, in order. A connection
//! is opened as soon as the pool makes it, and the driver may queue a
//! `Send` for a connection it is still opening. Time comes in through
//! [`Pool::tick`]; the pool never reads a clock.

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use super::{
    continuation::{Body, Continuation},
    lane::{self, Event, Lane, LaneStats},
};

pub type ConnectionId = u64;
pub type LaneId = u64;

/// A conversation's path of work: in tau, the id of the run whose
/// conversation it is, which is also its `prompt_cache_key`.
pub type PathId = Arc<str>;

/// The conversation a lane belongs to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Affinity {
    /// The lane's path. `None` for requests outside any conversation,
    /// which take any free connection.
    pub path: Option<PathId>,
    /// The path the lane's conversation forked from: its first request
    /// may take that path's connection, which holds the prefix the fork
    /// inherited.
    pub parent: Option<PathId>,
}

impl Affinity {
    /// The affinity of `path`, which forked from `parent`, if it did.
    pub fn new(path: impl Into<PathId>, parent: Option<PathId>) -> Self {
        Self {
            path: Some(path.into()),
            parent,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Age at which a connection starts draining.
    pub rotate_after: Duration,
    /// How long a free connection that serves no path stays open.
    pub idle_timeout: Duration,
    /// How long a connection with a request in flight may go without
    /// receiving anything before it is presumed dead.
    pub stall_timeout: Duration,
    /// How long a lane waits for a busy connection of its own or its
    /// parent's path before it takes another.
    pub affinity_wait: Duration,
    /// How many free connections stay open.
    pub max_idle: usize,
}

impl Default for Limits {
    /// Rotation 5 minutes before OpenAI's 60-minute cap, the margin pi
    /// uses; pi's 5-minute idle and stall timeouts; a 5-second wait for
    /// a conversation's connection, and at most 8 free connections.
    fn default() -> Self {
        Self {
            rotate_after: Duration::from_secs(55 * 60),
            idle_timeout: Duration::from_secs(5 * 60),
            stall_timeout: Duration::from_secs(5 * 60),
            affinity_wait: Duration::from_secs(5),
            max_idle: 8,
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
    /// Lanes placed on a connection of their own path.
    pub own_connection: u64,
    /// Forks placed on their parent's connection.
    pub handoffs: u64,
    /// Lanes that waited for a busy connection of their path or their
    /// parent's.
    pub waits: u64,
}

/// What the pool knows of one connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionState {
    pub id: ConnectionId,
    /// The lane that holds it.
    pub lane: Option<LaneId>,
    /// The path it serves.
    pub path: Option<PathId>,
    /// The path it served before that.
    pub served_before: Option<PathId>,
    pub draining: bool,
    /// When a lane last took it or sent on it.
    pub last_used: Duration,
}

#[derive(Debug, Default)]
struct Connection {
    opened_at: Duration,
    /// Takes no new lane: it reached `rotate_after`, or the server
    /// reported its age limit.
    draining: bool,
    lane: Option<LaneId>,
    /// When it was last left with no lane.
    empty_since: Option<Duration>,
    /// When a request was last sent on it or anything last came in.
    /// Only read while a request is in flight, and every request sent
    /// sets it.
    last_activity: Duration,
    /// When a lane last took it or sent on it.
    last_used: Duration,
    path: Option<PathId>,
    served_before: Option<PathId>,
    /// The continuation its last lane left, for the next lane of `path`.
    kept: Option<Continuation>,
}

#[derive(Debug)]
struct Slot {
    /// `None` until the lane takes a connection, and after it lost one.
    connection: Option<ConnectionId>,
    lane: Lane,
    affinity: Affinity,
    /// Whether one of its responses completed: from then on, it no
    /// longer looks for its parent's connection.
    answered: bool,
    /// Waiting for a busy connection of its path, or its parent's,
    /// until then.
    waiting_until: Option<Duration>,
    /// A request sent while it waits.
    pending: Option<Body>,
}

/// Where a lane goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placement {
    Own(ConnectionId),
    Parent(ConnectionId),
    Wait,
    Free(ConnectionId),
    New,
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
    own_connection: u64,
    handoffs: u64,
    waits: u64,
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

    /// Opens a lane for a new run of `affinity`'s conversation and gives
    /// it a connection, or has it wait for one.
    pub fn open_lane(
        &mut self,
        affinity: Affinity,
    ) -> (LaneId, Vec<PoolAction>) {
        let lane = self.next_lane;
        self.next_lane += 1;
        self.lanes.insert(
            lane,
            Slot {
                connection: None,
                lane: Lane::new(),
                affinity,
                answered: false,
                waiting_until: None,
                pending: None,
            },
        );
        let actions = self.place(lane, true);
        (lane, actions)
    }

    /// Closes a lane when its run ends. A request still in flight is
    /// dropped, as on cancel. Its connection stays open for the next
    /// lane, with the lane's continuation when it had no request.
    pub fn close_lane(
        &mut self,
        lane: LaneId,
    ) -> Result<Vec<PoolAction>, PoolError> {
        let mut slot = self
            .lanes
            .remove(&lane)
            .ok_or(PoolError::UnknownLane(lane))?;
        let kept = if slot.lane.is_busy() {
            None
        } else {
            slot.lane.take_continuation()
        };
        // Its request, if any, is dropped where it is: a lane that goes
        // does not move off a draining connection.
        slot.lane.handle(Event::Cancel);
        add_stats(&mut self.closed_lane_stats, slot.lane.stats());
        let mut actions = Vec::new();
        if let Some(connection) = slot.connection {
            actions.extend(self.release(connection, kept));
        }
        actions.extend(self.finish());
        Ok(actions)
    }

    /// Submits `lane`'s next request, given as it would be sent in full.
    pub fn submit(
        &mut self,
        lane: LaneId,
        full_body: Body,
    ) -> Result<Vec<PoolAction>, PoolError> {
        let slot = self.lanes.get(&lane).ok_or(PoolError::UnknownLane(lane))?;
        if self.is_busy(lane) {
            return Err(PoolError::Busy(lane));
        }
        if slot.connection.is_some() {
            return Ok(self.start(lane, full_body));
        }
        self.slot(lane).pending = Some(full_body);
        let mut actions = Vec::new();
        if self.lanes[&lane].waiting_until.is_none() {
            actions.extend(self.place(lane, true));
        }
        actions.extend(self.finish());
        Ok(actions)
    }

    /// Cancels `lane`'s request, if it has one.
    pub fn cancel(
        &mut self,
        lane: LaneId,
    ) -> Result<Vec<PoolAction>, PoolError> {
        let slot = self
            .lanes
            .get_mut(&lane)
            .ok_or(PoolError::UnknownLane(lane))?;
        if slot.pending.take().is_some() {
            return Ok(Vec::new());
        }
        let mut actions = self.apply(lane, Event::Cancel);
        actions.extend(self.finish());
        Ok(actions)
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
        if let Some(connection) = slot.connection {
            self.activity(connection);
        }
        let mut actions = self.apply(lane, event);
        actions.extend(self.finish());
        Ok(actions)
    }

    /// Records that something arrived on `connection`, as of the last
    /// tick. The driver calls this for every frame.
    pub fn activity(&mut self, connection: ConnectionId) {
        if let Some(c) = self.connections.get_mut(&connection) {
            c.last_activity = self.now;
        }
    }

    /// Reports that `connection` closed or failed. A request in flight
    /// on it is resent elsewhere or fails; an idle lane on it is left
    /// without a connection.
    pub fn connection_lost(
        &mut self,
        connection: ConnectionId,
    ) -> Vec<PoolAction> {
        let Some(lost) = self.connections.remove(&connection) else {
            return Vec::new();
        };
        let mut actions = Vec::new();
        if let Some(lane) = lost.lane {
            self.slot(lane).connection = None;
            let action = self.slot(lane).lane.handle(Event::ConnectionLost);
            actions.extend(match action {
                Some(lane::Action::Reconnect) => self.reconnect(lane),
                other => self.follow(lane, other),
            });
        }
        actions.extend(self.finish());
        actions
    }

    /// Advances the pool's clock to `now`, the time since an arbitrary
    /// origin: closes free connections that serve no path after
    /// `idle_timeout` and stalled ones, rotates connections that reached
    /// `rotate_after`, keeps at most `max_idle` free connections, and
    /// places lanes whose wait is over.
    pub fn tick(&mut self, now: Duration) -> Vec<PoolAction> {
        self.now = self.now.max(now);
        let idle: Vec<ConnectionId> = self
            .connections
            .iter()
            .filter(|(_, c)| {
                c.path.is_none()
                    && c.lane.is_none()
                    && c.empty_since.is_some_and(|since| {
                        self.now.saturating_sub(since)
                            >= self.limits.idle_timeout
                    })
            })
            .map(|(id, _)| *id)
            .collect();
        let mut actions = Vec::new();
        for connection in idle {
            self.connections.remove(&connection);
            actions.push(PoolAction::Close(connection));
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
                self.now.saturating_sub(c.opened_at) >= self.limits.rotate_after
            })
            .map(|(id, _)| *id)
            .collect();
        for connection in &aged {
            if let Some(c) = self.connections.get_mut(connection) {
                c.draining = true;
            }
        }
        for connection in aged {
            actions.extend(self.evacuate(connection));
        }
        actions.extend(self.finish());
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
            own_connection: self.own_connection,
            handoffs: self.handoffs,
            waits: self.waits,
        }
    }

    /// Every open connection, by id.
    pub fn connections(&self) -> Vec<ConnectionState> {
        self.connections
            .iter()
            .map(|(id, c)| ConnectionState {
                id: *id,
                lane: c.lane,
                path: c.path.clone(),
                served_before: c.served_before.clone(),
                draining: c.draining,
                last_used: c.last_used,
            })
            .collect()
    }

    /// The lane on `connection`, if it carries one.
    pub fn lane_on(&self, connection: ConnectionId) -> Option<LaneId> {
        self.connections.get(&connection)?.lane
    }

    /// The connection `lane` holds, if it holds one.
    pub fn connection_of(&self, lane: LaneId) -> Option<ConnectionId> {
        self.lanes.get(&lane).and_then(|slot| slot.connection)
    }

    /// Whether `lane` has a request in flight, or one waiting for its
    /// connection.
    pub fn is_busy(&self, lane: LaneId) -> bool {
        self.lanes
            .get(&lane)
            .is_some_and(|slot| slot.lane.is_busy() || slot.pending.is_some())
    }

    /// Whether `lane` waits for a busy connection of its path or its
    /// parent's.
    pub fn is_waiting(&self, lane: LaneId) -> bool {
        self.lanes
            .get(&lane)
            .is_some_and(|slot| slot.waiting_until.is_some())
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

    /// Where `lane` goes now, by the order in the module docs. `may_wait`
    /// allows waiting for a busy connection of its path or its parent's.
    fn select(&self, lane: LaneId, may_wait: bool) -> Placement {
        let slot = &self.lanes[&lane];
        let path = slot.affinity.path.as_ref();
        let parent = slot.affinity.parent.as_ref().filter(|_| !slot.answered);
        let serves = |c: &Connection, path: Option<&PathId>| {
            path.is_some() && c.path.as_ref() == path
        };
        let usable = || self.connections.iter().filter(|(_, c)| !c.draining);
        // The most recently used, then the lowest id.
        let latest = |candidates: Vec<(&ConnectionId, &Connection)>| {
            candidates
                .into_iter()
                .min_by_key(|(id, c)| (std::cmp::Reverse(c.last_used), **id))
                .map(|(id, _)| *id)
        };
        let own = latest(
            usable()
                .filter(|(_, c)| c.lane.is_none() && serves(c, path))
                .collect(),
        );
        if let Some(id) = own {
            return Placement::Own(id);
        }
        let parents = latest(
            usable()
                .filter(|(_, c)| !self.carries_request(c) && serves(c, parent))
                .collect(),
        );
        if let Some(id) = parents {
            return Placement::Parent(id);
        }
        let worth_waiting = usable().any(|(_, c)| {
            self.carries_request(c) && (serves(c, path) || serves(c, parent))
        });
        if may_wait && worth_waiting && !self.limits.affinity_wait.is_zero() {
            return Placement::Wait;
        }
        let free = usable()
            .filter(|(_, c)| c.lane.is_none())
            .min_by_key(|(id, c)| {
                let before = path.is_some() && c.served_before.as_ref() == path;
                (!before, c.path.is_some(), c.last_used, **id)
            })
            .map(|(id, _)| *id);
        match free {
            Some(id) => Placement::Free(id),
            None => Placement::New,
        }
    }

    /// Gives `lane`, which has no connection, one; or has it wait, if
    /// `may_wait` and a connection of its path or its parent's is busy.
    /// A lane that gets one sends its pending request, if it has one.
    fn place(&mut self, lane: LaneId, may_wait: bool) -> Vec<PoolAction> {
        let placement = self.select(lane, may_wait);
        if placement == Placement::Wait {
            self.waits += 1;
            let until = self.now + self.limits.affinity_wait;
            self.slot(lane).waiting_until = Some(until);
            return Vec::new();
        }
        let mut actions = self.attach(lane, placement);
        if let Some(body) = self.slot(lane).pending.take() {
            actions.extend(self.start(lane, body));
        }
        actions
    }

    /// Puts `lane` on the connection `placement` names, making it if it
    /// is new, and takes it from the lane that held it in a handoff.
    fn attach(
        &mut self,
        lane: LaneId,
        placement: Placement,
    ) -> Vec<PoolAction> {
        let mut actions = Vec::new();
        let connection = match placement {
            Placement::Own(id) => {
                self.own_connection += 1;
                self.connections_reused += 1;
                id
            }
            Placement::Parent(id) => {
                self.handoffs += 1;
                self.connections_reused += 1;
                if let Some(holder) = self.connections[&id].lane {
                    let slot = self.slot(holder);
                    slot.connection = None;
                    slot.lane.take_continuation();
                }
                id
            }
            Placement::Free(id) => {
                self.connections_reused += 1;
                id
            }
            Placement::Wait => unreachable!("a waiting lane takes nothing"),
            Placement::New => {
                let id = self.next_connection;
                self.next_connection += 1;
                self.connections_opened += 1;
                self.connections.insert(
                    id,
                    Connection {
                        opened_at: self.now,
                        ..Connection::default()
                    },
                );
                actions.push(PoolAction::Open(id));
                id
            }
        };
        let now = self.now;
        let path = self.lanes[&lane].affinity.path.clone();
        let c = self.connections.get_mut(&connection).expect("just chosen");
        if c.path != path {
            if c.path.is_some() {
                c.served_before = c.path.take();
            }
            c.path = path.clone();
            c.kept = None;
        }
        let kept = if path.is_some() { c.kept.take() } else { None };
        c.kept = None;
        c.lane = Some(lane);
        c.empty_since = None;
        c.last_used = now;
        let slot = self.slot(lane);
        slot.connection = Some(connection);
        slot.waiting_until = None;
        slot.lane.set_continuation(kept);
        actions
    }

    /// Leaves `connection` free, keeping `kept` for the next lane of its
    /// path. A draining one closes.
    fn release(
        &mut self,
        connection: ConnectionId,
        kept: Option<Continuation>,
    ) -> Vec<PoolAction> {
        let now = self.now;
        let Some(c) = self.connections.get_mut(&connection) else {
            return Vec::new();
        };
        c.lane = None;
        c.empty_since = Some(now);
        c.kept = kept.filter(|_| c.path.is_some());
        self.retire_if_empty(connection)
    }

    /// Ends an operation: places the lanes that wait, then keeps at most
    /// `max_idle` free connections.
    fn finish(&mut self) -> Vec<PoolAction> {
        let mut actions = self.settle();
        actions.extend(self.trim());
        actions
    }

    /// Places the lanes that wait: those whose connection became idle,
    /// and those whose wait is over, which take another.
    fn settle(&mut self) -> Vec<PoolAction> {
        let waiting: Vec<LaneId> = self
            .lanes
            .iter()
            .filter(|(_, slot)| slot.waiting_until.is_some())
            .map(|(id, _)| *id)
            .collect();
        let mut actions = Vec::new();
        for lane in waiting {
            let Some(until) = self.lanes[&lane].waiting_until else {
                continue;
            };
            let over = self.now >= until;
            match self.select(lane, !over) {
                Placement::Wait => {}
                placement => {
                    actions.extend(self.attach(lane, placement));
                    if let Some(body) = self.slot(lane).pending.take() {
                        actions.extend(self.start(lane, body));
                    }
                }
            }
        }
        actions
    }

    /// Sends a request for `lane`, which must hold a connection and be
    /// idle.
    fn start(&mut self, lane: LaneId, full_body: Body) -> Vec<PoolAction> {
        let slot = self.slot(lane);
        let action =
            slot.lane.submit(full_body).expect("start on an idle lane");
        self.follow(lane, Some(action))
    }

    /// A request goes out on `connection`: the stall timer starts over.
    fn touch(&mut self, connection: ConnectionId) {
        let now = self.now;
        if let Some(c) = self.connections.get_mut(&connection) {
            c.last_activity = now;
            c.last_used = now;
        }
    }

    /// Feeds an event to a lane, and moves the lane off a draining
    /// connection once its request finishes.
    fn apply(&mut self, lane: LaneId, event: Event) -> Vec<PoolAction> {
        let completed = matches!(event, Event::Completed { .. });
        let slot = self.slot(lane);
        let was_busy = slot.lane.is_busy();
        let action = slot.lane.handle(event);
        if completed && was_busy && !slot.lane.is_busy() {
            slot.answered = true;
        }
        let connection = slot.connection;
        let mut actions = self.follow(lane, action);
        if let Some(connection) = connection
            && was_busy
            && !self.lanes[&lane].lane.is_busy()
            && self.is_draining(connection)
        {
            actions.extend(self.evacuate(connection));
        }
        actions
    }

    /// Carries a lane action into pool actions.
    fn follow(
        &mut self,
        lane: LaneId,
        action: Option<lane::Action>,
    ) -> Vec<PoolAction> {
        match action {
            None => Vec::new(),
            Some(lane::Action::Send(body)) => {
                if body.previous_response_id.is_some() {
                    self.last_delta_items = body.input.len() as u64;
                }
                let connection = self.lanes[&lane]
                    .connection
                    .expect("a lane sends on its connection");
                self.touch(connection);
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
                if let Some(old) = self.slot(lane).connection.take() {
                    // A live connection only sends its lane away when the
                    // server reported its age limit.
                    if let Some(c) = self.connections.get_mut(&old) {
                        c.draining = true;
                    }
                    actions.extend(self.release(old, None));
                }
                actions.extend(self.reconnect(lane));
                actions
            }
        }
    }

    /// Moves `lane`, which has no connection and a request waiting for
    /// [`Event::Reconnected`], to another connection, and resends there.
    fn reconnect(&mut self, lane: LaneId) -> Vec<PoolAction> {
        let placement = self.select(lane, false);
        let mut actions = self.attach(lane, placement);
        let resend = self.slot(lane).lane.handle(Event::Reconnected);
        actions.extend(self.follow(lane, resend));
        actions
    }

    /// Takes an idle lane off a draining `connection`, and closes the
    /// connection once it is empty. The lane takes another connection
    /// when it next sends.
    fn evacuate(&mut self, connection: ConnectionId) -> Vec<PoolAction> {
        let mut actions = Vec::new();
        let idle = self
            .connections
            .get(&connection)
            .and_then(|c| c.lane)
            .filter(|lane| !self.lanes[lane].lane.is_busy());
        if let Some(lane) = idle {
            self.slot(lane).connection = None;
            self.slot(lane).lane.take_continuation();
            actions.extend(self.release(connection, None));
        } else {
            actions.extend(self.retire_if_empty(connection));
        }
        actions
    }

    /// Closes a draining connection that no lane uses any more.
    fn retire_if_empty(&mut self, connection: ConnectionId) -> Vec<PoolAction> {
        match self.connections.get(&connection) {
            Some(c) if c.draining && c.lane.is_none() => {
                self.connections.remove(&connection);
                vec![PoolAction::Close(connection)]
            }
            _ => Vec::new(),
        }
    }

    /// Closes free connections past `max_idle`: the least recently used,
    /// those that serve no path first.
    fn trim(&mut self) -> Vec<PoolAction> {
        let mut free: Vec<(bool, Duration, ConnectionId)> = self
            .connections
            .iter()
            .filter(|(_, c)| c.lane.is_none())
            .map(|(id, c)| (c.path.is_some(), c.last_used, *id))
            .collect();
        free.sort();
        let excess = free.len().saturating_sub(self.limits.max_idle);
        free.into_iter()
            .take(excess)
            .map(|(_, _, id)| {
                self.connections.remove(&id);
                PoolAction::Close(id)
            })
            .collect()
    }
}

fn add_stats(total: &mut LaneStats, stats: &LaneStats) {
    total.full_requests += stats.full_requests;
    total.delta_requests += stats.delta_requests;
    total.previous_response_not_found += stats.previous_response_not_found;
    total.connection_limit_reached += stats.connection_limit_reached;
    total.connection_lost += stats.connection_lost;
}
