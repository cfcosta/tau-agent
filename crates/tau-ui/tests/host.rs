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
    accounts::{Access, Credentials},
    github::{Api, Token},
    host::{Host, HostConfig},
    models::{AccessKind, Effort, ModelChoice},
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
        credentials: Credentials::new(
            std::env::temp_dir().join("tau-unused-credentials"),
        ),
        model: "gpt-5.5".into(),
        root: root.to_owned(),
        store: std::env::temp_dir().join("unused.db"),
        repos: std::env::temp_dir().join("unused-repos"),
        settings: std::env::temp_dir().join("unused-models.json"),
        repo_list: fresh_repo_list(),
    };
    Host::with_agent(runtime, agent, store, config)
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
    let mut view = host
        .start("Say hello, please", &ModelChoice::default(), "")
        .unwrap();
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
    let view = host.start("wait", &ModelChoice::default(), "").unwrap();
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
    let (host, mut events) = host_on(llm.clone(), src.path());
    let host = host.with_project(project);

    let main = host
        .start("write a.txt twice", &ModelChoice::default(), "")
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &main.id);
    let main_dir = host.workspace(&main.id).unwrap();
    assert_eq!(
        std::fs::read_to_string(main_dir.join("a.txt")).unwrap(),
        "two\n"
    );
    // The user's checkout is untouched.
    assert!(!src.path().join("a.txt").exists());

    let other = ModelChoice::new("gpt-6-sol", Effort::High);
    let fork = host
        .fork(&main.id, Some(1), "try it another way", &other)
        .unwrap();
    assert_eq!(fork.model, "gpt-6-sol");
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
    // The fork asked its own model, at its own effort.
    let asked = llm.requests();
    let last = &asked.last().unwrap().settings;
    assert_eq!(last.model, "gpt-6-sol");
    assert_eq!(
        last.reasoning,
        Some(tau_ai::responses::request::ReasoningEffort::High)
    );
    assert_eq!(asked[0].settings.model, "gpt-5.5");
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
    assert!(history.iter().all(|view| view.repo == host.home()));
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

#[test]
fn runs_come_back_under_their_repository() {
    let other = tempfile::tempdir().unwrap();
    let llm = ScriptedModel::new()
        .turn(|t| t.text("one"))
        .turn(|t| t.text("two"));
    let (host, mut events) = host(llm);
    let repo = host.add_repo(other.path().to_str().unwrap()).unwrap();

    let here = host
        .start("in the checkout", &ModelChoice::default(), "")
        .unwrap();
    assert_eq!(here.repo, host.home());
    until_end(&mut events);
    wait_until_done(&host, &here.id);
    let there = host
        .start("in the other", &ModelChoice::default(), &repo.name)
        .unwrap();
    assert_eq!(there.repo, repo.name);
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
    assert_eq!(repo_of(here.id.clone()), host.home());
    assert_eq!(repo_of(there.id.clone()), repo.name);
}

#[test]
fn repositories_are_listed_and_remembered() {
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let (a, b) = (
        elsewhere.path().join("a/proj"),
        elsewhere.path().join("b/proj"),
    );
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let names = |host: &Host| -> Vec<String> {
        host.catalog()
            .repos
            .iter()
            .map(|repo| repo.name.clone())
            .collect()
    };

    let (host, _events) =
        Host::new(config_on(home.path(), data.path())).unwrap();
    let home_name = host.home().to_owned();
    assert_eq!(names(&host), [home_name.as_str()]);
    // Two checkouts with one name get two names.
    assert_eq!(host.add_repo(a.to_str().unwrap()).unwrap().name, "proj");
    assert_eq!(host.add_repo(b.to_str().unwrap()).unwrap().name, "proj-2");
    // Adding one again keeps its name.
    assert_eq!(host.add_repo(a.to_str().unwrap()).unwrap().name, "proj");
    assert!(host.add_repo("/no/such/checkout").is_err());
    host.set_open_repos(vec!["proj-2".into()]).unwrap();
    assert_eq!(names(&host), [home_name.as_str(), "proj", "proj-2"]);
    drop(host);

    let (host, _events) =
        Host::new(config_on(home.path(), data.path())).unwrap();
    assert_eq!(names(&host), [home_name.as_str(), "proj", "proj-2"]);
    assert_eq!(host.catalog().open_repos, ["proj-2"]);
    host.hide_repo("proj").unwrap();
    assert_eq!(names(&host), [home_name.as_str(), "proj-2"]);
    drop(host);

    let (host, _events) =
        Host::new(config_on(home.path(), data.path())).unwrap();
    assert_eq!(names(&host), [home_name.as_str(), "proj-2"]);
    // Adding a removed one lists it again, under its name.
    assert_eq!(host.add_repo(a.to_str().unwrap()).unwrap().name, "proj");
    assert_eq!(names(&host), [home_name.as_str(), "proj", "proj-2"]);
}

fn config_on(root: &Path, data: &Path) -> HostConfig {
    HostConfig {
        access: Access::ApiKey("sk-test".into()),
        credentials: Credentials::new(data.join("config")),
        model: "gpt-5.5".into(),
        root: root.to_owned(),
        store: data.join("runs.db"),
        repos: data.join("repos"),
        settings: data.join("models.json"),
        repo_list: data.join("repos.json"),
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

#[test]
fn models_follow_the_sign_in_and_settings_persist() {
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();

    // An API key runs every model.
    let (host, _events) =
        Host::new(config_on(dir.path(), data.path())).unwrap();
    let models = host.models();
    assert!(models.options.iter().all(|option| option.available));
    // No settings yet: coder runs on the model the host was given.
    assert_eq!(models.settings.default_for("coder").model, "gpt-5.5");
    let mut settings = models.settings.clone();
    settings.set_default("coder", ModelChoice::new("gpt-6-sol", Effort::Low));
    settings.ask_above = None;
    host.save_settings(settings.clone()).unwrap();
    drop(host);

    // A ChatGPT sign-in runs only what Codex serves; the saved choices
    // come back.
    let credentials = data.path().join("codex.json");
    std::fs::write(
        &credentials,
        serde_json::json!({
            "access": "a", "refresh": "r", "expires": 0, "accountId": "x"
        })
        .to_string(),
    )
    .unwrap();
    let config = HostConfig {
        access: Access::Codex(credentials),
        ..config_on(dir.path(), data.path())
    };
    let (host, _events) = Host::new(config).unwrap();
    let models = host.models();
    assert_eq!(models.settings, settings);
    for option in &models.options {
        assert_eq!(
            option.available,
            tau_ai::codex::MODELS.contains(&option.id.as_str()),
            "{}",
            option.id
        );
    }
    assert!(models.access.chatgpt);
}

#[test]
fn signing_out_runs_on_what_is_left() {
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let config = config_on(dir.path(), data.path());
    let credentials = config.credentials.clone();
    credentials.save_api_key("sk-saved").unwrap();
    std::fs::write(
        credentials.codex(),
        serde_json::json!({
            "access": "a", "refresh": "r", "expires": 0, "accountId": "x"
        })
        .to_string(),
    )
    .unwrap();
    let access = credentials.access().unwrap();
    let (host, _events) = Host::new(HostConfig { access, ..config }).unwrap();
    let saved = |host: &Host| host.models().access.saved;
    assert!(host.models().access.chatgpt);
    assert_eq!(saved(&host), [AccessKind::ChatGpt, AccessKind::ApiKey]);

    // Signing out of ChatGPT leaves the key, which runs every model.
    let left = host.sign_out(AccessKind::ChatGpt).unwrap();
    assert_eq!(left, Some(Access::ApiKey("sk-saved".into())));
    assert!(!credentials.codex().exists());
    let models = host.models();
    assert!(models.access.api_key && !models.access.chatgpt);
    assert_eq!(models.access.saved, [AccessKind::ApiKey]);
    assert!(models.options.iter().all(|option| option.available));

    // Nothing left: no model is available and runs do not start.
    assert_eq!(host.sign_out(AccessKind::ApiKey).unwrap(), None);
    assert!(saved(&host).is_empty());
    assert!(host.models().options.iter().all(|option| !option.available));
    let error = host.start("hi", &ModelChoice::default(), "").unwrap_err();
    assert!(error.to_string().contains("signed out"), "{error}");

    // Signing in again brings runs back.
    host.set_access(Some(Access::ApiKey("sk-new".into())))
        .unwrap();
    assert!(host.models().access.api_key);
}

#[test]
fn github_repositories_clone_into_tau() {
    let (dir, data, remote) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    // What GitHub would serve, as a local repository at owner/name.git.
    let src = remote.path().join("cfcosta/hello.git");
    std::fs::create_dir_all(&src).unwrap();
    git(&src, &["init", "--quiet"]);
    std::fs::write(src.join("README.md"), "hello\n").unwrap();
    git(&src, &["add", "README.md"]);
    git(&src, &["commit", "--quiet", "-m", "first"]);

    let config = config_on(dir.path(), data.path());
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
    let project = host.project_of("hello").expect("the clone imports");
    assert!(!project.trunk().unwrap().is_empty());
    // Cloning it again lists the same repository, without fetching.
    assert_eq!(host.clone_github("cfcosta/hello").unwrap().name, "hello");
}
