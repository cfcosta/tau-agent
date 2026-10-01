//! The host runs agents and reports their events; the view folds them.
//! With a project, each run works in a workspace of its own, forks
//! start from a turn's code, and past runs come back as history.

mod support;

use std::{path::Path, process::Command, time::Duration};

use tau_agent::{
    agent::Agent,
    event::{RunEvent, StopReason},
};
use tau_store::Store;
use tau_testing::scripted::ScriptedModel;
use tau_ui::{
    accounts::Credentials,
    github::{Api, Token},
    host::{Host, HostConfig},
    models::{AccountState, Effort, ModelChoice},
    view::{DiffKind, FileStat, Item, Origin, RunStatus, ToolState},
};
use tau_vcs::{Identity, Project};
use tokio::sync::mpsc::UnboundedReceiver;

/// The repository a host under test lists, made from the checkout it
/// is given, as a clone from GitHub would be.
const REPO: &str = "repo";

fn host(llm: ScriptedModel) -> (Host, UnboundedReceiver<RunEvent>) {
    host_on(llm, &tempfile::tempdir().unwrap().keep())
}

/// A project imported from `checkout`, which is made a Git repository
/// with one commit first if it is not one.
fn project_of(checkout: &Path) -> Project {
    if !checkout.join(".git").exists() {
        git(checkout, &["init", "--quiet"]);
        git(
            checkout,
            &["commit", "--quiet", "--allow-empty", "-m", "first"],
        );
    }
    Project::import(
        checkout.to_str().unwrap(),
        tempfile::tempdir().unwrap().keep().join("p"),
        Identity::default(),
    )
    .unwrap()
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
    host_over(runtime, store, llm, root)
}

/// [`host_on`] over a store in the file `db`, which a test can open
/// again to look into.
fn host_with_store(
    llm: ScriptedModel,
    root: &Path,
    db: &Path,
) -> (Host, UnboundedReceiver<RunEvent>) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let store = runtime.block_on(Store::open(db)).unwrap();
    host_over(runtime, store, llm, root)
}

fn host_over(
    runtime: tokio::runtime::Runtime,
    store: Store,
    llm: ScriptedModel,
    root: &Path,
) -> (Host, UnboundedReceiver<RunEvent>) {
    let agent = Agent::new(llm).name("coder");
    let config = HostConfig {
        account: test_account(),
        credentials: Credentials::new(
            fresh_repo_list().with_extension("config"),
        ),
        model: Some("gpt-6-luna".into()),
        store: std::env::temp_dir().join("unused.db"),
        // Tau's directory for repositories, where memory lives too: the
        // test's own.
        repos: fresh_repo_list().with_extension("repos"),
        // The test's own: a test that saves settings leaves others be.
        settings: fresh_repo_list().with_extension("models"),
        repo_list: fresh_repo_list(),
    };
    let (host, events) = Host::with_agent(runtime, agent, store, config);
    (host.with_repo(REPO, project_of(root)), events)
}

/// The ChatGPT account a host under test runs on; nothing connects
/// until a run asks.
fn test_account() -> tau_ai::chatgpt::AccountId {
    tau_ai::chatgpt::AccountId::parse("test-account").unwrap()
}

/// A repository list of the test's own, so tests do not share one.
fn fresh_repo_list() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNT: AtomicU32 = AtomicU32::new(0);
    std::env::temp_dir().join(format!(
        "tau-ui-repos-{}-{}.json",
        std::process::id(),
        COUNT.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Receives until `RunEnd`, with a timeout so a hang fails the test.
/// How long a test waits for what a run does. Each wait ends as soon as
/// the thing happens; the bound is generous because a run's turn ends
/// with jj work, and the tests share the machine with every other test.
const WAIT: Duration = Duration::from_secs(60);

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

/// Each repository has a main chat from the start: finished, empty and
/// open for good. A new chat forks it, at its start while it has no
/// turn, then at its latest. The main chat commits on trunk: a chat that
/// lands on it moves trunk, so do its own commits, and it has nothing
/// to merge.
#[test]
fn new_chats_fork_the_repository_main_chat() {
    let write = |path: &str| serde_json::json!({ "path": path, "content": format!("{path}\n") });
    let commit = |message: &str| serde_json::json!({ "message": message });
    let llm = ScriptedModel::new()
        .turn(|t| t.text("first"))
        .turn(|t| t.text("hello back"))
        .turn(|t| t.text("second"))
        .turn(|t| t.tool_call("write", write("a.txt")))
        .turn(|t| t.tool_call("vcs_commit", commit("feat: a")))
        .turn(|t| t.text("wrote a"))
        .turn(|t| t.tool_call("write", write("b.txt")))
        .turn(|t| t.tool_call("vcs_commit", commit("feat: b")))
        .turn(|t| t.text("wrote b"));
    let (host, mut events) = host(llm.clone());
    let main = host.main_of(REPO).unwrap();
    assert_eq!(host.main_of(REPO).unwrap(), main, "made once");
    let listed = host.history().unwrap();
    let view = listed.iter().find(|view| view.id == main).unwrap();
    assert_eq!((view.title.as_str(), &view.origin), ("main", &Origin::Root));
    assert_eq!(view.status, RunStatus::Finished(StopReason::Stop));
    assert_eq!(view.repo, REPO);
    assert!(host.set_closed(&main, true).is_err(), "main stays open");
    assert_eq!(
        host.catalog().repos[0].main.as_ref(),
        Some(&main),
        "the sidebar knows it"
    );

    // Before main has a turn, a chat starts from nothing, on trunk.
    let first = host.start("one", &ModelChoice::default(), REPO).unwrap();
    assert_eq!(
        first.origin,
        Origin::Fork {
            from: main.clone(),
            turn: 0
        }
    );
    until_end(&mut events);
    wait_until_done(&host, &first.id);

    // A message to main goes on with it; the next chat has it.
    host.resume(&main, "hello main", &ModelChoice::default())
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &main);
    let second = host.start("two", &ModelChoice::default(), REPO).unwrap();
    assert_eq!(
        second.origin,
        Origin::Fork {
            from: main.clone(),
            turn: 1
        }
    );
    until_end(&mut events);
    wait_until_done(&host, &second.id);
    let asked = llm.requests().pop().unwrap();
    assert!(
        format!("{:?}", asked.transcript).contains("hello main"),
        "the chat saw main's conversation"
    );

    // A chat's work lands on main, which takes its workspace then.
    let third = host.start("three", &ModelChoice::default(), REPO).unwrap();
    until_end(&mut events);
    wait_until_done(&host, &third.id);
    host.land(&third.id).unwrap();
    let dir = host.workspace(&main).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("a.txt")).unwrap(),
        "a.txt\n"
    );
    let project = host.project_of(REPO).unwrap();
    let on_trunk = |path: &str| {
        project
            .file_at(&project.trunk().unwrap(), path)
            .unwrap()
            .map(|(bytes, _)| bytes)
    };
    assert_eq!(on_trunk("a.txt").as_deref(), Some(&b"a.txt\n"[..]));

    // The main chat's own commit moves trunk too.
    host.resume(&main, "write b", &ModelChoice::default())
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &main);
    assert_eq!(on_trunk("b.txt").as_deref(), Some(&b"b.txt\n"[..]));
    llm.assert_exhausted();
}

/// A run shows its prompt's first line until a model writes its title;
/// a written title comes back with history.
/// The Plugins screen lists every plugin that asks Jev, with a key or
/// without one, which it says it needs; tau-reasoning opens the run's
/// plan. Spend comes from what each plugin charged.
#[test]
fn every_jev_plugin_is_listed() {
    let jev_plugins = [
        tau_reasoning::NAME,
        tau_fast_compaction::NAME,
        tau_constitution::NAME,
        tau_goal::NAME,
    ];
    let (host, _events) = host(ScriptedModel::new());
    let catalog = host.catalog();
    for name in jev_plugins {
        let plugin = catalog
            .plugins
            .iter()
            .find(|plugin| plugin.name == name)
            .unwrap_or_else(|| panic!("{name} is listed"));
        assert!(
            plugin.description.contains("needs a TypeSafe key"),
            "{name}: {}",
            plugin.description
        );
        assert_eq!(plugin.spend, 0.0);
    }
    let reasoning = |catalog: &tau_ui::catalog::Catalog| {
        catalog
            .plugins
            .iter()
            .find(|plugin| plugin.name == tau_reasoning::NAME)
            .cloned()
            .unwrap()
    };
    assert_eq!(
        reasoning(&catalog).screen,
        Some(tau_ui::catalog::PluginScreen::Plan)
    );
    let host = host
        .with_jev(std::sync::Arc::new(tau_jev::fake::FakeJev::nouls(|_| 0.5)));
    let catalog = host.catalog();
    assert!(
        !reasoning(&catalog).description.contains("needs"),
        "{:?}",
        reasoning(&catalog)
    );
}

#[test]
fn written_titles_come_back_in_history() {
    let llm = ScriptedModel::new().turn(|t| t.text("Hello from tau"));
    let data = tempfile::tempdir().unwrap();
    let db = data.path().join("runs.db");
    let (host, mut events) = host_with_store(llm, &std::env::temp_dir(), &db);
    let view = host
        .start("Say hello\nto everyone", &ModelChoice::default(), REPO)
        .unwrap();
    assert_eq!(view.title, "Say hello");
    until_end(&mut events);
    wait_until_done(&host, &view.id);
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let store = Store::open(&db).await.unwrap();
            store.set_title(&view.id.0, "Greet everyone").await.unwrap();
        });
    let history = host.history().unwrap();
    assert_eq!(history[0].title, "Greet everyone");
}

#[test]
fn a_run_streams_into_its_view() {
    let llm = ScriptedModel::new().turn(|t| t.text("Hello from tau"));
    let (host, mut events) = host(llm);
    let mut view = host
        .start("Say hello, please", &ModelChoice::default(), REPO)
        .unwrap();
    assert_eq!(view.title, "Say hello, please");
    assert!(
        matches!(view.items.first(), Some(Item::User(text)) if text == "Say hello, please")
    );
    for event in until_end(&mut events) {
        view.apply(&event);
    }
    assert_eq!(view.status, RunStatus::Finished(StopReason::Stop));
    assert_eq!(view.last_text(), Some("Hello from tau"));
    // The run leaves the host's table once its outcome is in.
    let deadline = std::time::Instant::now() + WAIT;
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
    let view = host.start("wait", &ModelChoice::default(), REPO).unwrap();
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

/// Sends `prompt` to the repository's main chat, the top-level run the
/// others nest under (runs nest one level), and returns its id.
fn on_main(host: &Host, prompt: &str) -> tau_agent::tool::RunId {
    let main = host.main_of(REPO).unwrap();
    host.resume(&main, prompt, &ModelChoice::default()).unwrap();
    main
}

fn wait_until_done(host: &Host, run: &tau_agent::tool::RunId) {
    let deadline = std::time::Instant::now() + WAIT;
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
    let commit = |message: &str| serde_json::json!({ "message": message });
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("write", write("one\n")))
        .turn(|t| t.tool_call("write", write("two\n")))
        .turn(|t| t.tool_call("vcs_commit", commit("feat: a.txt says two")))
        .turn(|t| t.text("done"))
        // The fork starts on turn 1's files, uncommitted, and keeps them.
        .turn(|t| t.tool_call("vcs_commit", commit("feat: a.txt says one")))
        .turn(|t| t.text("forked"));
    let (host, mut events) = host_on(llm.clone(), src.path());
    let host = host.with_repo(REPO, project);

    let main = on_main(&host, "write a.txt twice");
    until_end(&mut events);
    wait_until_done(&host, &main);
    let main_dir = host.workspace(&main).unwrap();
    assert_eq!(
        std::fs::read_to_string(main_dir.join("a.txt")).unwrap(),
        "two\n"
    );
    // The user's checkout is untouched.
    assert!(!src.path().join("a.txt").exists());

    let other = ModelChoice::new("gpt-6-sol", Effort::High);
    let fork = host
        .fork(&main, Some(1), "try it another way", &other)
        .unwrap();
    assert_eq!(fork.model, "gpt-6-sol");
    assert_eq!(
        fork.origin,
        Origin::Fork {
            from: main.clone(),
            turn: 1
        }
    );
    until_end(&mut events);
    wait_until_done(&host, &fork.id);
    let fork_dir = host.workspace(&fork.id).unwrap();
    // The fork asked its own model, at its own effort.
    let asked = llm.requests();
    let last = &asked.last().unwrap().settings;
    assert_eq!(last.model, "gpt-6-sol");
    assert_eq!(
        last.reasoning,
        Some(tau_ai::responses::request::ReasoningEffort::High)
    );
    assert_eq!(asked[0].settings.model, "gpt-6.1-sol");
    assert_eq!(
        std::fs::read_to_string(fork_dir.join("a.txt")).unwrap(),
        "one\n"
    );

    let history = host.history().unwrap();
    let ids: Vec<_> = history.iter().map(|view| view.id.clone()).collect();
    assert_eq!(ids, [fork.id.clone(), main.clone()]);
    assert_eq!(history[0].title, "try it another way");
    assert_eq!(history[0].origin, fork.origin);
    assert_eq!(history[0].status, RunStatus::Finished(StopReason::Stop));
    assert!(history.iter().all(|view| view.repo == REPO));
    let old_main = &history[1];
    assert_eq!(old_main.children.len(), 1);
    assert_eq!(old_main.turn, 4);
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
    let code = host.block_on(host.branch_code(&main, &fork.id)).unwrap();
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

    // Runs nest one level: the fork, a chat under the main chat,
    // cannot be forked in turn.
    assert!(
        host.fork(&fork.id, Some(1), "a third way", &ModelChoice::default())
            .is_err()
    );

    // Keeping the fork keeps the main chat's checkout: it is the
    // repository's own, the default workspace, which never goes.
    host.keep_branch(&fork.id).unwrap();
    assert!(main_dir.exists());
    assert!(fork_dir.exists());
}

/// A fork lands on the run it forked (ADR 0009): its change restacks
/// onto the run's newest turn, and the fork closes.
#[test]
fn a_fork_lands_on_its_parent_and_closes() {
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

    let write =
        |path: &str| serde_json::json!({ "path": path, "content": "x\n" });
    let commit = |message: &str| serde_json::json!({ "message": message });
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("write", write("a.txt")))
        .turn(|t| t.tool_call("write", write("b.txt")))
        .turn(|t| t.tool_call("vcs_commit", commit("feat: a and b")))
        .turn(|t| t.text("done"))
        .turn(|t| t.tool_call("write", write("c.txt")))
        .turn(|t| t.tool_call("vcs_commit", commit("feat: a and c")))
        .turn(|t| t.text("forked"));
    let (host, mut events) = host_on(llm, src.path());
    let host = host.with_repo(REPO, project.clone());

    let main = on_main(&host, "write two files");
    until_end(&mut events);
    wait_until_done(&host, &main);
    let main_dir = host.workspace(&main).unwrap();
    let fork = host
        .fork(&main, Some(1), "write c instead", &ModelChoice::default())
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &fork.id);
    let fork_dir = host.workspace(&fork.id).unwrap();
    assert!(fork_dir.join("c.txt").exists());
    assert!(!fork_dir.join("b.txt").exists(), "forked before turn 2");

    // A repository's main chat has nothing to land on.
    assert!(host.land(&host.main_of(REPO).unwrap()).is_err());
    // From history, the finished fork waits in the main run's chat.
    let waiting = |host: &Host| {
        host.history()
            .unwrap()
            .iter()
            .find(|view| view.id == main)
            .unwrap()
            .items
            .iter()
            .any(|item| matches!(item, Item::ForkReady { fork: f } if *f == fork.id))
    };
    assert!(waiting(&host));

    let preview = host.preview_landing(&fork.id).unwrap();
    assert_eq!(preview.changes.len(), 1);
    assert!(preview.conflicts.is_empty());
    assert!(
        !main_dir.join("c.txt").exists(),
        "a preview changes nothing"
    );

    let landed = host.land(&fork.id).unwrap();
    assert_eq!(landed.changes.len(), 1);
    // The main run has both its own files and the fork's.
    for file in ["a.txt", "b.txt", "c.txt"] {
        assert!(main_dir.join(file).exists(), "{file}");
    }
    // The fork is closed: no workspace, no bookmark.
    assert!(!fork_dir.exists());
    let fork_bookmark = format!("tau/{}", fork.id.0);
    assert_eq!(project.bookmark(&fork_bookmark).unwrap(), None);
    // The main chat commits on trunk: landing on it moves main.
    assert_eq!(project.trunk().unwrap(), landed.head);
    // Landing again finds nothing to land.
    assert!(host.land(&fork.id).is_err());

    // Back from history, the main run shows the landing where it
    // happened: after its last turn, before its stop.
    let history = host.history().unwrap();
    let items = &history.iter().find(|view| view.id == main).unwrap().items;
    let [.., turn_end, Item::Landed(card), Item::Stop { .. }] =
        items.as_slice()
    else {
        panic!("no landed card before the stop: {items:?}");
    };
    assert!(
        matches!(turn_end, Item::TurnEnd { turn: 4 }),
        "{turn_end:?}"
    );
    assert_eq!(card.from, fork.id);
    assert_eq!(card.title, "write c instead");
    assert!(!waiting(&host), "landed, it no longer waits");
    assert_eq!(card.changes.len(), 1);
}

/// Runs nest one level: a chat under the main chat cannot be forked,
/// and it lands on the main chat.
#[test]
fn a_chat_under_main_is_not_forked() {
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

    let write =
        |path: &str| serde_json::json!({ "path": path, "content": "x\n" });
    let commit = |message: &str| serde_json::json!({ "message": message });
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("write", write("a.txt")))
        .turn(|t| t.tool_call("vcs_commit", commit("feat: a")))
        .turn(|t| t.text("done"))
        .turn(|t| t.tool_call("write", write("b.txt")))
        .turn(|t| t.tool_call("vcs_commit", commit("feat: a and b")))
        .turn(|t| t.text("forked"));
    let (host, mut events) = host_on(llm, src.path());
    let host = host.with_repo(REPO, project.clone());
    let choice = ModelChoice::default();

    let main = on_main(&host, "write a");
    until_end(&mut events);
    wait_until_done(&host, &main);
    let child = host.fork(&main, Some(1), "write b", &choice).unwrap();
    until_end(&mut events);
    wait_until_done(&host, &child.id);
    // A chat under the main chat has nothing under it: it cannot be
    // forked, and so it lands with nothing waiting on it.
    let err = host
        .fork(&child.id, Some(2), "write c", &choice)
        .unwrap_err()
        .to_string();
    assert!(err.contains("Only a repository's main chat"), "{err}");

    let landed = host.land(&child.id).unwrap();
    assert_eq!(landed.changes.len(), 1, "b.txt");
    let main_dir = host.workspace(&main).unwrap();
    assert!(main_dir.join("b.txt").exists());
}

/// A run delegates to a sub-agent (ADR 0009): the sub-agent works in a
/// workspace of its own, its change lands on the run, and it closes.
#[test]
fn a_run_delegates_and_the_sub_agent_lands() {
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

    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call(
                "delegate",
                serde_json::json!({ "task": "write c.txt" }),
            )
        })
        .turn(|t| {
            t.tool_call(
                "write",
                serde_json::json!({ "path": "c.txt", "content": "c\n" }),
            )
        })
        // The sub-agent commits its work; it lands as it returns.
        .turn(|t| {
            t.tool_call(
                "vcs_commit",
                serde_json::json!({ "message": "feat: add c.txt" }),
            )
        })
        .turn(|t| t.text("wrote c.txt"))
        .turn(|t| t.text("done"));
    let (host, mut events) = host_on(llm, src.path());
    let host = host.with_repo(REPO, project.clone());
    let main = on_main(&host, "delegate c.txt");
    until_end(&mut events);
    wait_until_done(&host, &main);

    let dir = host.workspace(&main).unwrap();
    assert_eq!(std::fs::read_to_string(dir.join("c.txt")).unwrap(), "c\n");
    // The main chat works in the repository's own checkout, the default
    // workspace.
    assert_eq!(dir, project.workspace_dir(tau_vcs::DEFAULT_WORKSPACE));
    // The sub-agent is closed: no run workspace, and no run bookmark, as
    // the main chat commits on trunk.
    assert!(project.workspaces().unwrap().is_empty());
    assert!(project.bookmarks("tau/").unwrap().is_empty());

    // From history, the run's delegate card says what landed, and the
    // sub-agent's chat comes back under it.
    let history = host.history().unwrap();
    let main_view = history.iter().find(|view| view.id == main).unwrap();
    let card = main_view
        .items
        .iter()
        .find_map(|item| match item {
            Item::Tool(card) if card.tool == "delegate" => Some(card),
            _ => None,
        })
        .expect("a delegate card");
    let tau_ui::view::ToolBody::Delegated(landed) = &card.body else {
        panic!("{:?}", card.body);
    };
    assert_eq!(landed.changes.len(), 1);
    assert_eq!(landed.title, "write c.txt");
    let child = history
        .iter()
        .find(|view| view.id == landed.from)
        .expect("the sub-agent's chat");
    assert_eq!(
        child.origin,
        Origin::SubAgent {
            parent: main.clone()
        }
    );
    assert!(main_view.children.iter().any(|kid| kid.id == landed.from));
}

/// A sub-agent that fails still comes back from history, under the run
/// that called it, with how it ended.
#[test]
fn a_failed_sub_agent_comes_back_from_history() {
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
    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call(
                "delegate",
                serde_json::json!({ "task": "write c.txt" }),
            )
        })
        .turn(|t| t.dropped())
        .turn(|t| t.text("it failed"));
    let (host, mut events) = host_on(llm, src.path());
    let host = host.with_repo(REPO, project);
    let main = on_main(&host, "delegate c.txt");
    until_end(&mut events);
    until_end(&mut events);
    wait_until_done(&host, &main);

    let history = host.history().unwrap();
    let main_view = history.iter().find(|view| view.id == main).unwrap();
    let child = main_view
        .children
        .iter()
        .find(|child| child.kind == tau_ui::view::ChildKind::SubAgent)
        .expect("the sub-agent under its parent");
    let view = history.iter().find(|view| view.id == child.id).unwrap();
    assert_eq!(
        view.origin,
        Origin::SubAgent {
            parent: main.clone()
        }
    );
    assert_eq!(view.title, "write c.txt");
    assert!(
        matches!(&view.status, RunStatus::Finished(StopReason::Error(_))),
        "{:?}",
        view.status
    );
}

#[test]
fn runs_come_back_under_their_repository() {
    let other = tempfile::tempdir().unwrap();
    let llm = ScriptedModel::new()
        .turn(|t| t.text("one"))
        .turn(|t| t.text("two"));
    let (host, mut events) = host(llm);
    let host = host.with_repo("other", project_of(other.path()));

    let here = host
        .start("in the first", &ModelChoice::default(), REPO)
        .unwrap();
    assert_eq!(here.repo, REPO);
    until_end(&mut events);
    wait_until_done(&host, &here.id);
    let there = host
        .start("in the other", &ModelChoice::default(), "other")
        .unwrap();
    assert_eq!(there.repo, "other");
    until_end(&mut events);
    wait_until_done(&host, &there.id);

    let history = host.history().unwrap();
    let repo_of = |id| {
        history
            .iter()
            .find(|view| view.id == id)
            .map(|view| view.repo.clone())
            .unwrap()
    };
    assert_eq!(repo_of(here.id.clone()), REPO);
    assert_eq!(repo_of(there.id.clone()), "other");
    // A repository that is not listed runs nothing.
    assert!(
        host.start("nowhere", &ModelChoice::default(), "none")
            .is_err()
    );
}

/// What GitHub would serve for `full_name`: a repository with one
/// commit at `owner/name.git` under `remote`.
fn served(remote: &Path, full_name: &str) -> std::path::PathBuf {
    let src = remote.join(format!("{full_name}.git"));
    std::fs::create_dir_all(&src).unwrap();
    git(&src, &["init", "--quiet"]);
    std::fs::write(src.join("README.md"), "hello\n").unwrap();
    git(&src, &["add", "README.md"]);
    git(&src, &["commit", "--quiet", "-m", "first"]);
    src
}

/// `host`, signed in to GitHub and reaching it at `remote`.
fn on_github(host: Host, remote: &Path) -> Host {
    Token {
        token: "ghu_token".into(),
        user: "cfcosta".into(),
        expires_at: None,
    }
    .save(&host_credentials(&host))
    .unwrap();
    let web = format!("file://{}", remote.display());
    host.with_github(Api::at(&web, "http://127.0.0.1:9"))
}

#[test]
fn repositories_are_listed_and_remembered() {
    let data = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    served(remote.path(), "a/proj");
    served(remote.path(), "b/proj");
    let names = |host: &Host| -> Vec<String> {
        host.catalog()
            .repos
            .iter()
            .map(|repo| repo.name.clone())
            .collect()
    };

    let (host, _events) = Host::new(config_on(data.path())).unwrap();
    let host = on_github(host, remote.path());
    // Nothing is listed until it is cloned: not even the directory tau
    // started in.
    assert!(names(&host).is_empty());
    // Two repositories with one name get two names.
    assert_eq!(host.clone_github("a/proj").unwrap().name, "proj");
    assert_eq!(host.clone_github("b/proj").unwrap().name, "proj-2");
    // Cloning one again keeps its name.
    assert_eq!(host.clone_github("a/proj").unwrap().name, "proj");
    assert!(host.clone_github("c/missing").is_err());
    host.set_open_repos(vec!["proj-2".into()]).unwrap();
    assert_eq!(names(&host), ["proj", "proj-2"]);
    drop(host);

    let (host, _events) = Host::new(config_on(data.path())).unwrap();
    let host = on_github(host, remote.path());
    assert_eq!(names(&host), ["proj", "proj-2"]);
    assert_eq!(host.catalog().open_repos, ["proj-2"]);
    host.hide_repo("proj").unwrap();
    assert_eq!(names(&host), ["proj-2"]);
    // Closed conversations are remembered too, until opened again.
    let (first, second) = (
        tau_agent::tool::RunId("a".into()),
        tau_agent::tool::RunId("b".into()),
    );
    host.set_closed(&first, true).unwrap();
    host.set_closed(&second, true).unwrap();
    host.set_closed(&second, false).unwrap();
    // So are flagged calls someone looked at.
    host.set_reviewed(&first, "call-1").unwrap();
    host.set_reviewed(&first, "call-1").unwrap();
    drop(host);

    let (host, _events) = Host::new(config_on(data.path())).unwrap();
    let host = on_github(host, remote.path());
    assert_eq!(names(&host), ["proj-2"]);
    let catalog = host.catalog();
    assert_eq!(catalog.closed_runs, std::slice::from_ref(&first));
    assert_eq!(catalog.reviewed, [(first, "call-1".to_owned())]);
    // Cloning a removed one lists it again, under its name.
    assert_eq!(host.clone_github("a/proj").unwrap().name, "proj");
    assert_eq!(names(&host), ["proj", "proj-2"]);
}

fn config_on(data: &Path) -> HostConfig {
    HostConfig {
        account: test_account(),
        credentials: Credentials::new(data.join("config")),
        model: Some("gpt-6-luna".into()),
        store: data.join("runs.db"),
        repos: data.join("repos"),
        settings: data.join("models.json"),
        repo_list: data.join("repos.json"),
    }
}

#[test]
fn models_follow_the_sign_in_and_settings_persist() {
    let data = tempfile::tempdir().unwrap();

    let (host, _events) = Host::new(config_on(data.path())).unwrap();
    let models = host.models();
    // No settings yet: coder runs on the model the host was given.
    assert_eq!(models.settings.default_for("coder").model, "gpt-6-luna");
    let mut settings = models.settings.clone();
    settings.set_default("coder", ModelChoice::new("gpt-6-astra", Effort::Low));
    settings.toggle_hidden("gpt-5.6-terra");
    host.save_settings(settings.clone()).unwrap();
    drop(host);

    // On a ChatGPT plan the picker offers the plan's models from the
    // model table at once; the saved choices come back.
    let credentials = config_on(data.path()).credentials;
    let account = sign_in_saved(&credentials, "a@example.com", PLAN);
    let config = HostConfig {
        account: account.clone(),
        ..config_on(data.path())
    };
    let (host, _events) = Host::new(config).unwrap();
    let models = host.models();
    assert_eq!(models.settings, settings);
    assert_eq!(models.options, tau_ui::models::plan_models());
    assert!(models.access.chatgpt);
    assert_eq!(models.access.label, "ChatGPT plan");
    let accounts = &models.access.accounts;
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].id, account.to_string());
    assert!(accounts[0].active);
    assert_eq!(accounts[0].state, AccountState::Plan);
    assert_eq!(host.refusal(), None);
}

/// Scopes of a sign-in that allows plan usage.
const PLAN: &[&str] = &["chatgpt.tokens.use.direct", "email", "openid"];

/// Saves a signed-in ChatGPT record for `email` with `scopes`, as a
/// finished sign-in would, and makes it the active account.
fn sign_in_saved(
    credentials: &Credentials,
    email: &str,
    scopes: &[&str],
) -> tau_ai::chatgpt::AccountId {
    let record = tau_ai::chatgpt::Credentials {
        label: email.into(),
        email: Some(email.into()),
        issuer: "https://auth.openai.com".into(),
        subject: format!("sub-{email}"),
        client_id: "oaiapp_test".into(),
        ext_agent_host_id: tau_ai::chatgpt::HostId::new_uuid(),
        id_token: None,
        access_token: Some("at".into()),
        refresh_token: Some("rt".into()),
        token_type: None,
        expires_in: None,
        expires_at: Some(u64::MAX),
        earliest_refresh_at: None,
        scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
        saved_at: String::new(),
    };
    let chatgpt = credentials.chatgpt().unwrap();
    chatgpt.store().save(&record).unwrap();
    chatgpt.store().set_active(&record.id()).unwrap();
    record.id()
}

#[test]
fn runs_use_the_plan_and_never_a_declined_one() {
    let data = tempfile::tempdir().unwrap();
    let config = config_on(data.path());
    let credentials = config.credentials.clone();
    // A key saved by an older tau is never read.
    std::fs::create_dir_all(&credentials.dir).unwrap();
    std::fs::write(credentials.dir.join("openai-key"), "sk-saved").unwrap();
    let plan = sign_in_saved(&credentials, "a@example.com", PLAN);
    let account = credentials.plan_account().unwrap();
    assert_eq!(account, plan);
    let (host, _events) = Host::new(HostConfig { account, ..config }).unwrap();
    let host =
        host.with_repo(REPO, project_of(&tempfile::tempdir().unwrap().keep()));
    assert!(host.models().access.chatgpt);

    // Switching to an account that declined plan usage leaves nothing
    // to run on: no model is offered and runs do not start.
    let declined = sign_in_saved(&credentials, "b@example.com", &["openid"]);
    let left = credentials.plan_account();
    assert_eq!(left, None);
    host.set_account(left).unwrap();
    let models = host.models();
    assert!(!models.access.chatgpt);
    assert!(models.options.is_empty());
    let active = models.access.active_account().unwrap();
    assert_eq!(active.id, declined.to_string());
    assert_eq!(active.state, AccountState::PlanDisabled);
    let error = host.start("hi", &ModelChoice::default(), REPO).unwrap_err();
    assert!(error.to_string().contains("no ChatGPT plan"), "{error}");

    // Back on the plan account, runs use the plan again.
    credentials.switch(plan.as_str()).unwrap();
    host.set_account(credentials.plan_account()).unwrap();
    assert!(host.models().access.chatgpt);
}

#[test]
fn github_repositories_clone_into_tau() {
    let (data, remote) =
        (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let src = served(remote.path(), "cfcosta/hello");

    let config = config_on(data.path());
    let credentials = config.credentials.clone();
    let (host, _events) = Host::new(config).unwrap();
    let web = format!("file://{}", remote.path().display());
    let host = host.with_github(Api::at(&web, "http://127.0.0.1:9"));
    let error = host.clone_github("cfcosta/hello").unwrap_err();
    assert!(error.to_string().contains("Sign in to GitHub"), "{error}");

    Token {
        token: "ghu_token".into(),
        user: "cfcosta".into(),
        expires_at: None,
    }
    .save(&credentials)
    .unwrap();
    assert!(host.clone_github("../escape").is_err());
    assert!(host.clone_github("cfcosta/..").is_err());
    let repo = host.clone_github("cfcosta/hello").unwrap();
    assert_eq!(repo.name, "hello");
    assert!(
        host.catalog()
            .repos
            .iter()
            .any(|listed| listed.name == "hello")
    );
    // Waiting for the project is what blocks, not cloning.
    let project = host.project_of("hello").expect("the clone imports");
    assert!(project.root().starts_with(data.path().join("repos")));
    assert!(!project.trunk().unwrap().is_empty());
    assert!(!host.is_importing());
    assert_eq!(
        host.catalog().project,
        tau_ui::catalog::ProjectStatus::Unknown
    );
    // Cloning it again lists the same repository, without fetching.
    assert_eq!(host.clone_github("cfcosta/hello").unwrap().name, "hello");

    // New commits on GitHub come in with an update.
    std::fs::write(src.join("NEW.md"), "new\n").unwrap();
    git(&src, &["add", "NEW.md"]);
    git(&src, &["commit", "--quiet", "-m", "second"]);
    let updated = host.update_repo("hello").unwrap();
    assert!(updated.changed());
    assert_eq!(project.trunk().unwrap(), updated.after);
    assert!(!host.update_repo("hello").unwrap().changed());
}

#[test]
fn a_finished_run_goes_on_in_its_workspace() {
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
    let write =
        |path: &str| serde_json::json!({ "path": path, "content": "x\n" });
    let commit = |message: &str| serde_json::json!({ "message": message });
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("write", write("a.txt")))
        .turn(|t| t.tool_call("vcs_commit", commit("feat: a")))
        .turn(|t| t.text("wrote a"))
        .turn(|t| t.tool_call("write", write("b.txt")))
        .turn(|t| t.tool_call("vcs_commit", commit("feat: b")))
        .turn(|t| t.text("wrote b"));
    let (host, mut events) = host_on(llm.clone(), src.path());
    let host = host.with_repo(REPO, project);

    let chat = host
        .start("write a.txt", &ModelChoice::default(), REPO)
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &chat.id);
    let dir = host.workspace(&chat.id).unwrap();

    // The chat goes on on another model, without a fork.
    let other = ModelChoice::new("gpt-6-sol", Effort::Auto);
    host.resume(&chat.id, "now b.txt", &other).unwrap();
    let turns: Vec<u32> = until_end(&mut events)
        .into_iter()
        .filter_map(|event| match event {
            RunEvent::TurnStart { run, turn } if run == chat.id => Some(turn),
            _ => None,
        })
        .collect();
    assert_eq!(turns, [4, 5, 6], "turns keep counting");
    wait_until_done(&host, &chat.id);
    // The same workspace, with both turns' files.
    assert_eq!(host.workspace(&chat.id).unwrap(), dir);
    assert!(dir.join("a.txt").exists() && dir.join("b.txt").exists());
    // The model saw the whole chat.
    let last = llm.requests().pop().unwrap();
    assert!(last.transcript.len() > 4, "{}", last.transcript.len());
    assert_eq!(last.settings.model, "gpt-6-sol");

    let history = host.history().unwrap();
    assert_eq!(history.len(), 2, "one chat, not two, and main");
    let view = &history[0];
    assert_eq!(view.id, chat.id);
    assert_eq!(view.title, "write a.txt");
    assert_eq!(
        view.model, "gpt-6-sol",
        "it reloads on the model it went on"
    );
    assert_eq!(view.turn, 6);
    let prompts: Vec<&str> = view
        .items
        .iter()
        .filter_map(|item| match item {
            Item::User(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(prompts, ["write a.txt", "now b.txt"]);
}

/// A stored constitution that cannot be read can be removed from the
/// UI, which no edit could do, and its settings are saved like its
/// rules.
#[test]
fn a_broken_constitution_can_be_removed_and_settings_are_saved() {
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let db = data.path().join("runs.db");
    let (host, _events) =
        host_with_store(ScriptedModel::new(), dir.path(), &db);
    let key = host
        .project_of(REPO)
        .unwrap()
        .root()
        .canonicalize()
        .unwrap()
        .display()
        .to_string();
    let store = || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let store = runtime.block_on(Store::open(&db)).unwrap();
        (runtime, store)
    };
    // Flagged past where it blocks: it does not check.
    let (runtime, written) = store();
    runtime
        .block_on(written.save_constitution(
            &key,
            &tau_store::StoredConstitution {
                on_error: "allow".into(),
                max_holds: 3,
                rules: vec![tau_store::StoredRule {
                    id: "R1".into(),
                    text: "Name the tests.".into(),
                    targets: vec!["final answer".into()],
                    review: 0.9,
                    block: 0.2,
                }],
            },
        ))
        .unwrap();
    let constitution = || host.catalog().repos[0].constitution.clone();
    assert!(constitution().error.is_some());
    let add = |host: &Host| {
        host.edit_rules(REPO, |rules| {
            rules
                .add("No unwrap.", &["write.content".into()], 0.5, 0.8)
                .map(drop)
        })
    };
    assert!(
        add(&host).is_err(),
        "no edit gets past rules that cannot be read"
    );

    host.reset_rules(REPO).unwrap();
    let reset = constitution();
    assert_eq!((reset.error, reset.rules.len()), (None, 0));
    add(&host).unwrap();
    assert_eq!(constitution().rules.len(), 1);

    host.edit_rules(REPO, |rules| {
        rules.on_error = tau_constitution::OnError::Block;
        rules.max_holds = 5;
        Ok(())
    })
    .unwrap();
    let saved = constitution();
    assert!(saved.blocks_unchecked);
    assert_eq!(saved.max_holds, 5);
    let (runtime, read) = store();
    let stored = runtime
        .block_on(tau_constitution::Constitution::load(&read, &key))
        .unwrap();
    assert_eq!(stored.on_error, tau_constitution::OnError::Block);
    assert_eq!((stored.max_holds, stored.rules.len()), (5, 1));
}

#[test]
fn the_constitution_blocks_a_call_that_breaks_a_rule() {
    let dir = tempfile::tempdir().unwrap();
    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call(
                "write",
                serde_json::json!({ "path": "a.txt", "content": "x.unwrap()" }),
            )
        })
        .turn(|t| t.text("I could not write it."));
    let data = tempfile::tempdir().unwrap();
    let db = data.path().join("runs.db");
    let (host, mut events) = host_with_store(llm, dir.path(), &db);
    let host = host
        .with_jev(std::sync::Arc::new(tau_jev::fake::FakeJev::nouls(|_| 0.95)));
    // What the store holds for the repository, as a run would read it.
    let root = host.project_of(REPO).unwrap().root().to_owned();
    let stored = || {
        let key = root.canonicalize().unwrap().display().to_string();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let store = Store::open(&db).await.unwrap();
                tau_constitution::Constitution::load(&store, &key).await
            })
            .unwrap()
    };
    // A rule saved the way the Constitution screen saves one.
    host.edit_rules(REPO, |rules| {
        rules
            .add("No unwrap.", &["write.content".into()], 0.5, 0.8)
            .map(drop)
    })
    .unwrap();
    let catalog = host.catalog();
    let rules = &catalog.repos[0].constitution;
    assert_eq!(rules.rules.len(), 1);
    assert_eq!(rules.rules[0].applies_to, ["write.content"]);
    assert!(rules.error.is_none());
    // Kept in the store, not in a file.
    assert_eq!(stored().rules.len(), 1);
    assert!(
        !walk(dir.path()).any(|path| path.ends_with("constitution.toml")),
        "no file is written"
    );
    assert!(catalog.plugins.iter().any(|p| p.name == "tau-constitution"));
    let stats = catalog.jev.clone().expect("Jev is set up");
    assert_eq!((stats.requests, stats.failed), (0, 0));

    let mut view = host
        .start("write a.txt", &ModelChoice::default(), REPO)
        .unwrap();
    for event in until_end(&mut events) {
        view.apply(&event);
    }
    wait_until_done(&host, &view.id);
    assert!(!dir.path().join("a.txt").exists(), "the write was refused");
    let blocked = |view: &tau_ui::view::RunView| {
        view.items.iter().any(|item| {
            matches!(item, Item::Tool(card)
                if matches!(&card.state, ToolState::Blocked { rule, .. } if rule == "R1"))
        })
    };
    assert!(blocked(&view), "the card shows the block live");
    assert_eq!(view.constitution.calls, 1);
    assert_eq!(view.constitution.blocked, ["R1"]);
    // The Plugins screen counts the check Jev answered.
    let stats = host.catalog().jev.expect("Jev is set up");
    assert!(stats.requests >= 1, "{stats:?}");
    assert!(stats.input_tokens > 0 && stats.spent > 0.0);
    assert_eq!((stats.model.as_str(), stats.failed), ("jev-fake", 0));
    // And in history, from what the plugin recorded.
    let history = host.history().unwrap();
    assert!(blocked(&history[0]));
    assert_eq!(history[0].constitution, view.constitution);

    // Editing a rule saves over it, in place.
    host.edit_rules(REPO, |rules| {
        rules.replace(
            "R1",
            "No unwrap, ever.",
            &["write.content".into()],
            0.3,
            0.9,
        )
    })
    .unwrap();
    let edited = host.catalog().repos[0].constitution.rules[0].clone();
    assert_eq!(
        (edited.id.as_str(), edited.text.as_str()),
        ("R1", "No unwrap, ever.")
    );
    assert_eq!(edited.block, 0.9);
    // Trying a rule asks Jev about each past call it reads.
    let calls = vec![
        (
            "write".to_owned(),
            serde_json::json!({ "path": "a", "content": "x.unwrap()" }),
        ),
        ("bash".to_owned(), serde_json::json!({ "command": "ls" })),
    ];
    let (trials, cost) = host
        .try_rule(
            "No unwrap.",
            &["write.content".into()],
            0.3,
            0.8,
            &calls,
            &[],
        )
        .unwrap();
    assert_eq!(trials.len(), 1);
    assert_eq!(trials[0].shown, "x.unwrap()");
    assert!((trials[0].score - 0.95).abs() < 1e-9 && cost > 0.0);
    assert!(
        host.try_rule("x", &["nowhere".into()], 0.3, 0.8, &calls, &[])
            .is_err(),
        "a place that names nothing is refused"
    );

    // Removing a rule saves it.
    host.edit_rules(REPO, |rules| {
        rules.remove("R1");
        Ok(())
    })
    .unwrap();
    let constitution = host.catalog().repos[0].constitution.clone();
    assert!(constitution.rules.is_empty());
    assert!(stored().rules.is_empty());
    // An edit the rules would refuse is not saved.
    assert!(
        host.edit_rules(REPO, |rules| rules
            .add("x", &["write.content".into()], 0.9, 0.1)
            .map(drop))
            .is_err()
    );
    assert!(stored().rules.is_empty());
}

/// A large `bash` output reaches the model pruned, with the whole of it
/// archived in tau's directory for the repository, readable only by the
/// user; the call's card says what was kept, live and from history.
#[test]
fn a_large_output_is_pruned_into_tau_s_archive() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    // Under bash's 2,000 lines and 50 KB, so nothing spills, and over
    // output pruning's 10,000 estimated tokens.
    let command = "for i in $(seq 1 1200); do \
                   echo \"::::::::::::::::::::::::: line $i\"; done";
    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call("bash", serde_json::json!({ "command": command }))
        })
        .turn(|t| t.text("built"));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let store = runtime.block_on(Store::memory()).unwrap();
    let (host, mut events) = Host::with_agent(
        runtime,
        Agent::new(llm).name("coder"),
        store,
        config_on(data.path()),
    );
    let host = host.with_repo(REPO, project_of(dir.path()));
    // Jev finds every chunk it is asked about disposable: the first
    // and last stay anyway.
    let host = host
        .with_jev(std::sync::Arc::new(tau_jev::fake::FakeJev::nouls(|_| 0.0)));
    let status = host
        .catalog()
        .plugins
        .into_iter()
        .find(|plugin| plugin.name == tau_fast_compaction::NAME)
        .expect("fast compaction runs with Jev");
    assert!(status.description.contains("bash outputs"), "{status:?}");

    let mut view = host
        .start("build it", &ModelChoice::default(), REPO)
        .unwrap();
    for event in until_end(&mut events) {
        view.apply(&event);
    }
    wait_until_done(&host, &view.id);
    let cut_of = |view: &tau_ui::view::RunView| {
        view.items.iter().find_map(|item| match item {
            Item::Tool(card) if card.tool == "bash" => card.cut.clone(),
            _ => None,
        })
    };
    let cut = cut_of(&view).expect("the card says what was cut");
    // And the empty one after the last newline.
    assert_eq!(cut.lines, 1201);
    assert!(cut.kept < cut.lines / 10, "{cut:?}");
    let archive = Path::new(&cut.archive);
    assert!(
        archive.starts_with(data.path().join("repos")),
        "{} is not in tau's directory",
        archive.display()
    );
    assert!(archive.parent().unwrap().ends_with("archive"));
    let mode = |path: &Path| {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    };
    assert_eq!(mode(archive), 0o600);
    assert_eq!(mode(archive.parent().unwrap()), 0o700);
    let whole = std::fs::read_to_string(archive).unwrap();
    assert!(whole.contains(":: line 600\n"));
    assert!(
        view.plugins
            .iter()
            .any(|plugin| plugin.name == tau_fast_compaction::NAME
                && plugin.state.contains("large outputs")),
        "{:?}",
        view.plugins
    );

    let history = host.history().unwrap();
    assert_eq!(cut_of(&history[0]), Some(cut));
}

/// Every file under `dir`.
fn walk(dir: &Path) -> impl Iterator<Item = std::path::PathBuf> {
    let mut stack = vec![dir.to_owned()];
    std::iter::from_fn(move || {
        while let Some(path) = stack.pop() {
            match std::fs::read_dir(&path) {
                Ok(entries) => {
                    stack.extend(entries.flatten().map(|entry| entry.path()))
                }
                Err(_) => return Some(path),
            }
        }
        None
    })
}

#[test]
fn a_run_becomes_a_pull_request_on_github() {
    // What GitHub serves, as a local repository at owner/name.git.
    let remote = tempfile::tempdir().unwrap();
    let src = remote.path().join("cfcosta/hello.git");
    std::fs::create_dir_all(&src).unwrap();
    git(&src, &["init", "--quiet"]);
    std::fs::write(src.join("README.md"), "hello\n").unwrap();
    git(&src, &["add", "README.md"]);
    git(&src, &["commit", "--quiet", "-m", "first"]);
    // GitHub's API, answering the calls a push and a pull request make.
    let (api, seen) = support::fake(|line| {
        let answer = |sha: &str| (201, serde_json::json!({ "sha": sha }));
        match line {
            l if l.starts_with("GET /repos/cfcosta/hello/git/commits/") => {
                (200, serde_json::json!({ "tree": { "sha": "tree-base" } }))
            }
            "POST /repos/cfcosta/hello/git/blobs" => answer("blob-1"),
            "POST /repos/cfcosta/hello/git/trees" => answer("tree-1"),
            "POST /repos/cfcosta/hello/git/commits" => answer("commit-1"),
            "POST /repos/cfcosta/hello/git/refs" => {
                (201, serde_json::json!({}))
            }
            "POST /repos/cfcosta/hello/pulls" => (
                201,
                serde_json::json!({
                    "number": 7,
                    "html_url": "https://github.com/cfcosta/hello/pull/7"
                }),
            ),
            "POST /repos/cfcosta/hello/pulls/7/requested_reviewers" => {
                (201, serde_json::json!({}))
            }
            l if l.starts_with(
                "GET /repos/cfcosta/hello/commits/commit-1/check-runs",
            ) =>
            {
                (
                    200,
                    serde_json::json!({ "check_runs": [
                        { "status": "completed", "conclusion": "success" }
                    ]}),
                )
            }
            _ => (404, serde_json::json!({ "message": "Not Found" })),
        }
    });
    let write =
        |path: &str| serde_json::json!({ "path": path, "content": "new\n" });
    let commit = |message: &str| serde_json::json!({ "message": message });
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("write", write("src.txt")))
        .turn(|t| t.tool_call("vcs_commit", commit("feat: add src.txt")))
        .turn(|t| t.text("Wrote src.txt. cargo test: 3 passed."))
        .turn(|t| t.tool_call("write", write("more.txt")))
        .turn(|t| t.tool_call("vcs_commit", commit("feat: add more.txt")))
        .turn(|t| t.text("Added more.txt."));
    let home = tempfile::tempdir().unwrap();
    let (host, mut events) = host_on(llm, home.path());
    let web = format!("file://{}", remote.path().display());
    let host = host.with_github(Api::at(&web, &api));
    Token {
        token: "ghu_token".into(),
        user: "cfcosta".into(),
        expires_at: None,
    }
    .save(&host_credentials(&host))
    .unwrap();
    let repo = host.clone_github("cfcosta/hello").unwrap();
    assert!(host.project_of(&repo.name).is_some());
    assert!(host.catalog().pull_requests);

    let run = host
        .start("write src.txt, please", &ModelChoice::default(), &repo.name)
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &run.id);

    let draft = host.prepare_pull_request(&run.id).unwrap();
    assert_eq!(draft.repo, "cfcosta/hello");
    assert_eq!(draft.base, "main");
    assert!(
        draft.head.starts_with("tau/write-src-txt-please-"),
        "{}",
        draft.head
    );
    assert!(draft.mergeable);
    assert_eq!(draft.title, "Write src.txt, please");
    assert_eq!(draft.tests.as_deref(), Some("3 tests passed"));
    assert!(draft.body.starts_with("Wrote src.txt."), "{}", draft.body);
    assert_eq!(draft.commits.len(), 1);
    assert_eq!(draft.commits[0].title, "feat: add src.txt");
    assert_eq!((draft.commits[0].added, draft.commits[0].removed), (1, 0));

    let (opened, head) = host
        .create_pull_request(
            &run.id,
            &draft,
            "Write src.txt",
            "The body",
            true,
            true,
            &["alice".into()],
        )
        .unwrap();
    assert_eq!(opened.number, 7);
    assert_eq!(opened.url, "https://github.com/cfcosta/hello/pull/7");
    assert_eq!(head, "commit-1");
    assert_eq!(
        host.pull_request_checks(&run.id).unwrap(),
        tau_ui::pull_request::Checks::Passed
    );
    let requests = seen.lock().unwrap().clone();
    let find = |line: &str| {
        requests
            .iter()
            .find(|request| request.starts_with(line))
            .unwrap_or_else(|| panic!("no {line}"))
            .clone()
    };
    // The file's content, then a tree on the base's, a commit on the
    // base, the branch, and the pull request.
    assert!(find("POST /repos/cfcosta/hello/git/blobs").contains("bmV3Cg=="));
    let tree = find("POST /repos/cfcosta/hello/git/trees");
    assert!(tree.contains("\"base_tree\":\"tree-base\""), "{tree}");
    assert!(tree.contains("\"path\":\"src.txt\""), "{tree}");
    let commit = find("POST /repos/cfcosta/hello/git/commits");
    // The model's own message.
    assert!(commit.contains("feat: add src.txt"), "{commit}");
    let base = project_base(&host, &repo.name);
    assert!(commit.contains(&base), "{commit}");
    assert!(find("POST /repos/cfcosta/hello/git/refs").contains(&draft.head));
    let pull = find("POST /repos/cfcosta/hello/pulls ");
    assert!(
        pull.contains("\"draft\":true") && pull.contains("\"base\":\"main\""),
        "{pull}"
    );
    assert!(
        find("POST /repos/cfcosta/hello/pulls/7/requested_reviewers")
            .contains("alice")
    );

    // A later commit of the run goes to the same branch.
    assert!(host.keeps_pushing(&run.id));
    host.resume(&run.id, "add more.txt", &ModelChoice::default())
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &run.id);
    assert!(host.push_later_commits(&run.id).unwrap());
    let requests = seen.lock().unwrap().clone();
    let trees = requests
        .iter()
        .filter(|request| {
            request.starts_with("POST /repos/cfcosta/hello/git/trees")
        })
        .count();
    assert_eq!(trees, 2);
    assert!(requests.iter().any(|request| {
        request.starts_with("PATCH /repos/cfcosta/hello/git/refs/heads/")
            || request.starts_with("POST /repos/cfcosta/hello/git/refs")
    }));
    assert!(!host.push_later_commits(&run.id).unwrap(), "nothing new");
}

/// The credentials directory the test hosts use.
fn host_credentials(host: &Host) -> tau_ui::accounts::Credentials {
    host.credentials().clone()
}

/// The trunk a repository's project had when its first run started.
fn project_base(host: &Host, repo: &str) -> String {
    host.project_of(repo).unwrap().trunk().unwrap()
}

#[test]
fn auto_reasoning_takes_the_effort_jev_picks() {
    let dir = tempfile::tempdir().unwrap();
    let llm = ScriptedModel::new().turn(|t| t.text("done"));
    let (host, mut events) = host_on(llm.clone(), dir.path());
    // Jev is sure the task wants high.
    let jev = tau_jev::fake::FakeJev::new(|request| {
        let answers = request
            .questions
            .keys()
            .map(|id| {
                let probabilities = [0.01, 0.02, 0.09, 0.84, 0.04]
                    .iter()
                    .enumerate()
                    .map(|(n, p)| (n.to_string(), *p))
                    .collect();
                (
                    id.clone(),
                    tau_jev::Answer::Score {
                        score: 3.0,
                        probabilities,
                        confidence: 0.84,
                    },
                )
            })
            .collect();
        Ok(tau_jev::fake::response(answers, request))
    });
    let host = host.with_jev(std::sync::Arc::new(jev));
    let auto = ModelChoice::new("gpt-5.5", Effort::Auto);
    let mut view = host.start("track down the race", &auto, REPO).unwrap();
    for event in until_end(&mut events) {
        view.apply(&event);
    }
    wait_until_done(&host, &view.id);
    assert_eq!(
        llm.requests()[0].settings.reasoning,
        Some(tau_ai::responses::request::ReasoningEffort::High)
    );
    let chosen = |view: &tau_ui::view::RunView| {
        view.plan.iter().any(|field| {
            field.name == "reasoning"
                && field.value == "high"
                && field.set_by.as_deref() == Some("tau-reasoning")
        })
    };
    assert!(chosen(&view), "{:?}", view.plan);
    assert!(
        view.items
            .iter()
            .any(|item| matches!(item, Item::Plugin(note)
        if note.plugin == "tau-reasoning"))
    );
    // History shows it again, from the plugin's record.
    assert!(chosen(&host.history().unwrap()[0]));

    // An effort someone chose is not scored.
    let (host, mut events) =
        host_on(ScriptedModel::new().turn(|t| t.text("ok")), dir.path());
    let host =
        host.with_jev(std::sync::Arc::new(tau_jev::fake::FakeJev::new(|_| {
            panic!("not asked")
        })));
    let low = ModelChoice::new("gpt-5.5", Effort::Low);
    let run = host.start("rename a variable", &low, REPO).unwrap();
    until_end(&mut events);
    wait_until_done(&host, &run.id);
}

/// The Models screen's reasoning settings reach tau-reasoning: asked to
/// decide again between steps, it asks Jev how long the effort holds;
/// asked for more confidence than Jev has, it keeps the default.
#[test]
fn reasoning_settings_reach_the_plugin() {
    let dir = tempfile::tempdir().unwrap();
    let llm = ScriptedModel::new().turn(|t| t.text("done"));
    let (host, mut events) = host_on(llm.clone(), dir.path());
    // 0.84 sure of high, and sure the effort holds for one call.
    let jev = tau_jev::fake::FakeJev::new(|request| {
        let answers = request
            .questions
            .keys()
            .map(|id| {
                let answer = if id == "lease" {
                    tau_jev::Answer::Choice {
                        choice: "one_call".into(),
                        probabilities: [("one_call".to_owned(), 0.9)].into(),
                        confidence: 0.9,
                    }
                } else {
                    tau_jev::Answer::Score {
                        score: 3.0,
                        probabilities: [0.01, 0.02, 0.09, 0.84, 0.04]
                            .iter()
                            .enumerate()
                            .map(|(n, p)| (n.to_string(), *p))
                            .collect(),
                        confidence: 0.84,
                    }
                };
                (id.clone(), answer)
            })
            .collect();
        Ok(tau_jev::fake::response(answers, request))
    });
    let host = host.with_jev(std::sync::Arc::new(jev.clone()));
    let mut settings = tau_ui::models::ModelSettings::default();
    settings.reasoning.redecide = true;
    settings.reasoning.threshold = 0.9;
    host.save_settings(settings).unwrap();
    let auto = ModelChoice::new("gpt-5.5", Effort::Auto);
    let view = host.start("track down the race", &auto, REPO).unwrap();
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

#[test]
fn a_goal_keeps_the_chat_going_until_it_holds() {
    let dir = tempfile::tempdir().unwrap();
    let llm = ScriptedModel::new()
        .turn(|t| t.text("tried"))
        .turn(|t| t.text("tests pass now"))
        .turn(|t| t.text("paused, so it stops"))
        .turn(|t| t.text("done for real"));
    let (host, mut events) = host_on(llm.clone(), dir.path());
    // Not met, then met; then not met on every later check.
    let answers =
        std::sync::Arc::new(std::sync::Mutex::new(vec![0.1, 0.95, 0.2]));
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
    let host = host.with_jev(std::sync::Arc::new(jev.clone()));
    assert!(
        host.catalog()
            .plugins
            .iter()
            .any(|p| p.name == tau_goal::NAME)
    );

    let prompt = "/goal --continuations 3 the tests pass";
    let mut view = host.start(prompt, &ModelChoice::default(), REPO).unwrap();
    assert_eq!(view.title, "the tests pass");
    for event in until_end(&mut events) {
        view.apply(&event);
    }
    wait_until_done(&host, &view.id);
    assert_eq!(llm.requests().len(), 2, "sent back once");
    let goal = view.goal.clone().expect("the goal shows live");
    assert_eq!(goal.condition, "the tests pass");
    assert_eq!(goal.status, tau_goal::Status::Met);
    assert_eq!((goal.continuations, goal.max_continuations), (1, 3));
    let notes: Vec<String> = view
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Plugin(note) if note.plugin == tau_goal::NAME => {
                Some(note.text.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(notes, ["the goal is not met yet", "the goal is met"]);

    // History has the goal: the card, the continuation as tau-goal's
    // note, and the state from the records.
    let history = host.history().unwrap();
    let stored = &history[0];
    assert_eq!(stored.goal, view.goal);
    assert!(matches!(&stored.items[0], Item::Goal(c) if c == "the tests pass"));
    assert!(stored.items.iter().any(|item| matches!(item,
        Item::Plugin(note) if note.plugin == tau_goal::NAME)));
    assert!(
        !stored
            .items
            .iter()
            .any(|item| matches!(item, Item::User(text)
            if text.starts_with(tau_goal::CONTINUATION_PREFIX))),
        "no continuation shows as the person's message"
    );

    // A met goal is not checked again; a new one is, until paused. This
    // one runs out at once, then gets one more continuation, paused.
    let goal_checks = || {
        jev.requests()
            .iter()
            .filter(|request| request.questions.contains_key("met"))
            .count()
    };
    let asked = goal_checks();
    host.resume(
        &view.id,
        "/goal --continuations 0 it is released",
        &ModelChoice::default(),
    )
    .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &view.id);
    assert_eq!(goal_checks(), asked + 1);
    host.goal_record(&view.id, &tau_goal::Record::Extended { by: 1 })
        .unwrap();
    host.goal_record(&view.id, &tau_goal::Record::Paused)
        .unwrap();
    host.resume(&view.id, "one more thing", &ModelChoice::default())
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &view.id);
    assert_eq!(goal_checks(), asked + 1, "paused: not checked");
    let goal = host.history().unwrap()[0].goal.clone().unwrap();
    assert_eq!(goal.condition, "it is released");
    assert_eq!(goal.status, tau_goal::Status::Paused);
    assert_eq!(goal.max_continuations, 1);
    llm.assert_exhausted();
}

#[test]
fn an_effort_the_model_does_not_take_runs_at_auto() {
    let dir = tempfile::tempdir().unwrap();
    let llm = ScriptedModel::new().turn(|t| t.text("ok"));
    let (host, mut events) = host_on(llm.clone(), dir.path());
    // gpt-5.5 stops at xhigh; the API would reject max.
    let max = ModelChoice::new("gpt-5.5", Effort::Max);
    let run = host.start("rename a variable", &max, REPO).unwrap();
    until_end(&mut events);
    wait_until_done(&host, &run.id);
    assert_eq!(llm.requests()[0].settings.reasoning, None);
    let reasoning = run.plan.iter().find(|field| field.name == "reasoning");
    assert_eq!(reasoning.unwrap().value, "auto");
}

/// Each message of a chat on auto is scored on its own: an effort
/// tau-reasoning picked does not carry over. History shows each choice
/// after the message it was for.
#[test]
fn each_message_is_scored_again() {
    let dir = tempfile::tempdir().unwrap();
    let llm = ScriptedModel::new()
        .turn(|t| t.text("fine, thanks"))
        .turn(|t| t.text("a long answer"));
    let (host, mut events) = host_on(llm.clone(), dir.path());
    // gpt-5.5's levels: none, low, medium, high, xhigh. The first
    // message wants none, the second xhigh.
    let wants = std::sync::Arc::new(std::sync::Mutex::new(vec![4, 0]));
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
    let host = host.with_jev(std::sync::Arc::new(jev));
    let auto = ModelChoice::new("gpt-5.5", Effort::Auto);
    let mut view = host.start("how are you?", &auto, REPO).unwrap();
    for event in until_end(&mut events) {
        view.apply(&event);
    }
    wait_until_done(&host, &view.id);
    // What the composer sends next: auto again, not the picked none.
    let next = tau_ui::Workspace::model_of(&view);
    assert_eq!(next, auto);
    host.resume(&view.id, "prove the Riemann hypothesis", &next)
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &view.id);
    use tau_ai::responses::request::ReasoningEffort;
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
    let history = host.history().unwrap();
    let order: Vec<String> = history[0]
        .items
        .iter()
        .filter_map(|item| match item {
            Item::User(text) => Some(format!("user: {text}")),
            Item::Plugin(note) if note.plugin == "tau-reasoning" => {
                Some(note.text.clone())
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

/// A run keeps a note in its repository's memory, the Memory screen
/// shows it, and a later run's commit that changes the file it is about,
/// even through bash, marks it as maybe stale.
#[test]
fn memory_notes_are_kept_shown_and_marked_stale_by_commits() {
    let src = tempfile::tempdir().unwrap();
    git(src.path(), &["init", "--quiet"]);
    std::fs::write(src.path().join("a.txt"), "one\n").unwrap();
    git(src.path(), &["add", "a.txt"]);
    git(src.path(), &["commit", "--quiet", "-m", "first"]);
    let repos = tempfile::tempdir().unwrap();
    let project = Project::import(
        src.path().to_str().unwrap(),
        repos.path().join("p"),
        Identity::default(),
    )
    .unwrap();
    let note = serde_json::json!({
        "type": "fact",
        "title": "a.txt holds the count",
        "description": "The count lives in a.txt, one number per line.",
        "body": "Read a.txt for the count.",
        "links": [{ "to": "a.txt", "type": "about" }],
    });
    let bash = serde_json::json!({ "command": "echo two > a.txt" });
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("memory_write", note.clone()))
        .turn(|t| t.text("noted"))
        .turn(|t| t.tool_call("bash", bash.clone()))
        .turn(|t| t.text("changed"));
    let (host, mut events) = host_on(llm, src.path());
    let host = host.with_repo(REPO, project);
    let memory_of = |host: &Host| {
        let catalog = host.catalog();
        let repo = catalog
            .repos
            .iter()
            .find(|repo| repo.name == REPO)
            .expect("the repository is listed")
            .clone();
        repo.memory
    };
    assert!(memory_of(&host).notes.is_empty());
    assert!(
        host.catalog()
            .plugins
            .iter()
            .any(|plugin| plugin.name == "tau-memory")
    );

    let first = host
        .start("note it", &ModelChoice::default(), REPO)
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &first.id);
    let memory = memory_of(&host);
    assert_eq!(memory.notes.len(), 1, "{memory:?}");
    let kept = &memory.notes[0];
    assert_eq!(kept.title, "a.txt holds the count");
    assert_eq!(kept.paths, ["a.txt"]);
    assert!(!kept.body[0].starts_with("May be stale"), "{:?}", kept.body);

    let second = host
        .start("change it", &ModelChoice::default(), REPO)
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &second.id);
    // The mark is made off the run's thread; give it a moment.
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        let memory = memory_of(&host);
        if memory.notes[0].body[0].starts_with("May be stale") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "never marked stale: {memory:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A repository's main chat has a view to send with it when the
/// repository is added: the sidebar lists the chat from that view.
#[test]
fn a_repository_main_chat_has_a_view() {
    let (host, _events) = host(ScriptedModel::new());
    let repo = host
        .catalog()
        .repos
        .into_iter()
        .find(|repo| repo.name == REPO)
        .expect("the repository is listed");
    let main = repo.main.clone().expect("it has a main chat");
    let view = host.main_view(&repo).unwrap().expect("the chat is stored");
    assert_eq!(view.id, main);
    assert_eq!(view.title, "main");
    assert_eq!(view.repo, REPO);
    assert_eq!(view.origin, Origin::Root);
}

/// Runs nest one level: the main chat can delegate, and a chat under it,
/// which could only nest a sub-agent under itself, is not given the
/// tool.
#[test]
fn only_the_main_chat_delegates() {
    let llm = ScriptedModel::new()
        .turn(|t| t.text("main"))
        .turn(|t| t.text("chat"));
    let (host, mut events) = host(llm.clone());
    let main = on_main(&host, "hello main");
    until_end(&mut events);
    wait_until_done(&host, &main);
    let chat = host
        .start("hello chat", &ModelChoice::default(), REPO)
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &chat.id);
    let tools = |n: usize| -> Vec<String> {
        llm.requests()[n]
            .settings
            .tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect()
    };
    assert!(
        tools(0).iter().any(|name| name == "delegate"),
        "{:?}",
        tools(0)
    );
    assert!(
        !tools(1).iter().any(|name| name == "delegate"),
        "{:?}",
        tools(1)
    );
}
