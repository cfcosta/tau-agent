//! tau-reasoning in a host: Luau now (ADR 0050), run by tau-luau-plugins,
//! so these tests install the app's plugins, the Luau half among them.
//! On auto, Jev's pick is the effort the model is asked at and the
//! plan's; an effort someone chose is not scored; the Models screen's
//! settings reach it; and each message is scored again, its notes after
//! the message they were for.

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
use tau_agent::{
    agent::Agent,
    event::{RunEvent, StopReason},
    tool::RunId,
};
use tau_ai::responses::request::ReasoningEffort;
use tau_testing::{git::git, scripted::ScriptedModel};
use tau_ui::{
    accounts::Credentials,
    host::{Host, HostConfig},
};
use tau_ui_remote::{
    models::{Effort, ModelChoice},
    view::{Item, RunView},
};
use tau_vcs_host::{Identity, Project};
use tokio::sync::mpsc::UnboundedReceiver;

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

/// A host on `llm` with one repository, its Jev `jev`.
fn host_on(
    llm: &ScriptedModel,
    jev: tau_jev::fake::FakeJev,
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
        .with_plugin_settings(
            tau_memory::NAME,
            json!({ "after_each_run": false }),
        )
        .with_repository(
            REPO,
            Arc::new(tau_vcs_host::JjRepository::new(
                project(),
                Identity::default(),
            )),
        )
        .with_service(tau_jev::Upstream(Arc::new(jev)));
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

/// tau-reasoning's state in `view`, as tau-luau-plugins' fold keeps it.
fn reasoning_of(view: &RunView) -> Value {
    view.plugin_states
        .get(tau_luau_plugins::NAME)
        .map(|state| {
            state.json()["plugins"][tau_reasoning::NAME]["state"].clone()
        })
        .unwrap_or_default()
}

/// The key a note of tau-reasoning's is anchored at, if `key` is one.
fn note_key(key: &str) -> Option<&str> {
    key.strip_prefix(&format!("note-{}-", tau_reasoning::NAME))
}

/// A Jev that scores each request with `probabilities`, one per level,
/// at `confidence`, and leases `lease` when asked.
fn scoring(
    probabilities: Vec<f64>,
    confidence: f64,
    lease: &'static str,
) -> tau_jev::fake::FakeJev {
    tau_jev::fake::FakeJev::new(move |request| {
        let answers = request
            .questions
            .keys()
            .map(|id| {
                let answer = if id == "lease" {
                    tau_jev::Answer::Choice {
                        choice: lease.into(),
                        probabilities: [(lease.to_owned(), 0.9)].into(),
                        confidence: 0.9,
                    }
                } else {
                    tau_jev::Answer::Score {
                        score: 3.0,
                        probabilities: probabilities
                            .iter()
                            .enumerate()
                            .map(|(n, p)| (n.to_string(), *p))
                            .collect(),
                        confidence,
                    }
                };
                (id.clone(), answer)
            })
            .collect();
        Ok(tau_jev::fake::response(answers, request))
    })
}

/// The Plugins screen lists tau-reasoning among tau's Luau plugins.
#[test]
fn it_is_listed_among_the_luau_plugins() {
    let llm = ScriptedModel::new();
    let (host, _events) = host_on(&llm, scoring(vec![1.0], 0.9, "one_call"));
    let catalog = host.block_on(host.catalog());
    let luau = catalog
        .plugins
        .iter()
        .find(|plugin| plugin.name == tau_luau_plugins::NAME)
        .expect("tau-luau-plugins is listed");
    let entry = luau
        .entries
        .iter()
        .find(|entry| entry.name == tau_reasoning::NAME)
        .unwrap_or_else(|| panic!("tau-reasoning is a row: {luau:?}"));
    assert_eq!(entry.note, None, "it loads and passes its tests");
}

#[test]
fn auto_reasoning_takes_the_effort_jev_picks() {
    let llm = ScriptedModel::new().turn(|t| t.text("done"));
    // Jev is sure the task wants high.
    let jev = scoring(vec![0.01, 0.02, 0.09, 0.84, 0.04], 0.84, "one_call");
    let (host, mut events) = host_on(&llm, jev);
    let auto = ModelChoice::new("gpt-5.5", Effort::Auto);
    let mut view = host
        .block_on(host.start("track down the race", &auto, REPO))
        .unwrap();
    for event in until_end(&mut events) {
        view.apply(&event);
    }
    wait_until_done(&host, &view.id);
    assert_eq!(
        llm.requests()[0].settings.reasoning,
        Some(ReasoningEffort::High)
    );
    // The plan's reasoning, as tau-reasoning's state says it.
    let chosen = |view: &RunView| reasoning_of(view)["plan"] == "high";
    assert!(chosen(&view), "{:?}", view.plugin_states);
    assert!(view.items.iter().any(|item| matches!(item,
        Item::Anchor { key, .. } if note_key(key).is_some())));
    // History shows it again, from the plugin's record.
    assert!(chosen(&host.block_on(host.history()).unwrap()[0]));

    // An effort someone chose is not scored: Jev is never asked, and
    // the run asks the model at that effort and ends as it should.
    let llm = ScriptedModel::new().turn(|t| t.text("ok"));
    let jev = tau_jev::fake::FakeJev::nouls(|_| 0.5);
    let (host, mut events) = host_on(&llm, jev.clone());
    let low = ModelChoice::new("gpt-5.5", Effort::Low);
    let run = host
        .block_on(host.start("rename a variable", &low, REPO))
        .unwrap();
    let stops: Vec<StopReason> = until_end(&mut events)
        .into_iter()
        .filter_map(|event| match event {
            RunEvent::RunEnd { stop, .. } => Some(stop),
            _ => None,
        })
        .collect();
    wait_until_done(&host, &run.id);
    assert_eq!(stops, [StopReason::Stop]);
    assert!(jev.requests().is_empty(), "{:?}", jev.requests());
    assert_eq!(
        llm.requests()[0].settings.reasoning,
        Some(ReasoningEffort::Low)
    );
}

/// The settings the person gives tau-reasoning, as tau-luau-plugins
/// keeps them, reach it: asked to decide again between steps, it asks
/// Jev how long the effort holds; asked for more confidence than Jev
/// has, it keeps the default.
#[test]
fn reasoning_settings_reach_the_plugin() {
    let llm = ScriptedModel::new().turn(|t| t.text("done"));
    // 0.84 sure of high, and sure the effort holds for one call.
    let jev = scoring(vec![0.01, 0.02, 0.09, 0.84, 0.04], 0.84, "one_call");
    let (host, mut events) = host_on(&llm, jev.clone());
    let mut settings = tau_ui_remote::models::ModelSettings::default();
    settings.plugins.insert(
        tau_luau_plugins::NAME.into(),
        json!({ "plugins": {
            tau_reasoning::NAME: { "redecide": true, "threshold": 0.9 },
        } }),
    );
    host.block_on(host.save_settings(settings)).unwrap();
    let auto = ModelChoice::new("gpt-5.5", Effort::Auto);
    let view = host
        .block_on(host.start("track down the race", &auto, REPO))
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &view.id);
    let asked = jev.requests();
    assert!(
        asked[0].questions.contains_key("lease"),
        "deciding again asks how long the effort holds"
    );
    assert_eq!(
        llm.requests()[0].settings.reasoning,
        None,
        "0.84 is short of 0.9: the model's default"
    );
}

/// Each message of a chat on auto is scored on its own: an effort
/// tau-reasoning picked does not carry over. History shows each choice
/// after the message it was for.
#[test]
fn each_message_is_scored_again() {
    let llm = ScriptedModel::new()
        .turn(|t| t.text("fine, thanks"))
        .turn(|t| t.text("a long answer"));
    // gpt-5.5's levels: none, low, medium, high, xhigh. The first
    // message wants none, the second xhigh.
    let wants = Arc::new(Mutex::new(vec![4, 0]));
    let jev = tau_jev::fake::FakeJev::new(move |request| {
        let level = wants.lock().unwrap().pop().unwrap();
        let answers = request
            .questions
            .keys()
            .map(|id| {
                let probabilities = (0..5)
                    .map(|n| {
                        (n.to_string(), if n == level { 0.96 } else { 0.01 })
                    })
                    .collect();
                (
                    id.clone(),
                    tau_jev::Answer::Score {
                        score: level as f64,
                        probabilities,
                        confidence: 0.96,
                    },
                )
            })
            .collect();
        Ok(tau_jev::fake::response(answers, request))
    });
    let (host, mut events) = host_on(&llm, jev);
    let auto = ModelChoice::new("gpt-5.5", Effort::Auto);
    let mut view = host
        .block_on(host.start("how are you?", &auto, REPO))
        .unwrap();
    for event in until_end(&mut events) {
        view.apply(&event);
    }
    wait_until_done(&host, &view.id);
    // What the composer sends next: auto again, not the picked none.
    let next = tau_ui_remote::Workspace::model_of(&view);
    assert_eq!(next, auto);
    host.block_on(host.resume(&view.id, "prove the Riemann hypothesis", &next))
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &view.id);
    let asked: Vec<_> = llm
        .requests()
        .iter()
        .map(|request| request.settings.reasoning)
        .collect();
    assert_eq!(
        asked,
        [Some(ReasoningEffort::None), Some(ReasoningEffort::Xhigh)]
    );

    // History: each choice right after its message.
    let history = host.block_on(host.history()).unwrap();
    let state = reasoning_of(&history[0]);
    let order: Vec<String> = history[0]
        .items
        .iter()
        .filter_map(|item| match item {
            Item::User(text) => Some(format!("user: {text}")),
            Item::Anchor { key, .. } => {
                let note = &state["notes"][note_key(key)?];
                note["text"]
                    .as_str()
                    .or(note["message"].as_str())
                    .map(str::to_owned)
            }
            Item::Text(text) => Some(format!("reply: {text}")),
            _ => None,
        })
        .collect();
    assert_eq!(
        order,
        [
            "user: how are you?",
            "picked **none** reasoning for this message",
            "reply: fine, thanks",
            "user: prove the Riemann hypothesis",
            "reasoning **none** → **xhigh**",
            "reply: a long answer",
        ]
    );
}
