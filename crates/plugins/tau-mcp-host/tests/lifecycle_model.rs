//! A connection's lifecycle against a model of the server it talks to
//! (`docs/reference/mcp.md`, "Connections", "Subscriptions").
//!
//! The server is the in-process one (`tests/common`), on 2026-07-28 or
//! on 2025-11-25: over a duplex stream on tokio's paused clock, or over
//! streamable HTTP on a loopback port on the wall's clock
//! (`tests/common/http.rs`). Random sequences of steps run against it:
//! tau connects; the server drops the connection; it restarts: over a
//! duplex stream its connection drops and it may say when a resource
//! changes or not from then on, over HTTP it forgets every session and
//! answers one it does not know with the spec's 404 or with the
//! TypeScript SDK's 400 and an error naming no request (26745cab); tau
//! starts the session again as after a sign-in; the server changes its tool
//! list or its resource list and says so; tool calls run side by side,
//! each sending some progress updates right before its result;
//! resources are watched and unwatched, and a watch dropped is taken
//! again at once.
//!
//! The model is what the server holds and what tau last listed. After
//! every step, once everything in flight has landed:
//! - a waiter that sees the status turn `Connected` reads the server's
//!   tool list through the plugin, never the list cached from before
//!   the connect (08e0ec42);
//! - connected, tau's tools and resources are the server's; dropped,
//!   they are the ones last listed;
//! - the resources tau holds subscriptions to are the ones the server
//!   has subscribed, one each however many watch them, and a watch
//!   whose session ended says so (2f945fbc);
//! - every call returns its result, the tool ran once per call (so a
//!   call right after an HTTP restart neither waited out its timeout
//!   nor ran twice), and
//!   each call got each of its own progress updates, in the order sent,
//!   before its result (e062fd2e);
//! - the connection's watchers saw a change whenever its status or its
//!   lists changed (b116c50f).
//!
//! Over HTTP the model found that tau never heard of a restart, so the
//! restarted server's list changes and the watches taken before it
//! were lost; the cases it shrank to are at the end.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    collections::BTreeSet,
    future::Future,
    sync::{
        Arc,
        Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

use common::{
    State as Fixture,
    http::{HttpServer, Unknown},
};
use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_mcp::config::{HttpConfig, Origin, ServerConfig, Transport};
use tau_mcp_host::{
    McpPlugin,
    connection::{
        Connection,
        Environment,
        Progress,
        ResourceWatch,
        State,
        WatchError,
    },
};
use tokio::{runtime::Runtime, sync::watch};
use tokio_util::sync::CancellationToken;

use crate::common;

/// Tools the server adds and removes; its own five stay.
const TOOLS: [&str; 3] = ["alpha", "beta", "gamma"];
/// Resources the server adds and removes; its own stay.
const RESOURCES: [&str; 3] = ["file:///r0", "file:///r1", "file:///r2"];
/// Resources tau watches: any URI may be subscribed to.
const WATCHED: [&str; 3] =
    ["file:///status", "file:///notes.txt", "file:///r0"];

/// How long a step's spawned work has to land: on the paused clock,
/// sleeping runs every task that can run before the time moves.
const SETTLE: Duration = Duration::from_secs(1);

/// How long an ended watch has to say so: over HTTP, on the wall's clock
/// of a loaded machine.
const ENDING: Duration = Duration::from_secs(5);

/// A server's tool names, as it lists them.
fn server_tools(fixture: &Fixture) -> Vec<String> {
    fixture
        .tools
        .lock()
        .unwrap()
        .iter()
        .map(|tool| tool.name.to_string())
        .collect()
}

/// A server's resource URIs, as tau lists them: MCP apps' left out.
fn server_resources(fixture: &Fixture) -> Vec<String> {
    fixture
        .resources
        .lock()
        .unwrap()
        .iter()
        .filter(|resource| {
            !tau_mcp_host::resources::is_app(
                resource["uri"].as_str().unwrap(),
                resource["mimeType"].as_str(),
            )
        })
        .map(|resource| resource["uri"].as_str().unwrap().to_owned())
        .collect()
}

/// What a connection shows: its state and its lists.
#[derive(Debug, Clone, PartialEq)]
struct Shown {
    state: State,
    tools: Vec<String>,
    resources: Vec<String>,
}

fn shown(connection: &Connection) -> Shown {
    Shown {
        state: connection.status().state,
        tools: connection.tools().into_iter().map(|t| t.name).collect(),
        resources: connection.resources().into_iter().map(|r| r.uri).collect(),
    }
}

/// A thread that reads the plugin's tools as fast as it can while the
/// connection connects, as a run waiting in `ready` would, and gives
/// back the tools it read on first seeing `Connected`.
struct Waiter {
    thread: std::thread::JoinHandle<Option<Vec<String>>>,
    stop: Arc<AtomicBool>,
}

impl Waiter {
    /// Starts it, and returns once it has read the tools cached now.
    fn start(plugin: &McpPlugin) -> Self {
        let plugin = plugin.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let (ready, started) = mpsc::channel();
        let stopped = stop.clone();
        let thread = std::thread::spawn(move || {
            let connection = plugin.connections()[0].clone();
            let names = || -> Vec<String> {
                plugin
                    .tools()
                    .iter()
                    .map(|t| t.info().name.clone())
                    .collect()
            };
            names();
            ready.send(()).unwrap();
            loop {
                // Read before the status: stopped means the connect is
                // over, so a status read after it is the last word.
                let stop = stopped.load(Ordering::SeqCst);
                if connection.status().state == State::Connected {
                    return Some(names());
                }
                if stop {
                    return None;
                }
                names();
                std::hint::spin_loop();
            }
        });
        started.recv().unwrap();
        Self { thread, stop }
    }

    /// The tools it read on seeing `Connected`, if it saw it.
    fn finish(self) -> Option<Vec<String>> {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.join().unwrap()
    }
}

/// A plugin with the one server `config`; it starts connecting.
fn plugin(config: ServerConfig) -> McpPlugin {
    McpPlugin::builder()
        .env(Arc::new(|_| None))
        .home(None)
        .server(config)
        .build()
}

/// One live watch, and the resource it watches.
struct Held {
    uri: &'static str,
    watch: ResourceWatch,
}

struct Machine {
    runtime: Runtime,
    fixture: Arc<Fixture>,
    /// The server over streamable HTTP, when the case runs it there;
    /// else over a duplex stream.
    http: Option<Arc<HttpServer>>,
    /// The server's entry, to open the plugin again after it closed.
    config: ServerConfig,
    plugin: McpPlugin,
    connection: Arc<Connection>,
    /// What the connection's watchers see.
    revisions: watch::Receiver<u64>,
    /// What the connection showed after the step before.
    before: Shown,
    /// A step knows the status changed even if it changed back.
    moved: bool,
    /// Whether the session is live: connected and not dropped.
    live: bool,
    /// Whether the live session says when a resource changes.
    session_subscribes: bool,
    /// The tools and resources tau listed last.
    listed: (Vec<String>, Vec<String>),
    /// Watches on the live session.
    held: Vec<Held>,
    /// Watches whose session ended.
    ended: Vec<ResourceWatch>,
    /// How many times `slow` ran, as the calls made it.
    slow_calls: usize,
    /// How many dials one connect makes, over a duplex stream.
    connect_dials: usize,
}

impl Machine {
    fn new(legacy: bool, http: bool) -> Self {
        // HTTP goes over a real loopback socket, which the paused clock
        // would skip ahead of.
        let runtime = if http {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap()
        } else {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .start_paused(true)
                .build()
                .unwrap()
        };
        let fixture = Fixture::subscribable(legacy);
        let http =
            http.then(|| runtime.block_on(HttpServer::start(fixture.clone())));
        let mut config = match &http {
            Some(http) => ServerConfig::new(
                "srv",
                Transport::Http(HttpConfig {
                    url: http.url.clone(),
                    headers: Vec::new(),
                    oauth: None,
                }),
            ),
            None => ServerConfig::new("srv", Transport::Stream(fixture.dial())),
        };
        // Long enough for a loaded machine; a call that waited it out
        // fails the case.
        config.timeout = 10.0;
        fixture.gate(false);
        if let Some(http) = &http {
            http.gate(false);
        }
        let plugin = {
            let _entered = runtime.enter();
            plugin(config.clone())
        };
        let connection = plugin.connections()[0].clone();
        let revisions = connection.watch();
        let before = shown(&connection);
        let mut machine = Self {
            runtime,
            fixture,
            http,
            config,
            plugin,
            connection,
            revisions,
            before,
            moved: false,
            live: false,
            session_subscribes: true,
            listed: (Vec::new(), Vec::new()),
            held: Vec::new(),
            ended: Vec::new(),
            slow_calls: 0,
            connect_dials: 0,
        };
        // The builder started the first connect: the waiter reads from
        // before it, an empty list.
        let waiter = Waiter::start(&machine.plugin);
        machine.gate(true);
        machine.quiet();
        machine.connected(waiter);
        machine.connect_dials = machine.fixture.dials();
        machine
    }

    /// Holds connects back, or lets them through.
    fn gate(&self, open: bool) {
        self.fixture.gate(open);
        if let Some(http) = &self.http {
            http.gate(open);
        }
    }

    fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(future)
    }

    /// Lets what a step started land, before the model knows its
    /// outcome. Over HTTP: until the connection is not connecting, and
    /// a moment more.
    fn quiet(&self) {
        if self.http.is_none() {
            self.settle();
            return;
        }
        self.block_on(async {
            self.connection.settled(&CancellationToken::new()).await;
            tokio::time::sleep(Duration::from_millis(20)).await;
        });
    }

    /// Lets everything spawned land. Over HTTP the clock is the wall's:
    /// it waits for the connection and the server to agree with the
    /// model, or 5 s, whichever comes first.
    fn settle(&self) {
        if self.http.is_none() {
            self.block_on(async { tokio::time::sleep(SETTLE).await });
            return;
        }
        self.block_on(async {
            for _ in 0..1000 {
                if self.agrees() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        });
    }

    /// Whether the connection and the server are as the model has them.
    fn agrees(&self) -> bool {
        let now = shown(&self.connection);
        let state = if self.live {
            State::Connected
        } else {
            State::Disconnected
        };
        now.state == state
            && (now.tools, now.resources) == self.listed
            && (!self.live
                || (self.listed.0 == server_tools(&self.fixture)
                    && self.listed.1 == server_resources(&self.fixture)))
            && self.connection.subscriptions() == self.watched().len()
            && self.fixture.subscribed_uris() == self.watched()
    }

    /// The connect that `waiter` watched is over: it connected, and the
    /// waiter read the server's tools.
    fn connected(&mut self, waiter: Waiter) {
        let status = self.connection.status();
        assert_eq!(status.state, State::Connected, "{status:?}");
        let tools = server_tools(&self.fixture);
        let seen = waiter.finish();
        assert_eq!(
            seen.as_ref(),
            Some(&tools),
            "a waiter that saw Connected read the tools from before"
        );
        self.became_live();
    }

    /// A session came up: tau listed what the server has.
    fn became_live(&mut self) {
        self.live = true;
        self.moved = true;
        self.session_subscribes =
            self.fixture.subscribable.load(Ordering::SeqCst);
        self.listed =
            (server_tools(&self.fixture), server_resources(&self.fixture));
        // A 2026-07-28 server gets its list changes through a listen that
        // rmcp acknowledges before the server's handler holds it: a
        // change made in between reaches no one.
        if !self.fixture.legacy {
            self.block_on(async {
                for _ in 0..500 {
                    if self.fixture.listening_for_tools() == 1 {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                panic!(
                    "the server never took the list listen: {} listens, {} dials, {:?}",
                    self.fixture.listening_for_tools(),
                    self.fixture.dials(),
                    self.connection.status()
                );
            });
        }
    }

    /// The session ended: its watches with it.
    fn lost(&mut self) {
        if self.live {
            self.moved = true;
        }
        self.live = false;
        self.ended
            .extend(self.held.drain(..).map(|held| held.watch));
    }

    /// A step that needs a session connects first, as a call does.
    fn connects(&mut self) {
        if !self.live {
            self.became_live();
        }
    }

    /// Watches `uri`, as the model expects it to go.
    fn watch(&mut self, uri: &'static str, after: Option<Held>) {
        let needs_connect = !self.live;
        let connection = self.connection.clone();
        let watched = self.block_on(async move {
            // Dropped in the runtime, so its unsubscribe goes out, and
            // right before the new watch, so the two race.
            drop(after);
            connection
                .watch_resource(uri, &CancellationToken::new())
                .await
        });
        if needs_connect {
            self.connects();
        }
        if self.session_subscribes {
            let watch = watched.unwrap_or_else(|error| {
                panic!("watching {uri} failed: {error:?}")
            });
            self.held.push(Held { uri, watch });
        } else {
            assert!(
                matches!(&watched, Err(WatchError::Unsupported(_))),
                "a server that cannot subscribe: {watched:?}"
            );
        }
    }

    /// The URIs held watches watch.
    fn watched(&self) -> BTreeSet<String> {
        self.held.iter().map(|held| held.uri.to_owned()).collect()
    }
}

#[hegel::state_machine]
impl Machine {
    /// tau connects a dropped server, a waiter reading its tools.
    #[rule]
    fn connect(&mut self, tc: TestCase) {
        if self.live {
            tc.reject();
        }
        self.gate(false);
        let waiter = Waiter::start(&self.plugin);
        {
            let _entered = self.runtime.enter();
            self.connection.connect();
        }
        self.gate(true);
        self.quiet();
        self.connected(waiter);
    }

    /// The server drops the connection.
    #[rule]
    fn server_drops(&mut self, tc: TestCase) {
        if !self.live || self.http.is_some() {
            tc.reject();
        }
        self.fixture.crash();
        self.quiet();
        self.lost();
    }

    /// The server restarts: the connection drops, its subscriptions are
    /// gone, and from now on it says when a resource changes or not.
    #[rule]
    fn server_restarts(&mut self, tc: TestCase) {
        match &self.http {
            Some(http) => {
                // The spec's 404, or the TypeScript SDK's 400.
                let spec = tc.draw(gs::booleans());
                http.restart(if spec {
                    Unknown::NotFound
                } else {
                    Unknown::BadRequest
                });
            }
            None => {
                let subscribable = tc.draw(gs::booleans());
                self.fixture.set_subscribable(subscribable);
                self.fixture.crash();
                self.quiet();
                self.lost();
            }
        }
    }

    /// tau starts the session again, as after a sign-in.
    #[rule]
    fn tau_restarts_the_session(&mut self, _tc: TestCase) {
        self.gate(false);
        let waiter = Waiter::start(&self.plugin);
        {
            let _entered = self.runtime.enter();
            self.connection.restart();
        }
        self.gate(true);
        self.quiet();
        self.fixture.end_stale();
        self.quiet();
        self.lost();
        self.connected(waiter);
    }

    /// tau starts the session again while a connect is under way, as a
    /// sign-in in the middle of one does: that connect, begun under the
    /// grant before, is dropped, and it connects again.
    #[rule]
    fn tau_restarts_during_a_connect(&mut self, tc: TestCase) {
        if self.live {
            tc.reject();
        }
        let dials = self.fixture.dials();
        self.gate(false);
        let waiter = Waiter::start(&self.plugin);
        {
            let _entered = self.runtime.enter();
            self.connection.connect();
            self.connection.restart();
        }
        self.gate(true);
        self.quiet();
        self.connected(waiter);
        if self.http.is_none() {
            assert_eq!(
                self.fixture.dials(),
                dials + 2 * self.connect_dials,
                "connected again"
            );
        }
    }

    /// The plugin closes the connection, as when its server is turned
    /// off, sometimes while a connect is under way: it stays closed,
    /// whatever that connect brings and whatever a call asks, and its
    /// watches end. The server is then turned on again.
    #[rule]
    fn tau_closes_the_connection(&mut self, tc: TestCase) {
        let during = !self.live && tc.draw(gs::booleans());
        if during {
            self.gate(false);
            let _entered = self.runtime.enter();
            self.connection.connect();
        }
        let connection = self.connection.clone();
        self.block_on(async move { connection.shutdown().await });
        self.gate(true);
        self.quiet();
        let dials = self.fixture.dials();
        let connection = self.connection.clone();
        let called = self.block_on(async move {
            connection
                .call(
                    "slow",
                    json!({"steps": 0, "ms": 0}),
                    &|_| {},
                    &CancellationToken::new(),
                )
                .await
        });
        assert!(called.is_err(), "a closed connection called: {called:?}");
        self.quiet();
        assert_eq!(self.connection.status().state, State::Closed);
        assert_eq!(self.fixture.dials(), dials, "a closed connection dialed");
        self.lost();

        // On again: a new connection, which connects.
        self.gate(false);
        let reopened = {
            let _entered = self.runtime.enter();
            plugin(self.config.clone())
        };
        let closed = std::mem::replace(&mut self.plugin, reopened);
        {
            let _entered = self.runtime.enter();
            drop(closed);
        }
        self.connection = self.plugin.connections()[0].clone();
        self.revisions = self.connection.watch();
        self.before = shown(&self.connection);
        let waiter = Waiter::start(&self.plugin);
        self.gate(true);
        self.quiet();
        self.connected(waiter);
    }

    /// The server adds or removes a tool, and says its list changed.
    #[rule(weight = 2)]
    fn server_changes_its_tools(&mut self, tc: TestCase) {
        let name = tc.draw(gs::sampled_from(TOOLS.to_vec()));
        let fixture = self.fixture.clone();
        if server_tools(&fixture).iter().any(|tool| tool == name) {
            self.block_on(fixture.remove_tool(name));
        } else {
            self.block_on(fixture.add_tool(common::tool(name, "Added later.")));
        }
        self.quiet();
        if self.live {
            self.listed.0 = server_tools(&self.fixture);
        }
    }

    /// The server adds or removes a resource, and says its list changed.
    #[rule(weight = 2)]
    fn server_changes_its_resources(&mut self, tc: TestCase) {
        let uri = tc.draw(gs::sampled_from(RESOURCES.to_vec()));
        let fixture = self.fixture.clone();
        if server_resources(&fixture).iter().any(|r| r == uri) {
            self.block_on(fixture.remove_resource(uri));
        } else {
            self.block_on(
                fixture.add_resource(json!({"uri": uri, "name": uri})),
            );
        }
        self.quiet();
        if self.live {
            self.listed.1 = server_resources(&self.fixture);
        }
    }

    /// Calls side by side, each sending some progress updates right
    /// before its result.
    #[rule(weight = 3)]
    fn calls(&mut self, tc: TestCase) {
        let steps: Vec<u64> = tc.draw(
            gs::vecs(gs::integers::<u64>().max_value(8))
                .min_size(1)
                .max_size(3),
        );
        let connection = self.connection.clone();
        let results = self.block_on(futures_util::future::join_all(
            steps.iter().map(|&steps| {
                let connection = connection.clone();
                async move {
                    let seen = Arc::new(Mutex::new(Vec::new()));
                    let record = {
                        let seen = seen.clone();
                        move |progress: Progress| {
                            seen.lock().unwrap().push(progress.progress)
                        }
                    };
                    let result = connection
                        .call(
                            "slow",
                            json!({"steps": steps, "ms": 0}),
                            &record,
                            &CancellationToken::new(),
                        )
                        .await;
                    // What reached the call by the time it returned.
                    let seen = seen.lock().unwrap().clone();
                    (result, seen)
                }
            }),
        ));
        self.connects();
        self.slow_calls += steps.len();
        for (steps, (result, seen)) in steps.into_iter().zip(results) {
            let result: Value =
                result.unwrap_or_else(|error| panic!("a call failed: {error}"));
            assert_eq!(result["content"][0]["text"], json!("done"));
            let sent: Vec<f64> = (1..=steps).map(|step| step as f64).collect();
            assert_eq!(seen, sent, "the progress a call got before its result");
        }
    }

    /// tau watches a resource.
    #[rule(weight = 2)]
    fn watch_a_resource(&mut self, tc: TestCase) {
        let uri = tc.draw(gs::sampled_from(WATCHED.to_vec()));
        self.watch(uri, None);
    }

    /// A watch is dropped.
    #[rule(weight = 2)]
    fn unwatch(&mut self, tc: TestCase) {
        if self.held.is_empty() {
            tc.reject();
        }
        let at = tc.draw(gs::integers().max_value(self.held.len() - 1));
        let held = self.held.remove(at);
        self.block_on(async move { drop(held) });
    }

    /// A watch is dropped and its resource watched again at once.
    #[rule(weight = 2)]
    fn unwatch_then_watch(&mut self, tc: TestCase) {
        if self.held.is_empty() {
            tc.reject();
        }
        let at = tc.draw(gs::integers().max_value(self.held.len() - 1));
        let held = self.held.remove(at);
        self.watch(held.uri, Some(held));
    }

    #[invariant(always_run)]
    fn the_connection_is_the_model(&mut self, _tc: TestCase) {
        self.settle();
        let now = shown(&self.connection);
        let want = if self.live {
            State::Connected
        } else {
            State::Disconnected
        };
        assert_eq!(now.state, want, "{:?}", self.connection.status());
        assert_eq!(
            (&now.tools, &now.resources),
            (&self.listed.0, &self.listed.1),
            "the lists tau shows"
        );
        if self.live {
            assert_eq!(self.listed.0, server_tools(&self.fixture));
            assert_eq!(self.listed.1, server_resources(&self.fixture));
        }
        assert_eq!(self.fixture.calls("slow"), self.slow_calls, "runs of slow");
    }

    #[invariant(always_run)]
    fn subscriptions_are_the_servers(&mut self, tc: TestCase) {
        let watched = self.watched();
        assert_eq!(self.connection.subscriptions(), watched.len());
        assert_eq!(
            self.fixture.subscribed_uris(),
            watched,
            "what the server has subscribed"
        );
        let ended = std::mem::take(&mut self.ended);
        self.ended = self.block_on(async move {
            let mut ended = ended;
            for watch in &mut ended {
                // What it was told before it ended comes first: the
                // read again after a renewal.
                while tokio::time::timeout(ENDING, watch.changed())
                    .await
                    .expect("an ended watch says so")
                {}
            }
            ended
        });
        tc.event_value("watches", self.held.len() as f64);
    }

    #[invariant(always_run)]
    fn watchers_see_every_change(&mut self, _tc: TestCase) {
        let now = shown(&self.connection);
        if now != self.before || self.moved {
            assert!(
                self.revisions.has_changed().unwrap(),
                "the connection changed from {:?} to {now:?} and told no one",
                self.before
            );
        }
        self.revisions.borrow_and_update();
        self.before = now;
        self.moved = false;
    }
}

/// Over a duplex stream, on the paused clock: see the module docs.
#[hegel::test(
    test_cases = 100,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn a_connection_keeps_to_its_server(tc: TestCase) {
    let legacy = tc.draw(gs::booleans());
    hegel::stateful::machine(Machine::new(legacy, false))
        .steps(30)
        .run(tc);
}

#[hegel::test(
    profile = "nightly_slow",
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
#[ignore = "nightly"]
fn a_connection_keeps_to_its_server_nightly(tc: TestCase) {
    let legacy = tc.draw(gs::booleans());
    hegel::stateful::machine(Machine::new(legacy, false))
        .steps(60)
        .run(tc);
}

/// Over streamable HTTP on a loopback port, on the wall's clock: a
/// restart forgets the sessions, answered then with 404 or with 400.
#[hegel::test(
    test_cases = 20,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn a_connection_keeps_to_its_server_over_http(tc: TestCase) {
    let legacy = tc.draw(gs::booleans());
    hegel::stateful::machine(Machine::new(legacy, true))
        .steps(20)
        .run(tc);
}

/// Fewer cases than the profile's: each takes seconds of the wall's
/// clock, and 80 take about ten minutes.
#[hegel::test(
    profile = "nightly_slow",
    test_cases = 80,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
#[ignore = "nightly"]
fn a_connection_keeps_to_its_server_over_http_nightly(tc: TestCase) {
    let legacy = tc.draw(gs::booleans());
    hegel::stateful::machine(Machine::new(legacy, true))
        .steps(40)
        .run(tc);
}

/// A connection to `server` over HTTP, connected.
async fn over_http(server: &HttpServer) -> Arc<Connection> {
    let mut config = ServerConfig::new(
        "srv",
        Transport::Http(HttpConfig {
            url: server.url.clone(),
            headers: Vec::new(),
            oauth: None,
        }),
    );
    config.timeout = 10.0;
    let environment = Environment {
        env: Arc::new(|_| None),
        home: None,
        repo: None,
        auth: None,
        launcher: Default::default(),
        refused: Default::default(),
    };
    let connection = Connection::new(config, Origin::User, environment);
    connection.connect();
    connection.settled(&CancellationToken::new()).await;
    assert_eq!(connection.status().state, State::Connected);
    connection
}

/// Whether `check` holds within 5 s of the wall's clock.
async fn within_5s(mut check: impl FnMut() -> bool) -> bool {
    for _ in 0..500 {
        if check() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

/// A call made right after an HTTP server restarted reaches it once.
async fn call_slow(connection: &Arc<Connection>) {
    connection
        .call(
            "slow",
            json!({"steps": 1, "ms": 0}),
            &|_: Progress| {},
            &CancellationToken::new(),
        )
        .await
        .unwrap();
}

/// Shrunk from the model over HTTP: a tool the server added after it
/// restarted is listed. On 2025-11-25 rmcp starts a new session on the
/// next request and posts it again, and the change the server said
/// before that reached no one; on 2026-07-28 the restart ends tau's
/// `subscriptions/listen`. Neither was listed again, nor listened for.
async fn a_tool_added_after_an_http_restart_is_listed(legacy: bool) {
    let fixture = Fixture::subscribable(legacy);
    let server = HttpServer::start(fixture.clone()).await;
    let connection = over_http(&server).await;
    server.restart(Unknown::NotFound);
    fixture.add_tool(common::tool("beta", "Added later.")).await;
    // The next call works: rmcp takes a new session.
    call_slow(&connection).await;
    let listed = within_5s(|| connection.offers("beta")).await;
    assert!(listed, "{:?}", shown(&connection).tools);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tool_added_after_an_http_restart_is_listed_2025() {
    a_tool_added_after_an_http_restart_is_listed(true).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tool_added_after_an_http_restart_is_listed_2026() {
    a_tool_added_after_an_http_restart_is_listed(false).await;
}

/// Shrunk from the model over HTTP: a watch taken before the server
/// restarted is subscribed again, and its watcher does nothing. It used
/// to go on waiting for changes the restarted server, which forgot the
/// subscription, never sent, so a trigger on it never fired again.
async fn a_watch_survives_an_http_restart(legacy: bool) {
    let fixture = Fixture::subscribable(legacy);
    let server = HttpServer::start(fixture.clone()).await;
    let connection = over_http(&server).await;
    let mut watch = connection
        .watch_resource("file:///status", &CancellationToken::new())
        .await
        .unwrap();
    assert!(
        within_5s(|| fixture.subscribed_uris().contains("file:///status"))
            .await
    );
    server.restart(Unknown::BadRequest);
    call_slow(&connection).await;
    let subscribed =
        within_5s(|| fixture.subscribed_uris().contains("file:///status"))
            .await;
    assert!(
        subscribed,
        "tau counts {} subscriptions, the server has {:?}",
        connection.subscriptions(),
        fixture.subscribed_uris()
    );
    // Subscribed again, it is told to read again.
    let told =
        tokio::time::timeout(Duration::from_secs(5), watch.changed()).await;
    assert_eq!(told, Ok(true), "the watch was told to read again");
    // The same watch hears the restarted server's changes.
    fixture.set_status("red").await;
    let heard =
        tokio::time::timeout(Duration::from_secs(5), watch.changed()).await;
    assert_eq!(heard, Ok(true), "the watch heard the change");
}

/// A change a restarted server made before tau subscribed again reached
/// no one: its watch is told to read again all the same.
async fn a_change_before_tau_subscribes_again_is_not_missed(legacy: bool) {
    let fixture = Fixture::subscribable(legacy);
    let server = HttpServer::start(fixture.clone()).await;
    let connection = over_http(&server).await;
    let mut watch = connection
        .watch_resource("file:///status", &CancellationToken::new())
        .await
        .unwrap();
    assert!(
        within_5s(|| fixture.subscribed_uris().contains("file:///status"))
            .await
    );
    server.restart(Unknown::NotFound);
    fixture.set_status("red").await;
    call_slow(&connection).await;
    let told =
        tokio::time::timeout(Duration::from_secs(5), watch.changed()).await;
    assert_eq!(told, Ok(true), "the watch was told to read again");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_change_before_tau_subscribes_again_is_not_missed_2025() {
    a_change_before_tau_subscribes_again_is_not_missed(true).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_change_before_tau_subscribes_again_is_not_missed_2026() {
    a_change_before_tau_subscribes_again_is_not_missed(false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_watch_survives_an_http_restart_2025() {
    a_watch_survives_an_http_restart(true).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_watch_survives_an_http_restart_2026() {
    a_watch_survives_an_http_restart(false).await;
}
