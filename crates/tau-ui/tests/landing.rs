//! Landing through the real host and the workspace (ADR 0014): a run
//! proposes its landing with `vcs_land` and its card opens once it
//! stops. Every chat forks its repository's main chat, which commits on
//! trunk, so landing on it moves main; a landing that conflicts starts
//! the main chat's turn to resolve it, and its commit moves main too.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
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
    catalog::Catalog,
    view::Item,
    workspace::{LandingState, WorkspaceEvent},
};
use tau_vcs_host::{Identity, Project};

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
        // A chat forked from main, as "New run" asks for once main
        // was talked to; on an untouched main the composer goes to main.
        cx.emit(WorkspaceEvent::NewRun {
            prompt: prompt.to_owned(),
            model: ws.next_model().clone(),
            repo: ws.selected_repo().unwrap_or_default().to_owned(),
        });
    });
    until(cx, "the run to start", |cx| {
        workspace.read_with(cx, |ws, _| ws.runs().len() > before)
    });
    workspace.read_with(cx, |ws, _| ws.runs()[0].id.clone())
}

fn finished(
    workspace: &Entity<Workspace>,
    cx: &mut VisualTestContext,
    run: &RunId,
) {
    until(cx, "the run to finish", |cx| {
        workspace.read_with(cx, |ws, _| {
            ws.run(run).is_some_and(|view| !view.status.is_live())
        })
    });
}

fn landing(
    workspace: &Entity<Workspace>,
    cx: &mut VisualTestContext,
    run: &RunId,
) -> Option<LandingState> {
    workspace.read_with(cx, |ws, _| ws.landing(run).cloned())
}

#[gpui::test]
fn runs_land_on_the_main_chat_and_move_main(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let src = tempfile::tempdir().unwrap();
    git(src.path(), &["init", "--quiet"]);
    std::fs::write(src.path().join("README.md"), "hello\n").unwrap();
    git(src.path(), &["add", "README.md"]);
    git(src.path(), &["commit", "--quiet", "-m", "first"]);
    let repos = tempfile::tempdir().unwrap();
    let project = tau_vcs_host::ProjectRepo::import(
        src.path().to_str().unwrap(),
        repos.path().join("p"),
        Identity::default(),
    )
    .map(Project::from)
    .unwrap();

    let readme = |text: &str| json!({ "path": "README.md", "content": text });
    let commit = |message: &str| json!({ "message": message });
    let llm = ScriptedModel::new()
        // The first run commits, and proposes its landing.
        .turn(|t| t.tool_call("write", readme("one\n")))
        .turn(|t| t.tool_call("vcs_commit", commit("docs: one")))
        .turn(|t| t.tool_call("vcs_land", json!({})))
        .turn(|t| t.text("one is done"))
        // The second changes the same line.
        .turn(|t| t.tool_call("write", readme("two\n")))
        .turn(|t| t.tool_call("vcs_commit", commit("docs: two")))
        .turn(|t| t.text("two is done"))
        // The first run's resolving turn.
        .turn(|t| t.tool_call("write", readme("one and two\n")))
        .turn(|t| t.tool_call("vcs_commit", commit("docs: keep both")))
        .turn(|t| t.text("resolved"));

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
    let store = runtime.block_on(tau_store_sqlite::memory()).unwrap();
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
    // Memory's pass after each run asks the model once more: these
    // scripts answer only the requests they write.
    let host = host.with_plugin_settings(
        tau_memory::NAME,
        serde_json::json!({ "after_each_run": false }),
    );
    let host = host.with_repo("hello", project.clone());
    cx.update(|_, cx| host.attach(&workspace, events, cx));
    // The host shows its catalog and history from its runtime, a moment
    // after.
    until(&mut cx, "the host's catalog and history", |cx| {
        workspace.read_with(cx, |ws, _| {
            !ws.catalog().repos.is_empty() && !ws.runs().is_empty()
        })
    });

    // The first run proposed its landing: once it stops, the card opens
    // with what landing on the main chat would do.
    let one = start(&workspace, &mut cx, "write one");
    finished(&workspace, &mut cx, &one);
    until(&mut cx, "the proposed landing's preview", |cx| {
        matches!(
            landing(&workspace, cx, &one),
            Some(LandingState::Preview(_))
        )
    });
    let Some(LandingState::Preview(Ok(preview))) =
        landing(&workspace, &mut cx, &one)
    else {
        panic!("{:?}", landing(&workspace, &mut cx, &one));
    };
    assert_eq!(preview.changes.len(), 1);
    assert!(preview.conflicts.is_empty());

    // Every chat forks the main chat, which commits on trunk: landing
    // on it moves main.
    let main = workspace.read_with(&cx, |ws, _| {
        ws.catalog().repos[0].main.clone().expect("a main chat")
    });

    // The second run lands on the main chat first, cleanly: main moves.
    let two = start(&workspace, &mut cx, "write two");
    finished(&workspace, &mut cx, &two);
    workspace.update(&mut cx, |ws, cx| ws.land(&two, cx));
    until(&mut cx, "the second run to land", |cx| {
        workspace.read_with(cx, |ws, _| ws.is_closed(&two))
    });
    workspace.read_with(&cx, |ws, _| {
        let items = &ws.run(&main).unwrap().items;
        assert!(
            matches!(items.last(), Some(Item::Landed(card)) if card.from == two),
            "{items:?}"
        );
    });
    let trunk = project.blocking().trunk().unwrap();
    assert_eq!(
        project
            .blocking()
            .file_at(&trunk, "README.md")
            .unwrap()
            .unwrap()
            .0,
        b"two\n"
    );

    // Now the first run's landing conflicts: the main chat resolves it
    // in a turn tau starts, and its commit moves main.
    workspace.update(&mut cx, |ws, cx| ws.preview_landing(&one, cx));
    until(&mut cx, "the new preview", |cx| {
        matches!(
            landing(&workspace, cx, &one),
            Some(LandingState::Preview(_))
        )
    });
    let Some(LandingState::Preview(Ok(preview))) =
        landing(&workspace, &mut cx, &one)
    else {
        panic!("{:?}", landing(&workspace, &mut cx, &one));
    };
    assert_eq!(preview.conflicts, ["README.md"]);
    workspace.update(&mut cx, |ws, cx| ws.land(&one, cx));
    until(&mut cx, "the main chat to resolve", |cx| {
        project
            .blocking()
            .file_at(&project.blocking().trunk().unwrap(), "README.md")
            .unwrap()
            .is_some_and(|(bytes, _)| bytes == b"one and two\n")
            && workspace.read_with(cx, |ws, _| {
                ws.is_closed(&one)
                    && ws.run(&main).is_some_and(|view| !view.status.is_live())
            })
    });
    workspace.read_with(&cx, |ws, _| {
        let items = &ws.run(&main).unwrap().items;
        let tau = items.iter().find_map(|item| match item {
            Item::Tau(text) => Some(text.clone()),
            _ => None,
        });
        let tau = tau.expect("tau's resolving turn");
        assert!(tau.contains("`README.md`"), "{tau}");
    });
    llm.assert_exhausted();
    let trunk = project.blocking().trunk().unwrap();
    assert_eq!(
        project
            .blocking()
            .file_at(&trunk, "README.md")
            .unwrap()
            .unwrap()
            .0,
        b"one and two\n"
    );
    let log: Vec<String> = project
        .blocking()
        .stack(&trunk)
        .unwrap()
        .into_iter()
        .map(|change| change.description.trim().to_owned())
        .collect();
    assert!(log.is_empty(), "trunk is the stack's end: {log:?}");
}
