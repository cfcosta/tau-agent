//! The OpenAI client (`tau_ai::client`) over `FakeOpenAi`, and the TLS
//! connector's upgrade request.

use std::{cell::RefCell, io, rc::Rc};

use hegel::{TestCase, generators as gs};
use serde_json::json;
use tau_ai::{
    client::{API_KEY_VAR, MissingApiKey, OpenAi},
    cost,
    event::{AssistantEvent, DoneReason},
    message::{Message, StopReason, Usage, UserContent, UserMessage},
    model::{ServiceTier, find},
    responses::{
        input::response_items,
        request::{ReasoningEffort, Settings},
    },
    ws::{
        io::{
            connection::Connector,
            tls::{OPENAI_URL, OpenAiConnector},
        },
        proto::pool::Limits,
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

/// A model missing from the table gets no cost, and keeps the caller's
/// reasoning setting; a known model's comes from the table.
#[hegel::test(test_cases = 10)]
fn reasoning_follows_the_table(tc: TestCase) {
    let usage = Usage {
        input: 1000,
        output: 1000,
        ..Usage::default()
    };
    let unknown = Settings {
        model: "not-a-model".into(),
        reasoning_model: false,
        ..Settings::default()
    };
    let (event, request) = one_response(&tc, unknown, usage.clone());
    let AssistantEvent::Done { usage: got, .. } = event else {
        panic!()
    };
    assert_eq!(got.cost.total, 0.0);
    assert!(request.get("include").is_none());

    // gpt-5.5 reasons, whatever the caller said.
    let known = Settings {
        model: "gpt-5.5".into(),
        reasoning_model: false,
        ..Settings::default()
    };
    let (_, request) = one_response(&tc, known, usage);
    assert_eq!(request["include"], json!(["reasoning.encrypted_content"]));
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

/// The TLS connector's upgrade request goes to OpenAI with a bearer
/// token that is marked sensitive and never printed.
#[test]
fn tls_request_carries_the_key() {
    let connector = OpenAiConnector::new("sk-test-123");
    let request = connector.request();
    assert_eq!(request.uri().to_string(), OPENAI_URL);
    let auth = &request.headers()["authorization"];
    assert_eq!(auth.to_str().unwrap(), "Bearer sk-test-123");
    assert!(auth.is_sensitive());
    assert_eq!(request.headers()["upgrade"], "websocket");
    let shown = format!("{connector:?}");
    assert!(!shown.contains("sk-test-123"), "{shown}");
    assert!(shown.contains("<redacted>"));
}

/// Without `OPENAI_API_KEY`, `from_env` says so.
#[test]
fn from_env_without_key() {
    // nextest runs each test in its own process, so changing the
    // environment here cannot race with another test.
    unsafe { std::env::remove_var(API_KEY_VAR) };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let _guard = runtime.enter();
    let error = OpenAi::from_env().unwrap_err();
    assert_eq!(error, MissingApiKey);
    assert_eq!(error.to_string(), "OPENAI_API_KEY is not set");
    // An empty key counts as missing; any other value is taken.
    unsafe { std::env::set_var(API_KEY_VAR, "") };
    assert_eq!(OpenAi::from_env().unwrap_err(), MissingApiKey);
    unsafe { std::env::set_var(API_KEY_VAR, "sk-test") };
    assert!(OpenAi::from_env().is_ok());
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

/// Two turns on `model`: the first at the model's default effort, the
/// second at `high`, set on the session between them. Returns the
/// request bodies the fake saw, and the lane's full and delta counts.
fn two_turns_changing_effort(
    tc: &TestCase,
    model: &str,
) -> (Vec<serde_json::Value>, u64, u64) {
    let mut first = tc.draw(openai::wire_assistant_message());
    first.stop_reason = StopReason::Stop;
    first.error_message = None;
    first
        .content
        .retain(|b| !matches!(b, tau_ai::message::AssistantBlock::ToolCall(_)));
    first.response_id = Some("resp_1".into());
    let mut second = first.clone();
    second.response_id = Some("resp_2".into());
    let fake = FakeOpenAi::new(vec![
        Reply::Respond {
            frames: openai::draw_response_frames(tc, &first),
            response_id: "resp_1".into(),
            output_items: response_items(&first),
        },
        Reply::Respond {
            frames: openai::draw_response_frames(tc, &second),
            response_id: "resp_2".into(),
            output_items: response_items(&second),
        },
    ]);
    let counts = Rc::new(RefCell::new((0, 0)));
    let seen = counts.clone();
    let model = model.to_owned();
    let mut sim = turmoil::Builder::new().build();
    fake.install(&mut sim, "api");
    sim.client("client", async move {
        let client = OpenAi::with_connector(SimConnector, Limits::default());
        let settings = Settings {
            model,
            ..Settings::default()
        };
        let mut session = client.session(settings).await.unwrap();
        let mut response = session.respond(&hello(), 0);
        while response.next().await.is_some() {}
        tau_ai::llm::LlmSession::set_reasoning(
            &mut session,
            Some(ReasoningEffort::High),
        );
        let mut transcript = hello();
        transcript.push(Message::Assistant(first));
        transcript.push(Message::User(UserMessage {
            content: UserContent::Text("again".into()),
            timestamp: 0,
        }));
        let mut response = session.respond(&transcript, 0);
        while response.next().await.is_some() {}
        let stats = client.stats().await.unwrap();
        *seen.borrow_mut() =
            (stats.lanes.full_requests, stats.lanes.delta_requests);
        Ok(())
    });
    sim.run().unwrap();
    let (full, delta) = *counts.borrow();
    let bodies = fake.received().into_iter().map(|r| r.body).collect();
    (bodies, full, delta)
}

/// An effort set on a session goes with the requests after it.
#[hegel::test(test_cases = 5)]
fn a_session_takes_a_new_effort_between_requests(tc: TestCase) {
    let (bodies, _, _) = two_turns_changing_effort(&tc, "gpt-6-sol");
    assert!(bodies[0].get("reasoning").is_none(), "the model's default");
    assert_eq!(bodies[1]["reasoning"]["effort"], json!("high"));
}
