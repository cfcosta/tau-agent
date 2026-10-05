//! Landing forecasts through the real host and the workspace: a fork
//! that finishes gets one in the background, and it follows its
//! parent's head. Every chat forks its repository's main chat, so a
//! chat that lands moves main, and the other chat's forecast changes
//! with it.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0027)"
)]

use std::time::Duration;

use gpui::{Entity, TestAppContext, VisualTestContext};
use serde_json::json;
use tau_agent::tool::RunId;
use tau_testing::{git::git, scripted::ScriptedModel};
use tau_ui::{
    accounts::Credentials,
    host::{Host, HostConfig},
};
use tau_ui_remote::{
    Workspace,
    attention::{Attention, Forecast},
    catalog::Catalog,
    route::Route,
};
use tau_vcs::{Identity, Project};

/// Runs GPUI until `done` holds, while runs work on the host's threads.
fn until(
    cx: &mut VisualTestContext,
    what: &str,
    mut done: impl FnMut(&mut VisualTestContext) -> bool,
) {
    for _ in 0..600 {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}

fn start(
    workspace: &Entity<Workspace>,
    cx: &mut VisualTestContext,
    prompt: &str,
) -> RunId {
    let before = workspace.read_with(cx, |ws, _| ws.runs().len());
    workspace.update(cx, |ws, cx| {
        ws.navigate(Route::NewRun, cx);
        ws.submit_prompt(prompt.to_owned(), cx);
    });
    until(cx, "the run to start", |cx| {
        workspace.read_with(cx, |ws, _| ws.runs().len() > before)
    });
    workspace.read_with(cx, |ws, _| ws.runs()[0].id.clone())
}

fn forecast(
    workspace: &Entity<Workspace>,
    cx: &mut VisualTestContext,
    run: &RunId,
) -> Option<Forecast> {
    workspace.read_with(cx, |ws, _| ws.run(run)?.forecast.clone())
}

#[gpui::test]
fn a_finished_fork_is_forecast_again_when_main_moves(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let src = tempfile::tempdir().unwrap();
    git(src.path(), &["init", "--quiet"]);
    std::fs::write(src.path().join("README.md"), "hello\n").unwrap();
    git(src.path(), &["add", "README.md"]);
    git(src.path(), &["commit", "--quiet", "-m", "first"]);
    let repos = tempfile::tempdir().unwrap();
    let project = Project::import(
        src.path().to_str().unwrap(),
        repos.path().join("p"),
        Identity::default(),
    )
    .unwrap();

    let readme = |text: &str| json!({ "path": "README.md", "content": text });
    let commit = |message: &str| json!({ "message": message });
    // Two chats change the same line.
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("write", readme("one\n")))
        .turn(|t| t.tool_call("vcs_commit", commit("docs: one")))
        .turn(|t| t.text("one is done"))
        .turn(|t| t.tool_call("write", readme("two\n")))
        .turn(|t| t.tool_call("vcs_commit", commit("docs: two")))
        .turn(|t| t.text("two is done"));

    cx.update(tau_ui_remote::init);
    let window = cx.add_window(|window, cx| {
        Workspace::new("tau", Vec::new(), Catalog::default(), window, cx)
    });
    let workspace = window.root(cx).unwrap();
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let store = runtime.block_on(tau_store::Store::memory()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let config = HostConfig {
        account: tau_ai::chatgpt::AccountId::parse("test-account").unwrap(),
        credentials: Credentials::new(dir.path().join("config")),
        model: Some("gpt-5.5".into()),
        store: dir.path().join("unused.db"),
        repos: dir.path().join("repos"),
        settings: dir.path().join("models.json"),
        repo_list: dir.path().join("repos.json"),
        skills: std::env::temp_dir().join("tau-test-skills-none"),
    };
    let agent = tau_agent::agent::Agent::new(llm.clone()).name("coder");
    let (host, events) = Host::with_agent(runtime, agent, store, config);
    let host = host.with_repo("hello", project.clone());
    cx.update(|_, cx| host.attach(&workspace, events, cx));

    // The first chat finishes: in the background, it is ready to land
    // its one change.
    let one = start(&workspace, &mut cx, "write one");
    until(&mut cx, "the first chat's forecast", |cx| {
        forecast(&workspace, cx, &one).is_some()
    });
    assert_eq!(
        forecast(&workspace, &mut cx, &one),
        Some(Forecast {
            changes: 1,
            conflicts: Vec::new()
        })
    );
    workspace.update(&mut cx, |ws, cx| {
        let view = ws.run(&one).unwrap().clone();
        assert_eq!(
            ws.attention(&view, cx),
            Attention::ReadyToLand { changes: 1 }
        );
    });

    // The second changes the same line, and lands first: main moves,
    // and the first chat would now conflict.
    let two = start(&workspace, &mut cx, "write two");
    until(&mut cx, "the second chat's forecast", |cx| {
        forecast(&workspace, cx, &two).is_some()
    });
    workspace.update(&mut cx, |ws, cx| ws.land(&two, cx));
    until(&mut cx, "the second chat to land", |cx| {
        workspace.read_with(cx, |ws, _| ws.is_closed(&two))
    });
    until(
        &mut cx,
        "the first chat's forecast after main moved",
        |cx| {
            forecast(&workspace, cx, &one)
                .is_some_and(|forecast| !forecast.conflicts.is_empty())
        },
    );
    assert_eq!(
        forecast(&workspace, &mut cx, &one),
        Some(Forecast {
            changes: 1,
            conflicts: vec!["README.md".into()]
        })
    );
    workspace.update(&mut cx, |ws, cx| {
        let view = ws.run(&one).unwrap().clone();
        assert_eq!(
            ws.attention(&view, cx),
            Attention::WouldConflict {
                files: vec!["README.md".into()]
            }
        );
        // The repository counts the one chat that needs the person.
        assert_eq!(ws.need_you("hello", cx), 1);
    });
    // Forecasting changed nothing: main has the second chat's line.
    let trunk = project.trunk().unwrap();
    assert_eq!(
        project.file_at(&trunk, "README.md").unwrap().unwrap().0,
        b"two\n"
    );
    llm.assert_exhausted();
}
