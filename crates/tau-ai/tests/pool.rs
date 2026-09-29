//! The connection pool (`tau_ai::ws::proto::pool`), driven by a Hegel
//! state machine under small limits and checked against a model built
//! from the actions it returns, with a simulated server that keeps each
//! connection's responses.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    time::Duration,
};

use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_ai::ws::proto::{
    continuation::Body,
    lane::{CONNECTION_LIMIT_REACHED, Event, PREVIOUS_RESPONSE_NOT_FOUND},
    pool::{ConnectionId, LaneId, Limits, Pool, PoolAction, PoolError},
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

/// A request as the server rebuilt it when it arrived.
#[derive(Debug)]
struct Sent {
    /// The full input: the held response's items and the delta, or the
    /// whole input of a full request.
    input: Vec<Value>,
    previous_response_id: Option<String>,
}

/// What the test knows from the actions alone, and what the simulated
/// server holds.
#[derive(Default, Debug)]
struct Model {
    open: BTreeSet<ConnectionId>,
    opened_at: BTreeMap<ConnectionId, Duration>,
    now: Duration,
    /// Lanes whose request was sent and has not finished, by connection.
    in_flight: BTreeMap<LaneId, ConnectionId>,
    /// Lanes whose request was submitted but not yet sent, in submission
    /// order, with the connection they wait on.
    waiting: VecDeque<(LaneId, ConnectionId)>,
    lanes: BTreeSet<LaneId>,
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
                        self.open.insert(*c),
                        "connection {c} opened twice"
                    );
                    self.opened_at.insert(*c, self.now);
                    self.held.insert(*c, BTreeMap::new());
                }
                PoolAction::Close(c) => {
                    assert!(self.open.remove(c), "closed unknown {c}");
                    self.held.remove(c);
                }
                PoolAction::Send {
                    connection,
                    lane,
                    body,
                } => {
                    assert!(
                        self.open.contains(connection),
                        "send on unknown connection"
                    );
                    // A request that waited on this connection starts only
                    // when every request that waited there before it has.
                    if self.waiting.contains(&(*lane, *connection)) {
                        let first = self
                            .waiting
                            .iter()
                            .find(|(_, c)| c == connection)
                            .map(|(l, _)| *l);
                        assert_eq!(
                            first,
                            Some(*lane),
                            "lane {lane} overtook a request waiting on \
                             {connection}"
                        );
                        tc.event("a waiting request started");
                    }
                    self.waiting.retain(|(l, _)| l != lane);
                    self.in_flight.insert(*lane, *connection);
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
            .iter()
            .copied()
            .filter(|l| {
                !self.in_flight.contains_key(l)
                    && !self.waiting.iter().any(|(w, _)| w == l)
            })
            .collect()
    }

    /// Forgets a lane's request, as cancel and close do.
    fn drop_request(&mut self, lane: LaneId) {
        self.in_flight.remove(&lane);
        self.waiting.retain(|(l, _)| *l != lane);
        self.sent.remove(&lane);
        self.started.remove(&lane);
    }
}

/// The pool, its model, and the timers the test keeps from the actions.
struct PoolMachine {
    limits: Limits,
    pool: Pool,
    model: Model,
    /// When each open connection last sent a request or received anything.
    last_activity: BTreeMap<ConnectionId, Duration>,
    /// When each open connection lost its last lane.
    empty_since: BTreeMap<ConnectionId, Duration>,
    now: Duration,
    next_item: u64,
    next_response: u64,
}

impl PoolMachine {
    fn new(limits: Limits) -> Self {
        Self {
            limits,
            pool: Pool::new(limits),
            model: Model::default(),
            last_activity: BTreeMap::new(),
            empty_since: BTreeMap::new(),
            now: Duration::ZERO,
            next_item: 0,
            next_response: 0,
        }
    }

    fn draw_lane(tc: &TestCase, lanes: Vec<LaneId>) -> LaneId {
        tc.draw(gs::sampled_from(lanes))
    }

    /// A frame for an in-flight lane: activity on its connection.
    fn frame_for(&mut self, lane: LaneId, event: Event) -> Vec<PoolAction> {
        let connection = self.pool.connection_of(lane).unwrap();
        self.last_activity.insert(connection, self.now);
        self.pool.handle(lane, event).unwrap()
    }

    /// Checks one step's actions and folds them into the model and the
    /// timers. `tick` says whether the step was a tick.
    fn after(&mut self, tc: &TestCase, actions: Vec<PoolAction>, tick: bool) {
        let busy_before: BTreeSet<ConnectionId> =
            self.model.in_flight.values().copied().collect();
        let limits = self.limits;
        let now = self.now;
        // Stalls: in a tick, a connection with requests in flight closes
        // only after `stall_timeout` without activity (rotation never
        // moves a busy lane; outside ticks, a lane sent elsewhere by the
        // server can empty a busy connection).
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
        let open = &self.model.open;
        self.last_activity.retain(|c, _| open.contains(c));
        if tick {
            for &c in self.model.in_flight.values() {
                assert!(
                    now - self.last_activity[&c] < limits.stall_timeout,
                    "{c} stalled since {:?}, still open at {now:?}",
                    self.last_activity[&c]
                );
            }
        }

        // Idle connections: an open connection with no lanes that is not
        // draining is only ever closed by a tick, once it has had no
        // lanes for `idle_timeout`, or when it reaches `rotate_after`
        // (draining connections close as soon as they are empty, so they
        // never sit empty).
        for action in &actions {
            if let PoolAction::Close(c) = action
                && let Some(&since) = self.empty_since.get(c)
            {
                assert!(tick, "idle {c} closed outside a tick");
                let aged = now.saturating_sub(self.model.opened_at[c])
                    >= limits.rotate_after;
                assert!(
                    aged || now - since >= limits.idle_timeout,
                    "{c} closed after {:?} idle",
                    now - since
                );
            }
        }
        self.empty_since.retain(|c, _| open.contains(c));
        for &c in &self.model.open {
            if self.pool.lane_count(c) == 0 && !self.pool.is_draining(c) {
                self.empty_since.entry(c).or_insert(now);
            } else {
                self.empty_since.remove(&c);
            }
        }
        if tick {
            for (&c, &since) in &self.empty_since {
                assert!(
                    now - since < limits.idle_timeout,
                    "{c} idle since {since:?} still open at {now:?}"
                );
            }
            // Rotation: a connection past `rotate_after` drains.
            for &c in &self.model.open {
                if now.saturating_sub(self.model.opened_at[&c])
                    >= limits.rotate_after
                {
                    assert!(
                        self.pool.is_draining(c),
                        "aged connection {c} not draining"
                    );
                }
            }
        }
    }
}

#[hegel::state_machine]
impl PoolMachine {
    #[rule]
    fn open_lane(&mut self, tc: TestCase) {
        let (lane, actions) = self.pool.open_lane();
        self.model.lanes.insert(lane);
        self.model.transcript.insert(lane, Vec::new());
        self.after(&tc, actions, false);
    }

    #[rule]
    fn close_lane(&mut self, tc: TestCase) {
        tc.assume(!self.model.lanes.is_empty());
        let lane =
            Self::draw_lane(&tc, self.model.lanes.iter().copied().collect());
        self.model.lanes.remove(&lane);
        self.model.drop_request(lane);
        self.model.transcript.remove(&lane);
        let actions = self.pool.close_lane(lane).unwrap();
        self.after(&tc, actions, false);
    }

    /// Submits the lane's transcript and one new item, so a lane whose
    /// last response completed on its connection can continue from it.
    #[rule(weight = 3)]
    fn submit(&mut self, tc: TestCase) {
        let idle = self.model.idle_lanes();
        tc.assume(!idle.is_empty());
        let lane = Self::draw_lane(&tc, idle);
        self.next_item += 1;
        let mut input = self.model.transcript[&lane].clone();
        input.push(
            json!({"type": "message", "text": self.next_item.to_string()}),
        );
        self.model.submitted.insert(lane, input.clone());
        let connection = self.pool.connection_of(lane).unwrap();
        self.model.waiting.push_back((lane, connection));
        let actions = self
            .pool
            .submit(lane, with_input(Value::Array(input)))
            .unwrap();
        assert_eq!(self.pool.submit(lane, body(0)), Err(PoolError::Busy(lane)));
        self.after(&tc, actions, false);
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
            .map(|i| json!({"type": "message", "text": format!("{response_id} out {i}")}))
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
        self.model.transcript.insert(lane, held);
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
            Self::draw_lane(&tc, self.model.lanes.iter().copied().collect());
        self.model.drop_request(lane);
        let actions = self.pool.cancel(lane).unwrap();
        self.after(&tc, actions, false);
    }

    #[rule]
    fn tick(&mut self, tc: TestCase) {
        self.now +=
            Duration::from_secs(tc.draw(gs::integers::<u64>().max_value(60)));
        self.model.now = self.now;
        let actions = self.pool.tick(self.now);
        self.after(&tc, actions, true);
    }

    #[rule]
    fn frame(&mut self, tc: TestCase) {
        tc.assume(!self.model.open.is_empty());
        let connection = tc.draw(gs::sampled_from(
            self.model.open.iter().copied().collect::<Vec<_>>(),
        ));
        self.last_activity.insert(connection, self.now);
        self.pool.activity(connection);
        self.after(&tc, Vec::new(), false);
    }

    #[rule]
    fn lose_connection(&mut self, tc: TestCase) {
        tc.assume(!self.model.open.is_empty());
        let connection = tc.draw(gs::sampled_from(
            self.model.open.iter().copied().collect::<Vec<_>>(),
        ));
        self.model.open.remove(&connection);
        self.model.held.remove(&connection);
        let actions = self.pool.connection_lost(connection);
        self.after(&tc, actions, false);
    }

    /// Limits, counts and placement agree with the model after every step.
    #[invariant(always_run)]
    fn pool_matches_the_model(&self, _tc: TestCase) {
        let limits = self.limits;
        // A draining connection only keeps lanes with a request in flight.
        for &c in &self.model.open {
            if self.pool.is_draining(c) {
                for &lane in &self.model.lanes {
                    if self.pool.connection_of(lane) == Some(c) {
                        assert!(
                            self.model.in_flight.contains_key(&lane),
                            "idle lane {lane} left on draining {c}"
                        );
                    }
                }
            }
        }
        // Limits hold on every connection.
        for &c in &self.model.open {
            assert!(
                self.pool.lane_count(c) <= limits.max_lanes,
                "lanes on {c}"
            );
            assert!(
                self.pool.in_flight(c) <= limits.max_in_flight,
                "in flight on {c}"
            );
        }
        // The pool's in-flight count per connection matches the requests
        // the model saw sent and not finished.
        let mut per_connection: BTreeMap<ConnectionId, usize> = BTreeMap::new();
        for (&lane, &connection) in &self.model.in_flight {
            assert_eq!(
                self.pool.connection_of(lane),
                Some(connection),
                "lane {lane} moved silently"
            );
            *per_connection.entry(connection).or_default() += 1;
        }
        for &c in &self.model.open {
            assert_eq!(
                self.pool.in_flight(c),
                per_connection.get(&c).copied().unwrap_or(0),
                "connection {c}"
            );
        }
        // Lane counts per connection match where the lanes live.
        let mut lanes_on: BTreeMap<ConnectionId, usize> = BTreeMap::new();
        for &lane in &self.model.lanes {
            *lanes_on
                .entry(self.pool.connection_of(lane).unwrap())
                .or_default() += 1;
        }
        for &c in &self.model.open {
            assert_eq!(
                self.pool.lane_count(c),
                lanes_on.get(&c).copied().unwrap_or(0),
                "lanes on {c}"
            );
        }
        // Request counters cover every lane, closed ones included.
        let stats = self.pool.stats();
        assert_eq!(stats.lanes.full_requests, self.model.full_sends);
        assert_eq!(stats.lanes.delta_requests, self.model.delta_sends);
        assert_eq!(
            stats.lanes.previous_response_not_found,
            self.model.not_found
        );
        // Every lane lives on an open connection.
        for &lane in &self.model.lanes {
            let c = self.pool.connection_of(lane).expect("known lane");
            assert!(
                self.model.open.contains(&c),
                "lane {lane} on closed connection {c}"
            );
            assert_eq!(
                self.pool.is_busy(lane),
                self.model.in_flight.contains_key(&lane)
                    || self.model.waiting.iter().any(|(l, _)| *l == lane),
                "lane {lane}"
            );
        }
        // A request waits only while its connection is full, on the
        // connection it was submitted on.
        for &(lane, connection) in &self.model.waiting {
            let c = self.pool.connection_of(lane).unwrap();
            assert_eq!(c, connection, "waiting lane {lane} moved silently");
            assert_eq!(
                self.pool.in_flight(c),
                limits.max_in_flight,
                "lane {lane} waits on a free connection"
            );
        }
    }
}

/// Under random operations and small limits, the pool keeps its lane and
/// in-flight limits, starts waiting requests in submission order, only
/// sends a delta on the connection that holds the response it continues
/// (and the server rebuilds exactly the submitted input), resends in full
/// after `previous_response_not_found`, and closes idle, stalled and aged
/// connections only on time.
#[hegel::test(test_cases = 300)]
fn pool_keeps_limits_and_order(tc: TestCase) {
    pool_keeps_limits_and_order_body(tc)
}

/// [`pool_keeps_limits_and_order`] with more cases, for the nightly tier.
#[hegel::test(profile = "nightly_slow")]
#[ignore = "nightly"]
fn pool_keeps_limits_and_order_nightly(tc: TestCase) {
    pool_keeps_limits_and_order_body(tc)
}

fn pool_keeps_limits_and_order_body(tc: TestCase) {
    let limits = Limits {
        max_lanes: tc.draw(gs::integers::<usize>().min_value(1).max_value(4)),
        max_in_flight: tc
            .draw(gs::integers::<usize>().min_value(1).max_value(3)),
        rotate_after: Duration::from_secs(
            tc.draw(gs::integers::<u64>().min_value(1).max_value(100)),
        ),
        idle_timeout: Duration::from_secs(
            tc.draw(gs::integers::<u64>().min_value(1).max_value(100)),
        ),
        stall_timeout: Duration::from_secs(
            tc.draw(gs::integers::<u64>().min_value(1).max_value(100)),
        ),
    };
    hegel::stateful::machine(PoolMachine::new(limits))
        .steps(60)
        .run(tc);
}

/// Waiting requests on one connection start in submission order.
#[test]
fn waiting_requests_start_in_order() {
    let mut pool = Pool::new(Limits {
        max_lanes: 4,
        max_in_flight: 1,
        ..Limits::default()
    });
    let lanes: Vec<LaneId> = (0..3).map(|_| pool.open_lane().0).collect();
    assert!(lanes.iter().all(|&l| pool.connection_of(l) == Some(0)));
    let first = pool.submit(lanes[0], body(0)).unwrap();
    assert!(
        matches!(first[..], [PoolAction::Send { lane, .. }] if lane == lanes[0])
    );
    assert!(pool.submit(lanes[2], body(2)).unwrap().is_empty());
    assert!(pool.submit(lanes[1], body(1)).unwrap().is_empty());
    let done = |pool: &mut Pool, lane| {
        pool.handle(
            lane,
            Event::Completed {
                response_id: format!("resp_{lane}"),
                output_items: vec![],
            },
        )
        .unwrap()
    };
    let next = done(&mut pool, lanes[0]);
    assert!(
        matches!(next[..], [PoolAction::Send { lane, .. }] if lane == lanes[2])
    );
    let next = done(&mut pool, lanes[2]);
    assert!(
        matches!(next[..], [PoolAction::Send { lane, .. }] if lane == lanes[1])
    );
}

/// A new lane opens a new connection once every connection is at a limit.
#[test]
fn new_connection_when_full() {
    let mut pool = Pool::new(Limits {
        max_lanes: 2,
        max_in_flight: 16,
        ..Limits::default()
    });
    let (_, first) = pool.open_lane();
    let (_, second) = pool.open_lane();
    let (third_lane, third) = pool.open_lane();
    assert_eq!(first, vec![PoolAction::Open(0)]);
    assert!(second.is_empty());
    assert_eq!(third, vec![PoolAction::Open(1)]);
    assert_eq!(pool.connection_of(third_lane), Some(1));
    let stats = pool.stats();
    assert_eq!((stats.connections_opened, stats.connections_reused), (2, 1));
}

/// The connection limit moves the lane to a fresh connection, resends in
/// full there, and the old connection takes no new lanes.
#[test]
fn connection_limit_moves_the_lane() {
    let mut pool = Pool::new(Limits::default());
    let (lane, _) = pool.open_lane();
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
    assert_eq!(pool.in_flight(0), 0);
    assert_eq!(pool.in_flight(1), 1);
    let (other, _) = pool.open_lane();
    assert_eq!(
        pool.connection_of(other),
        Some(1),
        "draining connection took a lane"
    );
}

/// Losing a connection moves every lane: requests without output resend
/// in full, those with output fail, idle lanes just move.
#[test]
fn lost_connection_moves_every_lane() {
    let mut pool = Pool::new(Limits::default());
    let (quiet, _) = pool.open_lane();
    let (talking, _) = pool.open_lane();
    let (idle, _) = pool.open_lane();
    pool.submit(quiet, body(1)).unwrap();
    pool.submit(talking, body(2)).unwrap();
    pool.handle(talking, Event::Output).unwrap();
    let actions = pool.connection_lost(0);
    assert_eq!(actions[0], PoolAction::Open(1));
    assert!(actions.iter().any(
        |a| matches!(a, PoolAction::Send { lane, connection: 1, body }
        if *lane == quiet && body.previous_response_id.is_none())
    ));
    assert!(actions.iter().any(
        |a| matches!(a, PoolAction::Fail { lane, .. } if *lane == talking)
    ));
    for lane in [quiet, talking, idle] {
        assert_eq!(pool.connection_of(lane), Some(1));
    }
    assert_eq!(pool.in_flight(1), 1);
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

/// Cancelling one waiting request leaves the others waiting, in order.
#[test]
fn cancel_waiting_keeps_the_others() {
    let mut pool = Pool::new(Limits {
        max_lanes: 4,
        max_in_flight: 1,
        ..Limits::default()
    });
    let lanes: Vec<LaneId> = (0..3).map(|_| pool.open_lane().0).collect();
    pool.submit(lanes[0], body(0)).unwrap();
    pool.submit(lanes[1], body(1)).unwrap();
    pool.submit(lanes[2], body(2)).unwrap();
    assert!(pool.cancel(lanes[1]).unwrap().is_empty());
    assert!(!pool.is_busy(lanes[1]));
    assert!(pool.is_busy(lanes[2]));
    let next = pool
        .handle(
            lanes[0],
            Event::Completed {
                response_id: "resp_0".into(),
                output_items: vec![],
            },
        )
        .unwrap();
    assert!(
        matches!(next[..], [PoolAction::Send { lane, .. }] if lane == lanes[2]),
        "{next:?}"
    );
}

/// A new lane skips a connection whose in-flight slots are all taken,
/// even when it has room for more lanes.
#[test]
fn new_lane_skips_connection_with_full_in_flight() {
    let mut pool = Pool::new(Limits {
        max_lanes: 8,
        max_in_flight: 1,
        ..Limits::default()
    });
    let (first, _) = pool.open_lane();
    pool.submit(first, body(1)).unwrap();
    let (second, actions) = pool.open_lane();
    assert_eq!(actions, vec![PoolAction::Open(1)]);
    assert_eq!(pool.connection_of(second), Some(1));
}

/// Recovery counters survive closing the lane that recovered.
#[test]
fn closed_lane_stats_are_kept() {
    let mut pool = Pool::new(Limits::default());
    let (lane, _) = pool.open_lane();
    pool.submit(lane, body(1)).unwrap();
    pool.handle(
        lane,
        Event::ServerError {
            code: Some(CONNECTION_LIMIT_REACHED.into()),
        },
    )
    .unwrap();
    pool.handle(
        lane,
        Event::Completed {
            response_id: "resp_1".into(),
            output_items: vec![],
        },
    )
    .unwrap();
    let (other, _) = pool.open_lane();
    pool.submit(other, body(2)).unwrap();
    pool.connection_lost(pool.connection_of(other).unwrap());
    // A third lane continues with a delta that the server has lost.
    let (third, _) = pool.open_lane();
    let first_input = json!([{"type": "message", "text": "a"}]);
    let first = with_input(first_input);
    pool.submit(third, first).unwrap();
    pool.handle(
        third,
        Event::Completed {
            response_id: "resp_3".into(),
            output_items: vec![],
        },
    )
    .unwrap();
    let next = with_input(
        json!([{"type": "message", "text": "a"}, {"type": "message", "text": "b"}]),
    );
    pool.submit(third, next).unwrap();
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

/// At `rotate_after`, idle lanes move at once, a busy lane moves when its
/// request finishes, and the empty connection closes. The moved lane's
/// next request is a full resend.
#[test]
fn rotation_moves_lanes_and_closes() {
    // Hours pass with a request in flight and no frames; stalls are
    // not what this test is about.
    let mut pool = Pool::new(Limits {
        stall_timeout: Duration::MAX,
        ..Limits::default()
    });
    let (busy, _) = pool.open_lane();
    let (idle, _) = pool.open_lane();
    pool.submit(idle, body(1)).unwrap();
    pool.handle(
        idle,
        Event::Completed {
            response_id: "resp_1".into(),
            output_items: vec![],
        },
    )
    .unwrap();
    pool.submit(busy, body(2)).unwrap();

    assert!(pool.tick(Duration::from_secs(54 * 60)).is_empty());
    let actions = pool.tick(Duration::from_secs(55 * 60));
    assert_eq!(actions, vec![PoolAction::Open(1)]);
    assert_eq!(pool.connection_of(idle), Some(1));
    assert_eq!(pool.connection_of(busy), Some(0));
    assert!(pool.is_draining(0));

    let (fresh, _) = pool.open_lane();
    assert_eq!(
        pool.connection_of(fresh),
        Some(1),
        "draining connection took a lane"
    );

    let actions = pool
        .handle(
            busy,
            Event::Completed {
                response_id: "resp_2".into(),
                output_items: vec![],
            },
        )
        .unwrap();
    assert_eq!(actions, vec![PoolAction::Close(0)]);
    assert_eq!(pool.connection_of(busy), Some(1));

    let next = with_input(
        json!([{"type": "message", "text": "1"}, {"type": "message", "text": "more"}]),
    );
    let sent = pool.submit(idle, next).unwrap();
    assert!(
        matches!(&sent[..], [PoolAction::Send { connection: 1, body, .. }]
        if body.previous_response_id.is_none()),
        "{sent:?}"
    );
}

/// Time never runs backwards inside the pool, and a connection's age
/// counts from when it opened.
#[test]
fn tick_ignores_earlier_times() {
    let mut pool = Pool::new(Limits::default());
    let (lane, _) = pool.open_lane();
    // The idle lane moves to connection 1, opened at 55 minutes.
    let actions = pool.tick(Duration::from_secs(55 * 60));
    assert_eq!(actions, vec![PoolAction::Open(1), PoolAction::Close(0)]);
    assert!(pool.tick(Duration::ZERO).is_empty());
    assert!(pool.tick(Duration::from_secs(109 * 60)).is_empty());
    assert_eq!(pool.connection_of(lane), Some(1));
    let actions = pool.tick(Duration::from_secs(110 * 60));
    assert_eq!(actions, vec![PoolAction::Open(2), PoolAction::Close(1)]);
}

/// When one lane leaves for the age limit, the requests still in flight
/// on the old connection keep their count.
#[test]
fn connection_limit_keeps_other_in_flight_count() {
    let mut pool = Pool::new(Limits::default());
    let (leaving, _) = pool.open_lane();
    let (staying, _) = pool.open_lane();
    pool.submit(leaving, body(1)).unwrap();
    pool.submit(staying, body(2)).unwrap();
    pool.handle(
        leaving,
        Event::ServerError {
            code: Some(CONNECTION_LIMIT_REACHED.into()),
        },
    )
    .unwrap();
    assert_eq!(pool.connection_of(staying), Some(0));
    assert_eq!(pool.in_flight(0), 1);
    assert_eq!(pool.in_flight(1), 1);
}

/// A request waiting on a connection that starts draining moves with its
/// lane, and the old connection never starts it.
#[test]
fn rotation_moves_waiting_requests() {
    let mut pool = Pool::new(Limits {
        stall_timeout: Duration::MAX,
        max_lanes: 4,
        max_in_flight: 1,
        ..Limits::default()
    });
    let (busy, _) = pool.open_lane();
    let (waiting, _) = pool.open_lane();
    pool.submit(busy, body(1)).unwrap();
    assert!(pool.submit(waiting, body(2)).unwrap().is_empty());
    let actions = pool.tick(Duration::from_secs(55 * 60));
    assert!(
        matches!(&actions[..], [PoolAction::Open(1), PoolAction::Send { connection: 1, lane, .. }] if *lane == waiting),
        "{actions:?}"
    );
    let actions = pool
        .handle(
            busy,
            Event::Completed {
                response_id: "resp_1".into(),
                output_items: vec![],
            },
        )
        .unwrap();
    // Connection 1's only in-flight slot is taken, so by the placement
    // rule the finished lane opens connection 2.
    assert_eq!(actions, vec![PoolAction::Open(2), PoolAction::Close(0)]);
}
