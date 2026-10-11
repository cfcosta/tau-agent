//! An agent's events, from what wakes it to the turn that hears them
//! (ADR 0037): schedules that come due and catch up, commands, the
//! coalescing window, a turn that is going, the daily cap, and making
//! an agent.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    collections::BTreeMap,
    path::Path,
    sync::{
        Arc,
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use serde_json::json;
use tau_agents::{Act, Settings, Status, event::Message};
use tau_agents_host::{Agents, clock::FakeClock, env::Env};
use tau_testing::block_on_io;
use tau_ui_plugin::{By, Conversations, Turn, registry::HostFuture};
use tau_vcs_host::{Identity, ProjectRepo};

/// 2026-10-12T08:00:00Z, a Monday, in UTC.
const MONDAY_EIGHT: i64 = 1_791_792_000;

const AGENT: &str = r#"
local tau = require("tau")
return tau.agent {
  name = "greet",
  triggers = {
    hourly = { every = "1h" },
    morning = { at = "09:00" },
    hello = { command = "hello", hint = "<who>" },
  },
}
"#;

#[derive(Default)]
struct Conversation {
    running: AtomicBool,
    refuse: AtomicBool,
    listed: Mutex<Vec<String>>,
    turns: Mutex<Vec<Turn>>,
}

impl Conversations for Conversation {
    fn list_agent(&self, name: &str) -> HostFuture<'_, anyhow::Result<()>> {
        self.listed.lock().unwrap().push(name.to_owned());
        Box::pin(async { Ok(()) })
    }

    fn running(&self, _repo: &str) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    fn turn(&self, turn: Turn) -> HostFuture<'_, anyhow::Result<()>> {
        let refused = self.refuse.load(Ordering::SeqCst);
        if !refused {
            self.turns.lock().unwrap().push(turn);
        }
        Box::pin(async move {
            if refused {
                anyhow::bail!("the run is still going");
            }
            Ok(())
        })
    }
}

#[derive(Default)]
struct Host {
    conversation: Arc<Conversation>,
    settings: Mutex<BTreeMap<String, Settings>>,
    costs: Mutex<BTreeMap<String, f64>>,
}

impl Env for Host {
    fn refresh(&self) {}

    fn settings(&self, agent: &str) -> Settings {
        self.settings
            .lock()
            .unwrap()
            .get(agent)
            .cloned()
            .unwrap_or_default()
    }

    fn save_settings(
        &self,
        agent: &str,
        settings: Settings,
    ) -> HostFuture<'_, anyhow::Result<()>> {
        self.settings
            .lock()
            .unwrap()
            .insert(agent.to_owned(), settings);
        Box::pin(async { Ok(()) })
    }

    fn costs(
        &self,
        _agent: &str,
    ) -> HostFuture<'_, anyhow::Result<BTreeMap<String, f64>>> {
        let costs = self.costs.lock().unwrap().clone();
        Box::pin(async move { Ok(costs) })
    }

    fn conversations(&self) -> Option<Arc<dyn Conversations>> {
        Some(self.conversation.clone())
    }
}

struct Fixture {
    _home: tempfile::TempDir,
    clock: Arc<FakeClock>,
    host: Arc<Host>,
    agents: Agents,
}

impl Fixture {
    /// The agent `greet` with [`AGENT`], over a fresh host.
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        ProjectRepo::init(
            home.path().join("agents").join("greet"),
            Identity::default(),
            &[("agent.luau".to_owned(), AGENT.to_owned())],
        )
        .unwrap();
        let clock = Arc::new(FakeClock::new(MONDAY_EIGHT, 0));
        let host = Arc::new(Host::default());
        let agents = open(home.path(), &clock, &host);
        block_on_io(agents.registry().scan());
        Self {
            _home: home,
            clock,
            host,
            agents,
        }
    }

    /// The same agents as tau opened again.
    fn restarted(&self) -> Agents {
        let agents = open(self._home.path(), &self.clock, &self.host);
        block_on_io(agents.registry().scan());
        agents
    }

    fn tick(&self) {
        block_on_io(self.agents.tick());
    }

    fn turns(&self) -> Vec<Turn> {
        self.host.conversation.turns.lock().unwrap().clone()
    }

    fn agent(&self) -> tau_agents::Agent {
        block_on_io(self.agents.overview()).agents[0].clone()
    }

    fn command(&self, who: &str) {
        block_on_io(self.agents.act(Act::Command {
            agent: "greet".into(),
            command: "hello".into(),
            args: who.into(),
            from: Some("run-1".into()),
        }))
        .unwrap();
    }
}

fn open(home: &Path, clock: &Arc<FakeClock>, host: &Arc<Host>) -> Agents {
    Agents::at(
        home.join("agents"),
        home.join("state"),
        host.clone(),
        clock.clone(),
    )
}

/// What a turn's message says, read back as the card reads it.
fn heard(turn: &Turn) -> Vec<(String, String)> {
    Message::parse(&turn.text)
        .expect("an event message")
        .events
        .into_iter()
        .map(|event| (event.trigger, event.data))
        .collect()
}

#[test]
fn a_schedule_fires_and_its_turn_hears_it_after_the_window() {
    let f = Fixture::new();
    f.tick();
    let agent = f.agent();
    assert_eq!(agent.status, Status::Waiting);
    let next = |name: &str| {
        f.agent()
            .triggers
            .iter()
            .find(|trigger| trigger.name == name)
            .unwrap()
            .next
    };
    assert_eq!(next("hourly"), Some(MONDAY_EIGHT + 3600));
    assert_eq!(next("morning"), Some(MONDAY_EIGHT + 3600));
    assert_eq!(next("hello"), None);

    // Both come due at 09:00; they wait the window, then one turn
    // hears both, as tau's.
    f.clock.set(MONDAY_EIGHT + 3600);
    f.tick();
    assert_eq!(f.agent().queued, 2);
    assert!(f.turns().is_empty());
    f.clock.advance(30);
    f.tick();
    let turns = f.turns();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].by, By::Tau);
    assert_eq!(turns[0].repo, "greet");
    assert_eq!(turns[0].model, tau_agents::default_model());
    assert_eq!(turns[0].effort, "low");
    let due = (MONDAY_EIGHT + 3600).to_string();
    assert_eq!(
        heard(&turns[0]),
        [
            ("hourly".to_owned(), format!("{{\"due\":{due}}}")),
            ("morning".to_owned(), format!("{{\"due\":{due}}}")),
        ]
    );
    let agent = f.agent();
    assert_eq!(agent.queued, 0);
    assert_eq!(agent.last.unwrap().line, "heard 2 events: hourly, morning");
    assert_eq!(next("hourly"), Some(MONDAY_EIGHT + 2 * 3600));
    assert_eq!(next("morning"), Some(MONDAY_EIGHT + 3600 + 86_400));
}

#[test]
fn a_schedule_missed_while_tau_was_closed_fires_once() {
    let f = Fixture::new();
    f.tick();
    // tau closes for five hours and a half.
    f.clock.set(MONDAY_EIGHT + 5 * 3600 + 1800);
    let again = f.restarted();
    block_on_io(again.tick());
    let overview = block_on_io(again.overview());
    assert_eq!(overview.agents[0].queued, 2, "hourly and morning, once each");
    f.clock.advance(30);
    block_on_io(again.tick());
    let turns = f.turns();
    assert_eq!(turns.len(), 1);
    let due = (MONDAY_EIGHT + 3600).to_string();
    assert_eq!(
        heard(&turns[0]),
        [
            ("hourly".to_owned(), format!("{{\"due\":{due}}}")),
            ("morning".to_owned(), format!("{{\"due\":{due}}}")),
        ],
        "each with the first time it missed"
    );
}

#[test]
fn commands_wait_for_the_window_and_a_turn_that_is_going() {
    let f = Fixture::new();
    f.tick();
    f.command("ada");
    f.clock.advance(10);
    f.command("grace");
    f.clock.advance(19);
    f.tick();
    assert!(f.turns().is_empty(), "the oldest waited 29 s");

    // A turn of it is going: they wait for the next.
    f.host.conversation.running.store(true, Ordering::SeqCst);
    f.clock.advance(1);
    f.tick();
    assert!(f.turns().is_empty());
    assert_eq!(f.agent().status, Status::Working);

    f.host.conversation.running.store(false, Ordering::SeqCst);
    f.tick();
    let turns = f.turns();
    assert_eq!(turns.len(), 1);
    assert_eq!(
        heard(&turns[0]),
        [
            (
                "hello".to_owned(),
                json!({ "args": "ada", "from": "run-1" }).to_string()
            ),
            (
                "hello".to_owned(),
                json!({ "args": "grace", "from": "run-1" }).to_string()
            ),
        ]
    );

    // A command it does not have is refused.
    let error = block_on_io(f.agents.act(Act::Command {
        agent: "greet".into(),
        command: "bye".into(),
        args: String::new(),
        from: None,
    }))
    .unwrap_err();
    assert!(error.to_string().contains("no command /bye"), "{error}");
}

#[test]
fn a_turn_that_does_not_start_keeps_its_events_and_tries_again() {
    let f = Fixture::new();
    f.tick();
    f.command("ada");
    f.host.conversation.refuse.store(true, Ordering::SeqCst);
    f.clock.advance(30);
    f.tick();
    assert_eq!(f.agent().queued, 1);
    f.host.conversation.refuse.store(false, Ordering::SeqCst);
    f.clock.advance(10);
    f.tick();
    assert!(f.turns().is_empty(), "it waits before trying again");
    f.clock.advance(20);
    f.tick();
    assert_eq!(f.turns().len(), 1);
    assert_eq!(f.agent().queued, 0);
}

#[test]
fn at_its_cap_it_waits_until_the_person_allows_more() {
    let f = Fixture::new();
    f.host.costs.lock().unwrap().insert("run-1".into(), 4.0);
    // The day's first count is where its spend starts.
    f.tick();
    assert_eq!(f.agent().spend_today, 0.0);
    f.host.costs.lock().unwrap().insert("run-1".into(), 5.25);
    f.command("ada");
    f.clock.advance(30);
    f.tick();
    assert!(f.turns().is_empty());
    let agent = f.agent();
    assert_eq!(agent.status, Status::AtCap);
    assert_eq!(agent.spend_today, 1.25);
    assert_eq!(agent.cap_today, 1.0);
    assert_eq!(agent.queued, 1, "events keep waiting");

    block_on_io(f.agents.act(Act::AllowMore {
        agent: "greet".into(),
        amount: 1.0,
    }))
    .unwrap();
    assert_eq!(f.agent().cap_today, 2.0);
    f.tick();
    assert_eq!(f.turns().len(), 1);

    // A new day: its spend starts over, and so does its cap.
    f.clock.advance(86_400);
    f.tick();
    let agent = f.agent();
    assert_eq!(agent.spend_today, 0.0);
    assert_eq!(agent.cap_today, 1.0);
}

#[test]
fn a_paused_agent_takes_no_turn() {
    let f = Fixture::new();
    f.tick();
    block_on_io(f.agents.act(Act::Pause {
        agent: "greet".into(),
    }))
    .unwrap();
    f.command("ada");
    f.clock.advance(60);
    f.tick();
    assert!(f.turns().is_empty());
    assert_eq!(f.agent().status, Status::Paused);
    block_on_io(f.agents.act(Act::Resume {
        agent: "greet".into(),
    }))
    .unwrap();
    f.tick();
    assert_eq!(f.turns().len(), 1);
}

#[test]
fn making_an_agent_lists_it_and_starts_its_conversation() {
    let f = Fixture::new();
    let answer = block_on_io(f.agents.act(Act::Create {
        name: "pr-triage".into(),
        purpose: "Label new pull requests.".into(),
        model: None,
        daily_cap: Some(2.5),
        window_secs: None,
        servers: vec!["github".into()],
    }))
    .unwrap();
    assert_eq!(answer, Some(json!({ "repo": "pr-triage" })));
    assert_eq!(
        *f.host.conversation.listed.lock().unwrap(),
        ["pr-triage"]
    );
    let settings = f.host.settings("pr-triage");
    assert_eq!(settings.daily_cap, 2.5);
    assert_eq!(settings.window_secs, 30);
    let turns = f.turns();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].by, By::Person);
    assert_eq!(turns[0].repo, "pr-triage");
    assert!(
        turns[0]
            .text
            .starts_with("/tau-agents Label new pull requests."),
        "{}",
        turns[0].text
    );
    assert!(turns[0].text.contains("github"), "{}", turns[0].text);

    let refused = block_on_io(f.agents.act(Act::Create {
        name: "Not An Agent".into(),
        purpose: "x".into(),
        model: None,
        daily_cap: None,
        window_secs: None,
        servers: Vec::new(),
    }));
    assert!(refused.is_err());
}

#[test]
fn a_version_that_fails_is_heard_as_an_event() {
    let f = Fixture::new();
    f.tick();
    // The agent commits a version that does not load.
    let home = f._home.path();
    let broken = ProjectRepo::init(
        home.join("v2"),
        Identity::default(),
        &[("agent.luau".to_owned(), "return tau.agent {".to_owned())],
    )
    .unwrap();
    let trunk = broken.trunk().unwrap();
    block_on_io(f.agents.registry().reload(
        "greet",
        &tau_vcs_host::Project::from(broken),
        &trunk,
    ));
    f.tick();
    let agent = f.agent();
    assert_eq!(agent.status, Status::Waiting, "the version before is live");
    assert_eq!(agent.queued, 1);
    f.clock.advance(30);
    f.tick();
    let turns = f.turns();
    assert_eq!(turns.len(), 1);
    let message = Message::parse(&turns[0].text).unwrap();
    assert_eq!(message.events[0].kind, tau_agents::event::Kind::Version);
    assert_eq!(message.events[0].trigger, "agent.luau");
}
