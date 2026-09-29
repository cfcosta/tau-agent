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
            fresh_repo_list().with_extension("config"),
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

    // Forking the fork at a turn it inherited forks the run that took
    // that turn, with the same conversation and code.
    let again = host
        .fork(&fork.id, Some(1), "a third way", &ModelChoice::default())
        .unwrap();
    assert_eq!(
        again.origin,
        Origin::Fork {
            from: main.id.clone(),
            turn: 1
        }
    );
    host.cancel(&again.id);
    until_end(&mut events);
    wait_until_done(&host, &again.id);

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

    let (host, _events) =
        Host::new(config_on(home.path(), data.path())).unwrap();
    assert_eq!(names(&host), [home_name.as_str(), "proj-2"]);
    let catalog = host.catalog();
    assert_eq!(catalog.closed_runs, std::slice::from_ref(&first));
    assert_eq!(catalog.reviewed, [(first, "call-1".to_owned())]);
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
fn a_checkout_updates_from_itself() {
    let (dir, data) =
        (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    git(dir.path(), &["init", "--quiet"]);
    std::fs::write(dir.path().join("README.md"), "hello\n").unwrap();
    git(dir.path(), &["add", "README.md"]);
    git(dir.path(), &["commit", "--quiet", "-m", "first"]);
    let (host, _events) =
        Host::new(config_on(dir.path(), data.path())).unwrap();
    let project = host.project().expect("the checkout imports");
    let before = project.trunk().unwrap();
    std::fs::write(dir.path().join("b.txt"), "b\n").unwrap();
    git(dir.path(), &["add", "b.txt"]);
    git(dir.path(), &["commit", "--quiet", "-m", "second"]);
    let updated = host.update_repo(host.home()).unwrap();
    assert_eq!(updated.before, before);
    assert!(updated.changed());
    assert!(host.update_repo("no-such-repo").is_err());
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
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("write", write("a.txt")))
        .turn(|t| t.text("wrote a"))
        .turn(|t| t.tool_call("write", write("b.txt")))
        .turn(|t| t.text("wrote b"));
    let (host, mut events) = host_on(llm.clone(), src.path());
    let host = host.with_project(project);

    let chat = host
        .start("write a.txt", &ModelChoice::default(), "")
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &chat.id);
    let dir = host.workspace(&chat.id).unwrap();

    host.resume(&chat.id, "now b.txt", &ModelChoice::default())
        .unwrap();
    let turns: Vec<u32> = until_end(&mut events)
        .into_iter()
        .filter_map(|event| match event {
            RunEvent::TurnStart { run, turn } if run == chat.id => Some(turn),
            _ => None,
        })
        .collect();
    assert_eq!(turns, [3, 4], "turns keep counting");
    wait_until_done(&host, &chat.id);
    // The same workspace, with both turns' files.
    assert_eq!(host.workspace(&chat.id).unwrap(), dir);
    assert!(dir.join("a.txt").exists() && dir.join("b.txt").exists());
    // The model saw the whole chat.
    let last = llm.requests().pop().unwrap();
    assert!(last.transcript.len() > 4, "{}", last.transcript.len());

    let history = host.history().unwrap();
    assert_eq!(history.len(), 1, "one chat, not two");
    let view = &history[0];
    assert_eq!(view.id, chat.id);
    assert_eq!(view.title, "write-a-txt");
    assert_eq!(view.turn, 4);
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
    let (host, mut events) = host_on(llm, dir.path());
    let host = host
        .with_jev(std::sync::Arc::new(tau_jev::fake::FakeJev::nouls(|_| 0.95)));
    // A rule saved the way the Constitution screen saves one.
    host.edit_rules(host.home(), |rules| {
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
    assert!(catalog.plugins.iter().any(|p| p.name == "tau-constitution"));
    let stats = catalog.jev.clone().expect("Jev is set up");
    assert_eq!((stats.requests, stats.failed), (0, 0));

    let mut view = host
        .start("write a.txt", &ModelChoice::default(), "")
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
    host.edit_rules(host.home(), |rules| {
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
    host.edit_rules(host.home(), |rules| {
        rules.remove("R1");
        Ok(())
    })
    .unwrap();
    let constitution = host.catalog().repos[0].constitution.clone();
    assert!(constitution.rules.is_empty());
    // Editing by hand opens the file, which exists once asked for.
    let file = host.constitution_file(host.home()).unwrap();
    assert!(file.exists());
    assert_eq!(file.display().to_string(), constitution.path);
    // A broken file says why, on the Constitution screen.
    std::fs::write(&constitution.path, "[[rule]]\nid = 'A'\n").unwrap();
    let error = host.catalog().repos[0].constitution.error.clone();
    assert!(error.is_some_and(|error| error.contains("not valid")));
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
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("write", write("src.txt")))
        .turn(|t| t.text("Wrote src.txt. cargo test: 3 passed."))
        .turn(|t| t.tool_call("write", write("more.txt")))
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
    assert!(commit.contains("Write src.txt (turn 1)"), "{commit}");
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

    // A later turn of the run goes to the same branch.
    assert!(host.keeps_pushing(&run.id));
    host.resume(&run.id, "add more.txt", &ModelChoice::default())
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &run.id);
    assert!(host.push_later_turns(&run.id).unwrap());
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
    assert!(!host.push_later_turns(&run.id).unwrap(), "nothing new");
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
    let mut view = host.start("track down the race", &auto, "").unwrap();
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
    let run = host.start("rename a variable", &low, "").unwrap();
    until_end(&mut events);
    wait_until_done(&host, &run.id);
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
    let mut view = host.start(prompt, &ModelChoice::default(), "").unwrap();
    assert_eq!(view.title, "the-tests-pass");
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
    let run = host.start("rename a variable", &max, "").unwrap();
    until_end(&mut events);
    wait_until_done(&host, &run.id);
    assert_eq!(llm.requests()[0].settings.reasoning, None);
    let reasoning = run.plan.iter().find(|field| field.name == "reasoning");
    assert_eq!(reasoning.unwrap().value, "auto");
}
