//! tau-goal in a host: Luau now (ADR 0050), run by tau-luau-plugins, so
//! these tests install the app's plugins, the Luau half among them. A
//! new chat shows the goal it inherits; a goal keeps the chat going
//! until it holds, live and in history, and takes what its banner's
//! buttons do.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    path::PathBuf,
    sync::{Arc, Mutex, Once},
    time::Duration,
};

use serde_json::{Value, json};
use tau_agent::{agent::Agent, event::RunEvent, tool::RunId};
use tau_testing::{git::git, scripted::ScriptedModel};
use tau_ui::{
    accounts::Credentials,
    host::{Host, HostConfig},
};
use tau_ui_remote::{
    models::ModelChoice,
    view::{Item, RunView},
};
use tau_vcs_host::{Identity, Project};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::common::LuauSettings as _;

const REPO: &str = "repo";
const WAIT: Duration = Duration::from_secs(60);

/// The app's plugins, the Luau ones among them: a process has one
/// registry.
fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| tau_ui_remote::plugins::install(tau::plugins()));
}

/// A directory of the test's own, where the host keeps its files.
fn own_dir() -> PathBuf {
    tempfile::tempdir().unwrap().keep()
}

/// A project imported from a new repository with one commit.
fn project() -> Project {
    let checkout = own_dir();
    git(&checkout, &["init", "--quiet"]);
    git(
        &checkout,
        &["commit", "--quiet", "--allow-empty", "-m", "first"],
    );
    tau_vcs_host::ProjectRepo::import(
        checkout.to_str().unwrap(),
        own_dir().join("p"),
        Identity::default(),
    )
    .map(Project::from)
    .unwrap()
}

/// A host on `llm` with one repository, its Jev `jev` if there is one.
fn host_on(
    llm: &ScriptedModel,
    jev: Option<tau_jev::fake::FakeJev>,
) -> (Host, UnboundedReceiver<RunEvent>) {
    install();
    let dir = own_dir();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let store = runtime.block_on(tau_store_sqlite::memory()).unwrap();
    let config = HostConfig {
        credentials: Credentials::new(dir.join("config")),
        model: Some("gpt-6-luna".into()),
        store: dir.join("runs.db"),
        repos: dir.join("repos"),
        settings: dir.join("models.json"),
        repo_list: dir.join("repos.json"),
        home: None,
        services: Default::default(),
    };
    let agent = Agent::new(llm.clone()).name("coder");
    let (host, events) = Host::with_agent(runtime, agent, store, config);
    // Memory's pass after each run asks the model once more: these
    // scripts answer only the requests they write.
    let host = host
        .with_luau_settings(
            tau_luau_plugins::names::MEMORY,
            json!({ "after_each_run": false }),
        )
        .with_repository(
            REPO,
            Arc::new(tau_vcs_host::JjRepository::new(
                project(),
                Identity::default(),
            )),
        );
    let host = match jev {
        Some(jev) => host.with_service(tau_jev::Upstream(Arc::new(jev))),
        None => host,
    };
    (host, events)
}

fn until_end(events: &mut UnboundedReceiver<RunEvent>) -> Vec<RunEvent> {
    let deadline = std::time::Instant::now() + WAIT;
    let mut seen = Vec::new();
    while std::time::Instant::now() < deadline {
        match events.try_recv() {
            Ok(event) => {
                let end = matches!(event, RunEvent::RunEnd { .. });
                seen.push(event);
                if end {
                    return seen;
                }
            }
            Err(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    panic!("no RunEnd within {WAIT:?}: {seen:?}");
}

fn wait_until_done(host: &Host, run: &RunId) {
    let deadline = std::time::Instant::now() + WAIT;
    while host.is_running(run) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!host.is_running(run));
}

/// tau-goal's state in `view`, as tau-luau-plugins' fold keeps it.
fn goal_of(view: &RunView) -> Value {
    view.plugin_states
        .get(tau_luau_plugins::NAME)
        .map(|state| {
            state.json()["plugins"][tau_luau_plugins::names::GOAL]["state"]
                .clone()
        })
        .unwrap_or_default()
}

/// The text of tau-goal's notes in `view`'s transcript, in order.
fn goal_notes(view: &RunView) -> Vec<String> {
    let state = goal_of(view);
    let prefix = format!("note-{}-", tau_luau_plugins::names::GOAL);
    view.items
        .iter()
        .filter_map(|item| match item {
            Item::Anchor { key, .. } => {
                let key = key.strip_prefix(&prefix)?;
                state["notes"][key]["text"].as_str().map(str::to_owned)
            }
            _ => None,
        })
        .collect()
}

/// Stores `state` as tau-goal's with `run`, as an interface does.
fn store_state(host: &Host, run: &RunId, state: Value) {
    let record = tau_luau_plugins::Record::State {
        plugin: tau_luau_plugins::names::GOAL.into(),
        state,
    };
    host.block_on(host.store_plugin_record(
        run,
        tau_luau_plugins::NAME,
        &serde_json::to_value(record).unwrap(),
    ))
    .unwrap();
}

/// Clicks the goal's banner button `what` on `run`, whose state is
/// `state`, and stores what it left, as the interface does.
fn click(host: &Host, run: &RunId, what: &str, state: Value) {
    let act = tau_luau_plugins::Act::Action {
        plugin: tau_luau_plugins::names::GOAL.into(),
        action: "act".into(),
        args: json!({ "what": what }),
        run: run.0.to_string(),
        repo: Some(REPO.into()),
        state,
        settings: None,
        live: false,
        jev: true,
    };
    let done = host
        .block_on(host.plugin_act(
            tau_luau_plugins::NAME,
            serde_json::to_value(act).unwrap(),
        ))
        .unwrap()
        .expect("what the action left");
    let done: tau_luau_plugins::ActionDone =
        serde_json::from_value(done).unwrap();
    assert_eq!(done.error, None);
    for record in done.records() {
        host.block_on(host.store_plugin_record(
            run,
            tau_luau_plugins::NAME,
            &serde_json::to_value(record).unwrap(),
        ))
        .unwrap();
    }
}

/// A new chat forks the main chat, and with it the goal set there: the
/// chat shows it from the start, and without a key nothing checks it.
#[test]
fn a_new_chat_shows_the_goal_it_inherits() {
    let llm = ScriptedModel::new()
        .turn(|t| t.text("hi"))
        .turn(|t| t.text("ok"));
    let (host, mut events) = host_on(&llm, None);
    let main = host.block_on(host.main_of(REPO)).unwrap();
    store_state(
        &host,
        &main,
        json!({ "goal": {
            "condition": "the docs build", "max_continuations": 4,
            "budget": 1.0, "status": "active", "continuations": 0,
            "spent": 0, "checks": [],
        } }),
    );
    // A turn after it: new chats fork from there, the goal before.
    host.block_on(host.resume(&main, "hello", &ModelChoice::default()))
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &main);
    let mut view = host
        .block_on(host.start("look around", &ModelChoice::default(), REPO))
        .unwrap();
    for event in until_end(&mut events) {
        view.apply(&event);
    }
    wait_until_done(&host, &view.id);
    let state = goal_of(&view);
    assert_eq!(state["goal"]["condition"], "the docs build");
    assert_eq!(state["goal"]["max_continuations"], 4);
    assert_eq!(state["checks"], false, "no key: nothing checks it");
}

#[test]
fn a_goal_keeps_the_chat_going_until_it_holds() {
    let llm = ScriptedModel::new()
        .turn(|t| t.text("tried"))
        .turn(|t| t.text("tests pass now"))
        .turn(|t| t.text("paused, so it stops"))
        .turn(|t| t.text("done for real"));
    // Not met, then met; then not met on every later check.
    let answers = Arc::new(Mutex::new(vec![0.1, 0.95, 0.2]));
    // Only the goal's question: tau-reasoning asks too, for "auto".
    let jev = tau_jev::fake::FakeJev::new(move |request| {
        if !request.questions.contains_key("met") {
            return Err(tau_jev::JevError::Status(400));
        }
        let mut answers = answers.lock().unwrap();
        let p = if answers.len() > 1 {
            answers.remove(0)
        } else {
            answers[0]
        };
        let answers = request
            .questions
            .keys()
            .map(|id| (id.clone(), tau_jev::Answer::Noul { noul: p }))
            .collect();
        Ok(tau_jev::fake::response(answers, request))
    });
    let (host, mut events) = host_on(&llm, Some(jev.clone()));

    let prompt = "/goal --continuations 3 the tests pass";
    let mut view = host
        .block_on(host.start(prompt, &ModelChoice::default(), REPO))
        .unwrap();
    assert_eq!(view.title, "the tests pass");
    for event in until_end(&mut events) {
        view.apply(&event);
    }
    wait_until_done(&host, &view.id);
    assert_eq!(
        goal_of(&view)["checks"],
        true,
        "with a key, tau-goal checks the run"
    );
    assert_eq!(llm.requests().len(), 2, "sent back once");
    let goal = goal_of(&view)["goal"].clone();
    assert_eq!(goal["condition"], "the tests pass");
    assert_eq!(goal["status"], "met");
    assert_eq!(
        (&goal["continuations"], &goal["max_continuations"]),
        (&json!(1), &json!(3))
    );
    assert_eq!(
        goal_notes(&view),
        ["the goal is not met yet", "the goal is met"]
    );

    // History has the goal: the message that set it, the same notes as
    // live, and the state; the check's note holds the continuation it
    // sent, which draws as nothing.
    let history = host.block_on(host.history()).unwrap();
    let stored = &history[0];
    assert_eq!(goal_of(stored)["goal"], goal_of(&view)["goal"]);
    assert!(matches!(&stored.items[0], Item::User(text)
        if goal_of(stored)["said"][text]["goal"] == "the tests pass"));
    assert_eq!(goal_notes(stored), goal_notes(&view));
    assert_eq!(goal_of(stored)["held"], json!({ "1": true }));

    // A met goal is not checked again; a new one is, until paused. This
    // one runs out at once, then gets more continuations, paused.
    let goal_checks = || {
        jev.requests()
            .iter()
            .filter(|request| request.questions.contains_key("met"))
            .count()
    };
    let asked = goal_checks();
    host.block_on(host.resume(
        &view.id,
        "/goal --continuations 0 it is released",
        &ModelChoice::default(),
    ))
    .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &view.id);
    assert_eq!(goal_checks(), asked + 1);
    let state = || goal_of(&host.block_on(host.history()).unwrap()[0]);
    assert_eq!(state()["goal"]["status"], "stopped");
    click(&host, &view.id, "keep_going", state());
    click(&host, &view.id, "pause", state());
    host.block_on(host.resume(
        &view.id,
        "one more thing",
        &ModelChoice::default(),
    ))
    .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &view.id);
    assert_eq!(goal_checks(), asked + 1, "paused: not checked");
    let goal = state()["goal"].clone();
    assert_eq!(goal["condition"], "it is released");
    assert_eq!(goal["status"], "paused");
    assert_eq!(goal["max_continuations"], 10);
    llm.assert_exhausted();
}
