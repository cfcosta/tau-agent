//! tau-constitution in a host: Luau now (ADR 0050), run by
//! tau-luau-plugins, so these tests install the app's plugins, the Luau
//! half among them. A rule written on the repository's page refuses a
//! call that breaks it, live and in history, and is edited, tried and
//! removed there; rules that cannot be read fail runs until they are
//! removed; and what waits for a person outlasts the host.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Once},
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
    view::{Item, RunView, ToolState},
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

/// A project imported from a new repository with one commit, under
/// `dir`.
fn project(dir: &Path) -> Project {
    let checkout = dir.join("checkout");
    if !checkout.exists() {
        std::fs::create_dir_all(&checkout).unwrap();
        git(&checkout, &["init", "--quiet"]);
        git(
            &checkout,
            &["commit", "--quiet", "--allow-empty", "-m", "first"],
        );
    }
    // Each host imports it afresh; its plugins' stores stay in `dir`.
    tau_vcs_host::ProjectRepo::import(
        checkout.to_str().unwrap(),
        own_dir().join("p"),
        Identity::default(),
    )
    .map(Project::from)
    .unwrap()
}

/// A host on `llm` keeping its files in `dir`, with one repository, and
/// a Jev that scores every rule `score`.
fn host_at(
    dir: &Path,
    llm: &ScriptedModel,
    score: Option<f64>,
) -> (Host, UnboundedReceiver<RunEvent>) {
    install();
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
                project(dir),
                Identity::default(),
            )),
        );
    let host = match score {
        Some(score) => {
            host.with_service(tau_jev::Upstream(Arc::new(jev(score))))
        }
        None => host,
    };
    (host, events)
}

/// A Jev that scores each rule `score`, and refuses what other plugins
/// ask.
fn jev(score: f64) -> tau_jev::fake::FakeJev {
    tau_jev::fake::FakeJev::new(move |request| {
        if !request.questions.keys().all(|id| id.starts_with("rule-")) {
            return Err(tau_jev::JevError::Status(400));
        }
        let answers = request
            .questions
            .keys()
            .map(|id| (id.clone(), tau_jev::Answer::Noul { noul: score }))
            .collect();
        Ok(tau_jev::fake::response(answers, request))
    })
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

/// Carries out the rules page's `action` with `args`, as a click on it
/// does.
fn page_act(host: &Host, action: &str, args: Value) {
    let act = tau_luau_plugins::Act::RepoAction {
        plugin: tau_constitution::NAME.into(),
        repo: REPO.into(),
        action: action.into(),
        args,
    };
    host.block_on(host.plugin_act(
        tau_luau_plugins::NAME,
        serde_json::to_value(act).unwrap(),
    ))
    .unwrap();
}

/// The rules page, as text.
fn page(host: &Host) -> String {
    let act = tau_luau_plugins::Act::Page {
        plugin: tau_constitution::NAME.into(),
        page: "rules".into(),
        params: [("repo".to_owned(), REPO.to_owned())].into(),
        jev: true,
    };
    let page = host
        .block_on(host.plugin_act(
            tau_luau_plugins::NAME,
            serde_json::to_value(act).unwrap(),
        ))
        .unwrap()
        .expect("a page answers");
    page["tree"].to_string()
}

/// What tau-constitution keeps of the repository, for its sidebar entry.
fn kept(host: &Host) -> Value {
    let catalog = host.block_on(host.catalog());
    let repo = catalog.repos.iter().find(|repo| repo.name == REPO).unwrap();
    repo.plugins[tau_luau_plugins::NAME].json()[tau_constitution::NAME].clone()
}

/// tau-constitution's state in `view`, as tau-luau-plugins' fold keeps it.
fn checks_of(view: &RunView) -> Value {
    view.plugin_states
        .get(tau_luau_plugins::NAME)
        .map(|state| {
            state.json()["plugins"][tau_constitution::NAME]["state"].clone()
        })
        .unwrap_or_default()
}

/// Writes a rule on the page: `text`, on `place`, at the thresholds the
/// editor starts with (0.3, 0.8).
fn write_rule(host: &Host, text: &str, place: &str) {
    page_act(host, "open", json!({}));
    page_act(host, "write", json!({ "text": text }));
    page_act(host, "place", json!({ "place": place }));
    page_act(host, "save", json!({}));
}

/// What tau-jev says Jev did this session, from its data.
fn jev_stats(host: &Host) -> tau_jev::ui::Stats {
    let catalog = host.block_on(host.catalog());
    let data = catalog.plugin_data[tau_jev::NAME].json();
    serde_json::from_value(data["stats"].clone()).expect("Jev is set up")
}

#[test]
fn the_constitution_blocks_a_call_that_breaks_a_rule() {
    let dir = own_dir();
    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call(
                "write",
                json!({ "path": "a.txt", "content": "x.unwrap()" }),
            )
        })
        .turn(|t| t.text("I could not write it."));
    let (host, mut events) = host_at(&dir, &llm, Some(0.95));
    // A rule saved the way the Constitution page saves one.
    write_rule(&host, "No unwrap.", "write.content");
    assert_eq!(kept(&host)["rules"], 1);
    assert_eq!(kept(&host)["nav"]["detail"], "1 rules");
    let drawn = page(&host);
    assert!(drawn.contains("Checks write.content"), "{drawn}");

    let mut view = host
        .block_on(host.start("write a.txt", &ModelChoice::default(), REPO))
        .unwrap();
    for event in until_end(&mut events) {
        view.apply(&event);
    }
    wait_until_done(&host, &view.id);
    let blocked = |view: &RunView| {
        view.items.iter().any(|item| {
            matches!(item, Item::Tool(card)
                if matches!(&card.state, ToolState::Blocked { .. }))
        }) && checks_of(view)["stats"]["blocked"] == json!(["R1"])
    };
    assert!(blocked(&view), "the card shows the block live");
    assert_eq!(checks_of(&view)["stats"]["calls"], 1);
    // tau-jev counts the check Jev answered.
    let stats = jev_stats(&host);
    assert!(stats.requests >= 1, "{stats:?}");
    assert!(stats.input_tokens > 0 && stats.spent > 0.0);
    // And in history, from what the plugin left.
    let history = host.block_on(host.history()).unwrap();
    assert!(blocked(&history[0]));
    assert_eq!(checks_of(&history[0])["stats"], checks_of(&view)["stats"]);
    // The page counts the run.
    let drawn = page(&host);
    assert!(
        drawn.contains("Across 1 run") && drawn.contains("1 blocked"),
        "{drawn}"
    );

    // Editing a rule saves over it, in place.
    page_act(&host, "open", json!({ "rule": "R1" }));
    page_act(&host, "write", json!({ "text": "No unwrap, ever." }));
    page_act(&host, "nudge", json!({ "mark": "block", "delta": 0.05 }));
    page_act(&host, "save", json!({}));
    let drawn = page(&host);
    assert!(
        drawn.contains("No unwrap, ever.") && drawn.contains("block 0.85"),
        "{drawn}"
    );
    assert_eq!(kept(&host)["rules"], 1);
    // Trying a rule asks Jev about the run's call it reads.
    page_act(&host, "open", json!({}));
    page_act(&host, "write", json!({ "text": "No unwrap." }));
    page_act(&host, "place", json!({ "place": "write.content" }));
    page_act(&host, "try", json!({}));
    let drawn = page(&host);
    assert!(
        drawn.contains("x.unwrap()") && drawn.contains("0.95 would block"),
        "{drawn}"
    );
    page_act(&host, "close", json!({}));

    // Removing a rule saves it.
    page_act(&host, "remove", json!({ "id": "R1" }));
    assert_eq!(kept(&host)["rules"], 0);
    assert!(page(&host).contains("No rules for repo"));
}

/// Rules that cannot be read fail runs at start once there is a key,
/// no edit gets past them, and the page removes them, after which its
/// settings are saved like its rules.
#[test]
fn a_broken_constitution_can_be_removed_and_settings_are_saved() {
    let dir = own_dir();
    let llm = ScriptedModel::new().turn(|t| t.text("hi"));
    let (host, _events) = host_at(&dir, &llm, Some(0.0));
    // Flagged past where it blocks: it does not check.
    let stores = tau_luau_plugins_host::store::Stores::at(
        host.plugin_dir(tau_luau_plugins::NAME).join("stores"),
    );
    host.block_on(async {
        let kv = stores.of(tau_constitution::NAME, REPO).await.unwrap();
        let mut session = kv.session().await;
        session.put(
            "constitution",
            json!({ "on_error": "allow", "max_holds": 3, "rules": [{
                "id": "R1", "text": "Name the tests.", "on": ["final answer"],
                "review": 0.9, "block": 0.2,
            }] }),
        );
        session.commit().await.unwrap();
    });
    let drawn = page(&host);
    assert!(drawn.contains("can't be read"), "{drawn}");
    assert!(
        drawn.contains("review (0.9) is above block (0.2)"),
        "{drawn}"
    );
    // A run fails at start, before it keeps the message, saying why.
    let started =
        host.block_on(host.start("hello", &ModelChoice::default(), REPO));
    let Err(error) = started else {
        panic!("the run fails")
    };
    let said = format!("{error:#}");
    assert!(
        said.contains("The constitution stored for repo is not valid"),
        "{said}"
    );
    // No edit gets past them.
    page_act(&host, "open", json!({}));
    page_act(&host, "write", json!({ "text": "No unwrap." }));
    page_act(&host, "place", json!({ "place": "write.content" }));
    let act = tau_luau_plugins::Act::RepoAction {
        plugin: tau_constitution::NAME.into(),
        repo: REPO.into(),
        action: "save".into(),
        args: json!({}),
    };
    assert!(
        host.block_on(host.plugin_act(
            tau_luau_plugins::NAME,
            serde_json::to_value(act).unwrap(),
        ))
        .is_err()
    );
    page_act(&host, "close", json!({}));
    assert!(page(&host).contains("can't be read"));

    page_act(&host, "ask_reset", json!({ "yes": true }));
    page_act(&host, "reset", json!({}));
    assert!(page(&host).contains("No rules for repo"));
    write_rule(&host, "No unwrap.", "write.content");
    assert_eq!(kept(&host)["rules"], 1);
    page_act(
        &host,
        "settings",
        json!({ "blocks_unchecked": true, "max_holds": 5 }),
    );
    let drawn = page(&host);
    assert!(
        drawn.contains("Calls it could not check are refused")
            && drawn.contains("Up to 5;"),
        "{drawn}"
    );
}

/// A flagged call waits for a person, also after the host restarts, until
/// it looks fine.
#[test]
fn what_waits_for_a_person_outlasts_the_host() {
    let dir = own_dir();
    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call("write", json!({ "path": "a.txt", "content": "maybe" }))
        })
        .turn(|t| t.text("wrote it"));
    let (host, mut events) = host_at(&dir, &llm, Some(0.5));
    write_rule(&host, "No unwrap.", "write.content");
    let mut view = host
        .block_on(host.start("write a.txt", &ModelChoice::default(), REPO))
        .unwrap();
    for event in until_end(&mut events) {
        view.apply(&event);
    }
    wait_until_done(&host, &view.id);
    // It ran, flagged.
    assert!(view.items.iter().any(|item| matches!(item, Item::Tool(card)
        if matches!(&card.state, ToolState::Flagged { .. }))));
    assert_eq!(kept(&host)["waiting"], 1);
    assert_eq!(kept(&host)["nav"]["badge"], "1");
    drop(host);

    let (host, _events) = host_at(&dir, &ScriptedModel::new(), Some(0.5));
    assert_eq!(kept(&host)["waiting"], 1);
    let drawn = page(&host);
    assert!(drawn.contains("Waiting for you · 1"), "{drawn}");
    page_act(&host, "fine", json!({ "id": "review/000000001" }));
    assert_eq!(kept(&host)["waiting"], 0);
    assert!(page(&host).contains("looked fine"));
}
