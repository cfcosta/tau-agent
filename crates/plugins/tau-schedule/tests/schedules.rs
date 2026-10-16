//! A chat's wake-ups (ADR 0039): scheduled once, kept across restarts,
//! waking an idle chat with their notes, waiting for one that is busy,
//! dropped with a chat that ended, and refused when they repeat.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use serde_json::{Value, json};
use tau_schedule::{
    MAX_WAITING,
    Plan,
    ScheduleHost,
    Schedules,
    Woken,
    clock::FakeClock,
    host::{Env, NO_REPEATS},
};
use tau_testing::block_on_io;
use tau_ui_plugin::{
    Conversations,
    HostHalf,
    RepoCtx,
    RunCtx,
    RunKind,
    Services,
    Turn,
    Waking,
    registry::HostFuture,
};

/// 2026-10-12T08:00:00Z, in UTC.
const NOW: i64 = 1_791_792_000;

/// Conversations that answer each wake as told, and keep what woke
/// them.
#[derive(Default)]
struct Chats {
    answers: Mutex<VecDeque<Result<Waking, String>>>,
    woken: Mutex<Vec<(String, String)>>,
}

impl Conversations for Chats {
    fn list_agent(&self, _name: &str) -> HostFuture<'_, anyhow::Result<()>> {
        Box::pin(async { Ok(()) })
    }

    fn main_of(&self, _repo: &str) -> Option<String> {
        None
    }

    fn running(&self, _repo: &str) -> bool {
        false
    }

    fn turn(&self, _turn: Turn) -> HostFuture<'_, anyhow::Result<()>> {
        Box::pin(async { Ok(()) })
    }

    fn wake(
        &self,
        run: &str,
        text: String,
    ) -> HostFuture<'_, anyhow::Result<Waking>> {
        self.woken.lock().unwrap().push((run.to_owned(), text));
        let answer = self
            .answers
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Ok(Waking::Started));
        Box::pin(async move { answer.map_err(anyhow::Error::msg) })
    }
}

struct Host(Arc<Chats>);

impl Env for Host {
    fn conversations(&self) -> Option<Arc<dyn Conversations>> {
        Some(self.0.clone())
    }

    fn refresh(&self) {}
}

struct Fixture {
    home: tempfile::TempDir,
    clock: Arc<FakeClock>,
    chats: Arc<Chats>,
    schedules: Schedules,
}

impl Fixture {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let clock = Arc::new(FakeClock::new(NOW, 0));
        let chats = Arc::new(Chats::default());
        let schedules = open(home.path(), &clock, &chats);
        Self {
            home,
            clock,
            chats,
            schedules,
        }
    }

    fn schedule(
        &self,
        run: &str,
        name: &str,
        when: Value,
    ) -> Result<(), String> {
        block_on_io(self.schedules.schedule(run, plan(name, when))).map(|_| ())
    }

    fn tick(&self) {
        block_on_io(self.schedules.tick());
    }

    fn woken(&self) -> Vec<(String, String)> {
        self.chats.woken.lock().unwrap().clone()
    }

    fn answer(&self, answer: Result<Waking, String>) {
        self.chats.answers.lock().unwrap().push_back(answer);
    }

    fn waiting(&self, run: &str) -> Vec<String> {
        block_on_io(self.schedules.waiting(run))
            .into_iter()
            .map(|wake| wake.name)
            .collect()
    }
}

fn open(home: &Path, clock: &Arc<FakeClock>, chats: &Arc<Chats>) -> Schedules {
    block_on_io(Schedules::open(
        home.join("wakes.json"),
        home.join("agents"),
        Arc::new(Host(chats.clone())),
        clock.clone(),
    ))
}

fn plan(name: &str, when: Value) -> Plan {
    let mut args = json!({ "name": name, "note": format!("{name}, then") });
    args.as_object_mut()
        .unwrap()
        .extend(when.as_object().unwrap().clone());
    serde_json::from_value(args).unwrap()
}

#[test]
fn a_wake_up_wakes_its_chat_once_with_its_note() {
    let f = Fixture::new();
    f.schedule("chat-1", "recheck", json!({ "in": "2h" }))
        .unwrap();
    f.clock.set(NOW + 2 * 3600 - 1);
    f.tick();
    assert!(f.woken().is_empty());
    f.clock.advance(1);
    f.tick();
    let woken = f.woken();
    assert_eq!(woken.len(), 1);
    assert_eq!(woken[0].0, "chat-1");
    assert_eq!(
        Woken::parse(&woken[0].1).unwrap(),
        [Woken {
            name: "recheck".into(),
            due: "2026-10-12 10:00".into(),
            note: "recheck, then".into(),
        }]
    );
    assert!(f.waiting("chat-1").is_empty(), "once, then it goes");
    f.clock.advance(3600);
    f.tick();
    assert_eq!(f.woken().len(), 1);
}

#[test]
fn what_came_due_together_wakes_the_chat_in_one_turn() {
    let f = Fixture::new();
    f.schedule("chat-1", "first", json!({ "in": "1h" }))
        .unwrap();
    f.schedule("chat-1", "second", json!({ "on": "2026-10-12 09:30" }))
        .unwrap();
    f.schedule("chat-1", "later", json!({ "in": "1d" }))
        .unwrap();
    f.schedule("chat-2", "other", json!({ "in": "1h" }))
        .unwrap();
    // tau was closed until 10:00: both of chat-1's, in one turn.
    f.clock.set(NOW + 2 * 3600);
    let again = open(f.home.path(), &f.clock, &f.chats);
    block_on_io(again.tick());
    let woken = f.woken();
    assert_eq!(woken.len(), 2, "{woken:?}");
    let chat = woken.iter().find(|(run, _)| run == "chat-1").unwrap();
    let names: Vec<String> = Woken::parse(&chat.1)
        .unwrap()
        .into_iter()
        .map(|woken| woken.name)
        .collect();
    assert_eq!(names, ["first", "second"]);
    assert_eq!(
        block_on_io(again.waiting("chat-1"))
            .into_iter()
            .map(|wake| wake.name)
            .collect::<Vec<_>>(),
        ["later"]
    );
}

#[test]
fn a_busy_chat_waits_and_one_that_ended_drops_what_it_had() {
    let f = Fixture::new();
    f.schedule("busy", "recheck", json!({ "in": "1h" }))
        .unwrap();
    f.schedule("ended", "recheck", json!({ "in": "1h" }))
        .unwrap();
    f.schedule("ended", "tomorrow", json!({ "in": "1d" }))
        .unwrap();
    f.clock.set(NOW + 3600);
    // In the order the chats are kept: `busy`, then `ended`.
    f.answer(Ok(Waking::Busy));
    f.answer(Ok(Waking::Gone));
    f.tick();
    assert_eq!(f.waiting("busy"), ["recheck"], "it waits for the turn");
    assert!(f.waiting("ended").is_empty(), "the ended chat's all go");
    f.clock.advance(1);
    f.tick();
    assert!(f.waiting("busy").is_empty(), "woken once idle");
    assert_eq!(f.woken().len(), 3);
}

#[test]
fn a_turn_that_did_not_start_is_tried_again_later() {
    let f = Fixture::new();
    f.schedule("chat-1", "recheck", json!({ "in": "1h" }))
        .unwrap();
    f.clock.set(NOW + 3600);
    f.answer(Err("the window is closing".into()));
    f.tick();
    assert_eq!(f.waiting("chat-1"), ["recheck"]);
    f.clock.advance(29);
    f.tick();
    assert_eq!(f.woken().len(), 1, "it waits half a minute");
    f.clock.advance(1);
    f.tick();
    assert_eq!(f.woken().len(), 2);
    assert!(f.waiting("chat-1").is_empty());
}

#[test]
fn a_chat_does_not_repeat_and_says_why_else_it_refuses() {
    let f = Fixture::new();
    let refused =
        |name: &str, when: Value| f.schedule("chat-1", name, when).unwrap_err();
    assert_eq!(refused("x", json!({ "every": "1h" })), NO_REPEATS);
    assert_eq!(refused("x", json!({ "at": "09:00" })), NO_REPEATS);
    assert_eq!(
        refused("x", json!({ "in": "1h", "days": ["Mon"] })),
        NO_REPEATS
    );
    assert!(refused("X", json!({ "in": "1h" })).contains("cannot name"));
    assert!(refused("x", json!({})).contains("Say when"));
    assert!(
        refused("x", json!({ "in": "1h", "on": "2026-10-13 09:00" }))
            .contains("Say when")
    );
    // A minute from now is the soonest, and a year (366 days) ahead the
    // furthest: each bound holds, and a step past it is refused.
    assert!(refused("x", json!({ "in": "59s" })).contains("soonest"));
    assert!(
        refused("x", json!({ "on": "2026-10-12 08:00" })).contains("soonest")
    );
    assert!(refused("x", json!({ "in": "367d" })).contains("year"));
    assert!(refused("x", json!({ "on": "2027-10-13 08:01" })).contains("year"));
    for (name, when) in [
        ("soonest", json!({ "in": "60s" })),
        ("on-soonest", json!({ "on": "2026-10-12 08:01" })),
        ("furthest", json!({ "in": "366d" })),
        ("on-furthest", json!({ "on": "2027-10-13 08:00" })),
    ] {
        f.schedule("chat-2", name, when).unwrap();
    }
    assert_eq!(f.waiting("chat-2").len(), 4);
    assert!(
        refused("x", json!({ "on": "soon" }))
            .contains("like \"2026-10-13 08:00\"")
    );
    let empty: Plan =
        serde_json::from_value(json!({ "name": "x", "note": " ", "in": "1h" }))
            .unwrap();
    assert!(
        block_on_io(f.schedules.schedule("chat-1", empty))
            .unwrap_err()
            .contains("note")
    );
    assert!(f.waiting("chat-1").is_empty());
}

#[test]
fn at_most_a_hundred_a_chat_and_a_name_again_replaces_it() {
    let f = Fixture::new();
    for n in 0..MAX_WAITING {
        f.schedule("chat-1", &format!("w{n}"), json!({ "in": "1h" }))
            .unwrap();
    }
    assert!(
        f.schedule("chat-1", "more", json!({ "in": "1h" }))
            .unwrap_err()
            .contains("the most it may")
    );
    f.schedule("chat-2", "more", json!({ "in": "1h" })).unwrap();
    let again = block_on_io(
        f.schedules
            .schedule("chat-1", plan("w3", json!({ "in": "3h" }))),
    )
    .unwrap();
    assert!(again.replaced);
    assert_eq!(again.wake.at, NOW + 3 * 3600);
    assert_eq!(f.waiting("chat-1").len(), MAX_WAITING);
    assert!(block_on_io(f.schedules.unschedule("chat-1", "w3")));
    assert!(!block_on_io(f.schedules.unschedule("chat-1", "w3")));
    assert_eq!(f.waiting("chat-1").len(), MAX_WAITING - 1);
}

/// Calls the chat `run`'s tool `name` as its run would.
fn call(
    f: &Fixture,
    run: &str,
    name: &str,
    args: Value,
) -> Result<String, String> {
    use tau_agent::{
        plugin::Plugin,
        tool::{RunId, ToolCtx, ToolUpdates},
    };
    let tools = tau_schedule::host::ChatTools {
        schedules: f.schedules.clone(),
        sub_agent: false,
    }
    .tools();
    let tool = tools.iter().find(|tool| tool.name() == name).unwrap();
    let (updates, _) = ToolUpdates::channel("call_1");
    let ctx = ToolCtx::new(
        tokio_util::sync::CancellationToken::new(),
        updates,
        RunId(run.into()),
    );
    block_on_io(tool.call(args, ctx))
        .map(|output| output.text_content())
        .map_err(|error| error.to_string())
}

#[test]
fn the_tools_act_on_the_chat_that_calls_them() {
    let f = Fixture::new();
    assert_eq!(
        call(&f, "chat-1", "scheduled", json!({})).unwrap(),
        "It is 2026-10-12 08:00. Nothing is scheduled."
    );
    assert_eq!(
        call(
            &f,
            "chat-1",
            "schedule",
            json!({ "name": "recheck", "note": "Did #412 merge?", "in": "90m" })
        )
        .unwrap(),
        "Scheduled `recheck` for 2026-10-12 09:30. This chat wakes then with \
         your note."
    );
    assert!(
        call(
            &f,
            "chat-1",
            "schedule",
            json!({ "name": "daily", "note": "n", "every": "1d" })
        )
        .unwrap_err()
        .contains("repeating is for agents")
    );
    assert_eq!(
        call(&f, "chat-1", "scheduled", json!({})).unwrap(),
        "It is 2026-10-12 08:00. 1 of the 100 this chat may have:\n\
         - recheck · 2026-10-12 09:30 · Did #412 merge?"
    );
    assert_eq!(
        call(&f, "chat-2", "scheduled", json!({})).unwrap(),
        "It is 2026-10-12 08:00. Nothing is scheduled.",
        "another chat's are not this one's"
    );
    assert_eq!(
        call(&f, "chat-2", "unschedule", json!({ "name": "recheck" })).unwrap(),
        "Nothing is scheduled as `recheck`."
    );
    assert_eq!(
        call(&f, "chat-1", "unschedule", json!({ "name": "recheck" })).unwrap(),
        "Cancelled `recheck`."
    );
}

#[test]
fn every_run_has_the_tools_but_an_agent_s_and_a_sub_agent_cannot_schedule() {
    let f = Fixture::new();
    let tools = |kind: RunKind, checkout: PathBuf| -> Vec<String> {
        let run = RunCtx {
            kind,
            repo: RepoCtx {
                name: "repo".into(),
                checkout,
                dir: f.home.path().join("dir"),
                workspaces: f.home.path().join("workspaces"),
            },
            model: "gpt-6-luna".into(),
            effort: None,
            services: Services::default(),
        };
        block_on_io(ScheduleHost.agent_plugins(&f.schedules, &run, &()))
            .unwrap()
            .iter()
            .flat_map(|plugin| plugin.tools())
            .map(|tool| tool.name().to_owned())
            .collect()
    };
    let repo = PathBuf::from("/src/tau-agent");
    assert_eq!(
        tools(RunKind::Main, repo.clone()),
        ["schedule", "scheduled", "unschedule"]
    );
    assert_eq!(tools(RunKind::Chat, repo.clone()).len(), 3);
    assert_eq!(
        tools(RunKind::SubAgent, repo).len(),
        3,
        "the same prefix as main's"
    );
    assert!(
        tools(RunKind::Main, f.home.path().join("agents/triage")).is_empty(),
        "an agent's runs have its own"
    );
}

#[test]
fn a_sub_agent_is_told_to_hand_it_back() {
    use tau_agent::{
        plugin::Plugin,
        tool::{RunId, ToolCtx, ToolUpdates},
    };
    let f = Fixture::new();
    let tools = tau_schedule::host::ChatTools {
        schedules: f.schedules.clone(),
        sub_agent: true,
    }
    .tools();
    let (updates, _) = ToolUpdates::channel("call_1");
    let ctx = ToolCtx::new(
        tokio_util::sync::CancellationToken::new(),
        updates,
        RunId("sub-1".into()),
    );
    let error = block_on_io(
        tools[0]
            .call(json!({ "name": "recheck", "note": "n", "in": "1h" }), ctx),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("for the chat that started you"), "{error}");
    assert!(f.waiting("sub-1").is_empty());
}
