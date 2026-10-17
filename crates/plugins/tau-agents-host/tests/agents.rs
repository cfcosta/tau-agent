//! An agent's events, from what wakes it to the turn that hears them
//! (ADR 0037): schedules that come due and catch up, commands, the
//! coalescing window, a turn that is going, the daily cap, making an
//! agent, and polls.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    collections::{BTreeMap, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc,
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use serde_json::{Value, json};
use tau_agents::{Act, Settings, Status, event::Message};
use tau_agents_host::{Agents, clock::FakeClock, env::Env};
use tau_testing::block_on_io;
use tau_tools_host::secrets::SecretStore;
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
    releases = {
      poll = { http = { url = "https://example.com/releases",
                        headers = { authorization = "Bearer " .. tau.secret("github") } } },
      every = "1h",
      key = "id",
    },
    pulls = {
      poll = { tool = "mcp__github__list_pull_requests", args = { state = "open" } },
      every = "10m",
      key = "number",
    },
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

    fn main_of(&self, repo: &str) -> Option<String> {
        Some(format!("main-of-{repo}"))
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
    secrets: std::sync::OnceLock<SecretStore>,
    settings: Mutex<BTreeMap<String, Settings>>,
    /// What the agent's repository was charged, and when.
    charges: Mutex<Vec<(i64, f64)>>,
    /// What each tool answers next, in turn; none is a failure.
    answers: Mutex<BTreeMap<String, VecDeque<Result<Value, String>>>>,
    /// The tools called outside a run: where, which, with what.
    called: Mutex<Vec<(PathBuf, String, Value)>>,
    /// The artifact grants stored, by run.
    grants: Mutex<Vec<(String, Value)>>,
    /// Whether requests go out; else they fail, offline.
    online: AtomicBool,
}

impl Host {
    fn answer(&self, tool: &str, answer: Result<Value, String>) {
        self.answers
            .lock()
            .unwrap()
            .entry(tool.to_owned())
            .or_default()
            .push_back(answer);
    }
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

    fn spent_since(
        &self,
        _agent: &str,
        since: i64,
    ) -> HostFuture<'_, anyhow::Result<f64>> {
        let spent = self
            .charges
            .lock()
            .unwrap()
            .iter()
            .filter(|(at, _)| *at >= since)
            .map(|(_, usd)| usd)
            .sum();
        Box::pin(async move { Ok(spent) })
    }

    fn conversations(&self) -> Option<Arc<dyn Conversations>> {
        Some(self.conversation.clone())
    }

    fn grant<'a>(
        &'a self,
        run: &'a str,
        record: Value,
    ) -> HostFuture<'a, anyhow::Result<()>> {
        self.grants.lock().unwrap().push((run.to_owned(), record));
        Box::pin(async { Ok(()) })
    }

    fn secrets(&self, agent: &str) -> SecretStore {
        let store = self.secrets.get().expect("the fixture sets it").clone();
        SecretStore::of_agent(
            store.path().parent().expect("a directory"),
            agent,
        )
    }

    fn call_tool<'a>(
        &'a self,
        checkout: &'a Path,
        name: &'a str,
        args: Value,
    ) -> HostFuture<'a, Result<Value, String>> {
        self.called.lock().unwrap().push((
            checkout.to_owned(),
            name.to_owned(),
            args,
        ));
        let answer = self
            .answers
            .lock()
            .unwrap()
            .get_mut(name)
            .and_then(VecDeque::pop_front)
            .unwrap_or_else(|| Err(format!("{name} has no answer")));
        Box::pin(async move { answer })
    }

    fn fetch<'a>(
        &'a self,
        agent: &'a str,
        spec: Value,
    ) -> HostFuture<'a, Result<Value, String>> {
        Box::pin(async move {
            if !self.online.load(Ordering::SeqCst) {
                return Err("offline".into());
            }
            tau_agents_host::poll::fetch(&self.secrets(agent), &spec).await
        })
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
        Self::with(AGENT)
    }

    /// The agent `greet` with `source` as its `agent.luau`.
    fn with(source: &str) -> Self {
        let home = tempfile::tempdir().unwrap();
        ProjectRepo::init(
            home.path().join("agents").join("greet"),
            Identity::default(),
            &[("agent.luau".to_owned(), source.to_owned())],
        )
        .unwrap();
        let clock = Arc::new(FakeClock::new(MONDAY_EIGHT, 0));
        let host = Arc::new(Host::default());
        host.secrets
            .set(SecretStore::in_dir(&home.path().join("config")))
            .unwrap();
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

    /// A tick, and the polls it started, to their end.
    fn tick(&self) {
        block_on_io(async {
            self.agents.tick().await;
            self.agents.polls_settled().await;
        });
    }

    /// The trigger `name` as the agent's page shows it.
    fn trigger(&self, name: &str) -> tau_agents::Trigger {
        self.agent()
            .triggers
            .into_iter()
            .find(|trigger| trigger.name == name)
            .unwrap()
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
    assert_eq!(agent.main.as_deref(), Some("main-of-greet"));
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
    assert_eq!(
        overview.agents[0].queued, 2,
        "hourly and morning, once each"
    );
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
    // What it spent before the day began does not count.
    f.host
        .charges
        .lock()
        .unwrap()
        .push((MONDAY_EIGHT - 9 * 3600, 4.0));
    f.tick();
    assert_eq!(f.agent().spend_today, 0.0);
    f.host.charges.lock().unwrap().push((MONDAY_EIGHT, 1.25));
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
    assert_eq!(*f.host.conversation.listed.lock().unwrap(), ["pr-triage"]);
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

#[test]
fn an_agent_names_its_secrets_and_the_person_fills_them() {
    let f = Fixture::new();
    let names = |f: &Fixture| -> Vec<(String, bool)> {
        f.agent()
            .secrets
            .into_iter()
            .map(|secret| (secret.name, secret.set))
            .collect()
    };
    assert_eq!(names(&f), [("github".to_owned(), false)]);
    // Another agent's key of the same name is not this one's.
    block_on_io(f.agents.act(Act::SetSecret {
        agent: "other".into(),
        name: "github".into(),
        value: "another-agents-value".into(),
    }))
    .unwrap();
    assert_eq!(names(&f), [("github".to_owned(), false)]);
    block_on_io(f.agents.act(Act::SetSecret {
        agent: "greet".into(),
        name: "github".into(),
        value: "a-value-never-shown".into(),
    }))
    .unwrap();
    assert_eq!(names(&f), [("github".to_owned(), true)]);
    // The overview holds names, never values.
    let shown = serde_json::to_string(&f.agent()).unwrap();
    assert!(!shown.contains("a-value-never-shown"));
    block_on_io(f.agents.act(Act::RemoveSecret {
        agent: "greet".into(),
        name: "github".into(),
    }))
    .unwrap();
    assert_eq!(names(&f), [("github".to_owned(), false)]);
    assert!(
        block_on_io(f.agents.act(Act::SetSecret {
            agent: "greet".into(),
            name: "not a name".into(),
            value: "x".into(),
        }))
        .is_err()
    );
    // The other agent's key is still its own.
    let other = block_on_io(f.host.secrets("other").read()).unwrap();
    assert_eq!(
        other.get("github").map(String::as_str),
        Some("another-agents-value")
    );
}

/// A tick of `agents`, and the polls it started, to their end.
fn tick_on(agents: &Agents) {
    block_on_io(async {
        agents.tick().await;
        agents.polls_settled().await;
    });
}

fn pulls(numbers: &[u64]) -> Result<Value, String> {
    Ok(numbers.iter().map(|n| json!({ "number": n })).collect())
}

#[test]
fn a_poll_records_what_is_there_then_fires_what_is_new() {
    let f = Fixture::new();
    let tool = "mcp__github__list_pull_requests";
    f.host.answer(tool, pulls(&[1, 2]));
    f.tick();
    // The first poll ran at once, with its args, in the agent's checkout;
    // it only recorded what is there.
    let called = f.host.called.lock().unwrap().clone();
    assert_eq!(called.len(), 1);
    assert!(called[0].0.ends_with("agents/greet"), "{:?}", called[0].0);
    assert_eq!(called[0].1, tool);
    assert_eq!(called[0].2, json!({ "state": "open" }));
    assert_eq!(f.agent().queued, 0);
    assert_eq!(f.trigger("pulls").next, Some(MONDAY_EIGHT + 600));
    assert_eq!(f.trigger("pulls").error, None);

    // Not due yet: no call.
    f.clock.advance(300);
    f.tick();
    assert_eq!(f.host.called.lock().unwrap().len(), 1);

    // Due: what is new fires, once.
    f.clock.set(MONDAY_EIGHT + 600);
    f.host.answer(tool, pulls(&[1, 2, 3]));
    f.tick();
    assert_eq!(f.agent().queued, 1);
    assert_eq!(f.trigger("pulls").last, Some(MONDAY_EIGHT + 600));
    f.clock.advance(30);
    f.tick();
    let turns = f.turns();
    assert_eq!(turns.len(), 1);
    assert_eq!(
        heard(&turns[0]),
        [("pulls".to_owned(), "{\"number\":3}".to_owned())]
    );
    assert!(
        turns[0].text.contains("pulls · poll · pulls:3"),
        "{}",
        turns[0].text
    );

    // Seen already: nothing more.
    f.clock.set(MONDAY_EIGHT + 1200);
    f.host.answer(tool, pulls(&[2, 3]));
    f.tick();
    assert_eq!(f.agent().queued, 0);
}

#[test]
fn every_poll_runs_at_once_when_tau_starts_and_keeps_what_it_saw() {
    let f = Fixture::new();
    let tool = "mcp__github__list_pull_requests";
    f.host.answer(tool, pulls(&[1]));
    f.tick();
    assert_eq!(f.trigger("pulls").next, Some(MONDAY_EIGHT + 600));

    // tau starts again two minutes on: the poll is not due, but runs.
    f.clock.advance(120);
    let again = f.restarted();
    f.host.answer(tool, pulls(&[1, 2]));
    tick_on(&again);
    assert_eq!(f.host.called.lock().unwrap().len(), 2);
    // What it saw before tau closed is still seen: only 2 is new.
    let agent = block_on_io(again.overview()).agents[0].clone();
    assert_eq!(agent.queued, 1);
}

#[test]
fn a_poll_that_fails_says_why_and_waits_longer_after_three() {
    let f = Fixture::new();
    let tool = "mcp__github__list_pull_requests";
    f.host.answer(tool, Err("the server is down".into()));
    f.tick();
    let trigger = f.trigger("pulls");
    assert_eq!(trigger.error.as_deref(), Some("the server is down"));
    assert_eq!(trigger.next, Some(MONDAY_EIGHT + 600));
    for n in 1..=2 {
        f.clock.set(MONDAY_EIGHT + 600 * n);
        f.tick();
    }
    // Three failures in a row: four intervals, not one.
    assert_eq!(f.trigger("pulls").next, Some(MONDAY_EIGHT + 1200 + 2400));
    assert_eq!(f.agent().queued, 0);
    // One that works starts over, and only records: what waits is the
    // two schedules due at 09:00 too.
    f.clock.set(MONDAY_EIGHT + 3600);
    f.host.answer(tool, pulls(&[7]));
    f.tick();
    let trigger = f.trigger("pulls");
    assert_eq!(trigger.error, None);
    assert_eq!(trigger.next, Some(MONDAY_EIGHT + 3600 + 600));
    assert_eq!(f.agent().queued, 2);
    assert_eq!(f.trigger("pulls").last, None);
}

/// A poll whose `key` fails on one of its items fires none of them, not
/// even the new ones whose keys it read, and says why; what it saw before
/// is still seen.
#[test]
fn a_poll_whose_key_fails_fires_nothing_and_says_why() {
    let source = r#"
local tau = require("tau")
return tau.agent {
  name = "greet",
  triggers = {
    pulls = {
      poll = { tool = "mcp__github__list_pull_requests" },
      every = "10m",
      key = function(pr)
        if pr.number == 2 then error("no key here") end
        return pr.number
      end,
    },
  },
}
"#;
    let f = Fixture::with(source);
    let tool = "mcp__github__list_pull_requests";
    f.host.answer(tool, pulls(&[1]));
    f.tick();
    assert_eq!(f.trigger("pulls").error, None);
    // Due: 3 is new and has a key, but 2's fails.
    f.clock.set(MONDAY_EIGHT + 600);
    f.host.answer(tool, pulls(&[1, 3, 2]));
    f.tick();
    let error = f.trigger("pulls").error.unwrap();
    assert!(error.contains("no key here"), "{error}");
    assert_eq!(f.agent().queued, 0);
    f.clock.advance(30);
    f.tick();
    assert!(f.turns().is_empty());
    // Once every key reads, what is new since fires, and only that.
    f.clock.set(MONDAY_EIGHT + 1200);
    f.host.answer(tool, pulls(&[1, 3]));
    f.tick();
    assert_eq!(f.trigger("pulls").error, None);
    assert_eq!(f.agent().queued, 1);
}

/// A server on 127.0.0.1 that answers each request with `status` and
/// `body`, and keeps what it was sent.
struct Server {
    url: String,
    seen: Arc<Mutex<Vec<String>>>,
}

fn serve(answers: Vec<(u16, String)>) -> Server {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/releases", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let kept = seen.clone();
    std::thread::spawn(move || {
        for (status, body) in answers {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut request = Vec::new();
            let mut buffer = [0; 4096];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let Ok(read) = stream.read(&mut buffer) else {
                    return;
                };
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
            }
            kept.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&request).into_owned());
            let answer = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\n\
                 content-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(answer.as_bytes());
        }
    });
    Server { url, seen }
}

fn http_agent(url: &str) -> String {
    format!(
        r#"
local tau = require("tau")
return tau.agent {{
  name = "greet",
  triggers = {{
    releases = {{
      poll = {{ http = {{ url = "{url}", headers = {{ authorization = "Bearer " .. tau.secret("github") }} }} }},
      every = "1h",
      key = "id",
      items = function(result) return result.releases end,
    }},
  }},
}}
"#
    )
}

#[test]
fn an_http_poll_sends_its_secret_and_fires_what_is_new() {
    let server = serve(vec![
        (200, r#"{"releases":[{"id":1}]}"#.to_owned()),
        (200, r#"{"releases":[{"id":1},{"id":2}]}"#.to_owned()),
    ]);
    let f = Fixture::with(&http_agent(&server.url));
    f.host.online.store(true, Ordering::SeqCst);
    block_on_io(f.host.secrets("greet").set("github", "tok-value-1234"))
        .unwrap();
    f.tick();
    assert_eq!(f.trigger("releases").error, None);
    let sent = server.seen.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert!(
        sent[0]
            .to_ascii_lowercase()
            .contains("authorization: bearer tok-value-1234"),
        "{}",
        sent[0]
    );
    f.clock.advance(3600);
    f.tick();
    assert_eq!(f.agent().queued, 1);
    f.clock.advance(30);
    f.tick();
    assert_eq!(
        heard(&f.turns()[0]),
        [("releases".to_owned(), "{\"id\":2}".to_owned())]
    );
}

#[test]
fn an_http_poll_that_fails_says_why_without_its_secret() {
    let server = serve(vec![(
        500,
        r#"{"error":"bad token tok-value-1234"}"#.to_owned(),
    )]);
    let f = Fixture::with(&http_agent(&server.url));
    f.host.online.store(true, Ordering::SeqCst);
    block_on_io(f.host.secrets("greet").set("github", "tok-value-1234"))
        .unwrap();
    f.tick();
    let error = f.trigger("releases").error.unwrap();
    assert!(error.contains("500"), "{error}");
    assert!(error.contains("[secret:github]"), "{error}");
    assert!(!error.contains("tok-value-1234"), "{error}");
}

/// An agent's commands run in its sandbox, with its home under tau's
/// data; a repository that is not an agent's gets none. Without `bwrap`
/// an agent has no sandbox, so no command tools, and its page says why.
#[test]
fn an_agents_commands_run_in_its_sandbox() {
    let f = Fixture::new();
    let checkout = f._home.path().join("agents").join("greet");
    let agents = open(f._home.path(), &f.clock, &f.host)
        .with_bwrap(Some("/bin/bwrap".into()));
    let sandbox = agents.sandbox("greet", &checkout).unwrap();
    assert_eq!(sandbox.bwrap, Path::new("/bin/bwrap"));
    assert_eq!(sandbox.home, f._home.path().join("state/homes/greet"));
    assert!(
        agents
            .sandbox("tau-agent", Path::new("/src/tau-agent"))
            .is_none()
    );
    block_on_io(agents.registry().scan());
    assert_eq!(block_on_io(agents.overview()).agents[0].sandbox, None);

    let without = open(f._home.path(), &f.clock, &f.host).with_bwrap(None);
    assert!(without.sandbox("greet", &checkout).is_none());
    block_on_io(without.registry().scan());
    let problem = block_on_io(without.overview()).agents[0].sandbox.clone();
    assert!(problem.unwrap().contains("bwrap"));
}

/// The person's messages to an agent go to the model and effort set for
/// it; another repository's are not the agents' to set.
#[test]
fn the_person_s_messages_go_to_the_agent_s_model() {
    use tau_ui_plugin::{HostHalf as _, ModelPick, RepoCtx};
    let f = Fixture::new();
    f.host.settings.lock().unwrap().insert(
        "greet".into(),
        Settings {
            model: Some("gpt-6-sol".into()),
            effort: Some("high".into()),
            ..Settings::default()
        },
    );
    let at = |checkout: PathBuf| RepoCtx {
        id: tau_ui_plugin::RepoId("repo".into()),
        remote: None,
        folder: None,
        name: "greet".into(),
        checkout,
        dir: PathBuf::new(),
        workspaces: PathBuf::new(),
    };
    let agents = f._home.path().join("agents");
    assert_eq!(
        block_on_io(
            tau_agents_host::AgentsHost
                .conversation_model(&f.agents, &at(agents.join("greet")),)
        ),
        Some(ModelPick {
            model: "gpt-6-sol".into(),
            effort: "high".into(),
        })
    );
    assert_eq!(
        block_on_io(
            tau_agents_host::AgentsHost.conversation_model(
                &f.agents,
                &at(f._home.path().join("code")),
            )
        ),
        None
    );
}

/// An agent's repository is one no person attends: its MCP servers start
/// in its sandbox, or, without `bwrap`, not at all, saying why. Another
/// repository is none of its.
#[test]
fn an_agent_s_servers_start_in_its_sandbox_or_not_at_all() {
    use tau_ui_plugin::UnattendedSource as _;
    let f = Fixture::new();
    let checkout = f._home.path().join("agents").join("greet");
    let with = open(f._home.path(), &f.clock, &f.host)
        .with_bwrap(Some("/bin/bwrap".into()));
    let sandbox = with.unattended(&checkout).unwrap().sandbox.unwrap();
    assert_eq!(sandbox.dir, checkout);
    assert!(with.unattended(Path::new("/src/tau-agent")).is_none());
    let without = open(f._home.path(), &f.clock, &f.host).with_bwrap(None);
    let why = without.unattended(&checkout).unwrap().sandbox.unwrap_err();
    assert!(why.contains("bwrap"), "{why}");
}

/// An event's data too large for its message is kept whole as an
/// artifact of the agent's repository, granted to its main chat, and
/// the message names it with its first bytes (ADR 0037).
#[test]
fn data_too_large_for_a_message_is_kept_as_an_artifact() {
    let f = Fixture::new();
    f.tick();
    let long = "x".repeat(tau_agents::event::INLINE_DATA + 100);
    f.command(&long);
    f.clock.advance(30);
    f.tick();
    let turns = f.turns();
    assert_eq!(turns.len(), 1);
    assert!(
        turns[0].text.len() < tau_agents::event::INLINE_DATA,
        "{}",
        turns[0].text.len()
    );
    let heard = heard(&turns[0]);
    let reference: Value = serde_json::from_str(&heard[0].1).unwrap();
    let id = reference["artifact"].as_str().unwrap().to_owned();
    assert_eq!(reference["read_with"], "artifact_read");
    assert!(
        reference["head"]
            .as_str()
            .unwrap()
            .starts_with("{\"args\":\"xxx")
    );
    // Granted to its main chat, as tau-tools' record, which
    // `artifact_read` folds.
    let grants = f.host.grants.lock().unwrap().clone();
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0].0, "main-of-greet");
    let folded =
        tau_tools_host::artifact_grant::fold_grants(&[grants[0].1.clone()])
            .unwrap();
    let grant = &folded[&id];
    assert_eq!(grant.owner_run_id, "main-of-greet");
    // The bytes are the event's data, whole.
    let whole = json!({ "args": long, "from": "run-1" }).to_string();
    assert_eq!(grant.artifact.size_bytes, whole.len() as u64);
    let objects = f._home.path().join("agents/greet/artifacts");
    assert!(objects.exists(), "{}", objects.display());
}

/// An agent that only a command wakes, so what it schedules is all that
/// comes due.
const QUIET: &str = r#"
local tau = require("tau")
return tau.agent {
  name = "greet",
  triggers = { hello = { command = "hello" } },
}
"#;

fn plan(name: &str, when: Value) -> tau_agents_host::Plan {
    let mut args = json!({ "name": name, "note": format!("{name}, then") });
    args.as_object_mut()
        .unwrap()
        .extend(when.as_object().unwrap().clone());
    serde_json::from_value(args).unwrap()
}

/// What a turn's events carried, as JSON.
fn heard_data(turn: &Turn) -> Vec<(String, Value)> {
    heard(turn)
        .into_iter()
        .map(|(trigger, data)| (trigger, serde_json::from_str(&data).unwrap()))
        .collect()
}

#[test]
fn an_agent_schedules_itself_once_and_hears_its_note() {
    let f = Fixture::with(QUIET);
    let made = block_on_io(
        f.agents
            .schedule("greet", plan("check-pr-412", json!({ "in": "2h" }))),
    )
    .unwrap();
    assert!(!made.replaced);
    assert_eq!(made.scheduled.next, MONDAY_EIGHT + 2 * 3600);
    let shown = f.agent().scheduled;
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].name, "check-pr-412");
    assert_eq!(f.agent().next(), Some(MONDAY_EIGHT + 2 * 3600));

    f.clock.set(MONDAY_EIGHT + 2 * 3600 - 1);
    f.tick();
    assert_eq!(f.agent().queued, 0);
    f.clock.advance(1);
    f.tick();
    assert_eq!(f.agent().queued, 1);
    assert!(f.agent().scheduled.is_empty(), "once, then it goes");
    f.clock.advance(30);
    f.tick();
    let turns = f.turns();
    assert_eq!(turns.len(), 1);
    let message = Message::parse(&turns[0].text).unwrap();
    assert_eq!(message.events[0].kind, tau_agents::event::Kind::Scheduled);
    assert_eq!(
        heard_data(&turns[0]),
        [(
            "check-pr-412".to_owned(),
            json!({
                "note": "check-pr-412, then",
                "due": MONDAY_EIGHT + 2 * 3600,
                "repeats": false,
            })
        )]
    );
}

#[test]
fn once_on_a_local_date_and_time() {
    let f = Fixture::with(QUIET);
    let made = block_on_io(f.agents.schedule(
        "greet",
        plan("tomorrow", json!({ "on": "2026-10-13 09:00" })),
    ))
    .unwrap();
    assert_eq!(made.scheduled.next, MONDAY_EIGHT + 86_400 + 3600);
    assert_eq!(f.agents.local_time(made.scheduled.next), "2026-10-13 09:00");
}

#[test]
fn what_it_schedules_again_and_again_moves_on_and_catches_up_once() {
    let f = Fixture::with(QUIET);
    block_on_io(
        f.agents
            .schedule("greet", plan("sweep", json!({ "every": "1h" }))),
    )
    .unwrap();
    block_on_io(f.agents.schedule(
        "greet",
        plan("standup", json!({ "at": "09:30", "days": ["Mon", "Wed"] })),
    ))
    .unwrap();
    let next = |f: &Fixture| -> Vec<(String, i64)> {
        f.agent()
            .scheduled
            .into_iter()
            .map(|scheduled| (scheduled.name, scheduled.next))
            .collect()
    };
    assert_eq!(
        next(&f),
        [
            ("sweep".to_owned(), MONDAY_EIGHT + 3600),
            ("standup".to_owned(), MONDAY_EIGHT + 5400),
        ],
        "soonest first"
    );
    f.clock.set(MONDAY_EIGHT + 3600);
    f.tick();
    assert_eq!(next(&f)[0], ("standup".to_owned(), MONDAY_EIGHT + 5400));
    assert_eq!(next(&f)[1], ("sweep".to_owned(), MONDAY_EIGHT + 7200));

    // tau closes until Tuesday noon: each fires once, and moves past now.
    f.clock.set(MONDAY_EIGHT + 86_400 + 4 * 3600);
    let again = f.restarted();
    block_on_io(again.tick());
    // The first sweep waited long enough: one turn hears it, and what
    // came due while tau was closed, once each.
    let turns = f.turns();
    assert_eq!(turns.len(), 1);
    assert_eq!(
        heard_data(&turns[0])
            .into_iter()
            .map(|(trigger, data)| (trigger, data["due"].as_i64().unwrap()))
            .collect::<Vec<_>>(),
        [
            ("sweep".to_owned(), MONDAY_EIGHT + 3600),
            ("standup".to_owned(), MONDAY_EIGHT + 5400),
            ("sweep".to_owned(), MONDAY_EIGHT + 7200),
        ]
    );
    let overview = block_on_io(again.overview());
    assert_eq!(
        overview.agents[0]
            .scheduled
            .iter()
            .map(|scheduled| (scheduled.name.as_str(), scheduled.next))
            .collect::<Vec<_>>(),
        [
            ("sweep", MONDAY_EIGHT + 86_400 + 5 * 3600),
            ("standup", MONDAY_EIGHT + 2 * 86_400 + 5400),
        ]
    );
}

/// A one-off comes from a minute to a year ahead (366 days), `in` a
/// duration or `on` a local date and time, and a repeat goes every minute
/// to every year: past either end it is refused, saying which, and
/// nothing is scheduled. Times are drawn at both ends and one either
/// side, and anywhere between.
#[hegel::test(test_cases = 60)]
fn a_schedule_is_kept_from_a_minute_to_a_year_ahead(tc: hegel::TestCase) {
    use hegel::generators as gs;
    use tau_agents_host::schedule::{LONGEST, SHORTEST};
    let way = tc.draw(gs::sampled_from(vec!["in", "on", "every"]));
    let ahead = if way == "on" {
        // A local time names a minute.
        let (soonest, furthest) = (SHORTEST / 60, LONGEST / 60);
        60 * tc.draw(hegel::one_of!(
            gs::sampled_from(vec![
                -1,
                0,
                soonest - 1,
                soonest,
                soonest + 1,
                furthest - 1,
                furthest,
                furthest + 1,
            ]),
            gs::integers::<i64>()
                .min_value(-10)
                .max_value(furthest + 10),
        ))
    } else {
        tc.draw(hegel::one_of!(
            gs::sampled_from(vec![
                1,
                SHORTEST - 1,
                SHORTEST,
                SHORTEST + 1,
                LONGEST - 1,
                LONGEST,
                LONGEST + 1,
            ]),
            gs::integers::<i64>().min_value(1).max_value(LONGEST + 100),
        ))
    };
    let f = Fixture::with(QUIET);
    let when = match way {
        "on" => json!({ "on": f.agents.local_time(MONDAY_EIGHT + ahead) }),
        "in" => json!({ "in": format!("{ahead}s") }),
        _ => json!({ "every": format!("{ahead}s") }),
    };
    let made = block_on_io(f.agents.schedule("greet", plan("x", when)));
    if ahead < SHORTEST {
        tc.event("too soon");
        let why = made.unwrap_err();
        let says = if way == "every" {
            "shorter than a minute"
        } else {
            "a minute from now is the soonest"
        };
        assert!(why.contains(says), "{way} {ahead}: {why}");
        assert!(f.agent().scheduled.is_empty());
    } else if ahead > LONGEST {
        tc.event("too far");
        let why = made.unwrap_err();
        assert!(why.contains("A year ahead is the furthest"), "{why}");
        assert!(f.agent().scheduled.is_empty());
    } else {
        let made = made.unwrap_or_else(|why| panic!("{way} {ahead}: {why}"));
        assert_eq!(made.scheduled.next, MONDAY_EIGHT + ahead);
        assert_eq!(f.agent().scheduled.len(), 1);
    }
}

/// What else keeps a schedule from being made says why: its name, how
/// it says when, its days and its note.
#[test]
fn a_schedule_it_cannot_keep_is_refused_and_says_why() {
    let f = Fixture::with(QUIET);
    let refused = |name: &str, when: Value| {
        block_on_io(f.agents.schedule("greet", plan(name, when))).unwrap_err()
    };
    assert!(refused("Check", json!({ "in": "1h" })).contains("cannot name"));
    assert!(
        refused("hello", json!({ "in": "1h" })).contains("trigger of yours")
    );
    assert!(refused("x", json!({})).contains("Say when"));
    assert!(
        refused("x", json!({ "in": "1h", "every": "1h" })).contains("Say when")
    );
    assert!(
        refused("x", json!({ "on": "tomorrow" }))
            .contains("like \"2026-10-13 08:00\"")
    );
    assert!(
        refused("x", json!({ "at": "09:00", "days": ["Someday"] }))
            .contains("not a day")
    );
    let empty: tau_agents_host::Plan =
        serde_json::from_value(json!({ "name": "x", "note": " ", "in": "1h" }))
            .unwrap();
    assert!(
        block_on_io(f.agents.schedule("greet", empty))
            .unwrap_err()
            .contains("note")
    );
    assert!(f.agent().scheduled.is_empty());
}

#[test]
fn at_most_a_hundred_and_a_name_again_replaces_it() {
    let f = Fixture::with(QUIET);
    for n in 0..tau_agents_host::MAX_SCHEDULED {
        block_on_io(
            f.agents.schedule(
                "greet",
                plan(&format!("s{n}"), json!({ "in": "1h" })),
            ),
        )
        .unwrap();
    }
    let error = block_on_io(
        f.agents
            .schedule("greet", plan("one-more", json!({ "in": "1h" }))),
    )
    .unwrap_err();
    assert!(error.contains("the most you may"), "{error}");
    let again = block_on_io(
        f.agents
            .schedule("greet", plan("s7", json!({ "in": "3h" }))),
    )
    .unwrap();
    assert!(again.replaced);
    assert_eq!(f.agent().scheduled.len(), tau_agents_host::MAX_SCHEDULED);
    assert_eq!(
        f.agent()
            .scheduled
            .last()
            .map(|scheduled| (scheduled.name.clone(), scheduled.next)),
        Some(("s7".to_owned(), MONDAY_EIGHT + 3 * 3600))
    );
}

#[test]
fn the_agent_or_the_person_cancels_what_it_scheduled() {
    let f = Fixture::with(QUIET);
    for name in ["one", "two"] {
        block_on_io(
            f.agents
                .schedule("greet", plan(name, json!({ "in": "1h" }))),
        )
        .unwrap();
    }
    assert!(block_on_io(f.agents.unschedule("greet", "one")));
    assert!(!block_on_io(f.agents.unschedule("greet", "one")));
    block_on_io(f.agents.act(Act::Unschedule {
        agent: "greet".into(),
        name: "two".into(),
    }))
    .unwrap();
    assert!(
        block_on_io(f.agents.act(Act::Unschedule {
            agent: "greet".into(),
            name: "two".into(),
        }))
        .is_err()
    );
    f.clock.advance(2 * 3600);
    f.tick();
    assert_eq!(f.agent().queued, 0, "nothing fires once cancelled");
}

/// Calls the agent `greet`'s tool `name` as its run would.
fn call(f: &Fixture, name: &str, args: Value) -> Result<String, String> {
    use tau_agent::{
        plugin::Plugin,
        tool::{RunId, ToolCtx, ToolUpdates},
    };
    let tools = tau_agents_host::tools::AgentTools::new(
        f.agents.clone(),
        "greet".into(),
    )
    .tools();
    let tool = tools.iter().find(|tool| tool.name() == name).unwrap();
    let (updates, _) = ToolUpdates::channel("call_1");
    let ctx = ToolCtx::new(
        tokio_util::sync::CancellationToken::new(),
        updates,
        RunId("run_1".into()),
    );
    block_on_io(tool.call(args, ctx))
        .map(|output| output.text_content())
        .map_err(|error| error.to_string())
}

#[test]
fn the_tools_say_what_was_scheduled_in_local_time() {
    let f = Fixture::with(QUIET);
    assert_eq!(
        call(&f, "scheduled", json!({})).unwrap(),
        "It is 2026-10-12 08:00. Nothing is scheduled."
    );
    assert_eq!(
        call(
            &f,
            "schedule",
            json!({ "name": "recheck", "note": "Did #412 merge?", "in": "90m" })
        )
        .unwrap(),
        "Scheduled `recheck`: once, next 2026-10-12 09:30. It comes as an \
         event `recheck · scheduled`."
    );
    assert_eq!(
        call(
            &f,
            "schedule",
            json!({ "name": "recheck", "note": "Did #412 merge?", "at": "17:00",
                    "days": ["Mon", "Fri"] })
        )
        .unwrap(),
        "Replaced `recheck`: 17:00 Mon Fri, next 2026-10-12 17:00. It comes \
         as an event `recheck · scheduled`."
    );
    assert_eq!(
        call(&f, "scheduled", json!({})).unwrap(),
        "It is 2026-10-12 08:00. 1 of the 100 you may have:\n\
         - recheck · 17:00 Mon Fri · next 2026-10-12 17:00 · Did #412 merge?"
    );
    assert!(
        call(
            &f,
            "schedule",
            json!({ "name": "x", "note": "n", "when": "1h" })
        )
        .is_err(),
        "an argument it does not know"
    );
    assert_eq!(
        call(&f, "unschedule", json!({ "name": "recheck" })).unwrap(),
        "Cancelled `recheck`."
    );
    assert_eq!(
        call(&f, "unschedule", json!({ "name": "recheck" })).unwrap(),
        "Nothing is scheduled as `recheck`."
    );
}
