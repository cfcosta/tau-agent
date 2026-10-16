//! An agent's `mcp` triggers (ADR 0037), against a host whose one
//! server holds a resource it says changed: the first read only records
//! it, a read that differs fires once with what it read, one that does
//! not fires nothing, a restart fires what changed while tau was closed,
//! a watch that ends watches again, a server that cannot subscribe says
//! so, and a new version without the trigger stops watching.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use serde_json::{Value, json};
use tau_agents::{Settings, Trigger, event::Message};
use tau_agents_host::{Agents, clock::FakeClock, env::Env};
use tau_tools_host::secrets::SecretStore;
use tau_ui_plugin::{
    Conversations,
    ResourceWatch,
    Turn,
    WatchError,
    registry::HostFuture,
};
use tau_vcs_host::{Identity, Project, ProjectRepo};
use tokio::sync::mpsc;

const URI: &str = "ci://tau-agent/main";

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

/// The server `ci`: what its resource holds, and its watches.
struct Server {
    /// It says when its resources change.
    subscribes: std::sync::atomic::AtomicBool,
    /// Its connection changed: a watch it refused may be tried again.
    reconnected: tokio::sync::Notify,
    build: Mutex<String>,
    watchers: Mutex<Vec<mpsc::UnboundedSender<()>>>,
    /// Watches held now.
    watching: Arc<AtomicUsize>,
    /// Where it was asked from, each watch.
    asked: Mutex<Vec<(PathBuf, String, String)>>,
}

impl Server {
    fn new(subscribes: bool) -> Arc<Self> {
        Arc::new(Self {
            subscribes: std::sync::atomic::AtomicBool::new(subscribes),
            reconnected: tokio::sync::Notify::new(),
            build: Mutex::new("green".into()),
            watchers: Mutex::default(),
            watching: Arc::default(),
            asked: Mutex::default(),
        })
    }

    /// Sets what the resource holds, and says it changed, though it
    /// may be what it held.
    fn set(&self, build: &str) {
        build.clone_into(&mut self.build.lock().unwrap());
        self.watchers
            .lock()
            .unwrap()
            .retain(|watcher| watcher.send(()).is_ok());
    }

    /// Ends every watch, as a connection that drops does.
    fn drop_connection(&self) {
        self.watchers.lock().unwrap().clear();
    }
}

/// Counts a watch held until it drops.
struct Held(Arc<AtomicUsize>);

impl Drop for Held {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

struct Host {
    conversation: Arc<Conversation>,
    secrets: SecretStore,
    server: Arc<Server>,
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

    fn secrets(&self, agent: &str) -> SecretStore {
        let store = self.secrets.clone();
        SecretStore::of_agent(
            store.path().parent().expect("a directory"),
            agent,
        )
    }

    fn call_tool<'a>(
        &'a self,
        _checkout: &'a Path,
        _name: &'a str,
        _args: Value,
    ) -> HostFuture<'a, Result<Value, String>> {
        Box::pin(async { Err("no tools here".into()) })
    }

    fn fetch<'a>(
        &'a self,
        _agent: &'a str,
        _spec: Value,
    ) -> HostFuture<'a, Result<Value, String>> {
        Box::pin(async { Err("no requests here".into()) })
    }

    fn watch_resource<'a>(
        &'a self,
        checkout: &'a Path,
        server: &'a str,
        uri: &'a str,
    ) -> HostFuture<'a, Result<ResourceWatch, WatchError>> {
        Box::pin(async move {
            self.server.asked.lock().unwrap().push((
                checkout.to_owned(),
                server.to_owned(),
                uri.to_owned(),
            ));
            if !self.server.subscribes.load(Ordering::SeqCst) {
                return Err(WatchError::Unsupported(
                    "MCP server ci does not say when a resource changes: a \
                     poll can read it instead"
                        .into(),
                ));
            }
            let (sender, updates) = mpsc::unbounded_channel();
            self.server.watchers.lock().unwrap().push(sender);
            self.server.watching.fetch_add(1, Ordering::SeqCst);
            Ok(ResourceWatch::new(
                updates,
                Held(self.server.watching.clone()),
            ))
        })
    }

    fn server_changed<'a>(
        &'a self,
        _checkout: &'a Path,
        _server: &'a str,
    ) -> HostFuture<'a, ()> {
        Box::pin(self.server.reconnected.notified())
    }

    fn read_resource<'a>(
        &'a self,
        _checkout: &'a Path,
        _server: &'a str,
        uri: &'a str,
    ) -> HostFuture<'a, Result<Value, String>> {
        let build = self.server.build.lock().unwrap().clone();
        Box::pin(async move {
            Ok(json!({ "contents": [{
                "uri": uri,
                "mimeType": "application/json",
                "text": json!({ "build": build }).to_string(),
            }] }))
        })
    }
}

/// The agent `watch`, watching `ci`'s resource as `status`.
const AGENT: &str = r#"
local tau = require("tau")
return tau.agent {
  name = "watch",
  triggers = {
    status = { mcp = { server = "ci", uri = "ci://tau-agent/main" } },
  },
}
"#;

struct Fixture {
    home: tempfile::TempDir,
    host: Arc<Host>,
    agents: Agents,
}

impl Fixture {
    async fn new(server: Arc<Server>) -> Self {
        let home = tempfile::tempdir().unwrap();
        ProjectRepo::init(
            home.path().join("agents").join("watch"),
            Identity::default(),
            &[("agent.luau".to_owned(), AGENT.to_owned())],
        )
        .unwrap();
        let host = Arc::new(Host {
            conversation: Arc::default(),
            secrets: SecretStore::in_dir(&home.path().join("config")),
            server,
        });
        let agents = Self::agents(home.path(), &host).await;
        Self { home, host, agents }
    }

    /// The agents on `home`, as tau starts them.
    async fn agents(home: &Path, host: &Arc<Host>) -> Agents {
        let agents = Agents::at(
            home.join("agents"),
            home.join("state"),
            host.clone(),
            Arc::new(FakeClock::new(1_791_792_000, 0)),
        )
        .with_watch_waits(
            Duration::from_millis(20),
            Duration::from_millis(100),
        );
        agents.registry().scan().await;
        agents
    }

    /// tau closes, and starts again on what it kept.
    async fn restart(&mut self) {
        self.agents.close_watches().await;
        self.agents = Self::agents(self.home.path(), &self.host).await;
    }

    /// The events every turn heard so far, by id, and their data.
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

    async fn trigger(&self) -> Option<Trigger> {
        self.agents.overview().await.agents[0]
            .triggers
            .iter()
            .find(|trigger| trigger.name == "status")
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

    /// Ticks a few times more: what would fire has.
    async fn settle(&self) {
        for _ in 0..10 {
            self.agents.tick().await;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn watching(&self) {
        self.until("it watches", || async {
            self.trigger().await.unwrap().live == Some(true)
        })
        .await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_change_fires_once_with_what_it_read() {
    let server = Server::new(true);
    let f = Fixture::new(server.clone()).await;
    f.watching().await;
    // It watches the agent's server and resource, from its checkout.
    let asked = server.asked.lock().unwrap()[0].clone();
    assert!(asked.0.ends_with("agents/watch"), "{:?}", asked.0);
    assert_eq!((asked.1.as_str(), asked.2.as_str()), ("ci", URI));
    // The first read only records.
    f.settle().await;
    assert!(f.heard().is_empty(), "{:?}", f.heard());

    server.set("red");
    f.until("red is heard", || async { f.heard().len() == 1 })
        .await;
    let (id, data) = f.heard()[0].clone();
    assert!(id.starts_with(&format!("{URI}:")), "{id}");
    assert!(data.contains("red"), "{data}");
    // Told again of what it holds: nothing new.
    server.set("red");
    f.settle().await;
    assert_eq!(f.heard().len(), 1);
    // Back to green: a change of its own.
    server.set("green");
    f.until("green is heard", || async { f.heard().len() == 2 })
        .await;
    assert_ne!(f.heard()[0].0, f.heard()[1].0);
    f.agents.close_watches().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restart_fires_what_changed_while_tau_was_closed() {
    let server = Server::new(true);
    let mut f = Fixture::new(server.clone()).await;
    f.watching().await;
    f.settle().await;
    // Started again with nothing changed: nothing fires.
    f.restart().await;
    f.watching().await;
    f.settle().await;
    assert!(f.heard().is_empty(), "{:?}", f.heard());
    // It changes while tau is closed: the read as it starts fires it.
    f.agents.close_watches().await;
    server.set("red");
    f.restart().await;
    f.until("red is heard", || async { f.heard().len() == 1 })
        .await;
    assert!(f.heard()[0].1.contains("red"));
    f.agents.close_watches().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_watch_that_ends_watches_again_and_reads() {
    let server = Server::new(true);
    let f = Fixture::new(server.clone()).await;
    f.watching().await;
    f.settle().await;
    let watched = server.asked.lock().unwrap().len();
    // The connection drops, and the resource changes before it is back.
    server.drop_connection();
    server.set("red");
    f.until("red is heard", || async { f.heard().len() == 1 })
        .await;
    assert!(server.asked.lock().unwrap().len() > watched);
    f.agents.close_watches().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_version_without_the_trigger_stops_watching() {
    let server = Server::new(true);
    let f = Fixture::new(server.clone()).await;
    f.watching().await;
    assert_eq!(server.watching.load(Ordering::SeqCst), 1);
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
    f.until("it stops watching", || async {
        server.watching.load(Ordering::SeqCst) == 0
    })
    .await;
    assert_eq!(f.trigger().await, None);
}

/// A server that cannot subscribe says so on the agent's page and fires
/// nothing; it is tried again as soon as its connection changes, not only
/// after the longest wait.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_server_that_could_not_subscribe_is_tried_again_when_it_changes() {
    let server = Server::new(false);
    let mut f = Fixture::new(server.clone()).await;
    // The longest wait is longer than the test.
    f.agents.close_watches().await;
    f.agents = Agents::at(
        f.home.path().join("agents"),
        f.home.path().join("state"),
        f.host.clone(),
        Arc::new(FakeClock::new(1_791_792_000, 0)),
    )
    .with_watch_waits(Duration::from_millis(20), Duration::from_secs(600));
    f.agents.registry().scan().await;
    f.until("it says why", || async {
        f.trigger().await.unwrap().error.is_some()
    })
    .await;
    // It is not watching, says a poll can read it instead, and hears
    // nothing.
    let trigger = f.trigger().await.unwrap();
    assert_eq!(trigger.live, Some(false));
    assert!(trigger.error.unwrap().contains("poll"));
    assert!(f.heard().is_empty());
    // It learns to, and its connection says so.
    server.subscribes.store(true, Ordering::SeqCst);
    server.reconnected.notify_one();
    f.until("it watches", || async {
        f.trigger().await.unwrap().live == Some(true)
    })
    .await;
    assert_eq!(f.trigger().await.unwrap().error, None);
    f.agents.close_watches().await;
}
