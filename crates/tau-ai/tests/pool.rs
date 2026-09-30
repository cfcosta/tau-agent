//! The connection pool (`tau_ai::ws::proto::pool`), driven by a Hegel
//! state machine under small time limits and checked against a model
//! built from the actions it returns, with a simulated server that keeps
//! each connection's responses.

use std::{
    collections::{BTreeMap, BTreeSet},
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
                    // One request in flight per connection.
                    assert!(
                        self.in_flight
                            .iter()
                            .all(|(l, c)| l == lane || c != connection),
                        "a second request on {connection}"
                    );
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

    /// Lanes with no request in flight.
    fn idle_lanes(&self) -> Vec<LaneId> {
        self.lanes
            .iter()
            .copied()
            .filter(|l| !self.in_flight.contains_key(l))
            .collect()
    }

    /// Forgets a lane's request, as cancel and close do.
    fn drop_request(&mut self, lane: LaneId) {
        self.in_flight.remove(&lane);
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
            if self.pool.lane_on(c).is_none() && !self.pool.is_draining(c) {
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

    /// Counts and placement agree with the model after every step.
    #[invariant(always_run)]
    fn pool_matches_the_model(&self, _tc: TestCase) {
        // A draining connection only keeps a lane with a request in
        // flight.
        for &c in &self.model.open {
            if self.pool.is_draining(c)
                && let Some(lane) = self.pool.lane_on(c)
            {
                assert!(
                    self.model.in_flight.contains_key(&lane),
                    "idle lane {lane} left on draining {c}"
                );
            }
        }
        // Every in-flight request is on its lane's connection, and each
        // connection carries the one lane placed on it.
        for (&lane, &connection) in &self.model.in_flight {
            assert_eq!(
                self.pool.connection_of(lane),
                Some(connection),
                "lane {lane} moved silently"
            );
        }
        let mut placed: BTreeSet<ConnectionId> = BTreeSet::new();
        for &lane in &self.model.lanes {
            let c = self.pool.connection_of(lane).expect("known lane");
            assert!(placed.insert(c), "two lanes on {c}");
            assert_eq!(self.pool.lane_on(c), Some(lane), "lane on {c}");
        }
        for &c in &self.model.open {
            if let Some(lane) = self.pool.lane_on(c) {
                assert!(self.model.lanes.contains(&lane), "closed lane on {c}");
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
        // Every lane lives on an open connection, or on one that opens
        // with its first request: after a lost connection, an idle lane
        // waits for a request before anything is opened for it.
        for &lane in &self.model.lanes {
            let c = self.pool.connection_of(lane).expect("known lane");
            assert!(
                self.model.open.contains(&c)
                    || !self.model.in_flight.contains_key(&lane),
                "lane {lane} on closed connection {c}"
            );
            assert_eq!(
                self.pool.is_busy(lane),
                self.model.in_flight.contains_key(&lane),
                "lane {lane}"
            );
        }
    }
}

/// Under random operations and small time limits, the pool keeps one
/// lane and one request per connection, only sends a delta on the
/// connection that holds the response it continues (and the server
/// rebuilds exactly the submitted input), resends in full after
/// `previous_response_not_found`, and closes idle, stalled and aged
/// connections only on time.
#[hegel::test(test_cases = 300)]
fn pool_keeps_one_lane_per_connection(tc: TestCase) {
    pool_keeps_one_lane_per_connection_body(tc)
}

/// [`pool_keeps_one_lane_per_connection`] with more cases, for the
/// nightly tier.
#[hegel::test(profile = "nightly_slow")]
#[ignore = "nightly"]
fn pool_keeps_one_lane_per_connection_nightly(tc: TestCase) {
    pool_keeps_one_lane_per_connection_body(tc)
}

fn pool_keeps_one_lane_per_connection_body(tc: TestCase) {
    let limits = Limits {
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

/// Each lane gets a connection of its own; a closed lane's connection
/// takes the next lane, and closes once it has had no lane for
/// `idle_timeout`.
#[test]
fn a_lane_per_connection_and_reuse() {
    let mut pool = Pool::new(Limits::default());
    let (first, opened) = pool.open_lane();
    assert_eq!(opened, vec![PoolAction::Open(0)]);
    let (second, opened) = pool.open_lane();
    assert_eq!(opened, vec![PoolAction::Open(1)]);
    assert_eq!(pool.lane_on(0), Some(first));
    assert_eq!(pool.lane_on(1), Some(second));
    assert!(pool.close_lane(first).unwrap().is_empty(), "stays open");
    assert_eq!(pool.lane_on(0), None);
    let (third, opened) = pool.open_lane();
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

/// A lane holds one request at a time.
#[test]
fn a_busy_lane_takes_no_second_request() {
    let mut pool = Pool::new(Limits::default());
    let (lane, _) = pool.open_lane();
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
    assert_eq!(pool.lane_on(0), None);
    assert_eq!(pool.lane_on(1), Some(lane));
    assert!(pool.is_busy(lane));
}

/// Losing a connection moves its lane: a request without output resends
/// in full, one with output fails, and an idle lane just moves.
#[test]
fn lost_connection_moves_its_lane() {
    let mut pool = Pool::new(Limits::default());
    let (quiet, _) = pool.open_lane();
    let (talking, _) = pool.open_lane();
    let (idle, _) = pool.open_lane();
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
    assert_eq!(pool.connection_of(talking), Some(4));
    assert!(pool.connection_lost(2).is_empty());
    assert_eq!(pool.connection_of(idle), Some(5));
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

/// At `rotate_after`, an idle lane moves at once, a busy lane moves when
/// its request finishes, and the emptied connections close. A moved
/// lane's next request is a full resend.
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
    assert_eq!(actions, vec![PoolAction::Open(2), PoolAction::Close(1)]);
    assert_eq!(pool.connection_of(idle), Some(2));
    assert_eq!(pool.connection_of(busy), Some(0));
    assert!(pool.is_draining(0));

    let (fresh, _) = pool.open_lane();
    assert_eq!(
        pool.connection_of(fresh),
        Some(3),
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
    assert_eq!(actions, vec![PoolAction::Open(4), PoolAction::Close(0)]);
    assert_eq!(pool.connection_of(busy), Some(4));

    let next = with_input(
        json!([{"type": "message", "text": "1"}, {"type": "message", "text": "more"}]),
    );
    let sent = pool.submit(idle, next).unwrap();
    assert!(
        matches!(&sent[..], [PoolAction::Send { connection: 2, body, .. }]
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

/// A lost connection's idle lanes open nothing until a request needs a
/// connection: a connection that keeps failing to open is never retried
/// in a loop behind idle lanes.
#[test]
fn idle_lanes_reopen_only_for_a_request() {
    let mut pool = Pool::new(Limits::default());
    let (lane, opened) = pool.open_lane();
    assert_eq!(opened, vec![PoolAction::Open(0)]);
    assert!(pool.connection_lost(0).is_empty(), "nothing opens yet");
    assert_eq!(pool.connection_of(lane), Some(1));
    // Lost again before it opened: still nothing, and no `Close` for a
    // connection the driver never saw.
    assert!(pool.connection_lost(1).is_empty());
    let sent = pool.submit(lane, body(1)).unwrap();
    assert!(
        matches!(
            &sent[..],
            [PoolAction::Open(2), PoolAction::Send { connection: 2, .. }]
        ),
        "{sent:?}"
    );
    assert_eq!(pool.stats().connections_opened, 2);
}
