//! Connection affinity end to end: the client, pool and driver against
//! `FakeOpenAi`'s per-connection prompt cache, in a turmoil simulation
//! (`docs/reference/openai-websocket.md`, "Prompt cache"). A resumed
//! conversation reads its prefix on its own connection, a fork's first
//! request reads its parent's, parallel forks get connections of their
//! own, and a different tool list reads nothing.

use std::{cell::RefCell, io, rc::Rc};

use hegel::TestCase;
use tau_ai::{
    client::{OpenAi, Session},
    event::{Accumulator, AssistantEvent},
    message::{
        AssistantBlock,
        AssistantMessage,
        Message,
        StopReason,
        UserContent,
        UserMessage,
    },
    responses::{
        input::response_items,
        request::{Lineage, Settings, ToolDefinition},
    },
    ws::{io::connection::Connector, proto::pool::Limits},
};
use tau_testing::{
    fake_openai::{FakeOpenAi, PORT, Received, Reply},
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

fn user(text: &str) -> Message {
    Message::User(UserMessage {
        content: UserContent::Text(text.into()),
        timestamp: 0,
    })
}

fn tool(name: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        description: format!("The {name} tool, which reads a file."),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {"path": {"type": "string"}},
        }),
        strict: false,
    }
}

/// A run's settings: long instructions, so the prefix is worth caching,
/// and the conversation `path`, forked from `parent`.
fn settings(path: &str, parent: Option<&str>, tools: &[&str]) -> Settings {
    Settings {
        model: "gpt-5.5".into(),
        instructions: Some("Keep answers short. ".repeat(200)),
        tools: tools.iter().map(|name| tool(name)).collect(),
        lineage: Some(Lineage {
            path: path.into(),
            parent: parent.map(str::to_owned),
        }),
        ..Settings::default()
    }
}

/// `n` text replies, drawn once before the simulation, as the fake
/// needs them.
fn replies(tc: &TestCase, n: usize) -> Vec<Reply> {
    (0..n)
        .map(|at| {
            let mut message = tc.draw(openai::wire_assistant_message());
            message.stop_reason = StopReason::Stop;
            message.error_message = None;
            message
                .content
                .retain(|block| !matches!(block, AssistantBlock::ToolCall(_)));
            message.response_id = Some(format!("resp_{at}"));
            Reply::Respond {
                frames: openai::draw_response_frames(tc, &message),
                response_id: format!("resp_{at}"),
                output_items: response_items(&message),
            }
        })
        .collect()
}

/// Asks for the next response to `transcript`, and returns it with the
/// tokens its usage read from cache.
async fn ask(
    session: &mut Session,
    transcript: &[Message],
) -> (AssistantMessage, u64) {
    let mut response = session.respond(transcript, 0);
    let mut accumulator = Accumulator::new();
    let mut cached = 0;
    while let Some(event) = response.next().await {
        if let AssistantEvent::Done { usage, .. } = &event {
            cached = usage.cache_read;
        }
        accumulator.push(event).unwrap();
    }
    (accumulator.finish().unwrap(), cached)
}

/// What each response's usage read from cache, by the path that asked,
/// in order.
type Reported = Rc<RefCell<Vec<(&'static str, u64)>>>;

/// Runs `scenario` against a fake with `n` drawn replies, and returns
/// what the fake received.
fn simulate<F, Fut>(tc: &TestCase, n: usize, scenario: F) -> Vec<Received>
where
    F: FnOnce(OpenAi, Reported) -> Fut + 'static,
    Fut: std::future::Future<Output = ()> + 'static,
{
    let fake = FakeOpenAi::new(replies(tc, n)).report_cache();
    let mut sim = turmoil::Builder::new().build();
    fake.install(&mut sim, "api");
    let reported: Reported = Rc::default();
    let seen = reported.clone();
    sim.client("client", async move {
        let client = OpenAi::with_connector(SimConnector, Limits::default());
        scenario(client, seen).await;
        Ok(())
    });
    sim.run().unwrap();
    let received = fake.received();
    // What each path's responses reported is what the fake's cache
    // held for its requests.
    for path in ["main", "fork", "a", "b", "chat"] {
        let cached: Vec<u64> = received
            .iter()
            .filter(|request| request.body["prompt_cache_key"] == path)
            .map(|request| request.cached_tokens)
            .collect();
        let usage: Vec<u64> = reported
            .borrow()
            .iter()
            .filter(|(by, _)| *by == path)
            .map(|(_, cached)| *cached)
            .collect();
        assert_eq!(usage, cached, "{path}");
    }
    received
}

/// The request `path` sent.
fn sent_by<'a>(received: &'a [Received], path: &str) -> &'a Received {
    received
        .iter()
        .find(|request| request.body["prompt_cache_key"] == path)
        .unwrap_or_else(|| panic!("no request from {path}"))
}

/// A resumed conversation's run takes the connection its last run left,
/// continues by delta, and reads the whole prefix from cache.
#[hegel::test(test_cases = 5)]
fn a_resumed_conversation_reads_its_prefix(tc: TestCase) {
    let received = simulate(&tc, 2, |client, reported| async move {
        let mut transcript = vec![user("hello")];
        let mut first = client
            .session(settings("main", None, &["read"]))
            .await
            .unwrap();
        let (answer, cached) = ask(&mut first, &transcript).await;
        reported.borrow_mut().push(("main", cached));
        drop(first);
        transcript.push(Message::Assistant(answer));
        transcript.push(user("and then?"));
        let mut again = client
            .session(settings("main", None, &["read"]))
            .await
            .unwrap();
        let (_, cached) = ask(&mut again, &transcript).await;
        reported.borrow_mut().push(("main", cached));
        let stats = client.stats().await.unwrap();
        assert_eq!(stats.own_connection, 1);
        assert_eq!(stats.connections_opened, 1);
    });
    assert_eq!(received[1].connection, received[0].connection);
    assert!(received[1].body.get("previous_response_id").is_some());
    assert_eq!(received[0].cached_tokens, 0);
    assert!(
        received[1].cached_tokens >= received[0].input_tokens,
        "{} cached of a {}-token prefix",
        received[1].cached_tokens,
        received[0].input_tokens
    );
}

/// A fork's first request goes on its parent's connection, even while
/// the parent's run lives, and reads the parent's prefix; the parent's
/// next request goes on a new connection, which holds nothing.
#[hegel::test(test_cases = 5)]
fn a_fork_reads_its_parent_s_prefix(tc: TestCase) {
    let received = simulate(&tc, 3, |client, reported| async move {
        let mut transcript = vec![user("hello")];
        let mut main = client
            .session(settings("main", None, &["read"]))
            .await
            .unwrap();
        let (answer, cached) = ask(&mut main, &transcript).await;
        reported.borrow_mut().push(("main", cached));
        transcript.push(Message::Assistant(answer));
        let mut fork_transcript = transcript.clone();
        fork_transcript.push(user("fork: do this part"));
        let mut fork = client
            .session(settings("fork", Some("main"), &["read"]))
            .await
            .unwrap();
        let (_, cached) = ask(&mut fork, &fork_transcript).await;
        reported.borrow_mut().push(("fork", cached));
        transcript.push(user("main goes on"));
        let (_, cached) = ask(&mut main, &transcript).await;
        reported.borrow_mut().push(("main", cached));
        assert_eq!(client.stats().await.unwrap().handoffs, 1);
    });
    assert_eq!(received[1].connection, received[0].connection);
    // The fork's own key, and a full request.
    assert_eq!(received[1].body["prompt_cache_key"], "fork");
    assert!(received[1].body.get("previous_response_id").is_none());
    assert!(received[1].cached_tokens >= received[0].input_tokens);
    assert_ne!(received[2].connection, received[0].connection);
    assert!(received[2].body.get("previous_response_id").is_none());
    assert_eq!(received[2].cached_tokens, 0);
}

/// Forks that run side by side each get a connection of their own: the
/// first takes the parent's and reads its prefix, the others open new
/// ones.
#[hegel::test(test_cases = 5)]
fn parallel_forks_get_their_own_connections(tc: TestCase) {
    let received = simulate(&tc, 3, |client, reported| async move {
        let mut transcript = vec![user("hello")];
        let mut main = client
            .session(settings("main", None, &["read"]))
            .await
            .unwrap();
        let (answer, cached) = ask(&mut main, &transcript).await;
        reported.borrow_mut().push(("main", cached));
        drop(main);
        transcript.push(Message::Assistant(answer));
        let mut a = client
            .session(settings("a", Some("main"), &["read"]))
            .await
            .unwrap();
        let mut b = client
            .session(settings("b", Some("main"), &["read"]))
            .await
            .unwrap();
        let mut ask_a = transcript.clone();
        ask_a.push(user("a"));
        let mut ask_b = transcript.clone();
        ask_b.push(user("b"));
        let ((_, cached_a), (_, cached_b)) =
            tokio::join!(ask(&mut a, &ask_a), ask(&mut b, &ask_b));
        reported
            .borrow_mut()
            .extend([("a", cached_a), ("b", cached_b)]);
    });
    let (main, a, b) = (
        sent_by(&received, "main"),
        sent_by(&received, "a"),
        sent_by(&received, "b"),
    );
    // `a` opened first, and took the parent's connection.
    assert_eq!(a.connection, main.connection);
    assert!(a.cached_tokens >= main.input_tokens);
    assert_ne!(b.connection, main.connection);
    assert_eq!(b.cached_tokens, 0);
}

/// A fork whose tool list differs from its parent's still takes the
/// parent's connection, but reads nothing from its cache: the tools come
/// first in the prompt.
#[hegel::test(test_cases = 5)]
fn a_different_tool_list_reads_nothing(tc: TestCase) {
    let received = simulate(&tc, 2, |client, reported| async move {
        let mut transcript = vec![user("hello")];
        let mut main = client
            .session(settings("main", None, &["read", "delegate"]))
            .await
            .unwrap();
        let (answer, cached) = ask(&mut main, &transcript).await;
        reported.borrow_mut().push(("main", cached));
        drop(main);
        transcript.push(Message::Assistant(answer));
        transcript.push(user("chat"));
        let mut chat = client
            .session(settings("chat", Some("main"), &["read"]))
            .await
            .unwrap();
        let (_, cached) = ask(&mut chat, &transcript).await;
        reported.borrow_mut().push(("chat", cached));
    });
    assert_eq!(received[1].connection, received[0].connection);
    assert_eq!(received[1].cached_tokens, 0);
}

/// Instructions that grow at the end still read the prefix up to the
/// change: the head and the instructions they share, not the input
/// after them.
#[hegel::test(test_cases = 5)]
fn longer_instructions_read_up_to_the_change(tc: TestCase) {
    let received = simulate(&tc, 2, |client, reported| async move {
        let transcript = vec![user("hello")];
        let mut first = client
            .session(settings("main", None, &["read"]))
            .await
            .unwrap();
        let (_, cached) = ask(&mut first, &transcript).await;
        reported.borrow_mut().push(("main", cached));
        drop(first);
        let mut longer = settings("main", None, &["read"]);
        if let Some(text) = &mut longer.instructions {
            text.push_str("Repository notes: use tabs.");
        }
        let mut again = client.session(longer).await.unwrap();
        let (_, cached) = ask(&mut again, &transcript).await;
        reported.borrow_mut().push(("main", cached));
    });
    assert_eq!(received[1].connection, received[0].connection);
    assert!(received[1].body.get("previous_response_id").is_none());
    let instructions = settings("main", None, &[]).instructions.unwrap();
    assert!(received[1].cached_tokens >= instructions.len() as u64 / 4);
    assert!(received[1].cached_tokens < received[0].input_tokens);
}
