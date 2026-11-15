//! A measurement, not a check: what a run costs the host when the model
//! answers at once, with every built-in plugin on. `scripts/perf.sh`
//! runs it and prints what `heiko-perf` kept.
//!
//! | Property | Kind |
//! |---|---|
//! | a scripted run of many turns ends, and its hooks were measured | example, ignored |

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    path::PathBuf,
    sync::{Arc, Once},
    time::{Duration, Instant},
};

use heiko_agent::{agent::Agent, event::RunEvent};
use heiko_testing::{git::git, scripted::ScriptedModel};
use heiko_ui::{
    accounts::Credentials,
    host::{Host, HostConfig},
};
use heiko_ui_remote::models::ModelChoice;
use heiko_vcs_host::{Identity, Project};
use serde_json::json;

use crate::common::LuauSettings as _;

const REPO: &str = "repo";
const WAIT: Duration = Duration::from_secs(300);

/// How many turns the run takes, each with three tool calls.
const TURNS: usize = 60;

fn own_dir() -> PathBuf {
    tempfile::tempdir().unwrap().keep()
}

fn project() -> Project {
    let checkout = own_dir();
    git(&checkout, &["init", "--quiet"]);
    git(
        &checkout,
        &["commit", "--quiet", "--allow-empty", "-m", "first"],
    );
    heiko_vcs_host::ProjectRepo::import(
        checkout.to_str().unwrap(),
        own_dir().join("p"),
        Identity::default(),
    )
    .map(Project::from)
    .unwrap()
}

#[test]
#[ignore = "a measurement: scripts/perf.sh runs it"]
fn a_scripted_run_of_many_turns_is_measured() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| heiko_ui_remote::plugins::install(heiko::plugins()));
    let mut llm = ScriptedModel::new();
    for turn in 0..TURNS {
        let path = format!("notes/{turn}.md");
        llm = llm.turn(|t| {
            t.text("Next file.")
                .tool_call("ls", json!({ "path": "." }))
                .tool_call(
                    "write",
                    json!({ "path": path, "content": "one\ntwo\n".repeat(40) }),
                )
                .tool_call("read", json!({ "path": path }))
                .usage(20_000 + 500 * turn as u64, 300)
        });
    }
    let llm = llm.turn(|t| t.text("All done."));

    let dir = own_dir();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let store = runtime.block_on(heiko_store_sqlite::memory()).unwrap();
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
    let (host, mut events) = Host::with_agent(runtime, agent, store, config);
    // Memory's pass after each run asks the model once more, which the
    // script does not answer.
    let host = host
        .with_luau_settings(
            heiko_luau_plugins::names::MEMORY,
            json!({ "after_each_run": false }),
        )
        .with_repository(
            REPO,
            Arc::new(heiko_vcs_host::JjRepository::new(
                project(),
                Identity::default(),
            )),
        );
    host.measure_lag();

    let started = Instant::now();
    let view = host
        .block_on(host.start("tidy the notes", &ModelChoice::default(), REPO))
        .unwrap();
    let mut turns = 0;
    loop {
        assert!(started.elapsed() < WAIT, "no RunEnd within {WAIT:?}");
        match events.try_recv() {
            Ok(RunEvent::RunEnd { run, .. }) if run == view.id => break,
            Ok(RunEvent::TurnEnd { run, .. }) if run == view.id => turns += 1,
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(1)),
        }
    }
    let whole = started.elapsed();
    heiko_perf::record("run", "whole", whole);
    assert!(turns > TURNS, "{turns} turns");

    let report = heiko_perf::report();
    assert!(report.groups.contains_key("hook"), "{report:?}");
    println!(
        "{TURNS} turns of three tool calls, the model answering at once: \
         {:.0} ms a turn\n",
        whole.as_secs_f64() * 1000. / turns as f64
    );
    println!("{}", report.table());
}
