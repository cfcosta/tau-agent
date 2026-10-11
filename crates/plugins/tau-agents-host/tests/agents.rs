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

    fn secrets(&self) -> SecretStore {
        self.secrets.get().expect("the fixture sets it").clone()
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

    fn fetch(&self, spec: Value) -> HostFuture<'_, Result<Value, String>> {
        Box::pin(async move {
            if !self.online.load(Ordering::SeqCst) {
                return Err("offline".into());
            }
            tau_agents_host::poll::fetch(&self.secrets(), &spec).await
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
    block_on_io(f.agents.act(Act::SetSecret {
        name: "github".into(),
        value: "a-value-never-shown".into(),
    }))
    .unwrap();
    assert_eq!(names(&f), [("github".to_owned(), true)]);
    // The overview holds names, never values.
    let shown = serde_json::to_string(&f.agent()).unwrap();
    assert!(!shown.contains("a-value-never-shown"));
    block_on_io(f.agents.act(Act::RemoveSecret {
        name: "github".into(),
    }))
    .unwrap();
    assert_eq!(names(&f), [("github".to_owned(), false)]);
    assert!(
        block_on_io(f.agents.act(Act::SetSecret {
            name: "not a name".into(),
            value: "x".into(),
        }))
        .is_err()
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
      key = function(pr) error("no key here") end,
    },
  },
}
"#;
    let f = Fixture::with(source);
    f.host
        .answer("mcp__github__list_pull_requests", pulls(&[1]));
    f.tick();
    let error = f.trigger("pulls").error.unwrap();
    assert!(error.contains("no key here"), "{error}");
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
    block_on_io(f.host.secrets().set("github", "tok-value-1234")).unwrap();
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
    block_on_io(f.host.secrets().set("github", "tok-value-1234")).unwrap();
    f.tick();
    let error = f.trigger("releases").error.unwrap();
    assert!(error.contains("500"), "{error}");
    assert!(error.contains("[secret:github]"), "{error}");
    assert!(!error.contains("tok-value-1234"), "{error}");
}

/// An agent's commands run in its sandbox, with its home under tau's
/// data; a repository that is not an agent's gets none.
#[test]
fn an_agents_commands_run_in_its_sandbox() {
    let f = Fixture::new();
    let agents = open(f._home.path(), &f.clock, &f.host)
        .with_bwrap(Some("/bin/bwrap".into()));
    let checkout = f._home.path().join("agents").join("greet");
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
}

/// Without `bwrap` an agent has no sandbox, so no command tools, and its page
/// says why.
#[test]
fn without_bwrap_an_agent_has_no_commands_and_says_so() {
    let f = Fixture::new();
    let agents = open(f._home.path(), &f.clock, &f.host).with_bwrap(None);
    let checkout = f._home.path().join("agents").join("greet");
    assert!(agents.sandbox("greet", &checkout).is_none());
    block_on_io(agents.registry().scan());
    let problem = block_on_io(agents.overview()).agents[0].sandbox.clone();
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
