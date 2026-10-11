//! An agent's streams (ADR 0037), against real servers on 127.0.0.1:
//! server-sent events that drop and come back with `Last-Event-ID`, a
//! websocket, a `parse` that fails without stopping its stream, repeats
//! that fire once, a secret header that never shows, and a stream that a
//! new version drops.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]
#![allow(
    clippy::result_large_err,
    reason = "tungstenite's handshake callback answers with its own error type"
)]

use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tau_agents::{Settings, Trigger, event::Message};
use tau_agents_host::{Agents, clock::FakeClock, env::Env};
use tau_tools_host::secrets::SecretStore;
use tau_ui_plugin::{Conversations, Turn, registry::HostFuture};
use tau_vcs_host::{Identity, Project, ProjectRepo};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// The secret the streams' headers name, and its value.
const SECRET: &str = "ci";
const VALUE: &str = "s3cr3t-stream-token";

#[derive(Default)]
struct Conversation {
    turns: Mutex<Vec<Turn>>,
}

impl Conversations for Conversation {
    fn list_agent(&self, _name: &str) -> HostFuture<'_, anyhow::Result<()>> {
        Box::pin(async { Ok(()) })
    }

    fn main_of(&self, _repo: &str) -> Option<String> {
        None
    }

    fn running(&self, _repo: &str) -> bool {
        false
    }

    fn turn(&self, turn: Turn) -> HostFuture<'_, anyhow::Result<()>> {
        self.turns.lock().unwrap().push(turn);
        Box::pin(async { Ok(()) })
    }
}

struct Host {
    conversation: Arc<Conversation>,
    secrets: SecretStore,
}

impl Env for Host {
    fn refresh(&self) {}

    fn settings(&self, _agent: &str) -> Settings {
        // Every event goes to a turn at the next tick.
        Settings {
            window_secs: 0,
            ..Settings::default()
        }
    }

    fn save_settings(
        &self,
        _agent: &str,
        _settings: Settings,
    ) -> HostFuture<'_, anyhow::Result<()>> {
        Box::pin(async { Ok(()) })
    }

    fn spent_since(
        &self,
        _agent: &str,
        _since: i64,
    ) -> HostFuture<'_, anyhow::Result<f64>> {
        Box::pin(async { Ok(0.0) })
    }

    fn conversations(&self) -> Option<Arc<dyn Conversations>> {
        Some(self.conversation.clone())
    }

    fn secrets(&self) -> SecretStore {
        self.secrets.clone()
    }

    fn call_tool<'a>(
        &'a self,
        _checkout: &'a Path,
        _name: &'a str,
        _args: Value,
    ) -> HostFuture<'a, Result<Value, String>> {
        Box::pin(async { Err("no tools here".into()) })
    }

    fn fetch(&self, _spec: Value) -> HostFuture<'_, Result<Value, String>> {
        Box::pin(async { Err("no requests here".into()) })
    }
}

struct Fixture {
    home: tempfile::TempDir,
    host: Arc<Host>,
    agents: Agents,
}

impl Fixture {
    /// The agent `watch` with `source` as its `agent.luau`, its secret
    /// in the store.
    async fn new(source: &str) -> Self {
        let home = tempfile::tempdir().unwrap();
        ProjectRepo::init(
            home.path().join("agents").join("watch"),
            Identity::default(),
            &[("agent.luau".to_owned(), source.to_owned())],
        )
        .unwrap();
        let secrets = SecretStore::in_dir(&home.path().join("config"));
        secrets.set(SECRET, VALUE).await.unwrap();
        let host = Arc::new(Host {
            conversation: Arc::default(),
            secrets,
        });
        let agents = Agents::at(
            home.path().join("agents"),
            home.path().join("state"),
            host.clone(),
            Arc::new(FakeClock::new(1_791_792_000, 0)),
        )
        .with_stream_waits(
            Duration::from_millis(20),
            Duration::from_millis(100),
        );
        agents.registry().scan().await;
        Self { home, host, agents }
    }

    /// The events every turn heard so far, by id.
    fn heard(&self) -> Vec<(String, String)> {
        self.host
            .conversation
            .turns
            .lock()
            .unwrap()
            .iter()
            .flat_map(|turn| Message::parse(&turn.text).unwrap().events)
            .map(|event| (event.id, event.data))
            .collect()
    }

    async fn trigger(&self, name: &str) -> Option<Trigger> {
        self.agents.overview().await.agents[0]
            .triggers
            .iter()
            .find(|trigger| trigger.name == name)
            .cloned()
    }

    /// Ticks until `done` holds, for at most ten seconds.
    async fn until<F: Future<Output = bool>>(
        &self,
        what: &str,
        mut done: impl FnMut() -> F,
    ) {
        for _ in 0..500 {
            self.agents.tick().await;
            if done().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("never: {what}; heard {:?}", self.heard());
    }
}

/// An agent with one stream at `stream`, whose `parse` reads each frame's
/// data as `{ "n": ... }`, gives `n:<n>` as its id, and fails on what is
/// not JSON.
fn agent(stream: &str) -> String {
    format!(
        r#"
local tau = require("tau")
return tau.agent {{
  name = "watch",
  triggers = {{
    feed = {{
      stream = {{ {stream}, headers = {{ authorization = "Bearer " .. tau.secret("ci") }} }},
      parse = function(frame, state)
        if frame.event == "ping" then return nil end
        local n = json.decode(frame.data).n
        state.frames = (state.frames or 0) + 1
        return {{ id = "n:" .. n, data = {{ n = n, frames = state.frames }} }}
      end,
    }},
  }},
}}
"#
    )
}

/// A request's head, as it came.
async fn read_head(stream: &mut tokio::net::TcpStream) -> String {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte).await.unwrap() == 0 {
            break;
        }
        head.push(byte[0]);
    }
    String::from_utf8_lossy(&head).to_lowercase()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_sent_events_come_back_from_the_last_id_and_fire_once() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let heads = Arc::new(Mutex::new(Vec::new()));
    let (seen, server_heads) =
        (Arc::new(tokio::sync::Notify::new()), heads.clone());
    let closed = seen.clone();
    tokio::spawn(async move {
        let mut connection = 0;
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let head = read_head(&mut stream).await;
            server_heads.lock().unwrap().push(head);
            connection += 1;
            let body: &[u8] = match connection {
                // A comment, an event, a ping, then one `parse` cannot
                // read; then it drops.
                1 => b": hello\n\nid: 1\ndata: {\"n\":1}\n\nevent: ping\ndata: x\n\nid: 2\ndata: not json\n\n",
                // From the last id: one it had, then a new one; it stays.
                _ => b"id: 3\ndata: {\"n\":1}\n\nid: 4\ndata: {\"n\":3}\n\n",
            };
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
                      connection: close\r\n\r\n",
                )
                .await
                .unwrap();
            stream.write_all(body).await.unwrap();
            stream.flush().await.unwrap();
            if connection == 1 {
                drop(stream);
            } else {
                // Held open until the client goes.
                let closed = closed.clone();
                tokio::spawn(async move {
                    let mut buffer = [0u8; 64];
                    while stream.read(&mut buffer).await.unwrap_or(0) > 0 {}
                    closed.notify_one();
                });
            }
        }
    });
    let f = Fixture::new(&agent(&format!("sse = \"http://{address}/events\"")))
        .await;
    f.until("both events are heard", || async { f.heard().len() >= 2 })
        .await;
    assert_eq!(
        f.heard(),
        [
            ("n:1".to_owned(), "{\"frames\":1,\"n\":1}".to_owned()),
            // The state `parse` keeps carried on across the drop: the
            // ping and the frame it could not read made no event, and
            // the repeat of n:1 fired nothing.
            ("n:3".to_owned(), "{\"frames\":3,\"n\":3}".to_owned()),
        ]
    );
    let heads = heads.lock().unwrap().clone();
    assert!(
        heads[0].contains("accept: text/event-stream"),
        "{}",
        heads[0]
    );
    assert!(heads[0].contains(&format!("authorization: bearer {VALUE}")));
    assert!(!heads[0].contains("last-event-id"), "{}", heads[0]);
    assert!(heads[1].contains("last-event-id: 2"), "{}", heads[1]);
    // Open now, and the `parse` failure is behind it.
    f.until("the stream shows open", || async {
        f.trigger("feed").await.unwrap().live == Some(true)
    })
    .await;
    assert_eq!(f.trigger("feed").await.unwrap().error, None);

    // A new version without the stream closes it.
    let next = ProjectRepo::init(
        f.home.path().join("next"),
        Identity::default(),
        &[(
            "agent.luau".to_owned(),
            "local tau = require(\"tau\")\n\
             return tau.agent { name = \"watch\", triggers = { hourly = { every = \"1h\" } } }\n"
                .to_owned(),
        )],
    )
    .unwrap();
    let trunk = next.trunk().unwrap();
    f.agents
        .registry()
        .reload("watch", &Project::from(next), &trunk)
        .await;
    f.agents.tick().await;
    tokio::time::timeout(Duration::from_secs(10), seen.notified())
        .await
        .expect("the server saw the stream close");
    assert_eq!(f.trigger("feed").await, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failing_stream_says_why_without_its_secret_and_tries_again() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let tries = Arc::new(Mutex::new(0));
    let counted = tries.clone();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_head(&mut stream).await;
            *counted.lock().unwrap() += 1;
            // It refuses, and says the token back.
            let body = format!("bad token {VALUE}");
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 401 Unauthorized\r\ncontent-length: {}\r\n\
                         connection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        }
    });
    let f = Fixture::new(&agent(&format!("sse = \"http://{address}/events\"")))
        .await;
    f.until("it tried three times", || async {
        *tries.lock().unwrap() >= 3
    })
    .await;
    let trigger = f.trigger("feed").await.unwrap();
    assert_eq!(trigger.live, Some(false));
    let error = trigger.error.unwrap();
    assert!(error.contains("401"), "{error}");
    assert!(!error.contains(VALUE), "{error}");
    assert!(f.heard().is_empty());
    f.agents.close_streams().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_websockets_messages_are_frames_and_it_opens_again() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let authorizations = Arc::new(Mutex::new(Vec::new()));
    let seen = authorizations.clone();
    tokio::spawn(async move {
        let mut connection = 0;
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            connection += 1;
            let seen = seen.clone();
            let mut socket = tokio_tungstenite::accept_hdr_async(
                stream,
                move |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
                      response| {
                    seen.lock().unwrap().push(
                        request
                            .headers()
                            .get("authorization")
                            .map(|value| value.to_str().unwrap().to_owned()),
                    );
                    Ok(response)
                },
            )
            .await
            .unwrap();
            if connection == 1 {
                socket.send(WsMessage::text("{\"n\":1}")).await.unwrap();
                socket.send(WsMessage::text("not json")).await.unwrap();
                socket.close(None).await.unwrap();
            } else {
                socket.send(WsMessage::text("{\"n\":2}")).await.unwrap();
                // Held open.
                while socket.next().await.is_some() {}
            }
        }
    });
    let source = agent(&format!("websocket = \"ws://{address}/ticks\""))
        .replace("frame.event == \"ping\"", "false");
    let f = Fixture::new(&source).await;
    f.until("both messages are heard", || async { f.heard().len() >= 2 })
        .await;
    let ids: Vec<String> = f.heard().into_iter().map(|(id, _)| id).collect();
    assert_eq!(ids, ["n:1", "n:2"]);
    let authorizations = authorizations.lock().unwrap().clone();
    assert_eq!(authorizations[0], Some(format!("Bearer {VALUE}")));
    assert!(authorizations.len() >= 2);
    f.agents.close_streams().await;
}
