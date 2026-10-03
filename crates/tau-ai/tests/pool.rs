//! The connection pool (`tau_ai::ws::proto::pool`), driven by a Hegel
//! state machine under small time limits and checked against a model
//! built from the actions it returns: a simulated server that keeps
//! each connection's responses, and a reference selector for which
//! connection a lane takes.

use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_ai::ws::proto::{
    continuation::Body,
    lane::{CONNECTION_LIMIT_REACHED, Event, PREVIOUS_RESPONSE_NOT_FOUND},
    pool::{
        Affinity,
        ConnectionId,
        LaneId,
        Limits,
        Pool,
        PoolAction,
        PoolError,
    },
};

fn body(n: u64) -> Body {
    with_input(json!([{"type": "message", "text": n.to_string()}]))
}

fn with_input(items: Value) -> Body {
    let mut body = serde_json::Map::new();
    body.insert("model".into(), json!("gpt-5.5"));
    body.insert("input".into(), items);
    Body::from(body)
}

/// A request of `path` with `items`: its `prompt_cache_key` is the path,
/// as tau sends it.
fn on_path(path: Option<&str>, items: Vec<Value>) -> Body {
    let mut body = serde_json::Map::new();
    body.insert("model".into(), json!("gpt-5.5"));
    if let Some(path) = path {
        body.insert("prompt_cache_key".into(), json!(path));
    }
    body.insert("input".into(), Value::Array(items));
    Body::from(body)
}

fn item(text: &str) -> Value {
    json!({"type": "message", "text": text})
}

/// A request as the server rebuilt it when it arrived.
#[derive(Debug)]
struct Sent {
    /// The full input: the held response's items and the delta, or the
    /// whole input of a full request.
    input: Vec<Value>,
    previous_response_id: Option<String>,
}

/// What the model knows of a connection.
#[derive(Debug, Clone, Default)]
struct Conn {
    opened_at: Duration,
    /// The lane that holds it.
    lane: Option<LaneId>,
    path: Option<String>,
    served_before: Option<String>,
    last_used: Duration,
    /// When it was last left with no lane.
    empty_since: Option<Duration>,
}

/// What the model knows of a lane.
#[derive(Debug, Clone, Default)]
struct LaneInfo {
    path: Option<String>,
    parent: Option<String>,
    /// Whether one of its responses completed.
    answered: bool,
    /// When it started waiting for a connection, if it waits.
    waiting_since: Option<Duration>,
}

/// Where the reference selector says a lane goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    Own(ConnectionId),
    Parent(ConnectionId),
    Wait,
    Free(ConnectionId),
    New,
}

/// What the test knows from the actions alone, and what the simulated
/// server holds.
#[derive(Default, Debug)]
struct Model {
    conns: BTreeMap<ConnectionId, Conn>,
    now: Duration,
    /// Lanes whose request was sent and has not finished, by connection.
    in_flight: BTreeMap<LaneId, ConnectionId>,
    lanes: BTreeMap<LaneId, LaneInfo>,
    /// Lanes with a request submitted and not yet sent: they wait for a
    /// connection.
    pending: BTreeSet<LaneId>,
    /// Lanes whose in-flight request has produced output.
    started: BTreeSet<LaneId>,
    full_sends: u64,
    delta_sends: u64,
    /// Deltas the server answered with `previous_response_not_found`.
    not_found: u64,
    /// The responses each open connection holds, by id: the request's
    /// full input followed by the response's output items.
    held: BTreeMap<ConnectionId, BTreeMap<String, Vec<Value>>>,
    /// Each lane's transcript: its last completed request's input and
    /// that response's output items.
    transcript: BTreeMap<LaneId, Vec<Value>>,
    /// Each path's transcript, as its last completed response left it.
    paths: BTreeMap<String, Vec<Value>>,
    /// The full input of each lane's last submitted request.
    submitted: BTreeMap<LaneId, Vec<Value>>,
    /// Each in-flight request as the server received it.
    sent: BTreeMap<LaneId, Sent>,
}

impl Model {
    fn apply(&mut self, tc: &TestCase, actions: &[PoolAction]) {
        for action in actions {
            match action {
                PoolAction::Open(c) => {
                    assert!(
                        !self.conns.contains_key(c),
                        "connection {c} opened twice"
                    );
                    self.conns.insert(
                        *c,
                        Conn {
                            opened_at: self.now,
                            last_used: self.now,
                            ..Conn::default()
                        },
                    );
                    self.held.insert(*c, BTreeMap::new());
                }
                PoolAction::Close(c) => {
                    assert!(
                        self.conns.remove(c).is_some(),
                        "closed unknown {c}"
                    );
                    self.held.remove(c);
                }
                PoolAction::Send {
                    connection,
                    lane,
                    body,
                } => {
                    assert!(
                        self.conns.contains_key(connection),
                        "send on unknown connection"
                    );
                    // One request in flight per connection.
                    assert!(
                        self.in_flight
                            .iter()
                            .all(|(l, c)| l == lane || c != connection),
                        "a second request on {connection}"
                    );
                    self.in_flight.insert(*lane, *connection);
                    self.pending.remove(lane);
                    self.conns.get_mut(connection).unwrap().last_used =
                        self.now;
                    let input: Vec<Value> = body
                        .input
                        .iter()
                        .map(|item| (**item).clone())
                        .collect();
                    let rebuilt = match &body.previous_response_id {
                        Some(previous) => {
                            self.delta_sends += 1;
                            tc.event("delta sent");
                            let held = self.held[connection]
                                .get(previous)
                                .unwrap_or_else(|| {
                                    panic!(
                                        "delta on {connection} continues \
                                         {previous}, which it does not hold"
                                    )
                                });
                            let mut items = held.clone();
                            items.extend(input);
                            items
                        }
                        None => {
                            self.full_sends += 1;
                            input
                        }
                    };
                    assert_eq!(
                        rebuilt, self.submitted[lane],
                        "the server's view of lane {lane}'s input"
                    );
                    self.sent.insert(
                        *lane,
                        Sent {
                            input: rebuilt,
                            previous_response_id: body
                                .previous_response_id
                                .clone(),
                        },
                    );
                    self.started.remove(lane);
                }
                PoolAction::Fail { lane, .. } => {
                    self.in_flight.remove(lane);
                    self.started.remove(lane);
                    self.sent.remove(lane);
                }
            }
        }
    }

    /// Lanes with no request in flight or waiting.
    fn idle_lanes(&self) -> Vec<LaneId> {
        self.lanes
            .keys()
            .copied()
            .filter(|l| {
                !self.in_flight.contains_key(l) && !self.pending.contains(l)
            })
            .collect()
    }

    /// Forgets a lane's request, as cancel and close do.
    fn drop_request(&mut self, lane: LaneId) {
        self.in_flight.remove(&lane);
        self.sent.remove(&lane);
        self.started.remove(&lane);
        self.pending.remove(&lane);
    }

    fn busy(&self) -> BTreeSet<ConnectionId> {
        self.in_flight.values().copied().collect()
    }

    /// The reference selector: where a lane of `path` goes now, by the
    /// order `pool`'s docs give, from the model's own view of the
    /// connections. `parent` is the lane's parent while it has not
    /// answered; `draining` are the connections that take no lane.
    fn select(
        &self,
        path: Option<&str>,
        parent: Option<&str>,
        draining: &BTreeSet<ConnectionId>,
        wait: Duration,
    ) -> Expected {
        let busy = self.busy();
        let usable: Vec<(&ConnectionId, &Conn)> = self
            .conns
            .iter()
            .filter(|(id, _)| !draining.contains(id))
            .collect();
        let serves =
            |c: &Conn, p: Option<&str>| p.is_some() && c.path.as_deref() == p;
        let latest = |pick: &dyn Fn(&ConnectionId, &Conn) -> bool| {
            usable
                .iter()
                .filter(|(id, c)| pick(id, c))
                .min_by_key(|(id, c)| (std::cmp::Reverse(c.last_used), **id))
                .map(|(id, _)| **id)
        };
        if let Some(id) = latest(&|_, c| c.lane.is_none() && serves(c, path)) {
            return Expected::Own(id);
        }
        if let Some(id) =
            latest(&|id, c| !busy.contains(id) && serves(c, parent))
        {
            return Expected::Parent(id);
        }
        let worth = usable.iter().any(|(id, c)| {
            busy.contains(id) && (serves(c, path) || serves(c, parent))
        });
        if worth && !wait.is_zero() {
            return Expected::Wait;
        }
        usable
            .iter()
            .filter(|(_, c)| c.lane.is_none())
            .min_by_key(|(id, c)| {
                let before =
                    path.is_some() && c.served_before.as_deref() == path;
                (!before, c.path.is_some(), c.last_used, **id)
            })
            .map_or(Expected::New, |(id, _)| Expected::Free(**id))
    }

    /// [`Self::select`] for `lane`.
    fn select_for(
        &self,
        lane: LaneId,
        draining: &BTreeSet<ConnectionId>,
        wait: Duration,
    ) -> Expected {
        let info = &self.lanes[&lane];
        let parent = info.parent.as_deref().filter(|_| !info.answered);
        self.select(info.path.as_deref(), parent, draining, wait)
    }
}

/// The pool, its model, and the timers the test keeps from the actions.
struct PoolMachine {
    limits: Limits,
    pool: Pool,
    model: Model,
    /// When each open connection last sent a request or received anything.
    last_activity: BTreeMap<ConnectionId, Duration>,
    now: Duration,
    next_item: u64,
    next_response: u64,
}

const PATHS: [&str; 3] = ["p0", "p1", "p2"];

impl PoolMachine {
    fn new(limits: Limits) -> Self {
        Self {
            limits,
            pool: Pool::new(limits),
            model: Model::default(),
            last_activity: BTreeMap::new(),
            now: Duration::ZERO,
            next_item: 0,
            next_response: 0,
        }
    }

    fn draw_lane(tc: &TestCase, lanes: Vec<LaneId>) -> LaneId {
        tc.draw(gs::sampled_from(lanes))
    }

    fn draining(&self) -> BTreeSet<ConnectionId> {
        self.model
            .conns
            .keys()
            .copied()
            .filter(|c| self.pool.is_draining(*c))
            .collect()
    }

    /// A frame for an in-flight lane: activity on its connection.
    fn frame_for(&mut self, lane: LaneId, event: Event) -> Vec<PoolAction> {
        let connection = self.pool.connection_of(lane).unwrap();
        self.last_activity.insert(connection, self.now);
        self.pool.handle(lane, event).unwrap()
    }

    /// Folds lanes moving between connections into the model's view of
    /// the connections: who holds each, the path it serves and served
    /// before, and when it was last used.
    fn follow_placements(&mut self) {
        let now = self.now;
        let held: BTreeMap<ConnectionId, LaneId> = self
            .model
            .lanes
            .keys()
            .filter_map(|lane| Some((self.pool.connection_of(*lane)?, *lane)))
            .collect();
        for (id, conn) in &mut self.model.conns {
            let now_held = held.get(id).copied();
            if conn.lane.is_some() && now_held.is_none() {
                conn.empty_since = Some(now);
            }
            if let Some(lane) = now_held
                && conn.lane != Some(lane)
            {
                let path = self.model.lanes[&lane].path.clone();
                if conn.path != path {
                    if conn.path.is_some() {
                        conn.served_before = conn.path.take();
                    }
                    conn.path = path;
                }
                conn.last_used = now;
                conn.empty_since = None;
            }
            conn.lane = now_held;
        }
    }

    /// Checks one step's actions and folds them into the model and the
    /// timers. `tick` says whether the step was a tick.
    fn after(&mut self, tc: &TestCase, actions: Vec<PoolAction>, tick: bool) {
        let busy_before = self.model.busy();
        let limits = self.limits;
        let now = self.now;
        // Stalls: in a tick, a connection with requests in flight closes
        // only after `stall_timeout` without activity.
        for action in &actions {
            if let PoolAction::Close(c) = action
                && busy_before.contains(c)
                && tick
            {
                assert!(
                    now - self.last_activity[c] >= limits.stall_timeout,
                    "busy {c} closed after {:?} quiet",
                    now - self.last_activity[c]
                );
            }
        }
        // A free connection closes when it ages out, when it serves no
        // path and was free for `idle_timeout`, or when the free ones are
        // over the cap: then it was the least recently used, those that
        // serve no path first, and `max_idle` stay.
        let mut trimmed = Vec::new();
        for action in &actions {
            if let PoolAction::Close(c) = action
                && let Some(conn) = self.model.conns.get(c)
                && conn.lane.is_none()
            {
                let aged =
                    now.saturating_sub(conn.opened_at) >= limits.rotate_after;
                let idle = conn.path.is_none()
                    && conn.empty_since.is_some_and(|since| {
                        now - since >= limits.idle_timeout
                    });
                if !aged && !idle {
                    trimmed.push((conn.path.is_some(), conn.last_used, *c));
                }
            }
        }
        self.model.apply(tc, &actions);
        for action in &actions {
            match action {
                PoolAction::Open(c)
                | PoolAction::Send { connection: c, .. } => {
                    self.last_activity.insert(*c, now);
                }
                _ => {}
            }
        }
        let open: BTreeSet<ConnectionId> =
            self.model.conns.keys().copied().collect();
        self.last_activity.retain(|c, _| open.contains(c));
        self.follow_placements();
        if let Some(&(tagged, used, c)) = trimmed.iter().max() {
            tc.event("free connections over the cap close");
            let free: Vec<&Conn> = self
                .model
                .conns
                .values()
                .filter(|conn| conn.lane.is_none())
                .collect();
            assert_eq!(free.len(), limits.max_idle, "closed {c} under the cap");
            for conn in free {
                assert!(
                    (conn.path.is_some(), conn.last_used) >= (tagged, used),
                    "closed {c} before a less recently used one"
                );
            }
        }
        for (&lane, info) in &mut self.model.lanes {
            if self.pool.is_waiting(lane) {
                info.waiting_since.get_or_insert(now);
            } else {
                info.waiting_since = None;
            }
        }
        if tick {
            for &c in self.model.in_flight.values() {
                assert!(
                    now - self.last_activity[&c] < limits.stall_timeout,
                    "{c} stalled since {:?}, still open at {now:?}",
                    self.last_activity[&c]
                );
            }
            for (id, conn) in &self.model.conns {
                // No connection outlives rotation: past `rotate_after`,
                // only one with a request in flight is still open, and it
                // drains.
                if now.saturating_sub(conn.opened_at) >= limits.rotate_after {
                    assert!(
                        self.pool.is_draining(*id),
                        "aged connection {id} not draining"
                    );
                    assert!(
                        self.model.busy().contains(id),
                        "aged connection {id} open with nothing in flight"
                    );
                }
                // A free connection that serves no path closes after
                // `idle_timeout`.
                if conn.lane.is_none()
                    && conn.path.is_none()
                    && let Some(since) = conn.empty_since
                {
                    assert!(
                        now - since < limits.idle_timeout,
                        "{id} idle since {since:?} still open at {now:?}"
                    );
                }
            }
            // No lane waits past `affinity_wait`.
            for (lane, info) in &self.model.lanes {
                if let Some(since) = info.waiting_since {
                    assert!(
                        now - since < limits.affinity_wait,
                        "lane {lane} waiting since {since:?} at {now:?}"
                    );
                }
            }
        }
    }

    /// Checks that `lane`, which had no connection before the step,
    /// went where the reference selector said. `known` are the
    /// connections open before the step.
    fn check_placement(
        &self,
        lane: LaneId,
        expected: Expected,
        known: &BTreeSet<ConnectionId>,
    ) {
        let got = self.pool.connection_of(lane);
        match expected {
            Expected::Own(c) | Expected::Parent(c) | Expected::Free(c) => {
                assert_eq!(got, Some(c), "lane {lane}: expected {expected:?}");
            }
            Expected::Wait => {
                assert!(self.pool.is_waiting(lane), "lane {lane} should wait");
                assert_eq!(got, None);
            }
            Expected::New => {
                let c = got.expect("a new connection");
                assert!(
                    !known.contains(&c),
                    "lane {lane} on {c}: expected a new connection"
                );
            }
        }
    }
}

#[hegel::state_machine]
impl PoolMachine {
    /// Opens a lane for a path no live lane has, or for no path, and
    /// checks where it went against the reference selector.
    #[rule(weight = 2)]
    fn open_lane(&mut self, tc: TestCase) {
        let live: BTreeSet<String> = self
            .model
            .lanes
            .values()
            .filter_map(|info| info.path.clone())
            .collect();
        let path = tc.draw(gs::optional(gs::sampled_from(PATHS.to_vec())));
        tc.assume(path.is_none_or(|p| !live.contains(p)));
        let parent = tc
            .draw(gs::optional(gs::sampled_from(PATHS.to_vec())))
            .filter(|parent| path.is_some_and(|p| p != *parent));
        let affinity = Affinity {
            path: path.map(Into::into),
            parent: parent.map(Into::into),
        };
        let known: BTreeSet<ConnectionId> =
            self.model.conns.keys().copied().collect();
        let expected = self.model.select(
            path,
            parent,
            &self.draining(),
            self.limits.affinity_wait,
        );
        let (lane, actions) = self.pool.open_lane(affinity);
        self.model.lanes.insert(
            lane,
            LaneInfo {
                path: path.map(str::to_owned),
                parent: parent.map(str::to_owned),
                ..LaneInfo::default()
            },
        );
        // A lane starts from its path's transcript, or its parent's, as
        // a resumed run or a fork does.
        let start = path
            .and_then(|p| self.model.paths.get(p))
            .or_else(|| parent.and_then(|p| self.model.paths.get(p)))
            .cloned()
            .unwrap_or_default();
        self.model.transcript.insert(lane, start);
        let holder = match expected {
            Expected::Parent(c) => {
                tc.event("a fork takes its parent's connection");
                self.model.conns[&c].lane
            }
            Expected::Own(_) => {
                tc.event("a lane takes its own path's connection");
                None
            }
            Expected::Wait => {
                tc.event("a lane waits for its conversation's connection");
                None
            }
            _ => None,
        };
        self.after(&tc, actions, false);
        if let Some(holder) = holder {
            tc.event("a live parent hands its connection over");
            assert_eq!(self.pool.connection_of(holder), None);
        }
        self.check_placement(lane, expected, &known);
    }

    #[rule(weight = 2)]
    fn close_lane(&mut self, tc: TestCase) {
        tc.assume(!self.model.lanes.is_empty());
        let lane =
            Self::draw_lane(&tc, self.model.lanes.keys().copied().collect());
        self.model.drop_request(lane);
        self.model.transcript.remove(&lane);
        let actions = self.pool.close_lane(lane).unwrap();
        self.model.lanes.remove(&lane);
        self.after(&tc, actions, false);
    }

    /// Submits the lane's transcript and one new item, so a lane whose
    /// last response completed on its connection can continue from it.
    /// A lane without a connection takes one first, as the reference
    /// selector says.
    #[rule(weight = 4)]
    fn submit(&mut self, tc: TestCase) {
        let idle = self.model.idle_lanes();
        tc.assume(!idle.is_empty());
        let lane = Self::draw_lane(&tc, idle);
        self.next_item += 1;
        let mut input = self.model.transcript[&lane].clone();
        input.push(item(&self.next_item.to_string()));
        self.model.submitted.insert(lane, input.clone());
        let placing = self.pool.connection_of(lane).is_none()
            && !self.pool.is_waiting(lane);
        let known: BTreeSet<ConnectionId> =
            self.model.conns.keys().copied().collect();
        let expected = placing.then(|| {
            self.model.select_for(
                lane,
                &self.draining(),
                self.limits.affinity_wait,
            )
        });
        let path = self.model.lanes[&lane].path.clone();
        let actions = self
            .pool
            .submit(lane, on_path(path.as_deref(), input))
            .unwrap();
        assert_eq!(self.pool.submit(lane, body(0)), Err(PoolError::Busy(lane)));
        let sent = actions.iter().any(
            |a| matches!(a, PoolAction::Send { lane: l, .. } if *l == lane),
        );
        if !sent {
            self.model.pending.insert(lane);
        }
        self.after(&tc, actions, false);
        if let Some(expected) = expected {
            self.check_placement(lane, expected, &known);
        }
    }

    #[rule]
    fn output(&mut self, tc: TestCase) {
        tc.assume(!self.model.in_flight.is_empty());
        let lane = Self::draw_lane(
            &tc,
            self.model.in_flight.keys().copied().collect(),
        );
        self.model.started.insert(lane);
        let actions = self.frame_for(lane, Event::Output);
        self.after(&tc, actions, false);
    }

    /// The server completes the request and keeps the response on its
    /// connection.
    #[rule(weight = 3)]
    fn complete(&mut self, tc: TestCase) {
        tc.assume(!self.model.in_flight.is_empty());
        let lane = Self::draw_lane(
            &tc,
            self.model.in_flight.keys().copied().collect(),
        );
        let outputs = tc.draw(gs::integers::<u64>().max_value(2));
        self.next_response += 1;
        let response_id = format!("resp_{}", self.next_response);
        let output_items: Vec<Value> = (0..outputs)
            .map(|i| item(&format!("{response_id} out {i}")))
            .collect();
        let connection = self.model.in_flight[&lane];
        let sent = self
            .model
            .sent
            .remove(&lane)
            .expect("an in-flight request was sent");
        let mut held = sent.input;
        held.extend(output_items.iter().cloned());
        self.model
            .held
            .get_mut(&connection)
            .unwrap()
            .insert(response_id.clone(), held.clone());
        self.model.transcript.insert(lane, held.clone());
        let info = self.model.lanes.get_mut(&lane).unwrap();
        info.answered = true;
        if let Some(path) = &info.path {
            self.model.paths.insert(path.clone(), held);
        }
        self.model.in_flight.remove(&lane);
        self.model.started.remove(&lane);
        let actions = self.frame_for(
            lane,
            Event::Completed {
                response_id,
                output_items,
            },
        );
        self.after(&tc, actions, false);
    }

    #[rule]
    fn server_error(&mut self, tc: TestCase) {
        tc.assume(!self.model.in_flight.is_empty());
        let lane = Self::draw_lane(
            &tc,
            self.model.in_flight.keys().copied().collect(),
        );
        self.model.drop_request(lane);
        let actions = self.frame_for(
            lane,
            Event::ServerError {
                code: Some("server_error".into()),
            },
        );
        self.after(&tc, actions, false);
    }

    #[rule]
    fn connection_limit(&mut self, tc: TestCase) {
        tc.assume(!self.model.in_flight.is_empty());
        let lane = Self::draw_lane(
            &tc,
            self.model.in_flight.keys().copied().collect(),
        );
        if self.model.started.contains(&lane) {
            // Fails the request.
            self.model.drop_request(lane);
        }
        let actions = self.frame_for(
            lane,
            Event::ServerError {
                code: Some(CONNECTION_LIMIT_REACHED.into()),
            },
        );
        self.after(&tc, actions, false);
    }

    /// The server evicts the response an in-flight delta continues, and
    /// answers it with `previous_response_not_found`: the pool resends the
    /// request in full on the same connection.
    #[rule]
    fn evict(&mut self, tc: TestCase) {
        let deltas: Vec<LaneId> = self
            .model
            .sent
            .iter()
            .filter(|(_, sent)| sent.previous_response_id.is_some())
            .map(|(&lane, _)| lane)
            .collect();
        tc.assume(!deltas.is_empty());
        let lane = Self::draw_lane(&tc, deltas);
        let connection = self.model.in_flight[&lane];
        let previous =
            self.model.sent[&lane].previous_response_id.clone().unwrap();
        self.model
            .held
            .get_mut(&connection)
            .unwrap()
            .remove(&previous);
        self.model.not_found += 1;
        let actions = self.frame_for(
            lane,
            Event::ServerError {
                code: Some(PREVIOUS_RESPONSE_NOT_FOUND.into()),
            },
        );
        assert!(
            matches!(
                &actions[..],
                [PoolAction::Send { connection: c, lane: l, body }]
                    if *c == connection
                        && *l == lane
                        && body.previous_response_id.is_none()
            ),
            "{actions:?}"
        );
        tc.event("resent in full after previous_response_not_found");
        self.after(&tc, actions, false);
    }

    #[rule]
    fn cancel(&mut self, tc: TestCase) {
        tc.assume(!self.model.lanes.is_empty());
        let lane =
            Self::draw_lane(&tc, self.model.lanes.keys().copied().collect());
        self.model.drop_request(lane);
        let actions = self.pool.cancel(lane).unwrap();
        self.after(&tc, actions, false);
    }

    #[rule]
    fn tick(&mut self, tc: TestCase) {
        // Mostly short steps, so connections differ in when they were
        // last used, and now and then a long one.
        self.now += Duration::from_secs(
            tc.draw(gs::sampled_from(vec![0, 1, 2, 3, 10, 60])),
        );
        self.model.now = self.now;
        let actions = self.pool.tick(self.now);
        self.after(&tc, actions, true);
    }

    #[rule]
    fn frame(&mut self, tc: TestCase) {
        tc.assume(!self.model.conns.is_empty());
        let connection = tc.draw(gs::sampled_from(
            self.model.conns.keys().copied().collect::<Vec<_>>(),
        ));
        self.last_activity.insert(connection, self.now);
        self.pool.activity(connection);
        self.after(&tc, Vec::new(), false);
    }

    #[rule]
    fn lose_connection(&mut self, tc: TestCase) {
        tc.assume(!self.model.conns.is_empty());
        let connection = tc.draw(gs::sampled_from(
            self.model.conns.keys().copied().collect::<Vec<_>>(),
        ));
        self.model.conns.remove(&connection);
        self.model.held.remove(&connection);
        let actions = self.pool.connection_lost(connection);
        self.after(&tc, actions, false);
    }

    /// Counts, placement and what each connection serves agree with the
    /// model after every step.
    #[invariant(always_run)]
    fn pool_matches_the_model(&self, _tc: TestCase) {
        let open: BTreeSet<ConnectionId> =
            self.model.conns.keys().copied().collect();
        let listed: BTreeSet<ConnectionId> =
            self.pool.connections().iter().map(|c| c.id).collect();
        assert_eq!(listed, open, "open connections");
        // What each connection serves, and served before, moves with the
        // lanes placed on it: to the fork after a handoff.
        for state in self.pool.connections() {
            let conn = &self.model.conns[&state.id];
            assert_eq!(state.lane, conn.lane, "holder of {}", state.id);
            assert_eq!(
                state.path.as_deref(),
                conn.path.as_deref(),
                "path of {}",
                state.id
            );
            assert_eq!(
                state.served_before.as_deref(),
                conn.served_before.as_deref(),
                "served before on {}",
                state.id
            );
            assert_eq!(state.last_used, conn.last_used, "use of {}", state.id);
            // A draining connection only keeps a lane with a request in
            // flight.
            if state.draining
                && let Some(lane) = state.lane
            {
                assert!(
                    self.model.in_flight.contains_key(&lane),
                    "idle lane {lane} left on draining {}",
                    state.id
                );
            }
        }
        // At most `max_idle` connections are free.
        let free = self
            .model
            .conns
            .values()
            .filter(|c| c.lane.is_none())
            .count();
        assert!(free <= self.limits.max_idle, "{free} free connections");
        // Every in-flight request is on its lane's connection, and each
        // connection carries at most one lane.
        for (&lane, &connection) in &self.model.in_flight {
            assert_eq!(
                self.pool.connection_of(lane),
                Some(connection),
                "lane {lane} moved silently"
            );
        }
        let mut placed: BTreeSet<ConnectionId> = BTreeSet::new();
        for &lane in self.model.lanes.keys() {
            if let Some(c) = self.pool.connection_of(lane) {
                assert!(placed.insert(c), "two lanes on {c}");
                assert!(open.contains(&c), "lane {lane} on closed {c}");
                assert_eq!(self.pool.lane_on(c), Some(lane), "lane on {c}");
                assert!(!self.pool.is_waiting(lane));
            }
            assert_eq!(
                self.pool.is_busy(lane),
                self.model.in_flight.contains_key(&lane)
                    || self.model.pending.contains(&lane),
                "lane {lane}"
            );
            // A request waits only while its lane waits.
            if self.model.pending.contains(&lane) {
                assert!(self.pool.is_waiting(lane), "lane {lane} stuck");
            }
        }
        // Request counters cover every lane, closed ones included.
        let stats = self.pool.stats();
        assert_eq!(stats.lanes.full_requests, self.model.full_sends);
        assert_eq!(stats.lanes.delta_requests, self.model.delta_sends);
        assert_eq!(
            stats.lanes.previous_response_not_found,
            self.model.not_found
        );
    }
}

/// Under random operations and small time limits, the pool places
/// each lane where a reference selector says (its own path's idle
/// connection, else its parent's, else a wait, else a free one, else a
/// new one), moves what a connection serves with the lane placed on it,
/// keeps one lane and one request per connection, only sends a delta on
/// the connection that holds the response it continues (and the server
/// rebuilds exactly the submitted input), keeps at most `max_idle` free
/// connections, never waits past `affinity_wait`, and closes idle,
/// stalled and aged connections only on time.
#[hegel::test(test_cases = 300)]
fn pool_places_lanes_by_affinity(tc: TestCase) {
    pool_places_lanes_by_affinity_body(tc)
}

/// [`pool_places_lanes_by_affinity`] with more cases, for the nightly
/// tier.
#[hegel::test(profile = "nightly_slow")]
#[ignore = "nightly"]
fn pool_places_lanes_by_affinity_nightly(tc: TestCase) {
    pool_places_lanes_by_affinity_body(tc)
}

fn pool_places_lanes_by_affinity_body(tc: TestCase) {
    let seconds = |min: u64| {
        Duration::from_secs(
            tc.draw(gs::integers::<u64>().min_value(min).max_value(100)),
        )
    };
    let limits = Limits {
        rotate_after: seconds(1),
        idle_timeout: seconds(1),
        stall_timeout: seconds(1),
        affinity_wait: seconds(0),
        max_idle: tc.draw(gs::integers::<usize>().max_value(6)),
    };
    hegel::stateful::machine(PoolMachine::new(limits))
        .steps(80)
        .run(tc);
}

/// Completes `lane`'s request with `response_id` and no output.
fn complete(
    pool: &mut Pool,
    lane: LaneId,
    response_id: &str,
) -> Vec<PoolAction> {
    pool.handle(
        lane,
        Event::Completed {
            response_id: response_id.into(),
            output_items: vec![],
        },
    )
    .unwrap()
}

/// Each lane gets a connection of its own; a closed lane's connection
/// takes the next lane, and one that serves no path closes once it has
/// had no lane for `idle_timeout`.
#[test]
fn a_lane_per_connection_and_reuse() {
    let mut pool = Pool::new(Limits::default());
    let (first, opened) = pool.open_lane(Affinity::default());
    assert_eq!(opened, vec![PoolAction::Open(0)]);
    let (second, opened) = pool.open_lane(Affinity::default());
    assert_eq!(opened, vec![PoolAction::Open(1)]);
    assert_eq!(pool.lane_on(0), Some(first));
    assert_eq!(pool.lane_on(1), Some(second));
    assert!(pool.close_lane(first).unwrap().is_empty(), "stays open");
    assert_eq!(pool.lane_on(0), None);
    let (third, opened) = pool.open_lane(Affinity::default());
    assert!(opened.is_empty(), "{opened:?}");
    assert_eq!(pool.connection_of(third), Some(0));
    let stats = pool.stats();
    assert_eq!((stats.connections_opened, stats.connections_reused), (2, 1));
    pool.close_lane(third).unwrap();
    assert!(pool.tick(Duration::from_secs(4 * 60)).is_empty());
    assert_eq!(
        pool.tick(Duration::from_secs(5 * 60)),
        vec![PoolAction::Close(0)]
    );
}

/// A run of a conversation that went on before takes the connection
/// the conversation last used, and continues there by delta from where
/// the last run's lane left it; a free connection that serves a path
/// stays open past `idle_timeout`, until rotation.
#[test]
fn a_resumed_conversation_continues_on_its_connection() {
    let mut pool = Pool::new(Limits::default());
    let (other, _) = pool.open_lane(Affinity::new("other", None));
    let (main, _) = pool.open_lane(Affinity::new("main", None));
    assert_eq!(pool.connection_of(main), Some(1));
    pool.submit(main, on_path(Some("main"), vec![item("a")]))
        .unwrap();
    complete(&mut pool, main, "resp_1");
    pool.close_lane(main).unwrap();
    pool.close_lane(other).unwrap();
    // Twenty minutes later, past `idle_timeout`: still open.
    assert!(pool.tick(Duration::from_secs(20 * 60)).is_empty());

    let (again, opened) = pool.open_lane(Affinity::new("main", None));
    assert!(opened.is_empty());
    assert_eq!(pool.connection_of(again), Some(1), "its own connection");
    let sent = pool
        .submit(again, on_path(Some("main"), vec![item("a"), item("b")]))
        .unwrap();
    assert!(
        matches!(
            &sent[..],
            [PoolAction::Send { connection: 1, body, .. }]
                if body.previous_response_id.as_deref() == Some("resp_1")
                    && body.input.len() == 1
        ),
        "{sent:?}"
    );
    assert_eq!(pool.stats().own_connection, 1);
    // A request whose input does not extend the baseline goes in full.
    complete(&mut pool, again, "resp_2");
    pool.close_lane(again).unwrap();
    let (third, _) = pool.open_lane(Affinity::new("main", None));
    let sent = pool
        .submit(third, on_path(Some("main"), vec![item("x")]))
        .unwrap();
    assert!(
        matches!(&sent[..], [PoolAction::Send { connection: 1, body, .. }]
            if body.previous_response_id.is_none()),
        "{sent:?}"
    );
}

/// A fork's first request takes its parent's idle connection, even from
/// the parent's live lane: the connection serves the fork from then on.
/// The parent's next request takes another connection, and once the
/// fork's run ends, the parent's next run takes back the connection it
/// served before.
#[test]
fn a_fork_takes_its_parent_s_connection() {
    let mut pool = Pool::new(Limits::default());
    let (main, _) = pool.open_lane(Affinity::new("main", None));
    pool.submit(main, on_path(Some("main"), vec![item("a")]))
        .unwrap();
    complete(&mut pool, main, "resp_1");

    let (fork, opened) =
        pool.open_lane(Affinity::new("fork", Some("main".into())));
    assert!(opened.is_empty(), "{opened:?}");
    assert_eq!(pool.connection_of(fork), Some(0));
    assert_eq!(pool.connection_of(main), None, "the parent gave it up");
    assert_eq!(pool.stats().handoffs, 1);
    let state = &pool.connections()[0];
    assert_eq!(state.path.as_deref(), Some("fork"));
    assert_eq!(state.served_before.as_deref(), Some("main"));
    // The fork's first request goes in full: the parent's continuation
    // is not the fork's.
    let sent = pool
        .submit(fork, on_path(Some("fork"), vec![item("a"), item("f")]))
        .unwrap();
    assert!(
        matches!(&sent[..], [PoolAction::Send { connection: 0, body, .. }]
            if body.previous_response_id.is_none()),
        "{sent:?}"
    );
    // The parent sends while the fork's request is in flight: a new
    // connection, in full.
    let sent = pool
        .submit(main, on_path(Some("main"), vec![item("a"), item("m")]))
        .unwrap();
    assert!(
        matches!(&sent[..], [PoolAction::Open(1), PoolAction::Send { connection: 1, body, .. }]
            if body.previous_response_id.is_none()),
        "{sent:?}"
    );
    complete(&mut pool, fork, "resp_2");
    complete(&mut pool, main, "resp_3");
    pool.close_lane(main).unwrap();
    pool.close_lane(fork).unwrap();
    // `main` serves 1 now, and takes it again.
    let (later, _) = pool.open_lane(Affinity::new("main", None));
    assert_eq!(pool.connection_of(later), Some(1));
    pool.close_lane(later).unwrap();
    // Without 1, `main` takes 0, which it served before the fork took
    // it, over a free connection that serves no path.
    pool.connection_lost(1);
    let (plain, _) = pool.open_lane(Affinity::default());
    pool.close_lane(plain).unwrap();
    let (back, _) = pool.open_lane(Affinity::new("main", None));
    assert_eq!(pool.connection_of(back), Some(0));
    let state = &pool.connections()[0];
    assert_eq!(state.path.as_deref(), Some("main"));
    assert_eq!(state.served_before.as_deref(), Some("fork"));
}

/// After its first response, a fork no longer looks for its parent's
/// connection.
#[test]
fn only_a_fork_s_first_request_follows_its_parent() {
    let mut pool = Pool::new(Limits::default());
    let (fork, _) = pool.open_lane(Affinity::new("fork", Some("main".into())));
    pool.submit(fork, on_path(Some("fork"), vec![item("a")]))
        .unwrap();
    complete(&mut pool, fork, "resp_1");
    let (main, _) = pool.open_lane(Affinity::new("main", None));
    pool.close_lane(main).unwrap();
    // The fork loses its connection; `main`'s free one is there, but the
    // fork has answered: it takes the free connection by the usual
    // rule, not as a handoff.
    pool.connection_lost(0);
    pool.submit(fork, on_path(Some("fork"), vec![item("b")]))
        .unwrap();
    assert_eq!(pool.connection_of(fork), Some(1));
    assert_eq!(pool.stats().handoffs, 0);
}

/// A fork whose parent's connection has a response in flight waits for
/// it, at most `affinity_wait`; a request it sends meanwhile goes once
/// the connection is idle.
#[test]
fn a_fork_waits_briefly_for_its_parent_s_busy_connection() {
    let mut pool = Pool::new(Limits::default());
    let (main, _) = pool.open_lane(Affinity::new("main", None));
    pool.submit(main, on_path(Some("main"), vec![item("a")]))
        .unwrap();
    let (fork, opened) =
        pool.open_lane(Affinity::new("fork", Some("main".into())));
    assert!(opened.is_empty());
    assert!(pool.is_waiting(fork));
    assert_eq!(pool.stats().waits, 1);
    assert!(
        pool.submit(fork, on_path(Some("fork"), vec![item("f")]))
            .unwrap()
            .is_empty()
    );
    assert!(pool.is_busy(fork));
    assert_eq!(
        pool.submit(fork, body(9)),
        Err(PoolError::Busy(fork)),
        "one request at a time, waiting or not"
    );
    let actions = complete(&mut pool, main, "resp_1");
    assert!(
        matches!(&actions[..], [PoolAction::Send { connection: 0, lane, .. }] if *lane == fork),
        "{actions:?}"
    );
    assert_eq!(pool.connection_of(main), None);

    // A second fork's wait ends at `affinity_wait`: it takes a new
    // connection and sends there.
    let (other, _) =
        pool.open_lane(Affinity::new("other", Some("fork".into())));
    assert!(pool.is_waiting(other));
    pool.submit(other, on_path(Some("other"), vec![item("o")]))
        .unwrap();
    assert!(pool.tick(Duration::from_secs(4)).is_empty());
    let actions = pool.tick(Duration::from_secs(5));
    assert!(
        matches!(
            &actions[..],
            [PoolAction::Open(1), PoolAction::Send { connection: 1, .. }]
        ),
        "{actions:?}"
    );
    assert!(!pool.is_waiting(other));
}

/// With `affinity_wait` zero, a lane never waits.
#[test]
fn no_wait_takes_another_connection_at_once() {
    let mut pool = Pool::new(Limits {
        affinity_wait: Duration::ZERO,
        ..Limits::default()
    });
    let (main, _) = pool.open_lane(Affinity::new("main", None));
    pool.submit(main, on_path(Some("main"), vec![item("a")]))
        .unwrap();
    let (fork, opened) =
        pool.open_lane(Affinity::new("fork", Some("main".into())));
    assert_eq!(opened, vec![PoolAction::Open(1)]);
    assert_eq!(pool.connection_of(fork), Some(1));
}

/// Past `max_idle` free connections, the least recently used close,
/// those that serve no path first.
#[test]
fn free_connections_are_capped() {
    let mut pool = Pool::new(Limits {
        max_idle: 2,
        ..Limits::default()
    });
    let (plain, _) = pool.open_lane(Affinity::default());
    let lanes: Vec<LaneId> = ["a", "b", "c"]
        .iter()
        .map(|path| pool.open_lane(Affinity::new(*path, None)).0)
        .collect();
    assert!(pool.close_lane(lanes[0]).unwrap().is_empty());
    assert!(pool.close_lane(plain).unwrap().is_empty());
    // Three free: the one that serves no path goes.
    assert_eq!(
        pool.close_lane(lanes[1]).unwrap(),
        vec![PoolAction::Close(0)]
    );
    pool.tick(Duration::from_secs(1));
    let (d, _) = pool.open_lane(Affinity::new("b", None));
    assert_eq!(pool.connection_of(d), Some(2), "b's connection");
    // Free now: a (used at 0 s) and, after d closes, b (used at 1 s); c
    // closing makes three, and a, the least recently used, goes.
    pool.close_lane(d).unwrap();
    assert_eq!(
        pool.close_lane(lanes[2]).unwrap(),
        vec![PoolAction::Close(1)]
    );
}

/// A lane holds one request at a time.
#[test]
fn a_busy_lane_takes_no_second_request() {
    let mut pool = Pool::new(Limits::default());
    let (lane, _) = pool.open_lane(Affinity::default());
    pool.submit(lane, body(1)).unwrap();
    assert_eq!(pool.submit(lane, body(2)), Err(PoolError::Busy(lane)));
    assert!(pool.is_busy(lane));
    assert!(pool.cancel(lane).unwrap().is_empty());
    assert!(!pool.is_busy(lane));
}

/// The connection limit moves the lane to a fresh connection, resends in
/// full there, and closes the old connection.
#[test]
fn connection_limit_moves_the_lane() {
    let mut pool = Pool::new(Limits::default());
    let (lane, _) = pool.open_lane(Affinity::default());
    pool.submit(lane, body(1)).unwrap();
    let actions = pool
        .handle(
            lane,
            Event::ServerError {
                code: Some(CONNECTION_LIMIT_REACHED.into()),
            },
        )
        .unwrap();
    assert!(
        matches!(
            &actions[..],
            [
                PoolAction::Close(0),
                PoolAction::Open(1),
                PoolAction::Send { connection: 1, body, .. },
            ] if body.previous_response_id.is_none()
        ),
        "{actions:?}"
    );
    assert_eq!(pool.lane_on(0), None);
    assert_eq!(pool.lane_on(1), Some(lane));
    assert!(pool.is_busy(lane));
}

/// Losing a connection: a request without output resends in full
/// elsewhere, one with output fails, and an idle lane is left without a
/// connection until it sends.
#[test]
fn lost_connection_moves_its_lane() {
    let mut pool = Pool::new(Limits::default());
    let (quiet, _) = pool.open_lane(Affinity::default());
    let (talking, _) = pool.open_lane(Affinity::default());
    let (idle, _) = pool.open_lane(Affinity::default());
    pool.submit(quiet, body(1)).unwrap();
    pool.submit(talking, body(2)).unwrap();
    pool.handle(talking, Event::Output).unwrap();

    let actions = pool.connection_lost(0);
    assert!(
        matches!(
            &actions[..],
            [PoolAction::Open(3), PoolAction::Send { lane, connection: 3, body }]
                if *lane == quiet && body.previous_response_id.is_none()
        ),
        "{actions:?}"
    );
    let actions = pool.connection_lost(1);
    assert!(
        matches!(&actions[..], [PoolAction::Fail { lane, .. }] if *lane == talking),
        "{actions:?}"
    );
    assert_eq!(pool.connection_of(talking), None);
    assert!(pool.connection_lost(2).is_empty());
    assert_eq!(pool.connection_of(idle), None);
    assert_eq!(pool.stats().lanes.connection_lost, 1);
}

/// Unknown lanes are rejected.
#[test]
fn unknown_lane_is_rejected() {
    let mut pool = Pool::new(Limits::default());
    assert_eq!(pool.submit(7, body(0)), Err(PoolError::UnknownLane(7)));
    assert_eq!(pool.cancel(7), Err(PoolError::UnknownLane(7)));
    assert_eq!(
        pool.handle(7, Event::Output),
        Err(PoolError::UnknownLane(7))
    );
    assert!(pool.close_lane(7).is_err());
}

/// Recovery counters survive closing the lane that recovered.
#[test]
fn closed_lane_stats_are_kept() {
    let mut pool = Pool::new(Limits::default());
    let (lane, _) = pool.open_lane(Affinity::default());
    pool.submit(lane, body(1)).unwrap();
    pool.handle(
        lane,
        Event::ServerError {
            code: Some(CONNECTION_LIMIT_REACHED.into()),
        },
    )
    .unwrap();
    complete(&mut pool, lane, "resp_1");
    let (other, _) = pool.open_lane(Affinity::default());
    pool.submit(other, body(2)).unwrap();
    pool.connection_lost(pool.connection_of(other).unwrap());
    // A third lane continues with a delta that the server has lost.
    let (third, _) = pool.open_lane(Affinity::default());
    pool.submit(third, with_input(json!([item("a")]))).unwrap();
    complete(&mut pool, third, "resp_3");
    pool.submit(third, with_input(json!([item("a"), item("b")])))
        .unwrap();
    pool.handle(
        third,
        Event::ServerError {
            code: Some(PREVIOUS_RESPONSE_NOT_FOUND.into()),
        },
    )
    .unwrap();
    let before = pool.stats();
    assert_eq!(before.lanes.delta_requests, 1);
    assert_eq!(before.lanes.previous_response_not_found, 1);
    pool.close_lane(lane).unwrap();
    pool.close_lane(other).unwrap();
    pool.close_lane(third).unwrap();
    let after = pool.stats();
    assert_eq!(after, before);
    assert_eq!(after.lanes.connection_limit_reached, 1);
    assert_eq!(after.lanes.connection_lost, 1);
    assert_eq!(after.lanes.full_requests, 6);
}

/// Error messages name the problem and the lane.
#[test]
fn errors_display() {
    assert_eq!(PoolError::UnknownLane(3).to_string(), "unknown lane 3");
    assert_eq!(
        PoolError::Busy(4).to_string(),
        "lane 4 already has a request"
    );
}

/// At `rotate_after`, an idle lane leaves at once, a busy lane leaves
/// when its request finishes, and the emptied connections close. A
/// lane that left takes a connection when it next sends, in full.
#[test]
fn rotation_moves_lanes_and_closes() {
    // Hours pass with a request in flight and no frames; stalls are
    // not what this test is about.
    let mut pool = Pool::new(Limits {
        stall_timeout: Duration::MAX,
        ..Limits::default()
    });
    let (busy, _) = pool.open_lane(Affinity::default());
    let (idle, _) = pool.open_lane(Affinity::default());
    pool.submit(idle, body(1)).unwrap();
    complete(&mut pool, idle, "resp_1");
    pool.submit(busy, body(2)).unwrap();

    assert!(pool.tick(Duration::from_secs(54 * 60)).is_empty());
    let actions = pool.tick(Duration::from_secs(55 * 60));
    assert_eq!(actions, vec![PoolAction::Close(1)]);
    assert_eq!(pool.connection_of(idle), None);
    assert_eq!(pool.connection_of(busy), Some(0));
    assert!(pool.is_draining(0));

    let (fresh, _) = pool.open_lane(Affinity::default());
    assert_eq!(
        pool.connection_of(fresh),
        Some(2),
        "draining connection took a lane"
    );

    let actions = complete(&mut pool, busy, "resp_2");
    assert_eq!(actions, vec![PoolAction::Close(0)]);
    assert_eq!(pool.connection_of(busy), None);

    let next = with_input(json!([item("1"), item("more")]));
    let sent = pool.submit(idle, next).unwrap();
    assert!(
        matches!(&sent[..], [PoolAction::Open(3), PoolAction::Send { connection: 3, body, .. }]
        if body.previous_response_id.is_none()),
        "{sent:?}"
    );
}

/// Time never runs backwards inside the pool, and a connection's age
/// counts from when it opened.
#[test]
fn tick_ignores_earlier_times() {
    let mut pool = Pool::new(Limits::default());
    let (lane, _) = pool.open_lane(Affinity::default());
    let actions = pool.tick(Duration::from_secs(55 * 60));
    assert_eq!(actions, vec![PoolAction::Close(0)]);
    assert!(pool.tick(Duration::ZERO).is_empty());
    // Its next request opens connection 1, whose age starts then.
    pool.submit(lane, body(1)).unwrap();
    complete(&mut pool, lane, "resp_1");
    assert!(pool.tick(Duration::from_secs(109 * 60)).is_empty());
    assert_eq!(pool.connection_of(lane), Some(1));
    let actions = pool.tick(Duration::from_secs(110 * 60));
    assert_eq!(actions, vec![PoolAction::Close(1)]);
}

/// A lost connection's idle lanes open nothing until a request needs a
/// connection: a connection that keeps failing to open is never retried
/// in a loop behind idle lanes.
#[test]
fn idle_lanes_reopen_only_for_a_request() {
    let mut pool = Pool::new(Limits::default());
    let (lane, opened) = pool.open_lane(Affinity::default());
    assert_eq!(opened, vec![PoolAction::Open(0)]);
    assert!(pool.connection_lost(0).is_empty(), "nothing opens yet");
    assert_eq!(pool.connection_of(lane), None);
    let sent = pool.submit(lane, body(1)).unwrap();
    assert!(
        matches!(
            &sent[..],
            [PoolAction::Open(1), PoolAction::Send { connection: 1, .. }]
        ),
        "{sent:?}"
    );
    assert_eq!(pool.stats().connections_opened, 2);
}
