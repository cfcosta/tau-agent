//! The host runs agents and reports their events; the view folds them.
//! With a project, each run works in a workspace of its own, forks
//! start from a turn's code, and past runs come back as history.

use std::{path::Path, process::Command, time::Duration};

use tau_agent::{
    agent::Agent,
    event::{RunEvent, StopReason},
};
use tau_store::Store;
use tau_testing::scripted::ScriptedModel;
use tau_ui::{
    host::{Access, Host, HostConfig},
    view::{DiffKind, FileStat, Item, Origin, RunStatus, ToolState},
};
use tau_vcs::{Identity, Project};
use tokio::sync::mpsc::UnboundedReceiver;

fn host(llm: ScriptedModel) -> (Host, UnboundedReceiver<RunEvent>) {
    host_on(llm, &std::env::temp_dir())
}

fn host_on(
    llm: ScriptedModel,
    root: &Path,
) -> (Host, UnboundedReceiver<RunEvent>) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let store = runtime.block_on(Store::memory()).unwrap();
    let agent = Agent::new(llm).name("coder");
    let config = HostConfig {
        access: Access::ApiKey("sk-test".into()),
        model: "gpt-5.5".into(),
        root: root.to_owned(),
        store: std::env::temp_dir().join("unused.db"),
        repos: std::env::temp_dir().join("unused-repos"),
    };
    Host::with_agent(runtime, agent, store, config)
}

/// Receives until `RunEnd`, with a timeout so a hang fails the test.
fn until_end(events: &mut UnboundedReceiver<RunEvent>) -> Vec<RunEvent> {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
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
    panic!("no RunEnd within 10 s: {seen:?}");
}

#[test]
fn a_run_streams_into_its_view() {
    let llm = ScriptedModel::new().turn(|t| t.text("Hello from tau"));
    let (host, mut events) = host(llm);
    let mut view = host.start("Say hello, please").unwrap();
    assert_eq!(view.title, "say-hello-please");
    assert!(
        matches!(view.items.first(), Some(Item::User(text)) if text == "Say hello, please")
    );
    for event in until_end(&mut events) {
        view.apply(&event);
    }
    assert_eq!(view.status, RunStatus::Finished(StopReason::Stop));
    assert_eq!(view.last_text(), Some("Hello from tau"));
    // The run leaves the host's table once its outcome is in.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while host.is_running(&view.id) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!host.is_running(&view.id));
}

#[test]
fn a_run_can_be_cancelled_from_the_ui() {
    // A response that takes 30 s to start: cancelled long before.
    let llm = ScriptedModel::new()
        .turn(|t| t.delay(Duration::from_secs(30)).text("too late"));
    let (host, mut events) = host(llm);
    let view = host.start("wait").unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert!(host.is_running(&view.id));
    host.cancel(&view.id);
    let ends: Vec<StopReason> = until_end(&mut events)
        .into_iter()
        .filter_map(|event| match event {
            RunEvent::RunEnd { stop, .. } => Some(stop),
            _ => None,
        })
        .collect();
    assert_eq!(ends, [StopReason::Cancelled]);
}

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(["-c", "init.defaultBranch=main"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git runs");
    assert!(output.status.success(), "git {args:?}: {output:?}");
}

fn wait_until_done(host: &Host, run: &tau_agent::tool::RunId) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while host.is_running(run) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!host.is_running(run));
}

#[test]
fn forks_start_from_a_turn_and_come_back_in_history() {
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

    let write = |content: &str| serde_json::json!({ "path": "a.txt", "content": content });
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("write", write("one\n")))
        .turn(|t| t.tool_call("write", write("two\n")))
        .turn(|t| t.text("done"))
        .turn(|t| t.text("forked"));
    let (host, mut events) = host_on(llm, src.path());
    let host = host.with_project(project);

    let main = host.start("write a.txt twice").unwrap();
    until_end(&mut events);
    wait_until_done(&host, &main.id);
    let main_dir = host.workspace(&main.id).unwrap();
    assert_eq!(
        std::fs::read_to_string(main_dir.join("a.txt")).unwrap(),
        "two\n"
    );
    // The user's checkout is untouched.
    assert!(!src.path().join("a.txt").exists());

    let fork = host.fork(&main.id, Some(1), "try it another way").unwrap();
    assert_eq!(
        fork.origin,
        Origin::Fork {
            from: main.id.clone(),
            turn: 1
        }
    );
    until_end(&mut events);
    wait_until_done(&host, &fork.id);
    let fork_dir = host.workspace(&fork.id).unwrap();
    assert_eq!(
        std::fs::read_to_string(fork_dir.join("a.txt")).unwrap(),
        "one\n"
    );

    let history = host.history().unwrap();
    let ids: Vec<_> = history.iter().map(|view| view.id.clone()).collect();
    assert_eq!(ids, [fork.id.clone(), main.id.clone()]);
    assert_eq!(history[0].title, "try-it-another-way");
    assert_eq!(history[0].origin, fork.origin);
    assert_eq!(history[0].status, RunStatus::Finished(StopReason::Stop));
    let old_main = &history[1];
    assert_eq!(old_main.children.len(), 1);
    assert_eq!(old_main.turn, 3);
    let writes = old_main
        .items
        .iter()
        .filter(|item| {
            matches!(item, Item::Tool(card)
                if card.tool == "write" && matches!(card.state, ToolState::Done { .. }))
        })
        .count();
    assert_eq!(writes, 2);

    // Compare: the run changed a.txt after the fork point, the fork did
    // nothing, and their code differs in a.txt.
    let code = host.block_on(host.branch_code(&main.id, &fork.id)).unwrap();
    let paths = |files: &[FileStat]| {
        files
            .iter()
            .map(|file| (file.path.clone(), file.added, file.removed))
            .collect::<Vec<_>>()
    };
    assert_eq!(paths(&code.main), [("a.txt".to_owned(), 1, 1)]);
    assert!(code.fork.is_empty());
    assert_eq!(code.between.len(), 1);
    let lines: Vec<(DiffKind, &str)> = code.between[0]
        .lines
        .iter()
        .map(|line| (line.kind, line.text.as_str()))
        .collect();
    assert_eq!(
        lines,
        [(DiffKind::Removed, "two"), (DiffKind::Added, "one")]
    );

    // Keeping the fork drops the main run's workspace, not its commits.
    host.keep_branch(&fork.id).unwrap();
    assert!(!main_dir.exists());
    assert!(fork_dir.exists());
}

fn config_on(root: &Path, data: &Path) -> HostConfig {
    HostConfig {
        access: Access::ApiKey("sk-test".into()),
        model: "gpt-5.5".into(),
        root: root.to_owned(),
        store: data.join("runs.db"),
        repos: data.join("repos"),
    }
}

#[test]
fn the_checkout_is_imported_in_the_background() {
    let src = tempfile::tempdir().unwrap();
    git(src.path(), &["init", "--quiet"]);
    std::fs::write(src.path().join("README.md"), "hello\n").unwrap();
    git(src.path(), &["add", "README.md"]);
    git(src.path(), &["commit", "--quiet", "-m", "first"]);
    let data = tempfile::tempdir().unwrap();
    let (host, _events) =
        Host::new(config_on(src.path(), data.path())).unwrap();
    // Waiting for the project is what blocks, not opening the host.
    let project = host.project().expect("the checkout imports");
    assert!(project.root().starts_with(data.path().join("repos")));
    assert!(!host.is_importing());
    assert!(matches!(
        host.catalog().project,
        tau_ui::catalog::ProjectStatus::Ready(_)
    ));
    let names: Vec<_> = host
        .catalog()
        .plugins
        .into_iter()
        .map(|plugin| plugin.name)
        .collect();
    assert!(names.contains(&"workspace".to_owned()));
}

#[test]
fn a_plain_directory_means_runs_work_in_it() {
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let (host, _events) =
        Host::new(config_on(dir.path(), data.path())).unwrap();
    assert!(host.project().is_none());
    assert!(matches!(
        host.catalog().project,
        tau_ui::catalog::ProjectStatus::Checkout(_)
    ));
}
