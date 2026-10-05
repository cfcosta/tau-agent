//! The host runs agents and reports their events; the view folds them.
//! With a project, each run works in a workspace of its own, forks
//! start from a turn's code, and past runs come back as history.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{path::Path, time::Duration};

use tau_agent::{
    agent::Agent,
    event::{RunEvent, StopReason},
};
use tau_constitution::ui::Act;
use tau_store::Store;
use tau_testing::{git::git, scripted::ScriptedModel};
use tau_ui::{
    accounts::Credentials,
    github::{Api, Token},
    host::{Host, HostConfig},
};
use tau_ui_remote::{
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
    tau_vcs::ProjectRepo::import(
        checkout.to_str().unwrap(),
        tempfile::tempdir().unwrap().keep().join("p"),
        Identity::default(),
    )
    .map(Project::from)
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
    host_of(runtime, store, Agent::new(llm).name("coder"), root)
}

/// A host whose runs start from `agent`.
fn host_of(
    runtime: tokio::runtime::Runtime,
    store: Store,
    agent: Agent,
    root: &Path,
) -> (Host, UnboundedReceiver<RunEvent>) {
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
        skills: std::env::temp_dir().join("tau-test-skills-none"),
    };
    let (host, events) = Host::with_agent(runtime, agent, store, config);
    (host.with_repo(REPO, project_of(root)), events)
}

/// [`host_on`], with the person's skills in `skills`.
fn host_with_skills(
    llm: ScriptedModel,
    root: &Path,
    skills: &Path,
) -> (Host, UnboundedReceiver<RunEvent>) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let store = runtime.block_on(Store::memory()).unwrap();
    let config = HostConfig {
        account: test_account(),
        credentials: Credentials::new(
            fresh_repo_list().with_extension("config"),
        ),
        model: Some("gpt-6-luna".into()),
        store: std::env::temp_dir().join("unused.db"),
        repos: fresh_repo_list().with_extension("repos"),
        settings: fresh_repo_list().with_extension("models"),
        repo_list: fresh_repo_list(),
        skills: skills.to_owned(),
    };
    let agent = Agent::new(llm).name("coder");
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
    // A directory of its own: a host keeps its plugins' files next to
    // its repositories, and tests must not share them.
    let dir = std::env::temp_dir().join(format!(
        "tau-ui-host-{}-{}",
        std::process::id(),
        COUNT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("repos.json")
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
    let main = host.block_on(host.main_of(REPO)).unwrap();
    assert_eq!(
        host.block_on(host.main_of(REPO)).unwrap(),
        main,
        "made once"
    );
    let listed = host.block_on(host.history()).unwrap();
    let view = listed.iter().find(|view| view.id == main).unwrap();
    assert_eq!((view.title.as_str(), &view.origin), ("main", &Origin::Root));
    assert_eq!(view.status, RunStatus::Finished(StopReason::Stop));
    assert_eq!(view.repo, REPO);
    assert!(
        host.block_on(host.set_closed(&main, true)).is_err(),
        "main stays open"
    );
    assert_eq!(
        host.block_on(host.catalog()).repos[0].main.as_ref(),
        Some(&main),
        "the sidebar knows it"
    );

    // Before main has a turn, a chat starts from nothing, on trunk.
    let first = host
        .block_on(host.start("one", &ModelChoice::default(), REPO))
        .unwrap();
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
    host.block_on(host.resume(&main, "hello main", &ModelChoice::default()))
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &main);
    let second = host
        .block_on(host.start("two", &ModelChoice::default(), REPO))
        .unwrap();
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
    let third = host
        .block_on(host.start("three", &ModelChoice::default(), REPO))
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &third.id);
    host.block_on(host.land(&third.id)).unwrap();
    let dir = host.block_on(host.workspace(&main)).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("a.txt")).unwrap(),
        "a.txt\n"
    );
    let project = host.block_on(host.project_of(REPO)).unwrap();
    let on_trunk = |path: &str| {
        project
            .blocking()
            .file_at(&project.blocking().trunk().unwrap(), path)
            .unwrap()
            .map(|(bytes, _)| bytes)
    };
    assert_eq!(on_trunk("a.txt").as_deref(), Some(&b"a.txt\n"[..]));

    // The main chat's own commit moves trunk too.
    host.block_on(host.resume(&main, "write b", &ModelChoice::default()))
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &main);
    assert_eq!(on_trunk("b.txt").as_deref(), Some(&b"b.txt\n"[..]));
    llm.assert_exhausted();
}

/// A run shows its prompt's first line until a model writes its title;
/// a written title comes back with history.
/// A new chat forks the main chat, and with it the goal set there, which
/// tau-goal goes on checking: the chat shows it from the start.
#[test]
fn a_new_chat_shows_the_goal_it_inherits() {
    let dir = tempfile::tempdir().unwrap();
    let llm = ScriptedModel::new()
        .turn(|t| t.text("hi"))
        .turn(|t| t.text("ok"));
    let (host, mut events) = host_on(llm, dir.path());
    let main = host.block_on(host.main_of(REPO)).unwrap();
    host.block_on(
        host.store_plugin_record(
            &main,
            tau_goal::NAME,
            &serde_json::to_value(&tau_goal::Record::Set {
                goal: "the docs build".into(),
                continuations: 4,
                budget: 1.0,
            })
            .unwrap(),
        ),
    )
    .unwrap();
    // A turn after it: new chats fork from there, the goal before.
    on_main(&host, "hello");
    until_end(&mut events);
    wait_until_done(&host, &main);
    let view = host
        .block_on(host.start("look around", &ModelChoice::default(), REPO))
        .unwrap();
    let state = goal_of(&view);
    let goal = state.goal.expect("the inherited goal shows");
    assert_eq!(goal.condition, "the docs build");
    assert_eq!(goal.max_continuations, 4);
    assert!(!state.checks, "no key: nothing checks it");
    until_end(&mut events);
    wait_until_done(&host, &view.id);
}

/// tau-goal's state in `view`, as its fold leaves it.
fn goal_of(view: &tau_ui_remote::view::RunView) -> tau_goal::ui::State {
    view.plugin_states
        .get(tau_goal::NAME)
        .map(|state| serde_json::from_value(state.json().clone()).unwrap())
        .unwrap_or_default()
}

/// The text of tau-goal's notes in `view`'s transcript, in order.
fn goal_notes(view: &tau_ui_remote::view::RunView) -> Vec<String> {
    let state = goal_of(view);
    view.items
        .iter()
        .filter_map(|item| match item {
            Item::Anchor { plugin, key } if plugin == tau_goal::NAME => {
                Some(state.notes[key].text.clone())
            }
            _ => None,
        })
        .collect()
}

/// tau-reasoning's state in `view`, as its fold leaves it.
fn reasoning_of(
    view: &tau_ui_remote::view::RunView,
) -> tau_reasoning::ui::State {
    view.plugin_states
        .get(tau_reasoning::NAME)
        .map(|state| serde_json::from_value(state.json().clone()).unwrap())
        .unwrap_or_default()
}

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
    let catalog = host.block_on(host.catalog());
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
    let reasoning = |catalog: &tau_ui_remote::catalog::Catalog| {
        catalog
            .plugins
            .iter()
            .find(|plugin| plugin.name == tau_reasoning::NAME)
            .cloned()
            .unwrap()
    };
    assert_eq!(
        reasoning(&catalog).page,
        Some(tau_ui_plugin::Link::page("choices").param("run", ""))
    );
    let host = host
        .with_jev(std::sync::Arc::new(tau_jev::fake::FakeJev::nouls(|_| 0.5)));
    let catalog = host.block_on(host.catalog());
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
        .block_on(host.start(
            "Say hello\nto everyone",
            &ModelChoice::default(),
            REPO,
        ))
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
    let history = host.block_on(host.history()).unwrap();
    assert_eq!(history[0].title, "Greet everyone");
}

/// A run read back from history shows the plan it showed live: its
/// model, effort, access and workspace, and the workspace's line.
#[test]
fn a_stored_run_shows_the_plan_it_ran_with() {
    let llm = ScriptedModel::new().turn(|t| t.text("done"));
    let (host, mut events) = host(llm);
    let live = host
        .block_on(host.start("Look around", &ModelChoice::default(), REPO))
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &live.id);
    let history = host.block_on(host.history()).unwrap();
    let stored = history.iter().find(|view| view.id == live.id).unwrap();
    assert_eq!(stored.plan, live.plan);
    assert!(
        stored
            .plan
            .iter()
            .any(|field| field.name == "workspace" && !field.value.is_empty())
    );
    let workspace = |view: &tau_ui_remote::view::RunView| {
        view.plugins.iter().any(|status| status.name == "workspace")
    };
    assert_eq!(workspace(stored), workspace(&live));
}

#[test]
fn a_run_streams_into_its_view() {
    let llm = ScriptedModel::new().turn(|t| t.text("Hello from tau"));
    let (host, mut events) = host(llm);
    let mut view = host
        .block_on(host.start(
            "Say hello, please",
            &ModelChoice::default(),
            REPO,
        ))
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
    let view = host
        .block_on(host.start("wait", &ModelChoice::default(), REPO))
        .unwrap();
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

/// Sends `prompt` to the repository's main chat, the top-level run the
/// others nest under (runs nest one level), and returns its id.
fn on_main(host: &Host, prompt: &str) -> tau_agent::tool::RunId {
    let main = host.block_on(host.main_of(REPO)).unwrap();
    host.block_on(host.resume(&main, prompt, &ModelChoice::default()))
        .unwrap();
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
    let project = tau_vcs::ProjectRepo::import(
        src.path().to_str().unwrap(),
        repos.path().join("p"),
        Identity::default(),
    )
    .map(Project::from)
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
    let main_dir = host.block_on(host.workspace(&main)).unwrap();
    assert_eq!(
        std::fs::read_to_string(main_dir.join("a.txt")).unwrap(),
        "two\n"
    );
    // The user's checkout is untouched.
    assert!(!src.path().join("a.txt").exists());

    let other = ModelChoice::new("gpt-6-sol", Effort::High);
    let fork = host
        .block_on(host.fork(&main, Some(1), "try it another way", &other))
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
    let fork_dir = host.block_on(host.workspace(&fork.id)).unwrap();
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

    let history = host.block_on(host.history()).unwrap();
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
        host.block_on(host.fork(
            &fork.id,
            Some(1),
            "a third way",
            &ModelChoice::default()
        ))
        .is_err()
    );

    // Keeping the fork keeps the main chat's checkout: it is the
    // repository's own, the default workspace, which never goes.
    host.block_on(host.keep_branch(&fork.id)).unwrap();
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
    let project = tau_vcs::ProjectRepo::import(
        src.path().to_str().unwrap(),
        repos.path().join("p"),
        Identity::default(),
    )
    .map(Project::from)
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
    let main_dir = host.block_on(host.workspace(&main)).unwrap();
    let fork = host
        .block_on(host.fork(
            &main,
            Some(1),
            "write c instead",
            &ModelChoice::default(),
        ))
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &fork.id);
    let fork_dir = host.block_on(host.workspace(&fork.id)).unwrap();
    assert!(fork_dir.join("c.txt").exists());
    assert!(!fork_dir.join("b.txt").exists(), "forked before turn 2");

    // A repository's main chat has nothing to land on.
    assert!(
        host.block_on(host.land(&host.block_on(host.main_of(REPO)).unwrap()))
            .is_err()
    );
    // From history, the finished fork waits in the main run's chat.
    let waiting = |host: &Host| {
        host.block_on(host.history())
            .unwrap()
            .iter()
            .find(|view| view.id == main)
            .unwrap()
            .items
            .iter()
            .any(|item| matches!(item, Item::ForkReady { fork: f } if *f == fork.id))
    };
    assert!(waiting(&host));

    let preview = host.block_on(host.preview_landing(&fork.id)).unwrap();
    assert_eq!(preview.changes.len(), 1);
    assert!(preview.conflicts.is_empty());
    assert!(
        !main_dir.join("c.txt").exists(),
        "a preview changes nothing"
    );

    let landed = host.block_on(host.land(&fork.id)).unwrap();
    assert_eq!(landed.changes.len(), 1);
    // The main run has both its own files and the fork's.
    for file in ["a.txt", "b.txt", "c.txt"] {
        assert!(main_dir.join(file).exists(), "{file}");
    }
    // The fork is closed: no workspace, no bookmark.
    assert!(!fork_dir.exists());
    let fork_bookmark = format!("tau/{}", fork.id.0);
    assert_eq!(project.blocking().bookmark(&fork_bookmark).unwrap(), None);
    // The main chat commits on trunk: landing on it moves main.
    assert_eq!(project.blocking().trunk().unwrap(), landed.head);
    // Landing again finds nothing to land.
    assert!(host.block_on(host.land(&fork.id)).is_err());

    // Back from history, the main run shows the landing where it
    // happened: after its last turn, before its stop.
    let history = host.block_on(host.history()).unwrap();
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

/// A main chat that committed `a.txt`, a chat forked from it that
/// committed `c.txt`, and an update that moved trunk to upstream's commit
/// beside the main chat's, which the chat stands on: the main chat has
/// not caught up yet.
struct AfterUpdate {
    host: Host,
    project: Project,
    main_dir: std::path::PathBuf,
    chat: tau_agent::tool::RunId,
    _dirs: (tempfile::TempDir, tempfile::TempDir),
}

fn a_chat_after_an_update() -> AfterUpdate {
    let src = tempfile::tempdir().unwrap();
    git(src.path(), &["init", "--quiet"]);
    std::fs::write(src.path().join("README.md"), "hello\n").unwrap();
    git(src.path(), &["add", "README.md"]);
    git(src.path(), &["commit", "--quiet", "-m", "first"]);
    let repos = tempfile::tempdir().unwrap();
    let project = tau_vcs::ProjectRepo::import(
        src.path().to_str().unwrap(),
        repos.path().join("p"),
        Identity::default(),
    )
    .map(Project::from)
    .unwrap();

    let write =
        |path: &str| serde_json::json!({ "path": path, "content": "x\n" });
    let commit = |message: &str| serde_json::json!({ "message": message });
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("write", write("a.txt")))
        .turn(|t| t.tool_call("vcs_commit", commit("feat: a")))
        .turn(|t| t.text("done"))
        .turn(|t| t.tool_call("write", write("c.txt")))
        .turn(|t| t.tool_call("vcs_commit", commit("feat: c")))
        .turn(|t| t.text("done too"));
    let (host, mut events) = host_on(llm, src.path());
    let host = host.with_repo(REPO, project.clone());

    let main = on_main(&host, "write a");
    until_end(&mut events);
    wait_until_done(&host, &main);
    let main_dir = host.block_on(host.workspace(&main)).unwrap();
    let chat = host
        .block_on(host.fork(&main, None, "write c", &ModelChoice::default()))
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &chat.id);

    std::fs::write(src.path().join("NEW.md"), "new\n").unwrap();
    git(src.path(), &["add", "NEW.md"]);
    git(src.path(), &["commit", "--quiet", "-m", "second"]);
    project
        .blocking()
        .update(tau_vcs::UpdateFrom::Checkout(src.path()))
        .unwrap();
    AfterUpdate {
        host,
        project,
        main_dir,
        chat: chat.id,
        _dirs: (src, repos),
    }
}

/// A chat lands after an update, before the main chat caught up.
/// `Host::land` used to read the chat's head before the catch-up that
/// restacks it, and so failed every time (`DivergentAfterLanding`).
#[test]
fn a_chat_lands_after_an_update() {
    let after = a_chat_after_an_update();
    let landed = after.host.block_on(after.host.land(&after.chat)).unwrap();
    assert_eq!(landed.changes.len(), 1);
    let trunk = after.project.blocking().trunk().unwrap();
    assert_eq!(trunk, landed.head);
    for file in ["README.md", "NEW.md", "a.txt", "c.txt"] {
        let at = after.project.blocking().file_at(&trunk, file).unwrap();
        assert!(at.is_some(), "{file}");
    }
}

/// A chat dropped after an update, before the main chat caught up,
/// takes only its own commits. `Host::drop_child` used to keep only what
/// trunk's bookmark had, upstream's commit, and so abandoned the main
/// chat's commit too, and its files left the main chat's workspace.
#[test]
fn a_chat_drops_after_an_update() {
    let after = a_chat_after_an_update();
    after
        .host
        .block_on(after.host.drop_child(&after.chat))
        .unwrap();
    for file in ["a.txt", "NEW.md"] {
        assert!(after.main_dir.join(file).exists(), "{file}");
    }
    assert!(!after.main_dir.join("c.txt").exists());
    let trunk = after.project.blocking().trunk().unwrap();
    for file in ["NEW.md", "a.txt"] {
        let at = after.project.blocking().file_at(&trunk, file).unwrap();
        assert!(at.is_some(), "{file} on trunk");
    }
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
    let project = tau_vcs::ProjectRepo::import(
        src.path().to_str().unwrap(),
        repos.path().join("p"),
        Identity::default(),
    )
    .map(Project::from)
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
    let child = host
        .block_on(host.fork(&main, Some(1), "write b", &choice))
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &child.id);
    // A chat under the main chat has nothing under it: it cannot be
    // forked, and so it lands with nothing waiting on it.
    let err = host
        .block_on(host.fork(&child.id, Some(2), "write c", &choice))
        .unwrap_err()
        .to_string();
    assert!(err.contains("Only a repository's main chat"), "{err}");

    let landed = host.block_on(host.land(&child.id)).unwrap();
    assert_eq!(landed.changes.len(), 1, "b.txt");
    let main_dir = host.block_on(host.workspace(&main)).unwrap();
    assert!(main_dir.join("b.txt").exists());
}

/// The main chat spawns a sub-agent and waits for it (ADR 0009, 0026):
/// the sub-agent works in a workspace of its own, its change lands on
/// main in the `wait`, and it closes.
#[test]
fn main_waits_for_its_sub_agent_and_it_lands() {
    let src = tempfile::tempdir().unwrap();
    git(src.path(), &["init", "--quiet"]);
    std::fs::write(src.path().join("README.md"), "hello\n").unwrap();
    git(src.path(), &["add", "README.md"]);
    git(src.path(), &["commit", "--quiet", "-m", "first"]);
    let repos = tempfile::tempdir().unwrap();
    let project = tau_vcs::ProjectRepo::import(
        src.path().to_str().unwrap(),
        repos.path().join("p"),
        Identity::default(),
    )
    .map(Project::from)
    .unwrap();

    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call("spawn", serde_json::json!({ "task": "write c.txt" }))
                .tool_call("wait", serde_json::json!({}))
        })
        .turn(|t| {
            t.tool_call(
                "write",
                serde_json::json!({ "path": "c.txt", "content": "c\n" }),
            )
        })
        // The sub-agent commits its work; it lands in the `wait`.
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
    let main = on_main(&host, "hand c.txt off");
    until_end(&mut events);
    wait_until_done(&host, &main);

    let dir = host.block_on(host.workspace(&main)).unwrap();
    assert_eq!(std::fs::read_to_string(dir.join("c.txt")).unwrap(), "c\n");
    // The main chat works in the repository's own checkout, the default
    // workspace.
    assert_eq!(dir, project.workspace_dir(tau_vcs::DEFAULT_WORKSPACE));
    // The sub-agent is closed: no run workspace, and no run bookmark, as
    // the main chat commits on trunk.
    assert!(project.blocking().workspaces().unwrap().is_empty());
    assert!(project.blocking().bookmarks("tau/").unwrap().is_empty());

    // From history, main's `wait` card says what landed, and the
    // sub-agent's chat comes back under it.
    let history = host.block_on(host.history()).unwrap();
    let main_view = history.iter().find(|view| view.id == main).unwrap();
    let card = main_view
        .items
        .iter()
        .find_map(|item| match item {
            Item::Tool(card) if card.tool == "wait" => Some(card),
            _ => None,
        })
        .expect("a wait card");
    let landed = tau_vcs::ui::waited(&card.data).pop().expect("a landing");
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
/// A sub-agent's chat stops on its own: it is dropped, the `wait` on it
/// says so, and the main chat goes on.
#[test]
fn a_sub_agent_stops_and_its_main_chat_goes_on() {
    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call("spawn", serde_json::json!({ "task": "wait" }))
                .tool_call("wait", serde_json::json!({}))
        })
        // The sub-agent's answer would take a minute.
        .turn(|t| t.text("too late").delay(Duration::from_secs(60)))
        .turn(|t| t.text("done without it"));
    let (host, mut events) = host(llm);
    let main = on_main(&host, "hand it off");
    let deadline = std::time::Instant::now() + WAIT;
    let child = loop {
        assert!(std::time::Instant::now() < deadline, "no sub-agent started");
        match events.try_recv() {
            Ok(RunEvent::RunStart {
                run,
                parent: Some(_),
                ..
            }) => break run,
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    };
    host.cancel(&child);
    // Everything up to main's own end.
    let mut seen = Vec::new();
    while !seen.iter().any(
        |event| matches!(event, RunEvent::RunEnd { run, .. } if *run == main),
    ) {
        seen.extend(until_end(&mut events));
    }
    let stopped = |run: &tau_agent::tool::RunId| {
        seen.iter().find_map(|event| match event {
            RunEvent::RunEnd {
                run: ended, stop, ..
            } if ended == run => Some(stop.clone()),
            _ => None,
        })
    };
    assert_eq!(stopped(&child), Some(StopReason::Cancelled));
    assert_eq!(stopped(&main), Some(StopReason::Stop), "main goes on");
    let waited = seen.iter().find_map(|event| match event {
        RunEvent::ToolEnd {
            run,
            output,
            is_error: false,
            ..
        } if *run == main && output.text_content().contains("stopped") => {
            Some(output.text_content())
        }
        _ => None,
    });
    assert!(
        waited.is_some_and(|text| text.contains("The person stopped it")),
        "the wait said it was stopped"
    );
}

/// Answers a sub-agent's requests from `child`, every other from
/// `main`: a sub-agent nobody waits for runs beside main, so one script
/// cannot say whose request comes next.
#[derive(Clone)]
struct Routed {
    main: ScriptedModel,
    child: ScriptedModel,
}

impl tau_ai::llm::Llm for Routed {
    fn open(
        &self,
        settings: tau_ai::responses::request::Settings,
    ) -> futures_util::future::BoxFuture<
        'static,
        Result<Box<dyn tau_ai::llm::LlmSession>, tau_ai::llm::LlmError>,
    > {
        use futures_util::FutureExt as _;
        let main = self.main.open(settings.clone());
        let child = self.child.open(settings);
        async move {
            Ok(Box::new(RoutedSession {
                main: main.await?,
                child: child.await?,
            }) as Box<dyn tau_ai::llm::LlmSession>)
        }
        .boxed()
    }
}

struct RoutedSession {
    main: Box<dyn tau_ai::llm::LlmSession>,
    child: Box<dyn tau_ai::llm::LlmSession>,
}

impl tau_ai::llm::LlmSession for RoutedSession {
    fn settings(&self) -> &tau_ai::responses::request::Settings {
        self.main.settings()
    }

    fn set_reasoning(
        &mut self,
        effort: Option<tau_ai::responses::request::ReasoningEffort>,
    ) {
        self.main.set_reasoning(effort);
        self.child.set_reasoning(effort);
    }

    fn respond(
        &mut self,
        transcript: &[tau_ai::message::Message],
        timestamp: tau_ai::message::Timestamp,
    ) -> tau_ai::llm::EventStream {
        // A sub-agent's transcript says which call it runs.
        let sub_agent = format!("{transcript:?}")
            .contains("You are the sub-agent running this call");
        let session = if sub_agent {
            &mut self.child
        } else {
            &mut self.main
        };
        session.respond(transcript, timestamp)
    }
}

/// A sub-agent nobody waits for (ADR 0026): main's turn ends at once,
/// the sub-agent works on, and once it ends, its work lands on main
/// through main's queue, and tau's turn on main reports what it said
/// and what landed.
#[test]
fn a_sub_agent_nobody_waits_for_lands_and_is_reported() {
    let src = tempfile::tempdir().unwrap();
    git(src.path(), &["init", "--quiet"]);
    std::fs::write(src.path().join("README.md"), "hello\n").unwrap();
    git(src.path(), &["add", "README.md"]);
    git(src.path(), &["commit", "--quiet", "-m", "first"]);
    let repos = tempfile::tempdir().unwrap();
    let project = tau_vcs::ProjectRepo::import(
        src.path().to_str().unwrap(),
        repos.path().join("p"),
        Identity::default(),
    )
    .map(Project::from)
    .unwrap();
    let main_llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call("spawn", serde_json::json!({ "task": "write c.txt" }))
        })
        .turn(|t| t.text("it works on c.txt"))
        // tau's turn with the report.
        .turn(|t| t.text("c.txt is in"));
    let child_llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call(
                "write",
                serde_json::json!({ "path": "c.txt", "content": "c\n" }),
            )
            .tool_call(
                "vcs_commit",
                serde_json::json!({ "message": "feat: add c.txt" }),
            )
        })
        .turn(|t| t.text("wrote c.txt"));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    // On disk, so the test reads the queue's records back itself.
    let db = tempfile::tempdir().unwrap();
    let store = runtime
        .block_on(Store::open(db.path().join("runs.db")))
        .unwrap();
    let agent = Agent::new(Routed {
        main: main_llm.clone(),
        child: child_llm.clone(),
    })
    .name("coder");
    let (host, mut events) = host_of(runtime, store, agent, src.path());
    let host = host.with_repo(REPO, project.clone());
    let main = on_main(&host, "hand c.txt off");
    // Both end: main without waiting, the sub-agent once it is done.
    let mut ended = Vec::new();
    while ended.len() < 2 {
        for event in until_end(&mut events) {
            if let RunEvent::RunEnd { run, stop, .. } = event {
                assert_eq!(stop, StopReason::Stop, "{run}");
                ended.push(run);
            }
        }
    }
    wait_until_done(&host, &main);
    let child = ended.into_iter().find(|run| *run != main).unwrap();
    assert!(host.is_sub_agent(&child));
    let dir = host.block_on(host.workspace(&main)).unwrap();
    // The coordinator hears both end, in either order: main's turn
    // ended, then the sub-agent's work lands on main, idle.
    let before = host.block_on(host.main_turn_ended(&main)).unwrap();
    assert!(before.landed.is_empty(), "nothing waited yet");
    let report = host.block_on(host.sub_agent_ended(&child)).unwrap();
    assert_eq!(report.landed.len(), 1, "{report:?}");
    assert_eq!(report.landed[0].0, child);
    assert_eq!(std::fs::read_to_string(dir.join("c.txt")).unwrap(), "c\n");
    assert!(report.queue.is_empty(), "{:?}", report.queue);
    // It joined the queue with the change it brings, not none.
    let queued = tokio::runtime::Runtime::new().unwrap().block_on(async {
        let store = Store::open(db.path().join("runs.db")).await.unwrap();
        store
            .plugin_entries(&main.0, tau_ui::host::QUEUE_PLUGIN)
            .await
            .unwrap()
    });
    let joined = queued
        .iter()
        .find(|(_, body)| body.contains("\"queued\""))
        .expect("it queued");
    assert!(joined.1.contains("\"changes\":1"), "{}", joined.1);
    let prompt = report.resolve.expect("tau's turn reports it");
    assert!(prompt.contains("wrote c.txt"), "{prompt}");
    assert!(
        prompt.contains("Its 1 change landed on top of yours."),
        "{prompt}"
    );
    // The sub-agent is closed, landed on main.
    assert!(project.blocking().workspaces().unwrap().is_empty());
    assert!(project.blocking().bookmarks("tau/").unwrap().is_empty());
    assert!(matches!(
        host.block_on(host.ending_of(&child)).unwrap(),
        Some(tau_ui_remote::view::Ending::Landed { .. })
    ));
    // tau's turn reports it to main.
    host.block_on(host.start_resolving(&main, &prompt)).unwrap();
    until_end(&mut events);
    wait_until_done(&host, &main);
    let asked = format!("{:?}", main_llm.requests().last().unwrap().transcript);
    assert!(asked.contains("wrote c.txt"), "{asked}");
    main_llm.assert_exhausted();
    child_llm.assert_exhausted();
}

#[test]
fn a_failed_sub_agent_comes_back_from_history() {
    let src = tempfile::tempdir().unwrap();
    git(src.path(), &["init", "--quiet"]);
    std::fs::write(src.path().join("README.md"), "hello\n").unwrap();
    git(src.path(), &["add", "README.md"]);
    git(src.path(), &["commit", "--quiet", "-m", "first"]);
    let repos = tempfile::tempdir().unwrap();
    let project = tau_vcs::ProjectRepo::import(
        src.path().to_str().unwrap(),
        repos.path().join("p"),
        Identity::default(),
    )
    .map(Project::from)
    .unwrap();
    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call("spawn", serde_json::json!({ "task": "write c.txt" }))
                .tool_call("wait", serde_json::json!({}))
        })
        .turn(|t| t.dropped())
        .turn(|t| t.text("it failed"));
    let (host, mut events) = host_on(llm, src.path());
    let host = host.with_repo(REPO, project);
    let main = on_main(&host, "hand c.txt off");
    until_end(&mut events);
    until_end(&mut events);
    wait_until_done(&host, &main);

    let history = host.block_on(host.history()).unwrap();
    let main_view = history.iter().find(|view| view.id == main).unwrap();
    let child = main_view
        .children
        .iter()
        .find(|child| child.kind == tau_ui_remote::view::ChildKind::SubAgent)
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
        .block_on(host.start("in the first", &ModelChoice::default(), REPO))
        .unwrap();
    assert_eq!(here.repo, REPO);
    until_end(&mut events);
    wait_until_done(&host, &here.id);
    let there = host
        .block_on(host.start("in the other", &ModelChoice::default(), "other"))
        .unwrap();
    assert_eq!(there.repo, "other");
    until_end(&mut events);
    wait_until_done(&host, &there.id);

    let history = host.block_on(host.history()).unwrap();
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
        host.block_on(host.start("nowhere", &ModelChoice::default(), "none"))
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
    let token = Token {
        token: "ghu_token".into(),
        user: "cfcosta".into(),
        expires_at: None,
    };
    tau_testing::block_on_io(token.save(&host_credentials(&host))).unwrap();
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
        host.block_on(host.catalog())
            .repos
            .iter()
            .map(|repo| repo.name.clone())
            .collect()
    };

    let (host, _events) = Host::new(config_on(data.path())).unwrap();
    let host = on_github(host, remote.path());
    // Nothing is listed until it is cloned but tau's own plugins: not
    // even the directory tau started in.
    assert_eq!(names(&host), ["tau-plugins"]);
    // Two repositories with one name get two names.
    assert_eq!(
        host.block_on(host.clone_github("a/proj")).unwrap().name,
        "proj"
    );
    assert_eq!(
        host.block_on(host.clone_github("b/proj")).unwrap().name,
        "proj-2"
    );
    // Cloning one again keeps its name.
    assert_eq!(
        host.block_on(host.clone_github("a/proj")).unwrap().name,
        "proj"
    );
    assert!(host.block_on(host.clone_github("c/missing")).is_err());
    host.block_on(host.set_open_repos(vec!["proj-2".into()]))
        .unwrap();
    assert_eq!(names(&host), ["tau-plugins", "proj", "proj-2"]);
    drop(host);

    let (host, _events) = Host::new(config_on(data.path())).unwrap();
    let host = on_github(host, remote.path());
    assert_eq!(names(&host), ["tau-plugins", "proj", "proj-2"]);
    assert_eq!(host.block_on(host.catalog()).open_repos, ["proj-2"]);
    host.block_on(host.hide_repo("proj")).unwrap();
    assert_eq!(names(&host), ["tau-plugins", "proj-2"]);
    // Closed conversations are remembered too, until opened again.
    let (first, second) = (
        tau_agent::tool::RunId("a".into()),
        tau_agent::tool::RunId("b".into()),
    );
    host.block_on(host.set_closed(&first, true)).unwrap();
    host.block_on(host.set_closed(&second, true)).unwrap();
    host.block_on(host.set_closed(&second, false)).unwrap();
    // So are flagged calls someone looked at.
    for _ in 0..2 {
        rules_act(
            &host,
            Act::Reviewed {
                run: "a".into(),
                key: "call-1".into(),
            },
        );
    }
    drop(host);

    let (host, _events) = Host::new(config_on(data.path())).unwrap();
    let host = on_github(host, remote.path());
    assert_eq!(names(&host), ["tau-plugins", "proj-2"]);
    let catalog = host.block_on(host.catalog());
    assert_eq!(catalog.closed_runs, std::slice::from_ref(&first));
    let reviewed: tau_constitution::ui::Data = serde_json::from_value(
        catalog.plugin_data[tau_constitution::NAME].json().clone(),
    )
    .unwrap();
    assert_eq!(reviewed.reviewed, [("a".to_owned(), "call-1".to_owned())]);
    // Cloning a removed one lists it again, under its name.
    assert_eq!(
        host.block_on(host.clone_github("a/proj")).unwrap().name,
        "proj"
    );
    assert_eq!(names(&host), ["tau-plugins", "proj", "proj-2"]);
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
        skills: std::env::temp_dir().join("tau-test-skills-none"),
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
    host.block_on(host.save_settings(settings.clone())).unwrap();
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
    assert_eq!(models.options, tau_ui_remote::models::plan_models());
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
    let error = host
        .block_on(host.start("hi", &ModelChoice::default(), REPO))
        .unwrap_err();
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
    let error = host
        .block_on(host.clone_github("cfcosta/hello"))
        .unwrap_err();
    assert!(error.to_string().contains("Sign in to GitHub"), "{error}");

    let token = Token {
        token: "ghu_token".into(),
        user: "cfcosta".into(),
        expires_at: None,
    };
    tau_testing::block_on_io(token.save(&credentials)).unwrap();
    assert!(host.block_on(host.clone_github("../escape")).is_err());
    assert!(host.block_on(host.clone_github("cfcosta/..")).is_err());
    let repo = host.block_on(host.clone_github("cfcosta/hello")).unwrap();
    assert_eq!(repo.name, "hello");
    assert!(
        host.block_on(host.catalog())
            .repos
            .iter()
            .any(|listed| listed.name == "hello")
    );
    // Waiting for the project is what blocks, not cloning.
    let project = host
        .block_on(host.project_of("hello"))
        .expect("the clone imports");
    assert!(project.root().starts_with(data.path().join("repos")));
    assert!(!project.blocking().trunk().unwrap().is_empty());
    assert!(!host.is_importing());
    assert_eq!(
        host.block_on(host.catalog()).project,
        tau_ui_remote::catalog::ProjectStatus::Unknown
    );
    // Cloning it again lists the same repository, without fetching.
    assert_eq!(
        host.block_on(host.clone_github("cfcosta/hello"))
            .unwrap()
            .name,
        "hello"
    );

    // New commits on GitHub come in with an update.
    std::fs::write(src.join("NEW.md"), "new\n").unwrap();
    git(&src, &["add", "NEW.md"]);
    git(&src, &["commit", "--quiet", "-m", "second"]);
    let updated = host.block_on(host.update_repo("hello")).unwrap();
    assert!(updated.changed());
    assert_eq!(project.blocking().trunk().unwrap(), updated.after);
    assert!(!host.block_on(host.update_repo("hello")).unwrap().changed());
}

#[test]
fn a_finished_run_goes_on_in_its_workspace() {
    let src = tempfile::tempdir().unwrap();
    git(src.path(), &["init", "--quiet"]);
    std::fs::write(src.path().join("README.md"), "hello\n").unwrap();
    git(src.path(), &["add", "README.md"]);
    git(src.path(), &["commit", "--quiet", "-m", "first"]);
    let repos = tempfile::tempdir().unwrap();
    let project = tau_vcs::ProjectRepo::import(
        src.path().to_str().unwrap(),
        repos.path().join("p"),
        Identity::default(),
    )
    .map(Project::from)
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
        .block_on(host.start("write a.txt", &ModelChoice::default(), REPO))
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &chat.id);
    let dir = host.block_on(host.workspace(&chat.id)).unwrap();

    // The chat goes on on another model, without a fork.
    let other = ModelChoice::new("gpt-6-sol", Effort::Auto);
    host.block_on(host.resume(&chat.id, "now b.txt", &other))
        .unwrap();
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
    assert_eq!(host.block_on(host.workspace(&chat.id)).unwrap(), dir);
    assert!(dir.join("a.txt").exists() && dir.join("b.txt").exists());
    // The model saw the whole chat.
    let last = llm.requests().pop().unwrap();
    assert!(last.transcript.len() > 4, "{}", last.transcript.len());
    assert_eq!(last.settings.model, "gpt-6-sol");

    let history = host.block_on(host.history()).unwrap();
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

/// Asks tau-constitution's host half to carry out `act`, as its page
/// does; what it answers, if anything.
fn rules_act(host: &Host, act: Act) -> Option<serde_json::Value> {
    host.block_on(
        host.plugin_act(
            tau_constitution::NAME,
            serde_json::to_value(act).unwrap(),
        ),
    )
    .unwrap()
}

/// A repository's constitution, as its page gets it.
fn rules_of(
    repo: &tau_ui_remote::catalog::Repo,
) -> tau_constitution::ui::Rules {
    serde_json::from_value(repo.plugins[tau_constitution::NAME].json().clone())
        .unwrap()
}

/// What the checks did in `view`, as tau-constitution's fold leaves it.
fn checks_of(
    view: &tau_ui_remote::view::RunView,
) -> tau_constitution::ui::State {
    view.plugin_states
        .get(tau_constitution::NAME)
        .map(|state| serde_json::from_value(state.json().clone()).unwrap())
        .unwrap_or_default()
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
        .block_on(host.project_of(REPO))
        .unwrap()
        .root()
        .canonicalize()
        .unwrap()
        .display()
        .to_string();
    let rules_db = host
        .plugin_dir(tau_constitution::NAME)
        .join("constitution.db");
    let open = || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let db = runtime
            .block_on(tau_constitution::db::Db::open(&rules_db))
            .unwrap();
        (runtime, db)
    };
    // Flagged past where it blocks: it does not check.
    let (runtime, written) = open();
    runtime
        .block_on(written.save_constitution(
            &key,
            &tau_constitution::db::StoredConstitution {
                on_error: "allow".into(),
                max_holds: 3,
                rules: vec![tau_constitution::db::StoredRule {
                    id: "R1".into(),
                    text: "Name the tests.".into(),
                    targets: vec!["final answer".into()],
                    review: 0.9,
                    block: 0.2,
                }],
            },
        ))
        .unwrap();
    let constitution = || rules_of(&host.block_on(host.catalog()).repos[0]);
    assert!(constitution().error.is_some());
    let add = |host: &Host| {
        rules_act(
            host,
            Act::Add {
                repo: REPO.into(),
                text: "No unwrap.".into(),
                on: vec!["write.content".into()],
                review: 0.5,
                block: 0.8,
            },
        )
    };
    add(&host);
    assert!(
        constitution().error.is_some(),
        "no edit gets past rules that cannot be read"
    );

    rules_act(&host, Act::Reset { repo: REPO.into() });
    let reset = constitution();
    assert_eq!((reset.error, reset.rules.len()), (None, 0));
    add(&host);
    assert_eq!(constitution().rules.len(), 1);

    rules_act(
        &host,
        Act::Settings {
            repo: REPO.into(),
            blocks_unchecked: true,
            max_holds: 5,
        },
    );
    let saved = constitution();
    assert!(saved.blocks_unchecked);
    assert_eq!(saved.max_holds, 5);
    let (runtime, read) = open();
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
    let root = host
        .block_on(host.project_of(REPO))
        .unwrap()
        .root()
        .to_owned();
    let rules_db = host
        .plugin_dir(tau_constitution::NAME)
        .join("constitution.db");
    let stored = || {
        let key = root.canonicalize().unwrap().display().to_string();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let db =
                    tau_constitution::db::Db::open(&rules_db).await.unwrap();
                tau_constitution::Constitution::load(&db, &key).await
            })
            .unwrap()
    };
    // A rule saved the way the Constitution page saves one.
    rules_act(
        &host,
        Act::Add {
            repo: REPO.into(),
            text: "No unwrap.".into(),
            on: vec!["write.content".into()],
            review: 0.5,
            block: 0.8,
        },
    );
    let catalog = host.block_on(host.catalog());
    let rules = &rules_of(&catalog.repos[0]);
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
        .block_on(host.start("write a.txt", &ModelChoice::default(), REPO))
        .unwrap();
    for event in until_end(&mut events) {
        view.apply(&event);
    }
    wait_until_done(&host, &view.id);
    assert!(!dir.path().join("a.txt").exists(), "the write was refused");
    let blocked = |view: &tau_ui_remote::view::RunView| {
        view.items.iter().any(|item| {
            matches!(item, Item::Tool(card)
                if matches!(&card.state, ToolState::Blocked { .. }))
        }) && checks_of(view).stats.blocked == ["R1"]
    };
    assert!(blocked(&view), "the card shows the block live");
    assert_eq!(checks_of(&view).stats.calls, 1);
    // The Plugins screen counts the check Jev answered.
    let stats = host.block_on(host.catalog()).jev.expect("Jev is set up");
    assert!(stats.requests >= 1, "{stats:?}");
    assert!(stats.input_tokens > 0 && stats.spent > 0.0);
    assert_eq!((stats.model.as_str(), stats.failed), ("jev-fake", 0));
    // And in history, from what the plugin recorded.
    let history = host.block_on(host.history()).unwrap();
    assert!(blocked(&history[0]));
    assert_eq!(checks_of(&history[0]).stats, checks_of(&view).stats);

    // Editing a rule saves over it, in place.
    rules_act(
        &host,
        Act::Update {
            repo: REPO.into(),
            id: "R1".into(),
            text: "No unwrap, ever.".into(),
            on: vec!["write.content".into()],
            review: 0.3,
            block: 0.9,
        },
    );
    let edited =
        rules_of(&host.block_on(host.catalog()).repos[0]).rules[0].clone();
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
    let try_rule =
        |text: &str, on: &str| -> tau_constitution::ui::TrialResult {
            let reply = rules_act(
                &host,
                Act::Try {
                    text: text.into(),
                    on: vec![on.into()],
                    review: 0.3,
                    block: 0.8,
                    calls: calls.clone(),
                    answers: Vec::new(),
                },
            )
            .expect("a trial answers");
            serde_json::from_value(reply).unwrap()
        };
    let (trials, cost) = try_rule("No unwrap.", "write.content").unwrap();
    assert_eq!(trials.len(), 1);
    assert_eq!(trials[0].shown, "x.unwrap()");
    assert!((trials[0].score - 0.95).abs() < 1e-9 && cost > 0.0);
    assert!(
        try_rule("x", "nowhere").is_err(),
        "a place that names nothing is refused"
    );

    // The store's history counts the run, loaded or not.
    let history = rules_of(&host.block_on(host.catalog()).repos[0]).history;
    assert_eq!(history.len(), 1);
    let (run, checks) = &history[0];
    assert_eq!(**run, *view.id.0);
    assert_eq!(
        (checks.calls, checks.blocked.as_slice()),
        (1, ["R1".to_owned()].as_slice())
    );

    // Removing a rule saves it.
    rules_act(
        &host,
        Act::Remove {
            repo: REPO.into(),
            id: "R1".into(),
        },
    );
    assert!(
        rules_of(&host.block_on(host.catalog()).repos[0])
            .rules
            .is_empty()
    );
    assert!(stored().rules.is_empty());
    // An edit the rules would refuse is not saved.
    rules_act(
        &host,
        Act::Add {
            repo: REPO.into(),
            text: "x".into(),
            on: vec!["write.content".into()],
            review: 0.9,
            block: 0.1,
        },
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
        .block_on(host.catalog())
        .plugins
        .into_iter()
        .find(|plugin| plugin.name == tau_fast_compaction::NAME)
        .expect("fast compaction runs with Jev");
    assert!(status.description.contains("bash outputs"), "{status:?}");

    let mut view = host
        .block_on(host.start("build it", &ModelChoice::default(), REPO))
        .unwrap();
    // With a key, pruning runs with it.
    let pruning: tau_fast_compaction::ui::State = serde_json::from_value(
        view.plugin_states[tau_fast_compaction::NAME].json().clone(),
    )
    .unwrap();
    assert_eq!(pruning.on, Some(true));
    for event in until_end(&mut events) {
        view.apply(&event);
    }
    wait_until_done(&host, &view.id);
    let cut_of = |view: &tau_ui_remote::view::RunView| {
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
    let pruning: tau_fast_compaction::ui::State = serde_json::from_value(
        view.plugin_states[tau_fast_compaction::NAME].json().clone(),
    )
    .unwrap();
    assert_eq!(pruning.outputs, 1);
    assert!(pruning.status().unwrap().contains("large outputs"));

    let history = host.block_on(host.history()).unwrap();
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

/// The credentials directory the test hosts use.
fn host_credentials(host: &Host) -> tau_ui::accounts::Credentials {
    host.credentials().clone()
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
    let mut view = host
        .block_on(host.start("track down the race", &auto, REPO))
        .unwrap();
    for event in until_end(&mut events) {
        view.apply(&event);
    }
    wait_until_done(&host, &view.id);
    assert_eq!(
        llm.requests()[0].settings.reasoning,
        Some(tau_ai::responses::request::ReasoningEffort::High)
    );
    // The plan's reasoning, as tau-reasoning's state says it.
    let chosen = |view: &tau_ui_remote::view::RunView| {
        reasoning_of(view).plan.as_deref() == Some("high")
    };
    assert!(chosen(&view), "{:?}", view.plugin_states);
    assert!(view.items.iter().any(|item| matches!(item,
        Item::Anchor { plugin, .. } if plugin == tau_reasoning::NAME)));
    // History shows it again, from the plugin's record.
    assert!(chosen(&host.block_on(host.history()).unwrap()[0]));

    // An effort someone chose is not scored.
    let (host, mut events) =
        host_on(ScriptedModel::new().turn(|t| t.text("ok")), dir.path());
    let host =
        host.with_jev(std::sync::Arc::new(tau_jev::fake::FakeJev::new(|_| {
            panic!("not asked")
        })));
    let low = ModelChoice::new("gpt-5.5", Effort::Low);
    let run = host
        .block_on(host.start("rename a variable", &low, REPO))
        .unwrap();
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
    let mut settings = tau_ui_remote::models::ModelSettings::default();
    settings.plugins.insert(
        tau_reasoning::NAME.into(),
        serde_json::json!({ "redecide": true, "threshold": 0.9 }),
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
        host.block_on(host.catalog())
            .plugins
            .iter()
            .any(|p| p.name == tau_goal::NAME)
    );

    let prompt = "/goal --continuations 3 the tests pass";
    let mut view = host
        .block_on(host.start(prompt, &ModelChoice::default(), REPO))
        .unwrap();
    assert_eq!(view.title, "the tests pass");
    assert!(goal_of(&view).checks, "with a key, tau-goal checks the run");
    for event in until_end(&mut events) {
        view.apply(&event);
    }
    wait_until_done(&host, &view.id);
    assert_eq!(llm.requests().len(), 2, "sent back once");
    let goal = goal_of(&view).goal.expect("the goal shows live");
    assert_eq!(goal.condition, "the tests pass");
    assert_eq!(goal.status, tau_goal::Status::Met);
    assert_eq!((goal.continuations, goal.max_continuations), (1, 3));
    assert_eq!(
        goal_notes(&view),
        ["the goal is not met yet", "the goal is met"]
    );

    // History has the goal: the message that set it, the same notes as
    // live, and the state from the records; the check's note holds the
    // continuation it sent.
    let history = host.block_on(host.history()).unwrap();
    let stored = &history[0];
    assert_eq!(goal_of(stored).goal, goal_of(&view).goal);
    assert!(matches!(&stored.items[0], Item::User(text)
        if tau_goal::set_message(text).as_deref() == Some("the tests pass")));
    assert_eq!(goal_notes(stored), goal_notes(&view));
    assert_eq!(goal_of(stored).held, [1].into());

    // A met goal is not checked again; a new one is, until paused. This
    // one runs out at once, then gets one more continuation, paused.
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
    for record in [
        tau_goal::Record::Extended { by: 1 },
        tau_goal::Record::Paused,
    ] {
        host.block_on(host.store_plugin_record(
            &view.id,
            tau_goal::NAME,
            &serde_json::to_value(&record).unwrap(),
        ))
        .unwrap();
    }
    host.block_on(host.resume(
        &view.id,
        "one more thing",
        &ModelChoice::default(),
    ))
    .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &view.id);
    assert_eq!(goal_checks(), asked + 1, "paused: not checked");
    let goal = goal_of(&host.block_on(host.history()).unwrap()[0])
        .goal
        .unwrap();
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
    let run = host
        .block_on(host.start("rename a variable", &max, REPO))
        .unwrap();
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
    let history = host.block_on(host.history()).unwrap();
    let state = reasoning_of(&history[0]);
    let order: Vec<String> = history[0]
        .items
        .iter()
        .filter_map(|item| match item {
            Item::User(text) => Some(format!("user: {text}")),
            Item::Anchor { plugin, key } if plugin == tau_reasoning::NAME => {
                match state.notes.get(key)? {
                    tau_reasoning::ui::Note::Choice { text, .. } => {
                        Some(text.clone())
                    }
                    tau_reasoning::ui::Note::Failed { message, .. } => {
                        Some(message.clone())
                    }
                }
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
    let project = tau_vcs::ProjectRepo::import(
        src.path().to_str().unwrap(),
        repos.path().join("p"),
        Identity::default(),
    )
    .map(Project::from)
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
        let catalog = host.block_on(host.catalog());
        let repo = catalog
            .repos
            .iter()
            .find(|repo| repo.name == REPO)
            .expect("the repository is listed")
            .clone();
        serde_json::from_value::<tau_memory::ui::Notebook>(
            repo.plugins[tau_memory::plugin::NAME].json().clone(),
        )
        .unwrap()
    };
    assert!(memory_of(&host).notes.is_empty());
    assert!(
        host.block_on(host.catalog())
            .plugins
            .iter()
            .any(|plugin| plugin.name == "tau-memory")
    );

    let first = host
        .block_on(host.start("note it", &ModelChoice::default(), REPO))
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
        .block_on(host.start("change it", &ModelChoice::default(), REPO))
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
        .block_on(host.catalog())
        .repos
        .into_iter()
        .find(|repo| repo.name == REPO)
        .expect("the repository is listed");
    let main = repo.main.clone().expect("it has a main chat");
    let view = host
        .block_on(host.main_view(&repo))
        .unwrap()
        .expect("the chat is stored");
    assert_eq!(view.id, main);
    assert_eq!(view.title, "main");
    assert_eq!(view.repo, REPO);
    assert_eq!(view.origin, Origin::Root);
}

/// A done chat is not necessarily finalized: an unusable commit-message
/// answer must not let manual landing delete its remaining edits.
#[test]
fn landing_keeps_a_chat_whose_final_commit_failed() {
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("write", serde_json::json!({"path": "committed.txt", "content": "committed\n"})))
        .turn(|t| t.tool_call("vcs_commit", serde_json::json!({"message": "feat: committed"})))
        .turn(|t| t.tool_call("write", serde_json::json!({"path": "pending.txt", "content": "pending\n"})))
        .turn(|t| t.text("done"))
        .turn(|t| t.text("still done"))
        .turn(|t| t.text(""));
    let (host, mut events) = host(llm.clone());
    let chat = host
        .block_on(host.start("write both files", &ModelChoice::default(), REPO))
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &chat.id);
    let dir = host.block_on(host.workspace(&chat.id)).unwrap();
    let project = host.block_on(host.project_of(REPO)).unwrap();
    let trunk = project.blocking().trunk().unwrap();
    for action in [
        host.block_on(host.preview_landing(&chat.id)),
        host.block_on(host.land(&chat.id)),
    ] {
        let error = action.unwrap_err().to_string();
        assert!(error.contains("workspace is retained"), "{error}");
    }
    assert_eq!(project.blocking().trunk().unwrap(), trunk);
    assert_eq!(
        std::fs::read(dir.join("pending.txt")).unwrap(),
        b"pending\n"
    );
    assert_eq!(
        std::fs::read(dir.join("committed.txt")).unwrap(),
        b"committed\n"
    );
    assert!(
        project
            .blocking()
            .bookmark(&format!("tau/{}", chat.id.0))
            .unwrap()
            .is_some()
    );
    llm.assert_exhausted();
}

/// Runs nest one level: the main chat can spawn sub-agents, and a chat
/// under it, which could only nest a sub-agent under itself, has the
/// tool only so its tools match main's: a call is refused, saying why.
#[test]
fn only_the_main_chat_spawns() {
    let llm = ScriptedModel::new()
        .turn(|t| t.text("main"))
        .turn(|t| t.tool_call("spawn", serde_json::json!({ "task": "nest" })))
        .turn(|t| t.text("chat"));
    let (host, mut events) = host(llm.clone());
    let main = on_main(&host, "hello main");
    until_end(&mut events);
    wait_until_done(&host, &main);
    let chat = host
        .block_on(host.start("hello chat", &ModelChoice::default(), REPO))
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &chat.id);
    let requests = llm.requests();
    assert_eq!(requests.len(), 3, "no sub-agent ran");
    let refused = requests[2]
        .transcript
        .iter()
        .find_map(|message| match message {
            tau_ai::message::Message::ToolResult(result)
                if result.tool_name == "spawn" =>
            {
                Some(result.clone())
            }
            _ => None,
        })
        .expect("the spawn call's result");
    assert!(refused.is_error);
    assert!(
        refused.content.iter().any(|block| matches!(
            block,
            tau_ai::message::InputBlock::Text(text)
                if text.text.contains(tau_vcs::ONLY_MAIN_SPAWNS)
        )),
        "{:?}",
        refused.content
    );
}

/// The instructions a request was sent with.
fn instructions_of(request: &tau_testing::scripted::Request) -> String {
    request.settings.instructions.clone().unwrap_or_default()
}

/// A run takes the repository's `AGENTS.md`, from its own workspace,
/// into its instructions as it starts. A chat that changes the file
/// sees its own version the next time it starts, and the main chat
/// keeps reading trunk's.
#[test]
fn a_run_reads_the_repository_s_agents_file() {
    let src = tempfile::tempdir().unwrap();
    git(src.path(), &["init", "--quiet"]);
    std::fs::write(
        src.path().join("AGENTS.md"),
        "Run `make check` before you commit.\n",
    )
    .unwrap();
    git(src.path(), &["add", "AGENTS.md"]);
    git(src.path(), &["commit", "--quiet", "-m", "first"]);
    let rules =
        serde_json::json!({ "path": "AGENTS.md", "content": "Use tabs.\n" });
    let llm = ScriptedModel::new()
        .turn(|t| t.text("hello"))
        .turn(|t| t.tool_call("write", rules))
        .turn(|t| {
            t.tool_call(
                "vcs_commit",
                serde_json::json!({ "message": "docs: tabs" }),
            )
        })
        .turn(|t| t.text("the rules say tabs"))
        .turn(|t| t.text("tabs it is"))
        .turn(|t| t.text("hello again"));
    let (host, mut events) = host_on(llm.clone(), src.path());

    let main = on_main(&host, "hi");
    until_end(&mut events);
    wait_until_done(&host, &main);
    let chat = host
        .block_on(host.start("change the rules", &ModelChoice::default(), REPO))
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &chat.id);
    host.block_on(host.resume(&chat.id, "and now?", &ModelChoice::default()))
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &chat.id);
    host.block_on(host.resume(&main, "and main?", &ModelChoice::default()))
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &main);

    let asked: Vec<String> =
        llm.requests().iter().map(instructions_of).collect();
    assert_eq!(asked.len(), 6);
    for (at, instructions) in asked.iter().enumerate() {
        assert!(
            instructions.contains(tau_ui::host::AGENTS_HEADING),
            "request {at}: {instructions}"
        );
    }
    // The main chat, and the chat as it started on trunk's files.
    for at in [0, 1, 5] {
        assert!(asked[at].contains("Run `make check`"), "{}", asked[at]);
    }
    // Within a start, the instructions stay as they were: the chat's
    // edit waits for its next start.
    assert_eq!(asked[1], asked[3]);
    assert!(asked[4].contains("Use tabs."), "{}", asked[4]);
    assert!(!asked[4].contains("make check"), "{}", asked[4]);
}

/// Without an `AGENTS.md`, a run's instructions are tau's alone.
#[test]
fn a_repository_without_an_agents_file_adds_nothing() {
    let llm = ScriptedModel::new().turn(|t| t.text("hello"));
    let (host, mut events) = host(llm.clone());
    let main = on_main(&host, "hi");
    until_end(&mut events);
    wait_until_done(&host, &main);
    let asked = llm.requests();
    assert_eq!(asked.len(), 1);
    assert!(!instructions_of(&asked[0]).contains(tau_ui::host::AGENTS_HEADING));
}

/// A file over the limit is cut, and the instructions say so.
#[test]
fn a_long_agents_file_is_cut() {
    let src = tempfile::tempdir().unwrap();
    git(src.path(), &["init", "--quiet"]);
    let long = "rule\n".repeat(tau_ui::host::AGENTS_LIMIT / 5 + 100);
    std::fs::write(src.path().join("AGENTS.md"), &long).unwrap();
    git(src.path(), &["add", "AGENTS.md"]);
    git(src.path(), &["commit", "--quiet", "-m", "first"]);
    let llm = ScriptedModel::new().turn(|t| t.text("hello"));
    let (host, mut events) = host_on(llm.clone(), src.path());
    let main = on_main(&host, "hi");
    until_end(&mut events);
    wait_until_done(&host, &main);
    let instructions = instructions_of(&llm.requests()[0]);
    assert!(instructions.contains("AGENTS.md was cut here"), "cut");
    assert!(instructions.len() < long.len() + 2048);
}

/// A chat that landed or was dropped takes no more messages: the host
/// refuses to resume or steer it, saying why, and makes it no new
/// workspace or bookmark, a branch nothing would land. History still
/// loads both, with how they ended.
#[test]
fn a_landed_or_dropped_chat_takes_no_more_messages() {
    use tau_ui_remote::view::Ending;
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
    let src = tempfile::tempdir().unwrap();
    let (host, mut events) = host_on(llm.clone(), src.path());
    let project = host.block_on(host.project_of(REPO)).unwrap();
    let main = host.block_on(host.main_of(REPO)).unwrap();
    let mut chat = |prompt: &str| {
        let view = host
            .block_on(host.start(prompt, &ModelChoice::default(), REPO))
            .unwrap();
        until_end(&mut events);
        wait_until_done(&host, &view.id);
        view.id
    };
    let landed = chat("write a");
    let dropped = chat("write b");
    host.block_on(host.land(&landed)).unwrap();
    host.block_on(host.drop_child(&dropped)).unwrap();
    assert_eq!(
        host.block_on(host.ending_of(&landed)).unwrap(),
        Some(Ending::Landed {
            on: main.clone(),
            changes: 1
        })
    );
    assert_eq!(
        host.block_on(host.ending_of(&dropped)).unwrap(),
        Some(Ending::Dropped)
    );

    let workspaces = project.blocking().workspaces().unwrap();
    for (run, why) in [(&landed, "landed on main"), (&dropped, "was dropped")] {
        let refused = host
            .block_on(host.resume(
                run,
                "one more thing",
                &ModelChoice::default(),
            ))
            .unwrap_err()
            .to_string();
        assert!(refused.contains(why), "{refused}");
        assert!(refused.contains("no longer takes messages"), "{refused}");
        let refused = host
            .block_on(host.steer(run, "one more thing"))
            .unwrap_err()
            .to_string();
        assert!(refused.contains(why), "{refused}");
        assert!(!host.is_running(run));
        assert_eq!(
            project
                .blocking()
                .bookmark(&format!("tau/{}", run.0))
                .unwrap(),
            None
        );
        // A dropped chat cannot land, nor a landed one be dropped.
        assert!(host.block_on(host.land(run)).is_err());
    }
    assert!(host.block_on(host.drop_child(&landed)).is_err());
    assert_eq!(
        project.blocking().workspaces().unwrap(),
        workspaces,
        "none made"
    );
    llm.assert_exhausted();

    let history = host.block_on(host.history()).unwrap();
    let ending = |run: &tau_agent::tool::RunId| {
        history
            .iter()
            .find(|view| view.id == *run)
            .expect("in history")
            .ending
            .clone()
    };
    assert_eq!(
        ending(&landed),
        Some(Ending::Landed {
            on: main.clone(),
            changes: 1
        })
    );
    assert_eq!(ending(&dropped), Some(Ending::Dropped));
    assert_eq!(ending(&main), None);
}

/// Main, a chat and a sub-agent of one repository send byte-identical
/// instructions and tools, in the same order, so each can read the
/// others' prompt cache: `spawn`, `wait` and `vcs_land` are on every run, and
/// say why where they do not apply. Kind-specific guidance goes in the
/// first message instead.
#[test]
fn main_chats_and_sub_agents_send_the_same_prefix() {
    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call("spawn", serde_json::json!({ "task": "look around" }))
                .tool_call("wait", serde_json::json!({}))
        })
        .turn(|t| t.text("looked"))
        .turn(|t| t.text("main done"))
        .turn(|t| t.text("chat done"));
    let (host, mut events) = host(llm.clone());
    let main = on_main(&host, "hand something off");
    until_end(&mut events);
    wait_until_done(&host, &main);
    let chat = host
        .block_on(host.start("hello chat", &ModelChoice::default(), REPO))
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &chat.id);

    let requests = llm.requests();
    assert_eq!(requests.len(), 4);
    let (main, sub_agent, chat) = (&requests[0], &requests[1], &requests[3]);
    let names = |request: &tau_testing::scripted::Request| -> Vec<String> {
        request
            .settings
            .tools
            .iter()
            .map(|t| t.name.clone())
            .collect()
    };
    assert!(names(main).iter().any(|name| name == "spawn"));
    assert!(names(main).iter().any(|name| name == "wait"));
    assert!(names(main).iter().any(|name| name == "vcs_land"));
    for (kind, other) in [("sub-agent", sub_agent), ("chat", chat)] {
        assert_eq!(names(other), names(main), "{kind}'s tool order");
        assert_eq!(other.settings.tools, main.settings.tools, "{kind}'s tools");
        assert_eq!(
            other.settings.instructions, main.settings.instructions,
            "{kind}'s instructions"
        );
    }
}

/// Over OpenAI's transport, against a fake whose prompt cache lives on
/// the connection: a main chat resumed within the idle window goes back
/// to its connection and reads its whole prefix; a chat forked from it
/// takes that connection for its first request and reads main's prefix;
/// and main, resumed again, takes back the connection it served.
#[test]
fn conversations_go_back_to_their_connections() {
    use tau_ai::{client::OpenAi, ws::proto::pool::Limits};
    use tau_testing::fake_openai::{FakeOpenAi, LocalConnector, Reply};

    let fake = FakeOpenAi::new(vec![
        Reply::text("resp_1", "hello"),
        Reply::text("resp_2", "again"),
        Reply::text("resp_3", "chat"),
        Reply::text("resp_4", "main once more"),
    ])
    .report_cache();
    let address = fake.listen();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let store = runtime.block_on(Store::memory()).unwrap();
    let client = {
        let _guard = runtime.enter();
        OpenAi::with_connector(LocalConnector(address), Limits::default())
    };
    let agent = Agent::new(client.clone()).name("coder");
    let root = tempfile::tempdir().unwrap().keep();
    let (host, mut events) = host_of(runtime, store, agent, &root);

    let main = on_main(&host, "hi");
    until_end(&mut events);
    wait_until_done(&host, &main);
    host.block_on(host.resume(&main, "and again", &ModelChoice::default()))
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &main);
    let chat = host
        .block_on(host.start("a chat", &ModelChoice::default(), REPO))
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &chat.id);
    host.block_on(host.resume(
        &main,
        "main once more",
        &ModelChoice::default(),
    ))
    .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &main);

    let received = fake.received();
    assert_eq!(received.len(), 4);
    let key = |at: usize| received[at].body["prompt_cache_key"].clone();
    assert_eq!(key(0), main.0.as_ref());
    assert_eq!(key(1), main.0.as_ref());
    assert_eq!(key(2), chat.id.0.as_ref());
    assert_eq!(key(3), main.0.as_ref());
    // Main's second run: its connection, by delta, all of it cached.
    assert_eq!(received[1].connection, received[0].connection);
    assert!(received[1].body.get("previous_response_id").is_some());
    assert!(received[1].cached_tokens >= received[0].input_tokens);
    // The chat's first request: main's connection and main's prefix.
    assert_eq!(received[2].connection, received[0].connection);
    assert!(received[2].cached_tokens >= received[1].input_tokens);
    // Main again: the connection it served before the chat took it.
    assert_eq!(received[3].connection, received[0].connection);
    let stats = host.block_on(client.stats()).unwrap();
    assert_eq!(stats.connections_opened, 1);
    assert_eq!(stats.handoffs, 1);
}

/// The person's skills are listed in a run's instructions, and the model
/// loads one by name: its instructions, and the folder its files are in.
#[test]
fn a_run_lists_the_persons_skills_and_loads_one() {
    let skills = tempfile::tempdir().unwrap();
    let folder = skills.path().join("release-notes");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join(tau_skills::SKILL_FILE),
        "---\nname: release-notes\ndescription: >\n  Writes release notes\n  between two tags\n---\n\nGroup the commits by kind.\n",
    )
    .unwrap();
    let llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call("skill", serde_json::json!({ "name": "release-notes" }))
        })
        .turn(|t| t.text("grouped"));
    let src = tempfile::tempdir().unwrap();
    let (host, mut events) =
        host_with_skills(llm.clone(), src.path(), skills.path());
    let main = on_main(&host, "write the notes");
    let seen = until_end(&mut events);
    wait_until_done(&host, &main);

    let instructions = instructions_of(&llm.requests()[0]);
    assert!(
        instructions.contains(tau_skills::scan::HEADING),
        "{instructions}"
    );
    assert!(
        instructions
            .contains("- release-notes: Writes release notes between two tags"),
        "{instructions}"
    );
    let loaded = seen.iter().find_map(|event| match event {
        RunEvent::ToolEnd {
            output, is_error, ..
        } => Some((output.text_content(), *is_error)),
        _ => None,
    });
    let (text, failed) = loaded.expect("the skill call ended");
    assert!(!failed, "{text}");
    assert!(text.contains("Group the commits by kind."), "{text}");
    assert!(text.contains(&folder.display().to_string()), "{text}");
}
