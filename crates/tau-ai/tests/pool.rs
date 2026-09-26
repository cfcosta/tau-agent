//! The connection pool (`tau_ai::ws::proto::pool`), driven by random
//! operation sequences under small limits and checked against a model
//! built from the actions it returns.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use hegel::{TestCase, generators as gs};
use serde_json::json;
use tau_ai::ws::proto::{
    continuation::Body,
    lane::{CONNECTION_LIMIT_REACHED, Event, PREVIOUS_RESPONSE_NOT_FOUND},
    pool::{ConnectionId, LaneId, Limits, Pool, PoolAction, PoolError},
};

fn body(n: u64) -> Body {
    let mut body = Body::new();
    body.insert("model".into(), json!("gpt-5.5"));
    body.insert(
        "input".into(),
        json!([{"type": "message", "text": n.to_string()}]),
    );
    body
}

/// What the test knows from the actions alone.
#[derive(Default, Debug)]
struct Model {
    open: BTreeSet<ConnectionId>,
    /// Lanes whose request was sent and has not finished, by connection.
    in_flight: BTreeMap<LaneId, ConnectionId>,
    /// Lanes whose request was submitted but not yet sent, in order.
    waiting: VecDeque<LaneId>,
    lanes: BTreeSet<LaneId>,
    /// Lanes whose in-flight request has produced output.
    started: BTreeSet<LaneId>,
    full_sends: u64,
    delta_sends: u64,
}

impl Model {
    fn apply(&mut self, actions: &[PoolAction]) {
        for action in actions {
            match action {
                PoolAction::Open(c) => {
                    assert!(
                        self.open.insert(*c),
                        "connection {c} opened twice"
                    );
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
                    self.waiting.retain(|l| l != lane);
                    self.in_flight.insert(*lane, *connection);
                    if body.contains_key("previous_response_id") {
                        self.delta_sends += 1;
                    } else {
                        self.full_sends += 1;
                    }
                    self.started.remove(lane);
                }
                PoolAction::Fail { lane, .. } => {
                    self.in_flight.remove(lane);
                    self.started.remove(lane);
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    OpenLane,
    CloseLane,
    Submit,
    Output,
    Complete,
    ServerError,
    ConnectionLimit,
    Cancel,
    LoseConnection,
}

#[hegel::test(test_cases = 300)]
fn pool_keeps_limits_and_order(tc: TestCase) {
    let limits = Limits {
        max_lanes: tc.draw(gs::integers::<usize>().min_value(1).max_value(4)),
        max_in_flight: tc
            .draw(gs::integers::<usize>().min_value(1).max_value(3)),
    };
    let mut pool = Pool::new(limits);
    let mut model = Model::default();
    let steps = tc.draw(gs::integers::<usize>().min_value(1).max_value(60));
    let mut next_body = 0;

    for step in 0..steps {
        let op = tc.draw(gs::sampled_from(vec![
            Op::OpenLane,
            Op::CloseLane,
            Op::Submit,
            Op::Output,
            Op::Complete,
            Op::ServerError,
            Op::ConnectionLimit,
            Op::Cancel,
            Op::LoseConnection,
        ]));
        let any_lane =
            |tc: &TestCase, lanes: &BTreeSet<LaneId>| -> Option<LaneId> {
                let lanes: Vec<_> = lanes.iter().copied().collect();
                (!lanes.is_empty()).then(|| tc.draw(gs::sampled_from(lanes)))
            };
        let flying: BTreeSet<LaneId> =
            model.in_flight.keys().copied().collect();
        tc.note(&format!("step {step}: {op:?}"));
        let actions = match op {
            Op::OpenLane => {
                let (lane, actions) = pool.open_lane();
                model.lanes.insert(lane);
                actions
            }
            Op::CloseLane => match any_lane(&tc, &model.lanes) {
                Some(lane) => {
                    model.lanes.remove(&lane);
                    model.in_flight.remove(&lane);
                    model.waiting.retain(|&l| l != lane);
                    pool.close_lane(lane).unwrap()
                }
                None => continue,
            },
            Op::Submit => {
                let idle: BTreeSet<_> = model
                    .lanes
                    .iter()
                    .copied()
                    .filter(|l| {
                        !model.in_flight.contains_key(l)
                            && !model.waiting.contains(l)
                    })
                    .collect();
                match any_lane(&tc, &idle) {
                    Some(lane) => {
                        next_body += 1;
                        model.waiting.push_back(lane);
                        let actions =
                            pool.submit(lane, body(next_body)).unwrap();
                        assert_eq!(
                            pool.submit(lane, body(0)),
                            Err(PoolError::Busy(lane))
                        );
                        actions
                    }
                    None => continue,
                }
            }
            Op::Output
            | Op::Complete
            | Op::ServerError
            | Op::ConnectionLimit => {
                let Some(lane) = any_lane(&tc, &flying) else {
                    continue;
                };
                let event = match op {
                    Op::Output => {
                        model.started.insert(lane);
                        Event::Output
                    }
                    Op::Complete => {
                        model.in_flight.remove(&lane);
                        Event::Completed {
                            response_id: format!("resp_{step}"),
                            output_items: vec![],
                        }
                    }
                    Op::ServerError => {
                        model.in_flight.remove(&lane);
                        Event::ServerError {
                            code: Some("server_error".into()),
                        }
                    }
                    _ => {
                        if model.started.contains(&lane) {
                            // Fails the request.
                            model.in_flight.remove(&lane);
                        }
                        Event::ServerError {
                            code: Some(CONNECTION_LIMIT_REACHED.into()),
                        }
                    }
                };
                pool.handle(lane, event).unwrap()
            }
            Op::Cancel => {
                let Some(lane) = any_lane(&tc, &model.lanes) else {
                    continue;
                };
                model.in_flight.remove(&lane);
                model.waiting.retain(|&l| l != lane);
                pool.cancel(lane).unwrap()
            }
            Op::LoseConnection => {
                let Some(connection) = any_lane(&tc, &model.open) else {
                    continue;
                };
                model.open.remove(&connection);
                pool.connection_lost(connection)
            }
        };
        model.apply(&actions);

        // Limits hold on every connection.
        for &c in &model.open {
            assert!(pool.lane_count(c) <= limits.max_lanes, "lanes on {c}");
            assert!(
                pool.in_flight(c) <= limits.max_in_flight,
                "in flight on {c}"
            );
        }
        // The pool's in-flight count per connection matches the requests
        // the model saw sent and not finished.
        let mut per_connection: BTreeMap<ConnectionId, usize> = BTreeMap::new();
        for (&lane, &connection) in &model.in_flight {
            assert_eq!(
                pool.connection_of(lane),
                Some(connection),
                "lane {lane} moved silently"
            );
            *per_connection.entry(connection).or_default() += 1;
        }
        for &c in &model.open {
            assert_eq!(
                pool.in_flight(c),
                per_connection.get(&c).copied().unwrap_or(0),
                "connection {c}"
            );
        }
        // Lane counts per connection match where the lanes live.
        let mut lanes_on: BTreeMap<ConnectionId, usize> = BTreeMap::new();
        for &lane in &model.lanes {
            *lanes_on
                .entry(pool.connection_of(lane).unwrap())
                .or_default() += 1;
        }
        for &c in &model.open {
            assert_eq!(
                pool.lane_count(c),
                lanes_on.get(&c).copied().unwrap_or(0),
                "lanes on {c}"
            );
        }
        // Request counters cover every lane, closed ones included.
        let stats = pool.stats();
        assert_eq!(stats.lanes.full_requests, model.full_sends);
        assert_eq!(stats.lanes.delta_requests, model.delta_sends);
        // Every lane lives on an open connection.
        for &lane in &model.lanes {
            let c = pool.connection_of(lane).expect("known lane");
            assert!(
                model.open.contains(&c),
                "lane {lane} on closed connection {c}"
            );
            assert_eq!(
                pool.is_busy(lane),
                model.in_flight.contains_key(&lane)
                    || model.waiting.contains(&lane),
                "lane {lane}"
            );
        }
        // A request waits only while its connection is full.
        for &lane in &model.waiting {
            let c = pool.connection_of(lane).unwrap();
            assert_eq!(
                pool.in_flight(c),
                limits.max_in_flight,
                "lane {lane} waits on a free connection"
            );
        }
    }
}

/// Waiting requests on one connection start in submission order.
#[test]
fn waiting_requests_start_in_order() {
    let mut pool = Pool::new(Limits {
        max_lanes: 4,
        max_in_flight: 1,
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
            [PoolAction::Open(1), PoolAction::Send { connection: 1, body, .. }]
                if !body.contains_key("previous_response_id")
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
        if *lane == quiet && !body.contains_key("previous_response_id"))
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
    let mut first = body(3);
    first.insert("input".into(), first_input);
    pool.submit(third, first).unwrap();
    pool.handle(
        third,
        Event::Completed {
            response_id: "resp_3".into(),
            output_items: vec![],
        },
    )
    .unwrap();
    let mut next = body(3);
    next.insert("input".into(), json!([{"type": "message", "text": "a"}, {"type": "message", "text": "b"}]));
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
