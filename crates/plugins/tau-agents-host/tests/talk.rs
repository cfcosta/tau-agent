//! What an agent says to the person and to other agents (ADR 0037):
//! notes and questions that wait on the person, answers heard as
//! events, messages between agents, and the guard against ping-pong.

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

use serde_json::{Value, json};
use tau_agents::{
    Act,
    Settings,
    Status,
    event::{Batch, Event, Kind, Message},
};
use tau_agents_host::{
    Agents,
    AgentsHost,
    MAX_HOPS,
    clock::FakeClock,
    env::Env,
    hops_of,
};
use tau_testing::block_on_io;
use tau_tools_host::secrets::SecretStore;
use tau_ui_plugin::{
    Conversations,
    HostHalf,
    RepoCtx,
    RunCtx,
    RunKind,
    Services,
    Turn,
    registry::HostFuture,
};
use tau_vcs_host::{Identity, ProjectRepo};

/// 2026-10-12T08:00:00Z.
const NOW: i64 = 1_791_792_000;

/// An agent that hears messages, in its `inbox`.
const LISTENS: &str = r#"
local tau = require("tau")
return tau.agent {
  name = "triage",
  triggers = { inbox = { message = true } },
}
"#;

/// An agent that hears none.
const DEAF: &str = r#"
local tau = require("tau")
return tau.agent {
  name = "digest",
  triggers = { morning = { at = "09:00" } },
}
"#;

#[derive(Default)]
struct Host {
    running: AtomicBool,
    turns: Mutex<Vec<Turn>>,
    /// What each agent spent today.
    spent: Mutex<BTreeMap<String, f64>>,
    secrets: std::sync::OnceLock<SecretStore>,
}

impl Conversations for Host {
    fn list_agent(&self, _name: &str) -> HostFuture<'_, anyhow::Result<()>> {
        Box::pin(async { Ok(()) })
    }

    fn main_of(&self, repo: &str) -> Option<String> {
        Some(format!("main-of-{repo}"))
    }

    fn running(&self, _repo: &str) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    fn turn(&self, turn: Turn) -> HostFuture<'_, anyhow::Result<()>> {
        self.turns.lock().unwrap().push(turn);
        Box::pin(async { Ok(()) })
    }
}

/// The host as the agents reach it, its conversations its own.
struct HostEnv(Arc<Host>);

impl Env for HostEnv {
    fn refresh(&self) {}

    fn settings(&self, _agent: &str) -> Settings {
        Settings::default()
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
        agent: &str,
        _since: i64,
    ) -> HostFuture<'_, anyhow::Result<f64>> {
        let spent = self.0.spent.lock().unwrap().get(agent).copied();
        Box::pin(async move { Ok(spent.unwrap_or(0.0)) })
    }

    fn conversations(&self) -> Option<Arc<dyn Conversations>> {
        Some(self.0.clone())
    }

    fn secrets(&self, agent: &str) -> SecretStore {
        let store = self.0.secrets.get().expect("the fixture sets it").clone();
        SecretStore::of_agent(
            store.path().parent().expect("a directory"),
            agent,
        )
    }

    fn call_tool<'a>(
        &'a self,
        _checkout: &'a Path,
        name: &'a str,
        _args: Value,
    ) -> HostFuture<'a, Result<Value, String>> {
        let error = format!("no {name} here");
        Box::pin(async move { Err(error) })
    }

    fn fetch<'a>(
        &'a self,
        _agent: &'a str,
        _spec: Value,
    ) -> HostFuture<'a, Result<Value, String>> {
        Box::pin(async { Err("offline".into()) })
    }
}

struct Fixture {
    home: tempfile::TempDir,
    clock: Arc<FakeClock>,
    host: Arc<Host>,
    agents: Agents,
}

impl Fixture {
    /// The agents `triage`, which hears messages, and `digest`, which
    /// does not.
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        for (name, source) in [("triage", LISTENS), ("digest", DEAF)] {
            ProjectRepo::init(
                home.path().join("agents").join(name),
                Identity::default(),
                &[("agent.luau".to_owned(), source.to_owned())],
            )
            .unwrap();
        }
        let clock = Arc::new(FakeClock::new(NOW, 0));
        let host = Arc::new(Host::default());
        host.secrets
            .set(SecretStore::in_dir(&home.path().join("config")))
            .unwrap();
        let agents = Agents::at(
            home.path().join("agents"),
            home.path().join("state"),
            Arc::new(HostEnv(host.clone())),
            clock.clone(),
        );
        block_on_io(agents.registry().scan());
        Self {
            home,
            clock,
            host,
            agents,
        }
    }

    fn agent(&self, name: &str) -> tau_agents::Agent {
        block_on_io(self.agents.overview())
            .agent(name)
            .unwrap()
            .clone()
    }

    fn act(&self, act: Act) -> anyhow::Result<()> {
        block_on_io(self.agents.act(act)).map(|_| ())
    }

    /// A tick past the coalescing window: the turns that start.
    fn tick_past_window(&self) -> Vec<Turn> {
        self.clock.advance(Settings::default().window_secs as i64);
        block_on_io(self.agents.tick());
        std::mem::take(&mut *self.host.turns.lock().unwrap())
    }
}

/// What the turn heard, as the card reads it: each event's trigger,
/// kind and data.
fn heard(turn: &Turn) -> Vec<(String, Kind, Value)> {
    Message::parse(&turn.text)
        .expect("an event message")
        .events
        .into_iter()
        .map(|event| {
            let data = serde_json::from_str(&event.data).unwrap();
            (event.trigger, event.kind, data)
        })
        .collect()
}

#[test]
fn notes_and_questions_wait_on_the_person_until_dealt_with() {
    let f = Fixture::new();
    let note =
        block_on_io(f.agents.notify("triage", "Build 2318 failed")).unwrap();
    let question = block_on_io(f.agents.ask(
        "triage",
        "Quarantine the flaky test?",
        vec!["Yes".into(), "No".into()],
    ))
    .unwrap();
    let agent = f.agent("triage");
    assert_eq!(agent.notices.len(), 2);
    assert!(!agent.notices[0].asks());
    assert_eq!(
        agent.notices[1].choices.as_deref(),
        Some(&["Yes".to_owned(), "No".to_owned()][..])
    );
    assert_eq!(agent.waits_on_you(), 2);
    assert!(agent.needs_you());
    assert_eq!(agent.last.unwrap().line, "said: Build 2318 failed");

    // A note goes when read; a question only with an answer.
    f.act(Act::Dismiss {
        agent: "triage".into(),
        notice: note,
    })
    .unwrap();
    assert!(
        f.act(Act::Dismiss {
            agent: "triage".into(),
            notice: question.clone(),
        })
        .is_err()
    );
    assert_eq!(f.agent("triage").waits_on_you(), 1);
    assert!(block_on_io(f.agents.notify("triage", "  ")).is_err());
}

#[test]
fn an_agent_keeps_its_latest_notes_and_every_question() {
    let f = Fixture::new();
    block_on_io(f.agents.ask("triage", "Which?", Vec::new())).unwrap();
    for n in 0..25 {
        block_on_io(f.agents.notify("triage", &format!("note {n}"))).unwrap();
    }
    let notices = f.agent("triage").notices;
    assert_eq!(notices.len(), 21);
    assert!(notices[0].asks());
    assert_eq!(notices[1].text, "note 5");
    assert_eq!(notices[20].text, "note 24");
}

#[test]
fn an_answer_reaches_the_agent_as_an_event_in_a_turn_of_its_own() {
    let f = Fixture::new();
    let question =
        block_on_io(f.agents.ask("triage", "Quarantine it?", Vec::new()))
            .unwrap();
    // Asking starts no turn.
    assert!(f.tick_past_window().is_empty());
    f.act(Act::Answer {
        agent: "triage".into(),
        notice: question.clone(),
        answer: "Yes, for a week".into(),
    })
    .unwrap();
    assert!(f.agent("triage").notices.is_empty());
    let turns = f.tick_past_window();
    assert_eq!(turns.len(), 1);
    assert_eq!(
        heard(&turns[0]),
        [(
            "ask".to_owned(),
            Kind::Answer,
            json!({ "question": "Quarantine it?", "answer": "Yes, for a week" })
        )]
    );
    // It was answered once.
    assert!(
        f.act(Act::Answer {
            agent: "triage".into(),
            notice: question,
            answer: "No".into(),
        })
        .is_err()
    );
}

#[test]
fn only_an_agents_runs_have_its_tools() {
    let f = Fixture::new();
    let run_in = |checkout: &Path| RunCtx {
        kind: RunKind::Main,
        repo: RepoCtx {
            name: checkout.file_name().unwrap().to_str().unwrap().into(),
            checkout: checkout.to_owned(),
            dir: f.home.path().join("dir"),
            workspaces: f.home.path().join("workspaces"),
        },
        model: "gpt-6-luna".into(),
        effort: None,
        services: Services::default(),
    };
    let tools = |checkout: &Path| -> Vec<String> {
        block_on_io(AgentsHost.agent_plugins(
            &f.agents,
            &run_in(checkout),
            &Settings::default(),
        ))
        .unwrap()
        .iter()
        .flat_map(|plugin| plugin.tools())
        .map(|tool| tool.name().to_owned())
        .collect()
    };
    assert_eq!(
        tools(&f.home.path().join("agents/triage")),
        ["notify", "ask", "agent_send"]
    );
    assert!(tools(Path::new("/src/tau-agent")).is_empty());
}

#[test]
fn a_message_reaches_an_agent_that_hears_them_and_no_other() {
    let f = Fixture::new();
    block_on_io(f.agents.send("digest", "triage", "#412 touches auth", 1))
        .unwrap();
    let turns = f.tick_past_window();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].repo, "triage");
    assert_eq!(
        heard(&turns[0]),
        [(
            "inbox".to_owned(),
            Kind::Message,
            json!({ "from": "digest", "text": "#412 touches auth", "hops": 1 })
        )]
    );

    let deaf =
        block_on_io(f.agents.send("triage", "digest", "hello", 1)).unwrap_err();
    assert!(deaf.contains("hears no messages"), "{deaf}");
    let itself =
        block_on_io(f.agents.send("triage", "triage", "hello", 1)).unwrap_err();
    assert!(itself.contains("That is you"), "{itself}");
    let nobody =
        block_on_io(f.agents.send("triage", "nobody", "hello", 1)).unwrap_err();
    assert!(nobody.contains("No agent nobody"), "{nobody}");
}

#[test]
fn a_message_spends_the_receivers_day() {
    let f = Fixture::new();
    // triage spent its dollar today.
    f.host.spent.lock().unwrap().insert("triage".into(), 1.5);
    block_on_io(f.agents.send("digest", "triage", "look at #412", 1)).unwrap();
    assert!(f.tick_past_window().is_empty());
    let triage = f.agent("triage");
    assert_eq!(triage.status, Status::AtCap);
    assert_eq!(triage.queued, 1);
}

/// The message a turn heard, with these events.
fn woken_by(events: &[(Kind, Value)]) -> String {
    Batch {
        events: events
            .iter()
            .enumerate()
            .map(|(n, (kind, data))| Event {
                trigger: "inbox".into(),
                kind: *kind,
                id: format!("e{n}"),
                at: NOW,
                data: data.clone(),
            })
            .collect(),
    }
    .message(0)
}

#[test]
fn a_chain_of_messages_stops_after_its_hops() {
    // The person, or anything but messages, starts a chain afresh.
    assert_eq!(hops_of("Look at #412."), 1);
    assert_eq!(
        hops_of(&woken_by(&[
            (Kind::Message, json!({ "hops": 5 })),
            (Kind::Schedule, json!({ "due": NOW })),
        ])),
        1
    );
    // Only messages: one more than the furthest of them.
    assert_eq!(
        hops_of(&woken_by(&[
            (Kind::Message, json!({ "hops": 2 })),
            (Kind::Message, json!({ "hops": 5 })),
        ])),
        6
    );
    // A message that does not say how far it went counts as far as any.
    assert_eq!(
        hops_of(&woken_by(&[(Kind::Message, json!({ "text": "hi" }))])),
        MAX_HOPS + 1
    );

    let f = Fixture::new();
    block_on_io(f.agents.send("digest", "triage", "the 8th", MAX_HOPS))
        .unwrap();
    let refused =
        block_on_io(f.agents.send("digest", "triage", "the 9th", MAX_HOPS + 1))
            .unwrap_err();
    assert!(refused.contains("tau stops at 8"), "{refused}");
    assert_eq!(f.agent("triage").queued, 1);
}
