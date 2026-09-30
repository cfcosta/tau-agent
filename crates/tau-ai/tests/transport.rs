//! The transport end to end: the real driver, pool, lanes, connection
//! tasks and WebSocket codec, against `FakeOpenAi` in a turmoil
//! simulation, with faults drawn by Hegel (`docs/reference/testing.md`,
//! "Testing the WebSocket layer").

use std::{cell::RefCell, io, rc::Rc};

use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_ai::{
    event::{Accumulator, AssistantEvent},
    message::{
        AssistantBlock,
        AssistantMessage,
        InputBlock,
        Message,
        StopReason,
        TextContent,
        ToolResultMessage,
        UserContent,
        UserMessage,
    },
    responses::{
        input::{response_items, to_input},
        request::{Settings, body},
    },
    ws::{
        io::{connection::Connector, driver::Transport},
        proto::{
            lane::{CONNECTION_LIMIT_REACHED, PREVIOUS_RESPONSE_NOT_FOUND},
            pool::{Limits, PoolStats},
        },
    },
};
use tau_testing::{
    fake_openai::{FakeOpenAi, PORT, Reply},
    openai,
};
use tokio_tungstenite::tungstenite::http;

struct SimConnector;

impl Connector for SimConnector {
    type Stream = turmoil::net::TcpStream;

    async fn connect(&self) -> io::Result<Self::Stream> {
        turmoil::net::TcpStream::connect(("api", PORT)).await
    }

    fn request(&self) -> http::Request<()> {
        http::Request::builder()
            .uri("ws://api/v1/responses")
            .header("Host", "api")
            .header("Authorization", "Bearer test")
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header(
                "Sec-WebSocket-Key",
                tokio_tungstenite::tungstenite::handshake::client::generate_key(
                ),
            )
            .body(())
            .unwrap()
    }
}

/// A fault the server applies to one turn's first attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, hegel::PrettyPrintable)]
enum Fault {
    None,
    /// The server forgets the lane's previous response.
    Evict,
    /// The connection reached its age limit.
    ConnectionLimit,
    /// The connection drops before any frame.
    DropBeforeOutput,
    /// The connection drops after output started: the turn fails, and
    /// the caller retries it.
    DropMidStream,
    /// The server goes silent before any frame: after the stall timeout
    /// the pool presumes the connection dead and resends.
    StallBeforeOutput,
    /// The server goes silent after output started: after the stall
    /// timeout the turn fails, as a drop mid-stream does.
    StallMidStream,
}

impl Fault {
    /// Whether the first attempt fails once output started.
    fn cuts_mid_stream(self) -> bool {
        matches!(self, Self::DropMidStream | Self::StallMidStream)
    }
}

/// The transport's limits in these tests: OpenAI's, with a short stall
/// timeout so a silent server is noticed within the simulation.
fn limits() -> Limits {
    Limits {
        stall_timeout: std::time::Duration::from_secs(3),
        ..Limits::default()
    }
}

/// One scripted turn: the response the model gives, and the fault on
/// the way.
#[derive(Debug, Clone)]
struct Turn {
    message: AssistantMessage,
    fault: Fault,
}

fn user(text: &str) -> Message {
    Message::User(UserMessage {
        content: UserContent::Text(text.into()),
        timestamp: 0,
    })
}

/// How OpenAI reports a completed response: `ToolUse` when it called a
/// tool, `Stop` otherwise.
fn completed_stop_reason(message: &AssistantMessage) -> StopReason {
    if message
        .content
        .iter()
        .any(|b| matches!(b, AssistantBlock::ToolCall(_)))
    {
        StopReason::ToolUse
    } else {
        StopReason::Stop
    }
}

/// The tool results the caller appends after an assistant turn.
fn results_for(message: &AssistantMessage) -> Vec<Message> {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            AssistantBlock::ToolCall(call) => {
                Some(Message::ToolResult(ToolResultMessage {
                    tool_call_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    content: vec![InputBlock::Text(TextContent {
                        text: "ok".into(),
                        text_signature: None,
                    })],
                    details: None,
                    is_error: false,
                    timestamp: 0,
                }))
            }
            _ => None,
        })
        .collect()
}

/// What the client saw: each turn's transcript, each message it got
/// back, and the pool's counters at the end.
type Outcome = (Vec<Vec<Message>>, Vec<AssistantMessage>, PoolStats);

/// A multi-turn run over the transport. The input the server rebuilds
/// for every accepted request equals the full input of that turn; the
/// caller sees one `Start` per turn and gets each message back intact;
/// recoveries never surface as errors; and `PoolStats` agrees with what
/// the server received.
#[hegel::test(test_cases = 40)]
fn run_over_transport(tc: TestCase) {
    run_over_transport_body(tc)
}

/// [`run_over_transport`] with more cases, for the nightly tier.
#[hegel::test(profile = "nightly_slow")]
#[ignore = "nightly"]
fn run_over_transport_nightly(tc: TestCase) {
    run_over_transport_body(tc)
}

fn run_over_transport_body(tc: TestCase) {
    let turn_count = tc.draw(gs::integers::<usize>().min_value(1).max_value(5));
    let turns: Vec<Turn> = (0..turn_count)
        .map(|i| {
            let mut message = tc.draw(openai::wire_assistant_message());
            // Every turn completes; failures are the faults' job.
            if matches!(
                message.stop_reason,
                StopReason::Error | StopReason::Aborted
            ) {
                message.stop_reason = completed_stop_reason(&message);
                message.error_message = None;
            }
            message.response_id = Some(format!("resp_{i}"));
            let fault = if i == 0 {
                // No previous response to lose on the first turn.
                tc.draw(gs::sampled_from(vec![
                    Fault::None,
                    Fault::DropBeforeOutput,
                ]))
            } else {
                tc.draw(gs::sampled_from(vec![
                    Fault::None,
                    Fault::Evict,
                    Fault::ConnectionLimit,
                    Fault::DropBeforeOutput,
                    Fault::DropMidStream,
                    Fault::StallBeforeOutput,
                    Fault::StallMidStream,
                ]))
            };
            Turn { message, fault }
        })
        .collect();

    let mut replies = Vec::new();
    for turn in &turns {
        let frames = openai::draw_response_frames(&tc, &turn.message);
        let respond = Reply::Respond {
            frames: frames.clone(),
            response_id: turn.message.response_id.clone().unwrap(),
            output_items: response_items(&turn.message),
        };
        match turn.fault {
            Fault::None => replies.push(respond),
            // The delta misses; the resend gets the response.
            Fault::Evict => replies.extend([Reply::Evict, respond]),
            Fault::ConnectionLimit => replies.extend([
                Reply::Error {
                    code: CONNECTION_LIMIT_REACHED.into(),
                },
                respond,
            ]),
            Fault::DropBeforeOutput => {
                replies.extend([Reply::DropAfter { frames, after: 0 }, respond])
            }
            Fault::DropMidStream => {
                // Everything but the terminal frame, so output started
                // whenever the message has any content.
                let after = frames.len() - 1;
                replies.extend([Reply::DropAfter { frames, after }, respond])
            }
            Fault::StallBeforeOutput => replies
                .extend([Reply::StallAfter { frames, after: 0 }, respond]),
            Fault::StallMidStream => {
                let after = frames.len() - 1;
                replies.extend([Reply::StallAfter { frames, after }, respond])
            }
        }
    }

    let seed = tc.draw(gs::integers::<u64>());
    let fake = FakeOpenAi::new(replies);
    let mut sim = turmoil::Builder::new()
        .rng_seed(seed)
        .simulation_duration(std::time::Duration::from_secs(120))
        .build();
    fake.install(&mut sim, "api");

    let settings = Settings {
        model: "gpt-5.5".into(),
        instructions: Some("Be brief.".into()),
        ..Settings::default()
    };
    let outcome: Rc<RefCell<Option<Outcome>>> = Rc::default();
    {
        let turns = turns.clone();
        let outcome = outcome.clone();
        sim.client("client", async move {
            let transport = Transport::start(SimConnector, limits());
            let lane = transport.open_lane().await.unwrap();
            let mut transcript = vec![user("start")];
            let mut inputs = Vec::new();
            let mut got = Vec::new();
            for (i, turn) in turns.iter().enumerate() {
                inputs.push(transcript.clone());
                if turn.fault.cuts_mid_stream() {
                    // The first attempt fails after output started: one
                    // Start, then the Error, then nothing.
                    let full = body(&settings, to_input(&transcript));
                    let mut response = lane.request(
                        full,
                        turn.message.model.clone(),
                        turn.message.timestamp,
                    );
                    let mut events = Vec::new();
                    while let Some(event) = response.next().await {
                        events.push(event);
                    }
                    let has_content = !turn.message.content.is_empty();
                    let starts = events
                        .iter()
                        .filter(|e| matches!(e, AssistantEvent::Start { .. }))
                        .count();
                    assert_eq!(starts, 1, "turn {i}: {events:?}");
                    if has_content {
                        assert!(
                            matches!(events.last(), Some(AssistantEvent::Error { .. })),
                            "turn {i}: a stream cut after output must fail: {events:?}"
                        );
                    }
                    let mut accumulator = Accumulator::new();
                    for event in events {
                        accumulator.push(event).expect("events follow the grammar");
                    }
                    if !has_content {
                        // Nothing streamed, so the lane resent it
                        // transparently and this attempt completed.
                        let message = accumulator.finish().unwrap();
                        transcript.push(Message::Assistant(message.clone()));
                        transcript.extend(results_for(&message));
                        transcript.push(user(&format!("next {i}")));
                        got.push(message);
                        continue;
                    }
                }
                let full = body(&settings, to_input(&transcript));
                let mut response = lane.request(
                    full,
                    turn.message.model.clone(),
                    turn.message.timestamp,
                );
                let mut accumulator = Accumulator::new();
                let mut starts = 0;
                while let Some(event) = response.next().await {
                    if matches!(event, AssistantEvent::Start { .. }) {
                        starts += 1;
                    }
                    assert!(
                        !matches!(event, AssistantEvent::Error { .. }),
                        "turn {i}: a recovery surfaced as {event:?}"
                    );
                    accumulator.push(event).expect("events follow the grammar");
                }
                assert_eq!(starts, 1, "turn {i}");
                let message =
                    accumulator.finish().expect("the response finished");
                transcript.push(Message::Assistant(message.clone()));
                transcript.extend(results_for(&message));
                transcript.push(user(&format!("next {i}")));
                got.push(message);
            }
            let stats = transport.stats().await.unwrap();
            *outcome.borrow_mut() = Some((inputs, got, stats));
            Ok(())
        });
    }
    sim.run().unwrap();

    let (inputs, got, stats) =
        outcome.borrow_mut().take().expect("the client finished");
    // Every turn gives back its message.
    for (turn, message) in turns.iter().zip(&got) {
        let mut expected = turn.message.clone();
        expected.usage.cost = message.usage.cost.clone();
        assert_eq!(message, &expected);
    }
    // Every accepted request rebuilds the full input of its turn.
    let received = fake.received();
    let accepted: Vec<_> = received
        .iter()
        .filter_map(|r| r.rebuilt_input.clone())
        .collect();
    let expected_inputs: Vec<Vec<Value>> =
        inputs.iter().map(|t| to_input(t)).collect();
    // Each accepted request belongs to a turn, in order, and every turn
    // has one.
    let turn_of: Vec<usize> = accepted
        .iter()
        .map(|rebuilt| {
            expected_inputs
                .iter()
                .position(|e| e == rebuilt)
                .unwrap_or_else(|| {
                    panic!(
                        "the server rebuilt an input no turn had: {rebuilt:?}"
                    )
                })
        })
        .collect();
    assert!(turn_of.windows(2).all(|w| w[0] <= w[1]), "{turn_of:?}");
    let mut covered = turn_of.clone();
    covered.dedup();
    assert_eq!(covered, (0..turns.len()).collect::<Vec<_>>());
    // Counters agree with the server's view.
    let deltas = received
        .iter()
        .filter(|r| r.body.get("previous_response_id").is_some())
        .count() as u64;
    assert_eq!(stats.lanes.delta_requests, deltas);
    assert_eq!(
        stats.lanes.full_requests + stats.lanes.delta_requests,
        received.len() as u64
    );
    let misses = received
        .iter()
        .filter(|r| r.rebuilt_input.is_none())
        .count() as u64;
    assert_eq!(stats.lanes.previous_response_not_found, misses);
    assert!(
        received
            .iter()
            .all(|r| r.body.get("store") == Some(&json!(false)))
    );
    assert!(received.iter().all(|r| r.body.get("stream_id").is_none()));
    let _ = PREVIOUS_RESPONSE_NOT_FOUND;
}

/// With no faults, every turn after the first is a delta on one
/// connection.
#[hegel::test(test_cases = 20)]
fn clean_run_is_all_deltas(tc: TestCase) {
    let turn_count = tc.draw(gs::integers::<usize>().min_value(2).max_value(5));
    let mut messages = Vec::new();
    let mut replies = Vec::new();
    for i in 0..turn_count {
        let mut message = tc.draw(openai::wire_assistant_message());
        message.stop_reason = completed_stop_reason(&message);
        message.error_message = None;
        message.response_id = Some(format!("resp_{i}"));
        replies.push(Reply::Respond {
            frames: openai::draw_response_frames(&tc, &message),
            response_id: format!("resp_{i}"),
            output_items: response_items(&message),
        });
        messages.push(message);
    }
    let fake = FakeOpenAi::new(replies);
    let mut sim = turmoil::Builder::new().build();
    fake.install(&mut sim, "api");
    let stats: Rc<RefCell<Option<PoolStats>>> = Rc::default();
    {
        let stats = stats.clone();
        sim.client("client", async move {
            let transport = Transport::start(SimConnector, Limits::default());
            let lane = transport.open_lane().await.unwrap();
            let settings = Settings {
                model: "gpt-5.5".into(),
                ..Settings::default()
            };
            let mut transcript = vec![user("start")];
            for message in &messages {
                let mut response = lane.request(
                    body(&settings, to_input(&transcript)),
                    message.model.clone(),
                    0,
                );
                let mut accumulator = Accumulator::new();
                while let Some(event) = response.next().await {
                    accumulator.push(event).unwrap();
                }
                let message = accumulator.finish().unwrap();
                transcript.push(Message::Assistant(message.clone()));
                transcript.extend(results_for(&message));
                transcript.push(user("next"));
            }
            *stats.borrow_mut() = Some(transport.stats().await.unwrap());
            Ok(())
        });
    }
    sim.run().unwrap();
    let stats = stats.borrow_mut().take().unwrap();
    assert_eq!(stats.connections_opened, 1);
    assert_eq!(stats.lanes.full_requests, 1);
    assert_eq!(stats.lanes.delta_requests, turn_count as u64 - 1);
    assert_eq!(fake.connections(), 1);
}

/// Runs `client` against a fake with `replies`, returning the fake.
fn simulate<F>(replies: Vec<Reply>, client: F) -> FakeOpenAi
where
    F: std::future::Future<Output = turmoil::Result> + 'static,
{
    let fake = FakeOpenAi::new(replies);
    let mut sim = turmoil::Builder::new().build();
    fake.install(&mut sim, "api");
    sim.client("client", client);
    sim.run().unwrap();
    fake
}

/// A response for `id`, with its frames and output items.
fn respond(tc: &TestCase, id: &str) -> (AssistantMessage, Reply) {
    let mut message = tc.draw(openai::wire_assistant_message());
    message.stop_reason = completed_stop_reason(&message);
    message.error_message = None;
    message.response_id = Some(id.into());
    let reply = Reply::Respond {
        frames: openai::draw_response_frames(tc, &message),
        response_id: id.into(),
        output_items: response_items(&message),
    };
    (message, reply)
}

async fn collect(
    mut response: tau_ai::ws::io::driver::Response,
) -> AssistantMessage {
    let mut accumulator = Accumulator::new();
    while let Some(event) = response.next().await {
        accumulator.push(event).unwrap();
    }
    accumulator.finish().unwrap()
}

/// Two runs get a connection each, no request names a lane, and each
/// run gets the response its own connection carried.
#[hegel::test(test_cases = 20)]
fn runs_get_a_connection_each(tc: TestCase) {
    let (first, first_reply) = respond(&tc, "resp_a");
    let (second, second_reply) = respond(&tc, "resp_b");
    let got: Rc<RefCell<Vec<AssistantMessage>>> = Rc::default();
    let seen = got.clone();
    let labels = [
        (first.model.clone(), first.timestamp),
        (second.model.clone(), second.timestamp),
    ];
    let fake = simulate(vec![first_reply, second_reply], async move {
        let transport = Transport::start(SimConnector, Limits::default());
        let settings = Settings {
            model: "gpt-5.5".into(),
            ..Settings::default()
        };
        let input = to_input(&[user("hi")]);
        // Open lanes keep their connections, so the next run cannot take
        // one. One at a time, so the fake's reply order is the lanes'.
        let mut open = Vec::new();
        for (model, timestamp) in labels {
            let lane = transport.open_lane().await.unwrap();
            let response =
                lane.request(body(&settings, input.clone()), model, timestamp);
            let message = collect(response).await;
            seen.borrow_mut().push(message);
            open.push(lane);
        }
        Ok(())
    });
    assert_eq!(fake.connections(), 2);
    let received = fake.received();
    assert_eq!(received.len(), 2);
    assert!(received.iter().all(|r| r.body.get("stream_id").is_none()));
    assert_ne!(received[0].connection, received[1].connection);
    let got = got.borrow();
    for (have, want) in got.iter().zip([&first, &second]) {
        let mut want = want.clone();
        want.usage.cost = have.usage.cost.clone();
        assert_eq!(have, &want);
    }
}

/// Dropping an unfinished response cancels its request, so the lane can
/// take the next one; dropping a finished response later does not
/// cancel the request that followed it.
#[hegel::test(test_cases = 10)]
fn dropping_responses(tc: TestCase) {
    let (_, cut) = respond(&tc, "resp_1");
    let Reply::Respond { frames, .. } = cut else {
        unreachable!()
    };
    let (second, second_reply) = respond(&tc, "resp_2");
    let (third, third_reply) = respond(&tc, "resp_3");
    let got: Rc<RefCell<Vec<AssistantMessage>>> = Rc::default();
    let seen = got.clone();
    let labels = [
        (second.model.clone(), second.timestamp),
        (third.model.clone(), third.timestamp),
    ];
    simulate(
        vec![
            // The client drops this response after its first event; the
            // server still streams the rest, as a real one would.
            Reply::Respond {
                response_id: "resp_1".into(),
                output_items: vec![],
                frames,
            },
            second_reply,
            third_reply,
        ],
        async move {
            let transport = Transport::start(SimConnector, Limits::default());
            let lane = transport.open_lane().await.unwrap();
            let settings = Settings {
                model: "gpt-5.5".into(),
                ..Settings::default()
            };
            let input = to_input(&[user("hi")]);
            let mut first = lane.request(
                body(&settings, input.clone()),
                "gpt-5.5".into(),
                0,
            );
            assert!(matches!(
                first.next().await,
                Some(AssistantEvent::Start { .. })
            ));
            drop(first);
            // Without the cancel, the lane would refuse this request.
            let second_response = lane.request(
                body(&settings, input.clone()),
                labels[0].0.clone(),
                labels[0].1,
            );
            let finished = collect(second_response).await;
            // A finished response dropped after the next request started
            // must not cancel it.
            let mut kept = lane.request(
                body(&settings, input.clone()),
                labels[1].0.clone(),
                labels[1].1,
            );
            let first_event = kept.next().await;
            assert!(matches!(first_event, Some(AssistantEvent::Start { .. })));
            let mut accumulator = Accumulator::new();
            accumulator.push(first_event.unwrap()).unwrap();
            while let Some(event) = kept.next().await {
                accumulator.push(event).unwrap();
            }
            seen.borrow_mut()
                .extend([finished, accumulator.finish().unwrap()]);
            Ok(())
        },
    );
    let got = got.borrow();
    for (have, want) in got.iter().zip([&second, &third]) {
        let mut want = want.clone();
        want.usage.cost = have.usage.cost.clone();
        assert_eq!(have, &want);
    }
}

/// Dropping a lane handle closes the lane, freeing its connection for
/// the next one.
#[test]
fn dropping_a_lane_frees_its_connection() {
    let stats: Rc<RefCell<Option<PoolStats>>> = Rc::default();
    let seen = stats.clone();
    simulate(vec![], async move {
        let transport = Transport::start(SimConnector, Limits::default());
        let first = transport.open_lane().await.unwrap();
        drop(first);
        let _second = transport.open_lane().await.unwrap();
        *seen.borrow_mut() = Some(transport.stats().await.unwrap());
        Ok(())
    });
    assert_eq!(stats.borrow().as_ref().unwrap().connections_opened, 1);
}

#[test]
fn stopped_displays() {
    assert_eq!(
        tau_ai::ws::io::driver::Stopped.to_string(),
        "the WebSocket transport stopped"
    );
}

/// A request the server rejects outright fails with one `Start` and one
/// `Error`, even though no response frame ever arrived.
#[test]
fn server_error_fails_with_start_and_error() {
    let events: Rc<RefCell<Vec<AssistantEvent>>> = Rc::default();
    let seen = events.clone();
    simulate(
        vec![Reply::Error {
            code: "server_error".into(),
        }],
        async move {
            let transport = Transport::start(SimConnector, Limits::default());
            let lane = transport.open_lane().await.unwrap();
            let settings = Settings {
                model: "gpt-5.5".into(),
                ..Settings::default()
            };
            let mut response = lane.request(
                body(&settings, to_input(&[user("hi")])),
                "gpt-5.5".into(),
                7,
            );
            while let Some(event) = response.next().await {
                seen.borrow_mut().push(event);
            }
            Ok(())
        },
    );
    let events = events.borrow();
    assert!(
        matches!(&events[..], [AssistantEvent::Start { timestamp: 7, .. }, AssistantEvent::Error { message, .. }] if message.contains("server_error")),
        "{events:?}"
    );
}

/// A cancelled response whose connection drops before its terminal frame
/// leaves nothing to skip: the lane's next request, on a new connection,
/// gets its response.
#[hegel::test(test_cases = 10)]
fn lost_connection_ends_the_skip(tc: TestCase) {
    let (_, cut) = respond(&tc, "resp_1");
    let Reply::Respond { frames, .. } = cut else {
        unreachable!()
    };
    let (next, next_reply) = respond(&tc, "resp_2");
    let label = (next.model.clone(), next.timestamp);
    let got: Rc<RefCell<Option<AssistantMessage>>> = Rc::default();
    let seen = got.clone();
    let fake = simulate(
        vec![
            // Streams all but the terminal frame, then drops the socket.
            Reply::DropAfter {
                after: frames.len() - 1,
                frames,
            },
            next_reply,
        ],
        async move {
            let transport = Transport::start(SimConnector, Limits::default());
            let lane = transport.open_lane().await.unwrap();
            let settings = Settings {
                model: "gpt-5.5".into(),
                ..Settings::default()
            };
            let input = to_input(&[user("hi")]);
            let mut first = lane.request(
                body(&settings, input.clone()),
                "gpt-5.5".into(),
                0,
            );
            assert!(matches!(
                first.next().await,
                Some(AssistantEvent::Start { .. })
            ));
            drop(first);
            // Let the drop and the lost connection play out.
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            let second = lane.request(body(&settings, input), label.0, label.1);
            *seen.borrow_mut() = Some(collect(second).await);
            Ok(())
        },
    );
    assert_eq!(fake.connections(), 2);
    let have = got.borrow_mut().take().unwrap();
    let mut want = next.clone();
    want.usage.cost = have.usage.cost.clone();
    assert_eq!(have, want);
}

/// Many runs at once: a drawn number of lanes each send one request,
/// and the server answers each after a drawn delay, so responses
/// overlap. Each run gets a connection of its own, the server sees no
/// broken limit, and every request completes without an error.
#[hegel::test(test_cases = 15)]
fn concurrent_runs_get_a_connection_each(tc: TestCase) {
    let lanes = tc.draw(gs::integers::<usize>().min_value(1).max_value(12));
    let mut replies = Vec::new();
    for i in 0..lanes {
        let (_, reply) = respond(&tc, &format!("resp_{i}"));
        let delay = tc.draw(gs::integers::<u64>().max_value(500));
        replies.push(Reply::Delay(
            std::time::Duration::from_millis(delay),
            Box::new(reply),
        ));
    }
    let stats: Rc<RefCell<Option<PoolStats>>> = Rc::default();
    let seen = stats.clone();
    let fake = simulate(replies, async move {
        let transport = Transport::start(SimConnector, Limits::default());
        let mut handles = Vec::new();
        for _ in 0..lanes {
            handles.push(transport.open_lane().await.unwrap());
        }
        let settings = Settings {
            model: "gpt-5.5".into(),
            ..Settings::default()
        };
        let requests = handles.iter().map(|lane| {
            let response = lane.request(
                body(&settings, to_input(&[user("hi")])),
                "gpt-5.5".into(),
                0,
            );
            async move {
                let mut response = response;
                let mut events = Vec::new();
                while let Some(event) = response.next().await {
                    events.push(event);
                }
                events
            }
        });
        for events in futures_util::future::join_all(requests).await {
            assert!(
                matches!(events.last(), Some(AssistantEvent::Done { .. })),
                "{events:?}"
            );
        }
        *seen.borrow_mut() = Some(transport.stats().await.unwrap());
        Ok(())
    });
    assert_eq!(fake.violations(), Vec::<String>::new());
    let stats = stats.borrow_mut().take().unwrap();
    assert_eq!(stats.connections_opened as usize, lanes);
    assert_eq!(fake.connections() as usize, lanes);
}

/// Runs one after another reuse one connection: each run's lane closes
/// before the next opens, and the next takes the connection it left.
#[hegel::test(test_cases = 5)]
fn runs_one_after_another_reuse_a_connection(tc: TestCase) {
    let runs = tc.draw(gs::integers::<usize>().min_value(2).max_value(6));
    let mut replies = Vec::new();
    for i in 0..runs {
        replies.push(respond(&tc, &format!("resp_{i}")).1);
    }
    let stats: Rc<RefCell<Option<PoolStats>>> = Rc::default();
    let seen = stats.clone();
    let fake = simulate(replies, async move {
        let transport = Transport::start(SimConnector, Limits::default());
        let settings = Settings {
            model: "gpt-5.5".into(),
            ..Settings::default()
        };
        for _ in 0..runs {
            let lane = transport.open_lane().await.unwrap();
            let mut response = lane.request(
                body(&settings, to_input(&[user("hi")])),
                "gpt-5.5".into(),
                0,
            );
            let mut last = None;
            while let Some(event) = response.next().await {
                assert!(
                    !matches!(event, AssistantEvent::Error { .. }),
                    "{event:?}"
                );
                last = Some(event);
            }
            assert!(matches!(last, Some(AssistantEvent::Done { .. })));
            drop(lane);
        }
        *seen.borrow_mut() = Some(transport.stats().await.unwrap());
        Ok(())
    });
    let stats = stats.borrow_mut().take().unwrap();
    assert_eq!(stats.connections_opened, 1);
    assert_eq!(stats.connections_reused as usize, runs - 1);
    assert_eq!(fake.connections(), 1);
}

/// A run cancelled mid-response leaves its tail streaming on the
/// connection. The next run, placed on that connection, skips the tail
/// and gets its own response.
#[hegel::test(test_cases = 10)]
fn the_next_run_skips_a_cancelled_tail(tc: TestCase) {
    let (_, cut) = respond(&tc, "resp_1");
    let (next, next_reply) = respond(&tc, "resp_2");
    let label = (next.model.clone(), next.timestamp);
    let got: Rc<RefCell<Option<AssistantMessage>>> = Rc::default();
    let seen = got.clone();
    let fake = simulate(vec![cut, next_reply], async move {
        let transport = Transport::start(SimConnector, Limits::default());
        let settings = Settings {
            model: "gpt-5.5".into(),
            ..Settings::default()
        };
        let input = to_input(&[user("hi")]);
        let first = transport.open_lane().await.unwrap();
        let mut response =
            first.request(body(&settings, input.clone()), "gpt-5.5".into(), 0);
        assert!(matches!(
            response.next().await,
            Some(AssistantEvent::Start { .. })
        ));
        drop(response);
        drop(first);
        let second = transport.open_lane().await.unwrap();
        let response = second.request(body(&settings, input), label.0, label.1);
        *seen.borrow_mut() = Some(collect(response).await);
        Ok(())
    });
    assert_eq!(fake.connections(), 1);
    let have = got.borrow_mut().take().unwrap();
    let mut want = next.clone();
    want.usage.cost = have.usage.cost.clone();
    assert_eq!(have, want);
}
