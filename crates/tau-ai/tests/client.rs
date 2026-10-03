//! The OpenAI client (`tau_ai::client`) over `FakeOpenAi`. The plan
//! connector's own upgrade is in `chatgpt_transport.rs`.

use std::{cell::RefCell, io, rc::Rc};

use hegel::{
    TestCase,
    generators::{self as gs, Generator as _},
};
use serde_json::json;
use tau_ai::{
    client::OpenAi,
    cost,
    event::{AssistantEvent, DoneReason},
    llm::Llm,
    message::{
        AssistantBlock,
        AssistantMessage,
        Message,
        StopReason,
        TextContent,
        Usage,
        UserContent,
        UserMessage,
    },
    model::{ServiceTier, find},
    responses::{
        input::response_items,
        request::{ReasoningEffort, Settings},
    },
    ws::{io::connection::Connector, proto::pool::Limits},
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
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        "ws://api/v1/responses".into_client_request().unwrap()
    }
}

fn hello() -> Vec<Message> {
    vec![Message::User(UserMessage {
        content: UserContent::Text("hello".into()),
        timestamp: 0,
    })]
}

/// Runs one response for `settings` against a fake that answers with
/// `message`; returns the terminal event and the request the fake saw.
fn one_response(
    tc: &TestCase,
    settings: Settings,
    usage: Usage,
) -> (AssistantEvent, serde_json::Value) {
    let mut message = tc.draw(openai::wire_assistant_message());
    message.stop_reason = StopReason::Stop;
    message.error_message = None;
    message
        .content
        .retain(|b| !matches!(b, tau_ai::message::AssistantBlock::ToolCall(_)));
    message.response_id = Some("resp_1".into());
    message.usage = usage;
    let fake = FakeOpenAi::new(vec![Reply::Respond {
        frames: openai::draw_response_frames(tc, &message),
        response_id: "resp_1".into(),
        output_items: response_items(&message),
    }]);
    let mut sim = turmoil::Builder::new().build();
    fake.install(&mut sim, "api");
    let last: Rc<RefCell<Option<AssistantEvent>>> = Rc::default();
    let seen = last.clone();
    let model_name = settings.model.clone();
    sim.client("client", async move {
        let client = OpenAi::with_connector(SimConnector, Limits::default());
        assert!(!client.supports_output_token_limit());
        let mut session = client.session(settings).await.unwrap();
        // The session keeps the caller's settings, with whether the model
        // reasons taken from the model table.
        assert_eq!(session.settings().model, model_name);
        if let Some(model) = find(&session.settings().model) {
            assert_eq!(session.settings().reasoning_model, model.reasoning);
        }
        let mut response = session.respond(&hello(), 0);
        while let Some(event) = response.next().await {
            *seen.borrow_mut() = Some(event);
        }
        let stats = client.stats().await.unwrap();
        assert_eq!(stats.lanes.full_requests, 1);
        assert_eq!(stats.connections_opened, 1);
        Ok(())
    });
    sim.run().unwrap();
    let event = last.borrow_mut().take().expect("a terminal event");
    (event, fake.received()[0].body.clone())
}

/// A completed response's token counts pass through unchanged, and its
/// cost is the model table's price for the drawn counts at the session's
/// service tier.
#[hegel::test(test_cases = 20)]
fn done_usage_gets_its_cost(tc: TestCase) {
    let model = tc.draw(gs::sampled_from(vec![
        "gpt-5.5",
        "gpt-5.4-mini",
        "gpt-6-sol",
    ]));
    let tier = tc.draw(gs::sampled_from(vec![
        None,
        Some("flex"),
        Some("priority"),
        Some("fast"),
    ]));
    let tokens = || gs::integers::<u64>().max_value(1_000_000);
    let usage = Usage {
        input: tc.draw(tokens()),
        output: tc.draw(tokens()),
        cache_read: tc.draw(tokens()),
        cache_write: tc.draw(tokens()),
        reasoning: tc.draw(gs::optional(tokens())),
        total_tokens: tc.draw(tokens()),
        cost: Default::default(),
    };
    let settings = Settings {
        model: model.into(),
        service_tier: tier.map(str::to_owned),
        ..Settings::default()
    };
    let (event, _) = one_response(&tc, settings, usage.clone());
    let AssistantEvent::Done {
        usage: got, reason, ..
    } = event
    else {
        panic!("expected Done, got {event:?}");
    };
    assert_eq!(reason, DoneReason::Stop);
    let tier = match tier {
        Some("flex") => ServiceTier::Flex,
        Some(_) => ServiceTier::PriorityOrFast,
        None => ServiceTier::Default,
    };
    let tokens = |u: &Usage| {
        (
            u.input,
            u.output,
            u.cache_read,
            u.cache_write,
            u.reasoning,
            u.total_tokens,
        )
    };
    assert_eq!(tokens(&got), tokens(&usage));
    // Priced from the drawn counts, not from what came back.
    let expected = cost::cost(find(model).unwrap(), &usage, tier);
    assert_eq!(got.cost, expected);
    assert!(
        got.cost.total > 0.0
            || usage.input + usage.output + usage.cache_read == 0
    );
}

/// Property inventory: the current ChatGPT route strips the output ceiling
/// whether configured or absent. The fake server's captured request is the
/// oracle. Generate valid positive u32 limits so shrinking tends toward a
/// small counterexample; `hegel.toml` sets local and CI case counts.
#[hegel::test]
fn chatgpt_route_omits_output_ceiling(tc: TestCase) {
    let limit = tc.draw(gs::integers::<u32>().min_value(1).max_value(8192));
    let usage = Usage::default();
    let settings = Settings {
        model: "gpt-5.5".into(),
        max_output_tokens: Some(limit),
        ..Settings::default()
    };
    let (_, request) = one_response(&tc, settings, usage.clone());
    assert!(request.get("max_output_tokens").is_none());

    let settings = Settings {
        model: "gpt-5.5".into(),
        ..Settings::default()
    };
    let (_, request) = one_response(&tc, settings, usage);
    assert!(request.get("max_output_tokens").is_none());
}

/// A warm-up sends the session's settings with no input and
/// `generate: false`; the first real turn then continues from it, as a
/// delta carrying the whole transcript, and the server rebuilds that
/// transcript. The warm-up's usage comes back with its cost.
#[hegel::test(test_cases = 10)]
fn the_first_turn_continues_from_the_warm_up(tc: TestCase) {
    let mut message = tc.draw(openai::wire_assistant_message());
    message.stop_reason = StopReason::Stop;
    message.error_message = None;
    message
        .content
        .retain(|b| !matches!(b, tau_ai::message::AssistantBlock::ToolCall(_)));
    message.response_id = Some("resp_1".into());
    let mut warm = tc.draw(openai::wire_assistant_message());
    warm.content.clear();
    warm.stop_reason = StopReason::Stop;
    warm.error_message = None;
    warm.response_id = Some("resp_warm".into());
    warm.usage = Usage {
        input: 1_000,
        ..Usage::default()
    };
    let fake = FakeOpenAi::new(vec![
        Reply::Respond {
            frames: openai::draw_response_frames(&tc, &warm),
            response_id: "resp_warm".into(),
            output_items: Vec::new(),
        },
        Reply::Respond {
            frames: openai::draw_response_frames(&tc, &message),
            response_id: "resp_1".into(),
            output_items: response_items(&message),
        },
    ]);
    let mut sim = turmoil::Builder::new().build();
    fake.install(&mut sim, "api");
    sim.client("client", async move {
        let client = OpenAi::with_connector(SimConnector, Limits::default());
        let settings = Settings {
            model: "gpt-5.5".into(),
            instructions: Some("Be brief.".into()),
            ..Settings::default()
        };
        let mut session = client.session(settings).await.unwrap();
        let usage = tau_ai::llm::LlmSession::warm_up(&mut session, 0)
            .await
            .unwrap();
        assert_eq!(usage.input, 1_000);
        assert!(usage.cost.total > 0.0);
        let mut response = session.respond(&hello(), 0);
        while response.next().await.is_some() {}
        let stats = client.stats().await.unwrap();
        assert_eq!(stats.lanes.full_requests, 1);
        assert_eq!(stats.lanes.delta_requests, 1);
        Ok(())
    });
    sim.run().unwrap();
    let received = fake.received();
    assert_eq!(received.len(), 2);
    let warm_up = &received[0].body;
    assert_eq!(warm_up["generate"], json!(false));
    assert_eq!(warm_up["input"], json!([]));
    assert_eq!(warm_up["instructions"], json!("Be brief."));
    let turn = &received[1].body;
    assert_eq!(turn["previous_response_id"], json!("resp_warm"));
    assert!(turn.get("generate").is_none());
    assert_eq!(
        received[1].rebuilt_input.as_ref().unwrap(),
        turn["input"].as_array().unwrap()
    );
}

/// A warm-up the server rejects is an error, not a hang.
#[test]
fn a_failed_warm_up_is_an_error() {
    let fake = FakeOpenAi::new(vec![Reply::Error {
        code: "server_error".into(),
    }]);
    let mut sim = turmoil::Builder::new().build();
    fake.install(&mut sim, "api");
    sim.client("client", async move {
        let client = OpenAi::with_connector(SimConnector, Limits::default());
        let mut session = client
            .session(Settings {
                model: "gpt-5.5".into(),
                ..Settings::default()
            })
            .await
            .unwrap();
        let error = tau_ai::llm::LlmSession::warm_up(&mut session, 0)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("server_error"), "{error}");
        Ok(())
    });
    sim.run().unwrap();
}

/// Through the `Llm` trait, `OpenAi` opens sessions with the settings
/// the provider will send, and streams the same response as a session
/// used directly.
#[hegel::test(test_cases = 10)]
fn llm_trait_streams_the_response(tc: TestCase) {
    use futures_util::StreamExt;
    use tau_ai::llm::Llm;

    let mut message = tc.draw(openai::wire_assistant_message());
    message.stop_reason = StopReason::Stop;
    message.error_message = None;
    message
        .content
        .retain(|b| !matches!(b, tau_ai::message::AssistantBlock::ToolCall(_)));
    message.response_id = Some("resp_1".into());
    let fake = FakeOpenAi::new(vec![Reply::Respond {
        frames: openai::draw_response_frames(&tc, &message),
        response_id: "resp_1".into(),
        output_items: response_items(&message),
    }]);
    let mut sim = turmoil::Builder::new().build();
    fake.install(&mut sim, "api");
    let got: Rc<RefCell<Option<tau_ai::message::AssistantMessage>>> =
        Rc::default();
    let seen = got.clone();
    let label = (message.model.clone(), message.timestamp);
    let model_name = message.model.clone();
    sim.client("client", async move {
        let llm: Box<dyn Llm> =
            Box::new(OpenAi::with_connector(SimConnector, Limits::default()));
        let mut session = llm
            .open(Settings {
                model: label.0,
                ..Settings::default()
            })
            .await
            .unwrap();
        assert_eq!(session.settings().model, model_name);
        let reasons =
            find(&session.settings().model).is_some_and(|m| m.reasoning);
        assert_eq!(session.settings().reasoning_model, reasons);
        let mut events = session.respond(&hello(), label.1);
        let mut accumulator = tau_ai::event::Accumulator::new();
        while let Some(event) = events.next().await {
            accumulator.push(event).unwrap();
        }
        *seen.borrow_mut() = Some(accumulator.finish().unwrap());
        Ok(())
    });
    sim.run().unwrap();
    let have = got.borrow_mut().take().unwrap();
    let mut want = message;
    want.usage.cost = have.usage.cost.clone();
    assert_eq!(have, want);
}

#[test]
fn llm_error_displays_its_message() {
    let error = tau_ai::llm::LlmError {
        message: "transport stopped".into(),
    };
    assert_eq!(error.to_string(), "transport stopped");
}

/// Plays a bounded history on one simulated lane. Every scripted answer has
/// one fixed text item, so the next transcript contains exactly the server's
/// recorded output item before the next user item.
fn send_reasoning_history(
    settings: Settings,
    transitions: Vec<Option<ReasoningEffort>>,
) -> (
    Vec<tau_testing::fake_openai::Received>,
    (u64, u64),
    Vec<Usage>,
) {
    let turns = transitions.len() + 1;
    let messages: Vec<_> = (0..turns)
        .map(|turn| AssistantMessage {
            content: vec![AssistantBlock::Text(TextContent {
                text: format!("answer {turn}"),
                text_signature: Some(format!(
                    "{{\"v\":1,\"id\":\"msg_{turn}\"}}"
                )),
            })],
            model: settings.model.clone(),
            response_id: Some(format!("resp_{turn}")),
            usage: Usage {
                input: 1000,
                output: 1000,
                ..Usage::default()
            },
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 0,
        })
        .collect();
    let replies = messages
        .iter()
        .enumerate()
        .map(|(turn, message)| Reply::Respond {
            frames: vec![
                json!({"type": "response.created", "response": {"id": format!("resp_{turn}")}}),
                json!({"type": "response.output_item.added", "output_index": 0,
                    "item": {"type": "message", "id": format!("msg_{turn}"),
                        "role": "assistant", "status": "in_progress", "content": []}}),
                json!({"type": "response.output_text.delta", "output_index": 0,
                    "delta": format!("answer {turn}")}),
                json!({"type": "response.output_item.done", "output_index": 0,
                    "item": {"type": "message", "id": format!("msg_{turn}"),
                        "role": "assistant", "status": "completed",
                        "content": [{"type": "output_text", "text": format!("answer {turn}"),
                            "annotations": []}]}}),
                json!({"type": "response.completed", "response": {
                    "id": format!("resp_{turn}"),
                    "status": "completed",
                    "usage": {"input_tokens": 1000, "output_tokens": 1000,
                        "total_tokens": 2000,
                        "output_tokens_details": {"reasoning_tokens": 0}}
                }}),
            ],
            response_id: format!("resp_{turn}"),
            output_items: response_items(message),
        })
        .collect();
    let fake = FakeOpenAi::new(replies);
    let results = Rc::new(RefCell::new(((0, 0), Vec::new())));
    let seen = results.clone();
    let mut sim = turmoil::Builder::new().build();
    fake.install(&mut sim, "api");
    sim.client("client", async move {
        let client = OpenAi::with_connector(SimConnector, Limits::default());
        let mut session = client.session(settings).await.unwrap();
        let mut transcript = Vec::new();
        for (turn, message) in messages.into_iter().enumerate() {
            if turn > 0 {
                tau_ai::llm::LlmSession::set_reasoning(
                    &mut session,
                    transitions[turn - 1],
                );
            }
            transcript.push(Message::User(UserMessage {
                content: UserContent::Text(format!("question {turn}")),
                timestamp: 0,
            }));
            let mut response = session.respond(&transcript, 0);
            let mut done = None;
            while let Some(event) = response.next().await {
                if let AssistantEvent::Done { usage, .. } = event {
                    done = Some(usage);
                }
            }
            seen.borrow_mut()
                .1
                .push(done.expect("completed scripted turn"));
            transcript.push(Message::Assistant(message));
        }
        let stats = client.stats().await.unwrap();
        seen.borrow_mut().0 =
            (stats.lanes.full_requests, stats.lanes.delta_requests);
        Ok(())
    });
    sim.run().unwrap();
    let results = results.borrow();
    assert!(fake.violations().is_empty());
    (fake.received(), results.0, results.1.clone())
}

/// Property inventory: session model-class overrides and effort changes
/// determine literal wire fields; equal wire fields continue with only the
/// next user item, changed wire fields resend the whole ordinary transcript.
/// Oracle: fixed model classes and independently built JSON input/fields.
/// Generator: an initial optional effort and 3-5 transitions, with changed,
/// repeated, and unset steps built in; shrinking keeps those steps and moves
/// optional suffixes and efforts toward simpler histories.
#[hegel::test(test_cases = 24)]
fn session_reasoning_history_follows_effective_wire_fields(tc: TestCase) {
    let initial = tc.draw(
        gs::sampled_from(vec![
            None,
            Some(ReasoningEffort::Low),
            Some(ReasoningEffort::High),
        ])
        .print_as_debug(),
    );
    let changed = if initial == Some(ReasoningEffort::High) {
        ReasoningEffort::Low
    } else {
        ReasoningEffort::High
    };
    let mut transitions = vec![Some(changed), Some(changed), None];
    let suffix = tc.draw(
        gs::vecs(gs::sampled_from(vec![
            None,
            Some(ReasoningEffort::Low),
            Some(ReasoningEffort::High),
        ]))
        .max_size(2)
        .print_as_debug(),
    );
    transitions.extend(suffix);

    for (model, caller_reasoning, effective_reasoning) in [
        ("gpt-6-sol", false, true),
        ("gpt-4o", true, false),
        ("not-a-model", false, false),
        ("not-a-model", true, true),
    ] {
        let settings = Settings {
            model: model.into(),
            reasoning_model: caller_reasoning,
            reasoning: initial,
            ..Settings::default()
        };
        let (received, counts, usages) =
            send_reasoning_history(settings, transitions.clone());
        assert_eq!(received.len(), transitions.len() + 1, "{model}");
        assert_eq!(usages.len(), received.len(), "{model}");
        let mut previous_wire_effort = None;
        let mut expected_full = 0;
        let mut expected_delta = 0;
        let mut expected_input = Vec::new();
        for (turn, request) in received.iter().enumerate() {
            let effort = if turn == 0 {
                initial
            } else {
                transitions[turn - 1]
            };
            let wire_effort = effective_reasoning.then_some(effort).flatten();
            let expected_reasoning = wire_effort.map(|effort| {
                let label = match effort {
                    ReasoningEffort::Low => "low",
                    ReasoningEffort::High => "high",
                    _ => unreachable!("history uses only low and high"),
                };
                json!({"effort": label, "summary": "auto"})
            });
            let expected_include = effective_reasoning
                .then(|| json!(["reasoning.encrypted_content"]));
            assert_eq!(
                request.body.get("reasoning"),
                expected_reasoning.as_ref(),
                "{model} turn {turn}"
            );
            assert_eq!(
                request.body.get("include"),
                expected_include.as_ref(),
                "{model} turn {turn}"
            );
            assert_eq!(request.body["model"], json!(model));
            expected_input.push(json!({"role": "user", "content": [
                {"type": "input_text", "text": format!("question {turn}")}
            ]}));
            let full = turn == 0 || wire_effort != previous_wire_effort;
            if full {
                expected_full += 1;
                assert!(request.body.get("previous_response_id").is_none());
                assert_eq!(request.body["input"], json!(expected_input));
            } else {
                expected_delta += 1;
                assert_eq!(
                    request.body["previous_response_id"],
                    json!(format!("resp_{}", turn - 1))
                );
                assert_eq!(
                    request.body["input"],
                    json!([expected_input.last().unwrap()])
                );
            }
            assert_eq!(
                request.rebuilt_input.as_ref(),
                Some(&expected_input),
                "{model} turn {turn}"
            );
            if model == "not-a-model" {
                assert_eq!(usages[turn].cost.total, 0.0);
            }
            expected_input.push(json!({
                "type": "message", "role": "assistant", "id": format!("msg_{turn}"),
                "status": "completed", "content": [{"type": "output_text",
                    "text": format!("answer {turn}"), "annotations": []}]
            }));
            previous_wire_effort = wire_effort;
        }
        assert_eq!(counts, (expected_full, expected_delta), "{model}");
    }
}
