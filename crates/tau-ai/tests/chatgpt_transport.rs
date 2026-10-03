//! ChatGPT plan usage over the WebSocket transport: a real sign-in
//! against `FakeChatGpt`, then `OpenAi::chatgpt` against `FakeOpenAi` on
//! the same simulated network (`tau_ai::client`, `tau_ai::refusal`).

use std::{future::Future, time::Duration};

use hegel::{TestCase, generators as gs};
use serde_json::json;
use tau_ai::{
    chatgpt::{
        AccountId,
        ChatGpt,
        Config,
        RedirectUri,
        Store,
        UNSUPPORTED_FIELDS,
    },
    client::OpenAi,
    event::AssistantEvent,
    message::{Message, UserContent, UserMessage},
    responses::request::{Lineage, Settings, ToolDefinition},
    retry::{Class, Recovery},
};
use tau_testing::{
    fake_chatgpt::{Consent, FakeChatGpt, SimDialer},
    fake_openai::{FakeOpenAi, Refusal, Reply},
};

/// Where the fake Responses WebSocket listens; `api.openai.com` is the
/// fake auth server's API host, which serves plain HTTP.
const WS_HOST: &str = "responses";

const USAGE_LIMIT: &str = "subscription_sharing_usage_limit_exceeded";

fn config(fake: &FakeChatGpt) -> Config {
    Config {
        websocket_url: format!("ws://{WS_HOST}/v1/responses"),
        ..fake.config()
    }
}

/// Signs in on `chat`, then runs `test` with a plan client for the
/// account, beside `api`.
fn simulate<F, Fut>(chat: FakeChatGpt, api: FakeOpenAi, test: F)
where
    F: FnOnce(FakeChatGpt, FakeOpenAi, ChatGpt<SimDialer>, AccountId) -> Fut
        + 'static,
    Fut: Future<Output = ()> + 'static,
{
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(600))
        .build();
    chat.install(&mut sim);
    api.install(&mut sim, WS_HOST);
    sim.client("tau", async move {
        let dir = tempfile::tempdir().unwrap();
        let chatgpt = ChatGpt::with_dialer(
            Store::open(dir.path()).unwrap(),
            SimDialer,
            config(&chat),
        );
        let sign_in = chatgpt
            .start_sign_in(None, RedirectUri { port: 1455 }, false)
            .unwrap();
        let callback = sign_in.callback(&chat.approve(sign_in.url())).unwrap();
        let signed_in =
            chatgpt.finish_sign_in(&sign_in, &callback).await.unwrap();
        test(chat, api, chatgpt, signed_in.account).await;
        drop(dir);
        Ok(())
    });
    sim.run().unwrap();
}

fn hello() -> Vec<Message> {
    vec![Message::User(UserMessage {
        content: UserContent::Text("hello".into()),
        timestamp: 0,
    })]
}

fn settings() -> Settings {
    Settings {
        model: "gpt-5.5".into(),
        instructions: Some("Be brief.".into()),
        ..Settings::default()
    }
}

/// A short completed response.
fn completed(id: &str) -> Reply {
    Reply::Respond {
        frames: vec![
            json!({"type": "response.created", "response": {"id": id}}),
            json!({"type": "response.output_item.added", "output_index": 0,
                   "item": {"type": "message", "id": "msg_1", "role": "assistant", "content": []}}),
            json!({"type": "response.content_part.added", "output_index": 0,
                   "content_index": 0, "part": {"type": "output_text", "text": ""}}),
            json!({"type": "response.output_text.delta", "output_index": 0,
                   "content_index": 0, "delta": "Hi"}),
            json!({"type": "response.output_item.done", "output_index": 0,
                   "item": {"type": "message", "id": "msg_1", "role": "assistant",
                            "content": [{"type": "output_text", "text": "Hi"}]}}),
            json!({"type": "response.completed", "response": {
                "id": id, "status": "completed",
                "usage": {"input_tokens": 3, "output_tokens": 1, "total_tokens": 4},
            }}),
        ],
        response_id: id.into(),
        output_items: Vec::new(),
    }
}

/// Every event of one response to `hello`.
async fn respond(client: &OpenAi, settings: Settings) -> Vec<AssistantEvent> {
    let mut session = client.session(settings).await.unwrap();
    let mut response = session.respond(&hello(), 0);
    let mut events = Vec::new();
    while let Some(event) = response.next().await {
        events.push(event);
    }
    events
}

/// The class and message of a response's terminal error.
fn failure(events: &[AssistantEvent]) -> (Class, String) {
    assert!(
        matches!(events.first(), Some(AssistantEvent::Start { .. })),
        "{events:?}"
    );
    match events.last() {
        Some(AssistantEvent::Error { class, message, .. }) => {
            (*class, message.clone())
        }
        other => panic!("not an error: {other:?}"),
    }
}

/// An expired token is refreshed before the connection opens, and the
/// upgrade carries the new one.
#[test]
fn the_bearer_is_refreshed_before_the_connection() {
    let api = FakeOpenAi::new(vec![completed("resp_1")]);
    simulate(
        FakeChatGpt::new(),
        api,
        |chat, api, chatgpt, account| async move {
            let before = chatgpt.store().load(&account).unwrap().access_token;
            chat.advance(3600);
            let client = OpenAi::chatgpt(chatgpt.clone(), account.clone());
            let events = respond(&client, settings()).await;
            assert!(matches!(events.last(), Some(AssistantEvent::Done { .. })));
            assert_eq!(chat.refresh_grants(), 1);
            let after = chatgpt.store().load(&account).unwrap().access_token;
            assert_ne!(after, before);
            assert_eq!(
                api.authorizations(),
                vec![after.map(|token| format!("Bearer {token}"))]
            );
            assert_eq!(client.refusal(), None);
        },
    );
}

/// Two clients of one sign-in share its refresh: one grant, whatever
/// the order they connect in.
#[test]
fn clones_share_one_refresh() {
    let api = FakeOpenAi::new(vec![completed("resp_1"), completed("resp_2")]);
    simulate(
        FakeChatGpt::new(),
        api,
        |chat, api, chatgpt, account| async move {
            chat.advance(3600);
            let first = OpenAi::chatgpt(chatgpt.clone(), account.clone());
            let second = OpenAi::chatgpt(chatgpt.clone(), account.clone());
            let (a, b) = tokio::join!(
                respond(&first, settings()),
                respond(&second, settings())
            );
            assert!(matches!(a.last(), Some(AssistantEvent::Done { .. })));
            assert!(matches!(b.last(), Some(AssistantEvent::Done { .. })));
            assert_eq!(chat.refresh_grants(), 1);
            assert_eq!(chat.violations(), Vec::<String>::new());
            assert_eq!(api.authorizations().len(), 2);
        },
    );
}

/// Whatever the settings ask for, the plan route gets none of the fields
/// it does not take, `store: false`, and no `stream_id`.
#[hegel::test(test_cases = 5)]
fn unsupported_fields_never_go_out(tc: TestCase) {
    let lineage = tc
        .draw(gs::optional(gs::text().min_size(1).max_size(80)))
        .map(|path| Lineage { path, parent: None });
    let tools = tc.draw(gs::booleans());
    let settings = Settings {
        lineage,
        tools: if tools {
            vec![ToolDefinition {
                name: "get_weather".into(),
                description: "The weather.".into(),
                parameters: json!({"type": "object", "properties": {}}),
                strict: false,
            }]
        } else {
            Vec::new()
        },
        ..settings()
    };
    let api = FakeOpenAi::new(vec![completed("resp_1")]);
    let seen = api.clone();
    simulate(
        FakeChatGpt::new(),
        api,
        |_, _, chatgpt, account| async move {
            let client = OpenAi::chatgpt(chatgpt, account);
            let events = respond(&client, settings).await;
            assert!(matches!(events.last(), Some(AssistantEvent::Done { .. })));
        },
    );
    let received = seen.received();
    assert_eq!(received.len(), 1);
    let body = &received[0].body;
    for field in UNSUPPORTED_FIELDS {
        assert!(body.get(field).is_none(), "{field} went out: {body}");
    }
    assert_eq!(body["store"], json!(false));
    assert!(body.get("stream_id").is_none());
    assert_eq!(body["instructions"], json!("Be brief."));
    // Tools go as plain top-level functions: the route takes them.
    if tools {
        assert_eq!(body["tools"][0]["type"], json!("function"));
    }
}

/// A usage limit at the upgrade stops the run at once: one attempt, a
/// fatal error with the status, code and request id, and no reconnect
/// behind the idle lane afterwards.
#[test]
fn a_usage_limit_at_the_upgrade_stops_without_retry() {
    let api = FakeOpenAi::new(Vec::new());
    for _ in 0..4 {
        api.refuse_upgrade(Refusal {
            status: 429,
            body: json!({"error": {"code": USAGE_LIMIT, "message": "Usage limit reached."}}),
            request_id: Some("req_limit".into()),
        });
    }
    simulate(
        FakeChatGpt::new(),
        api,
        |_, api, chatgpt, account| async move {
            let client = OpenAi::chatgpt(chatgpt, account);
            let events = respond(&client, settings()).await;
            let (class, message) = failure(&events);
            assert_eq!(class, Class::Fatal);
            assert!(message.contains("429"), "{message}");
            assert!(message.contains(USAGE_LIMIT), "{message}");
            assert!(message.contains("req_limit"), "{message}");
            let refusal = client.refusal().expect("the refusal is kept");
            assert_eq!(refusal.recovery, Recovery::UsageLimit);
            assert_eq!(refusal.status, Some(429));
            assert_eq!(refusal.code.as_deref(), Some(USAGE_LIMIT));
            assert_eq!(refusal.request_id.as_deref(), Some("req_limit"));
            let attempts = api.authorizations().len();
            assert!((1..=2).contains(&attempts), "{attempts} upgrades");
            // Nothing asks again until a run does.
            tokio::time::sleep(Duration::from_secs(120)).await;
            assert_eq!(api.authorizations().len(), attempts);
            assert!(api.received().is_empty());
        },
    );
}

/// A usage limit after streaming began arrives as `response.failed`: the
/// run stops with a fatal error, and the refusal is kept.
#[test]
fn a_usage_limit_mid_stream_stops_without_retry() {
    let api = FakeOpenAi::new(vec![Reply::Respond {
        frames: vec![
            json!({"type": "response.created", "response": {"id": "resp_1"}}),
            json!({"type": "response.failed", "response": {
                "id": "resp_1", "status": "failed",
                "error": {"code": USAGE_LIMIT, "message": "Usage limit reached."},
            }}),
        ],
        response_id: "resp_1".into(),
        output_items: Vec::new(),
    }]);
    simulate(
        FakeChatGpt::new(),
        api,
        |_, api, chatgpt, account| async move {
            let client = OpenAi::chatgpt(chatgpt, account);
            let events = respond(&client, settings()).await;
            let (class, message) = failure(&events);
            assert_eq!(class, Class::Fatal);
            assert!(message.contains(USAGE_LIMIT), "{message}");
            let refusal = client.refusal().expect("the refusal is kept");
            assert_eq!(refusal.recovery, Recovery::UsageLimit);
            assert!(refusal.body.contains("response.failed"));
            assert_eq!(api.received().len(), 1, "never resent");
        },
    );
}

/// A temporary refusal takes one transparent reconnect, then fails
/// retryable with the refusal's details: the run's policy backs off.
#[test]
fn a_temporary_refusal_is_retryable_with_its_details() {
    let api = FakeOpenAi::new(Vec::new());
    for _ in 0..3 {
        api.refuse_upgrade(Refusal {
            status: 503,
            body: json!({"detail": "Direct routing is unavailable."}),
            request_id: Some("req_busy".into()),
        });
    }
    simulate(
        FakeChatGpt::new(),
        api,
        |_, _, chatgpt, account| async move {
            let client = OpenAi::chatgpt(chatgpt, account);
            let events = respond(&client, settings()).await;
            let (class, message) = failure(&events);
            assert_eq!(class, Class::Retryable);
            assert!(message.contains("503"), "{message}");
            assert!(message.contains("Direct routing"), "{message}");
            assert_eq!(
                client.refusal().map(|refusal| refusal.recovery),
                Some(Recovery::RetryLater)
            );
        },
    );
}

/// A refresh token OpenAI no longer takes: no upgrade is tried, and the
/// run stops asking to sign in again.
#[test]
fn a_dead_sign_in_asks_to_sign_in_again() {
    let api = FakeOpenAi::new(Vec::new());
    simulate(
        FakeChatGpt::new(),
        api,
        |chat, api, chatgpt, account| async move {
            chat.advance(3600);
            chat.fail_refresh(400, json!({"error": "invalid_grant"}));
            let client = OpenAi::chatgpt(chatgpt, account);
            let events = respond(&client, settings()).await;
            let (class, _) = failure(&events);
            assert_eq!(class, Class::Fatal);
            assert_eq!(
                client.refusal().map(|refusal| refusal.recovery),
                Some(Recovery::SignInAgain)
            );
            assert!(api.authorizations().is_empty());
        },
    );
}

/// A sign-in without plan usage never reaches the Responses endpoint.
#[test]
fn a_sign_in_without_plan_usage_connects_nothing() {
    let chat = FakeChatGpt::new();
    chat.consent(Consent::GrantWithoutPlanUsage);
    simulate(
        chat,
        FakeOpenAi::new(Vec::new()),
        |_, api, chatgpt, account| async move {
            let client = OpenAi::chatgpt(chatgpt, account);
            let events = respond(&client, settings()).await;
            let (class, _) = failure(&events);
            assert_eq!(class, Class::Fatal);
            assert_eq!(
                client.refusal().map(|refusal| refusal.recovery),
                Some(Recovery::EnablePlanUsage)
            );
            assert!(api.authorizations().is_empty());
        },
    );
}

/// A response that completes clears the refusal before it.
#[test]
fn a_completed_response_clears_the_refusal() {
    let api = FakeOpenAi::new(vec![completed("resp_1")]);
    api.refuse_upgrade(Refusal {
        status: 429,
        body: json!({"error": {"code": USAGE_LIMIT, "message": "limit"}}),
        request_id: None,
    });
    simulate(
        FakeChatGpt::new(),
        api,
        |_, _, chatgpt, account| async move {
            let client = OpenAi::chatgpt(chatgpt, account);
            let first = respond(&client, settings()).await;
            let (class, _) = failure(&first);
            assert_eq!(class, Class::Fatal);
            assert!(client.refusal().is_some());
            let second = respond(&client, settings()).await;
            assert!(
                matches!(second.last(), Some(AssistantEvent::Done { .. })),
                "{second:?}"
            );
            assert_eq!(client.refusal(), None);
        },
    );
}
