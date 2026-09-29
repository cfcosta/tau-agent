//! One lane (`tau_ai::ws::proto::lane`) driven against a simulated server,
//! with faults drawn by Hegel.
//!
//! The simulated server keeps OpenAI's rules: each connection holds the
//! responses it produced, a request whose `previous_response_id` the
//! connection does not hold fails with `previous_response_not_found`, and
//! a new connection starts empty.

use std::collections::HashMap;

use hegel::{TestCase, generators as gs};
use serde_json::{Map, Value, json};
use tau_ai::ws::proto::{
    continuation::Body,
    lane::{
        Action,
        CONNECTION_LIMIT_REACHED,
        Event,
        Failure,
        Lane,
        LaneError,
        PREVIOUS_RESPONSE_NOT_FOUND,
        STREAM_LIMIT_REACHED,
    },
};
use tau_testing::generators::{
    lane::{LaneHistory, Turn, item, lane_history},
    text,
};

/// What the server does with one request, drawn per request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, hegel::DefaultGenerator)]
enum Fault {
    None,
    /// The server forgets every response before this request arrives.
    Evict,
    ConnectionLimit,
    StreamLimit,
    LostBeforeOutput,
    LostAfterOutput,
    /// The connection is lost before any output, and the reconnect fails
    /// too.
    LostDuringReconnect,
    OtherError,
    /// An `error` frame with no code.
    UncodedError,
    /// The run cancels after some output.
    Cancel,
}

impl Fault {
    /// How the turn must fail, if it must: `second` says whether the
    /// resend after a recovery meets the same fault.
    fn required_failure(self, second: bool) -> Option<Failure> {
        let server = |code: &str| Failure::Server {
            code: Some(code.into()),
        };
        match self {
            Self::OtherError => Some(server("server_error")),
            Self::UncodedError => Some(Failure::Server { code: None }),
            Self::LostAfterOutput => Some(Failure::ConnectionLost {
                before_first_event: false,
            }),
            Self::LostDuringReconnect => Some(Failure::ConnectionLost {
                before_first_event: true,
            }),
            // One recovery per request: the same refusal on the resend
            // fails it.
            Self::ConnectionLimit if second => {
                Some(server(CONNECTION_LIMIT_REACHED))
            }
            Self::StreamLimit if second => Some(server(STREAM_LIMIT_REACHED)),
            Self::LostBeforeOutput if second => Some(Failure::ConnectionLost {
                before_first_event: true,
            }),
            _ => None,
        }
    }
}

#[derive(Default)]
struct Server {
    /// Responses held by the current connection.
    held: HashMap<String, Vec<Value>>,
}

/// Everything observed while driving the lane.
#[derive(Default, Debug)]
struct Log {
    sends: u64,
    delta_sends: u64,
    last_delta_items: u64,
    previous_response_not_found: u64,
    connection_limit_reached: u64,
    stream_limit_reached: u64,
    connection_lost: u64,
}

/// A lane, the server it talks to, and the history of its turns so far.
struct LaneMachine {
    lane: Lane,
    server: Server,
    log: Log,
    history: LaneHistory,
    /// Whether, from the lane's point of view, the connection still holds
    /// its last completed response: true after a completion, false after
    /// anything that evicts or replaces the connection's cache. A server-
    /// side eviction the lane cannot see leaves it true; the lane then
    /// learns of it through `previous_response_not_found`.
    cache_valid: bool,
}

#[hegel::state_machine]
impl LaneMachine {
    /// One turn: the run submits its next request, and the server meets
    /// it (and, when `second` is drawn, the resend after a recovery) with
    /// the drawn fault. The turn fails exactly when the recovery ladder
    /// gives up, and otherwise completes unless it was cancelled.
    #[rule]
    fn turn(&mut self, tc: TestCase) {
        let turn = Turn {
            new_items: tc.draw(gs::vecs(item()).min_size(1).max_size(3)),
            output_items: tc.draw(gs::vecs(item()).max_size(3)),
            response_id: format!("resp_{}", self.history.turns.len()),
        };
        let fault = tc.draw(gs::default::<Fault>());
        // A second fault on the resend, drawn only when the first recovers.
        let second = tc.draw(gs::booleans());
        self.history.turns.push(turn.clone());
        let i = self.history.turns.len() - 1;
        let full = self.history.full_body(i);
        let expected_input = full["input"].clone();
        let required = fault.required_failure(second);

        let lane = &mut self.lane;
        let mut action = Some(lane.submit(Body::from(full.clone())).unwrap());
        assert_eq!(lane.submit(Body::from(full.clone())), Err(LaneError::Busy));
        let mut attempts = 0;
        let mut failed = false;
        let mut completed = false;

        while let Some(next) = action.take() {
            match next {
                Action::Send(body) => {
                    let body = body.to_map();
                    attempts += 1;
                    assert!(
                        attempts <= 2,
                        "more than one transparent recovery"
                    );
                    self.log.sends += 1;
                    let previous = body.get("previous_response_id").cloned();
                    if let Some(Value::String(_)) = &previous {
                        self.log.delta_sends += 1;
                        self.log.last_delta_items =
                            body["input"].as_array().unwrap().len() as u64;
                        assert!(
                            self.cache_valid,
                            "delta after the cache was invalidated"
                        );
                    }
                    let this_fault = if attempts == 1 || second {
                        fault
                    } else {
                        Fault::None
                    };
                    if this_fault == Fault::Evict {
                        self.server.held.clear();
                    }
                    // The server's view of the input.
                    let rebuilt = match &previous {
                        Some(Value::String(id)) => {
                            self.server.held.get(id).map(|held| {
                                let mut items = held.clone();
                                items.extend(
                                    body["input"]
                                        .as_array()
                                        .unwrap()
                                        .iter()
                                        .cloned(),
                                );
                                items
                            })
                        }
                        _ => Some(body["input"].as_array().unwrap().clone()),
                    };
                    let Some(rebuilt) = rebuilt else {
                        assert_eq!(
                            this_fault,
                            Fault::Evict,
                            "unexpected cache miss"
                        );
                        tc.event("delta evicted");
                        self.log.previous_response_not_found += 1;
                        self.cache_valid = false;
                        action = lane.handle(Event::ServerError {
                            code: Some(PREVIOUS_RESPONSE_NOT_FOUND.into()),
                        });
                        continue;
                    };
                    assert_eq!(
                        Value::Array(rebuilt.clone()),
                        expected_input,
                        "turn {i}"
                    );
                    if attempts == 1 {
                        match this_fault {
                            Fault::ConnectionLimit => {
                                self.log.connection_limit_reached += 1
                            }
                            Fault::StreamLimit => {
                                self.log.stream_limit_reached += 1
                            }
                            Fault::LostBeforeOutput
                            | Fault::LostDuringReconnect => {
                                self.log.connection_lost += 1
                            }
                            _ => {}
                        }
                    }
                    let refuse = |code: &str| Event::ServerError {
                        code: Some(code.into()),
                    };
                    action = match this_fault {
                        Fault::None | Fault::Evict => {
                            assert_eq!(lane.handle(Event::Output), None);
                            let id = format!("{}_{attempts}", turn.response_id);
                            let mut held = rebuilt;
                            held.extend(turn.output_items.iter().cloned());
                            self.server.held.insert(id.clone(), held);
                            self.cache_valid = true;
                            completed = true;
                            lane.handle(Event::Completed {
                                response_id: id,
                                output_items: turn.output_items.clone(),
                            })
                        }
                        Fault::ConnectionLimit => {
                            lane.handle(refuse(CONNECTION_LIMIT_REACHED))
                        }
                        Fault::StreamLimit => {
                            lane.handle(refuse(STREAM_LIMIT_REACHED))
                        }
                        Fault::LostBeforeOutput
                        | Fault::LostDuringReconnect => {
                            lane.handle(Event::ConnectionLost)
                        }
                        Fault::LostAfterOutput => {
                            assert_eq!(lane.handle(Event::Output), None);
                            lane.handle(Event::ConnectionLost)
                        }
                        Fault::OtherError => {
                            lane.handle(refuse("server_error"))
                        }
                        Fault::UncodedError => {
                            lane.handle(Event::ServerError { code: None })
                        }
                        Fault::Cancel => {
                            assert_eq!(lane.handle(Event::Output), None);
                            assert_eq!(lane.handle(Event::Cancel), None);
                            // The tail of the cancelled response is ignored.
                            assert_eq!(lane.handle(Event::Output), None);
                            assert_eq!(
                                lane.handle(Event::Completed {
                                    response_id: "resp_late".into(),
                                    output_items: vec![],
                                }),
                                None
                            );
                            None
                        }
                    };
                    if this_fault != Fault::None && this_fault != Fault::Evict {
                        self.cache_valid = false;
                    }
                }
                Action::Reconnect => {
                    self.server.held.clear();
                    self.cache_valid = false;
                    if fault == Fault::LostDuringReconnect {
                        tc.event("reconnect failed");
                        action = lane.handle(Event::ConnectionLost);
                        assert!(
                            matches!(action, Some(Action::Fail(_))),
                            "{action:?}"
                        );
                    } else {
                        action = lane.handle(Event::Reconnected);
                        assert!(
                            matches!(action, Some(Action::Send(_))),
                            "{action:?}"
                        );
                    }
                }
                Action::Fail(failure) => {
                    assert!(!failed, "turn {i} failed twice");
                    failed = true;
                    // A failure is allowed only where the ladder gives up,
                    // and says why.
                    assert_eq!(
                        Some(&failure),
                        required.as_ref(),
                        "unexpected failure for {fault:?}"
                    );
                }
            }
        }
        // A failure the ladder requires is never skipped.
        assert_eq!(failed, required.is_some(), "turn {i}: {fault:?}");
        if failed {
            tc.event("turn failed");
        }
        // A turn that neither failed nor was cancelled completed.
        assert_eq!(
            completed,
            !failed && fault != Fault::Cancel,
            "turn {i}: {fault:?}"
        );
        assert!(!lane.is_busy(), "turn {i} left the lane busy");
    }

    /// The lane's counters agree with what the server saw.
    #[invariant(always_run)]
    fn stats_match_the_log(&self, _tc: TestCase) {
        let stats = self.lane.stats();
        let log = &self.log;
        assert_eq!(stats.full_requests + stats.delta_requests, log.sends);
        assert_eq!(stats.delta_requests, log.delta_sends);
        assert_eq!(stats.last_delta_items, log.last_delta_items);
        assert_eq!(
            stats.previous_response_not_found,
            log.previous_response_not_found
        );
        assert_eq!(
            stats.connection_limit_reached,
            log.connection_limit_reached
        );
        assert_eq!(stats.stream_limit_reached, log.stream_limit_reached);
        assert_eq!(stats.connection_lost, log.connection_lost);
    }
}

/// A lane driven turn by turn against the simulated server, with a fault
/// per request: every request the server rebuilds is the turn's full
/// input, deltas go out only while the connection holds their base, each
/// request recovers at most once, and a turn fails exactly when the
/// recovery ladder gives up.
#[hegel::test(test_cases = 300)]
fn lane_against_simulated_server(tc: TestCase) {
    lane_against_simulated_server_body(tc)
}

/// [`lane_against_simulated_server`] with more cases, for the nightly tier.
#[hegel::test(profile = "nightly_slow")]
#[ignore = "nightly"]
fn lane_against_simulated_server_nightly(tc: TestCase) {
    lane_against_simulated_server_body(tc)
}

fn lane_against_simulated_server_body(tc: TestCase) {
    let mut settings = Map::new();
    settings.insert("type".into(), json!("response.create"));
    settings.insert("model".into(), json!("gpt-5.5"));
    settings.insert("store".into(), json!(false));
    settings.insert("instructions".into(), json!(tc.draw(text(16))));
    let machine = LaneMachine {
        lane: Lane::new(),
        server: Server::default(),
        log: Log::default(),
        history: LaneHistory {
            settings,
            turns: Vec::new(),
        },
        cache_valid: false,
    };
    hegel::stateful::machine(machine).steps(8).run(tc);
}

/// A clean lane sends every request after the first as a delta.
#[hegel::test]
fn clean_lane_sends_deltas(tc: TestCase) {
    let history = tc.draw(lane_history());
    let mut lane = Lane::new();
    for (i, turn) in history.turns.iter().enumerate() {
        let Action::Send(body) =
            lane.submit(Body::from(history.full_body(i))).unwrap()
        else {
            panic!("expected a send");
        };
        assert_eq!(body.previous_response_id.is_some(), i > 0, "turn {i}");
        lane.handle(Event::Output);
        lane.handle(Event::Completed {
            response_id: turn.response_id.clone(),
            output_items: turn.output_items.clone(),
        });
    }
    let stats = lane.stats();
    assert_eq!(stats.full_requests, 1);
    assert_eq!(stats.delta_requests, history.turns.len() as u64 - 1);
}

fn body(items: Value) -> Body {
    let mut body = serde_json::Map::new();
    body.insert("model".into(), json!("gpt-5.5"));
    body.insert("input".into(), items);
    Body::from(body)
}

/// A completed lane whose next request would be a delta.
fn lane_with_continuation() -> Lane {
    let mut lane = Lane::new();
    lane.submit(body(json!([{"type": "message", "text": "a"}])))
        .unwrap();
    lane.handle(Event::Completed {
        response_id: "resp_1".into(),
        output_items: vec![json!({"type": "message", "text": "b"})],
    });
    lane
}

fn next_body() -> Body {
    body(json!([
        {"type": "message", "text": "a"},
        {"type": "message", "text": "b"},
        {"type": "message", "text": "c"}
    ]))
}

fn is_full(action: &Option<Action>) -> bool {
    matches!(action, Some(Action::Send(b)) if b.previous_response_id.is_none())
}

/// Each event that invalidates the connection's cache makes the next
/// request full.
#[test]
fn cache_breaking_events_force_full_resend() {
    let events = [
        Event::Cancel,
        Event::ConnectionLost,
        Event::Reconnected,
        Event::ServerError {
            code: Some("server_error".into()),
        },
    ];
    for event in events {
        let mut lane = lane_with_continuation();
        assert_eq!(
            lane.handle(event.clone()),
            None,
            "{event:?} on an idle lane"
        );
        assert!(
            is_full(&Some(lane.submit(next_body()).unwrap())),
            "{event:?}"
        );
    }
    let mut lane = lane_with_continuation();
    assert!(!is_full(&Some(lane.submit(next_body()).unwrap())));
}

/// `previous_response_not_found` on a delta resends in full on the same
/// connection, once; the second time the request fails.
#[test]
fn previous_response_not_found_resends_once() {
    let not_found = || Event::ServerError {
        code: Some(PREVIOUS_RESPONSE_NOT_FOUND.into()),
    };
    let mut lane = lane_with_continuation();
    lane.submit(next_body()).unwrap();
    let resend = lane.handle(not_found());
    assert!(is_full(&resend), "{resend:?}");
    assert_eq!(lane.stats().previous_response_not_found, 1);
    assert_eq!(
        lane.handle(not_found()),
        Some(Action::Fail(Failure::Server {
            code: Some(PREVIOUS_RESPONSE_NOT_FOUND.into())
        }))
    );
    assert!(!lane.is_busy());
}

/// `previous_response_not_found` on a full request is a server fault, not
/// a recovery step.
#[test]
fn previous_response_not_found_on_full_request_fails() {
    let mut lane = Lane::new();
    lane.submit(next_body()).unwrap();
    assert!(matches!(
        lane.handle(Event::ServerError {
            code: Some(PREVIOUS_RESPONSE_NOT_FOUND.into())
        }),
        Some(Action::Fail(_))
    ));
}

/// The connection and stream limits and a lost connection reconnect only
/// before any output; the resend after reconnecting is full and counted.
/// A second refusal after that one recovery fails the request.
#[test]
fn reconnect_only_before_output() {
    for (event, counter) in [
        (
            Event::ServerError {
                code: Some(CONNECTION_LIMIT_REACHED.into()),
            },
            "connection_limit_reached",
        ),
        (
            Event::ServerError {
                code: Some(STREAM_LIMIT_REACHED.into()),
            },
            "stream_limit_reached",
        ),
        (Event::ConnectionLost, "connection_lost"),
    ] {
        let mut lane = lane_with_continuation();
        lane.submit(next_body()).unwrap();
        assert_eq!(
            lane.handle(event.clone()),
            Some(Action::Reconnect),
            "{event:?}"
        );
        assert!(is_full(&lane.handle(Event::Reconnected)), "{event:?}");
        let stats = lane.stats();
        let count = match counter {
            "connection_limit_reached" => stats.connection_limit_reached,
            "stream_limit_reached" => stats.stream_limit_reached,
            _ => stats.connection_lost,
        };
        assert_eq!(count, 1, "{counter}");
        assert!(
            matches!(lane.handle(event.clone()), Some(Action::Fail(_))),
            "{event:?} twice"
        );

        let mut lane = lane_with_continuation();
        lane.submit(next_body()).unwrap();
        lane.handle(Event::Output);
        assert!(
            matches!(lane.handle(event.clone()), Some(Action::Fail(_))),
            "{event:?} after output"
        );
    }
}

/// A failed reconnect fails the request as a connection lost before the
/// first event.
#[test]
fn failed_reconnect_fails_request() {
    let mut lane = lane_with_continuation();
    lane.submit(next_body()).unwrap();
    assert_eq!(lane.handle(Event::ConnectionLost), Some(Action::Reconnect));
    assert_eq!(
        lane.handle(Event::ConnectionLost),
        Some(Action::Fail(Failure::ConnectionLost {
            before_first_event: true
        }))
    );
    assert!(!lane.is_busy());
}

/// Frames for a request the lane no longer has change nothing.
#[test]
fn stale_frames_are_ignored() {
    let mut lane = Lane::new();
    for event in [
        Event::Output,
        Event::Completed {
            response_id: "resp_x".into(),
            output_items: vec![],
        },
        Event::ServerError {
            code: Some(PREVIOUS_RESPONSE_NOT_FOUND.into()),
        },
        Event::ConnectionLost,
        Event::Reconnected,
        Event::Cancel,
    ] {
        assert_eq!(lane.handle(event), None);
    }
    assert!(!lane.is_busy());
    // Nothing was recorded, so the next request is full.
    assert!(is_full(&Some(lane.submit(next_body()).unwrap())));
}

/// While the lane waits to reconnect, a late completion from the old
/// connection neither completes the request nor records a continuation.
#[test]
fn late_completion_while_reconnecting_is_ignored() {
    let mut lane = lane_with_continuation();
    lane.submit(next_body()).unwrap();
    assert_eq!(lane.handle(Event::ConnectionLost), Some(Action::Reconnect));
    assert_eq!(
        lane.handle(Event::Completed {
            response_id: "resp_old".into(),
            output_items: vec![],
        }),
        None
    );
    assert!(lane.is_busy());
    assert!(is_full(&lane.handle(Event::Reconnected)));
}

/// A reconnect notice for a request that is not reconnecting changes
/// nothing: no resend, and the request stays in flight.
#[test]
fn stray_reconnected_is_ignored() {
    let mut lane = lane_with_continuation();
    lane.submit(next_body()).unwrap();
    assert_eq!(lane.handle(Event::Reconnected), None);
    assert!(lane.is_busy());
}

#[test]
fn busy_error_displays() {
    assert_eq!(
        LaneError::Busy.to_string(),
        "a request is already in flight on this lane"
    );
}
