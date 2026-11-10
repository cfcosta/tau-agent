//! Sending work to GitHub (ADR 0023): a repository's main chat pushes
//! trunk with `git`, and a chat's pull request carries only the chat's
//! commits, replayed onto GitHub's `main`. GitHub is a bare repository
//! reached through a `file://` URL, and the forge a fake that opens the
//! pull requests and answers their checks.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use tau_agent::{agent::Agent, event::RunEvent, tool::RunId};
use tau_testing::{
    git::{self, git},
    scripted::ScriptedModel,
};
use tau_ui::{
    accounts::Credentials,
    host::{Host, HostConfig},
};
use tau_ui_remote::{models::ModelChoice, push::PushFailure};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::common::{self, forge::FakeForge};

const FULL_NAME: &str = "cfcosta/hello";
const WAIT: Duration = Duration::from_secs(60);

/// GitHub as a bare repository at `<remote>/cfcosta/hello.git`, whose
/// `main` has one commit with `README.md`, and a checkout that pushes to
/// it as someone else would.
struct GitHub {
    remote: tempfile::TempDir,
    bare: PathBuf,
    work: PathBuf,
}

impl GitHub {
    fn new() -> Self {
        let remote = tempfile::tempdir().unwrap();
        let work = remote.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "--quiet"]);
        std::fs::write(work.join("README.md"), "hello\n").unwrap();
        git(&work, &["add", "README.md"]);
        git(&work, &["commit", "--quiet", "-m", "first"]);
        let bare = remote.path().join(format!("{FULL_NAME}.git"));
        std::fs::create_dir_all(bare.parent().unwrap()).unwrap();
        git(
            remote.path(),
            &["clone", "--quiet", "--bare", "work", bare.to_str().unwrap()],
        );
        git(&work, &["remote", "add", "github", bare.to_str().unwrap()]);
        Self { remote, bare, work }
    }

    /// The forge, signed in, its clones and pushes going here.
    fn forge(&self) -> std::sync::Arc<FakeForge> {
        let web = format!("file://{}", self.remote.path().display());
        std::sync::Arc::new(FakeForge::at(web).signed_in("cfcosta"))
    }

    /// GitHub's `main`, or another branch.
    fn branch(&self, name: &str) -> String {
        git(&self.bare, &["rev-parse", &format!("refs/heads/{name}")])
    }

    /// Whether `commit` on GitHub has `path`.
    fn has(&self, commit: &str, path: &str) -> bool {
        git::output(
            &self.bare,
            &["cat-file", "-e", &format!("{commit}:{path}")],
        )
        .status
        .success()
    }

    /// Someone else pushes a commit writing `path` to GitHub's `main`.
    fn push_upstream(&self, path: &str) {
        git(
            &self.work,
            &["pull", "--quiet", "--ff-only", "github", "main"],
        );
        std::fs::write(self.work.join(path), "theirs\n").unwrap();
        git(&self.work, &["add", path]);
        git(&self.work, &["commit", "--quiet", "-m", "upstream"]);
        git(&self.work, &["push", "--quiet", "github", "HEAD:main"]);
    }
}

/// A host signed in to GitHub, reaching it at `github` through `forge`,
/// on `llm`, with nothing cloned yet.
fn host(
    llm: ScriptedModel,
    forge: std::sync::Arc<FakeForge>,
    data: &Path,
) -> (Host, UnboundedReceiver<RunEvent>) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let store = runtime.block_on(tau_store_sqlite::memory()).unwrap();
    let config = HostConfig {
        credentials: Credentials::new(data.join("config")),
        model: Some("gpt-6-luna".into()),
        store: data.join("unused.db"),
        repos: data.join("repos"),
        settings: data.join("models.json"),
        repo_list: data.join("repos.json"),
        home: None,
        services: Default::default(),
    };
    let agent = Agent::new(llm).name("coder");
    let (host, events) = Host::with_agent(runtime, agent, store, config);
    // Memory's pass after each run asks the model once more: these
    // scripts answer only the requests they write.
    let host = host.with_plugin_settings(
        tau_memory::NAME,
        serde_json::json!({ "after_each_run": false }),
    );
    (host.with_forge(forge), events)
}

fn until_end(events: &mut UnboundedReceiver<RunEvent>) {
    let deadline = std::time::Instant::now() + WAIT;
    while std::time::Instant::now() < deadline {
        match events.try_recv() {
            Ok(RunEvent::RunEnd { .. }) => return,
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    panic!("no RunEnd within {WAIT:?}");
}

fn wait_until_done(host: &Host, run: &RunId) {
    let deadline = std::time::Instant::now() + WAIT;
    while host.is_running(run) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!host.is_running(run));
}

/// Runs `run` on `prompt` to its end: the main chat goes on, as a
/// message to it does.
fn go_on(
    host: &Host,
    events: &mut UnboundedReceiver<RunEvent>,
    run: &RunId,
    prompt: &str,
) {
    host.block_on(host.resume(run, prompt, &ModelChoice::default()))
        .unwrap();
    until_end(events);
    wait_until_done(host, run);
}

fn write(path: &str, text: &str) -> serde_json::Value {
    serde_json::json!({ "path": path, "content": text })
}

fn commit(message: &str) -> serde_json::Value {
    serde_json::json!({ "message": message })
}

/// The repository as the sidebar lists it.
fn listed(host: &Host, name: &str) -> tau_ui_remote::catalog::Repo {
    host.block_on(host.catalog()).repo(name).unwrap().clone()
}

/// The main chat's commits go to GitHub as they are, and the count
/// ahead of GitHub follows. After someone else pushed, the push is
/// refused as GitHub's `main` having moved and changes nothing; fetch
/// and push puts the main chat's commit on top of theirs, with its
/// change id, and pushes it.
#[test]
fn main_pushes_and_a_moved_main_fetches_first() {
    let github = GitHub::new();
    let data = tempfile::tempdir().unwrap();
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("write", write("m.txt", "main\n")))
        .turn(|t| t.tool_call("vcs_commit", commit("feat: add m")))
        .turn(|t| t.text("done"))
        .turn(|t| t.tool_call("write", write("n.txt", "main\n")))
        .turn(|t| t.tool_call("vcs_commit", commit("feat: add n")))
        .turn(|t| t.text("done"));
    let (host, mut events) = host(llm, github.forge(), data.path());
    let repo = host.block_on(host.clone_from_forge(FULL_NAME)).unwrap();
    let project = common::project(&host, &repo.name);
    let main = host.block_on(host.main_of(&repo.name)).unwrap();
    assert_eq!(listed(&host, &repo.name).unpushed, 0);

    go_on(&host, &mut events, &main, "write m");
    let ahead = listed(&host, &repo.name);
    assert_eq!(ahead.unpushed, 1);
    assert_eq!(ahead.trunk.as_deref(), Some("main"));

    let pushed = host.block_on(host.push_main(&repo.name, false)).unwrap();
    assert_eq!(pushed.branch, "main");
    assert_eq!(pushed.changes.len(), 1);
    assert_eq!(pushed.changes[0].title, "feat: add m");
    assert_eq!(github.branch("main"), project.blocking().trunk().unwrap());
    assert_eq!(pushed.to, project.blocking().trunk().unwrap());
    assert!(github.has(&pushed.to, "m.txt"));
    assert_eq!(listed(&host, &repo.name).unpushed, 0);

    go_on(&host, &mut events, &main, "write n");
    let waiting = project.blocking().unpushed().unwrap();
    assert_eq!(waiting.len(), 1);
    github.push_upstream("u.txt");
    let before = github.branch("main");
    let refused = host
        .block_on(host.push_main(&repo.name, false))
        .unwrap_err();
    assert_eq!(
        refused,
        PushFailure::Moved {
            branch: "main".into(),
            ahead: 1
        }
    );
    assert_eq!(github.branch("main"), before, "nothing was pushed");

    let pushed = host.block_on(host.push_main(&repo.name, true)).unwrap();
    assert_eq!(pushed.from.as_deref(), Some(before.as_str()));
    assert_eq!(pushed.changes.len(), 1);
    assert_eq!(pushed.changes[0].change_id, waiting[0].change_id);
    let head = github.branch("main");
    assert_eq!(head, project.blocking().trunk().unwrap());
    for path in ["m.txt", "n.txt", "u.txt"] {
        assert!(github.has(&head, path), "{path}");
    }
    assert!(project.blocking().unpushed().unwrap().is_empty());
}

/// A chat forked after the main chat committed what GitHub does not
/// have: its pull request carries the chat's commits alone, replayed on
/// GitHub's `main`, pushed with `git` to the pull request's branch. The
/// chat's later commits go to the same branch, on top. The main chat
/// opens none: it pushes.
#[test]
fn a_chat_pull_request_carries_only_its_commits_after_main_committed() {
    let github = GitHub::new();
    let data = tempfile::tempdir().unwrap();
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("write", write("m.txt", "main\n")))
        .turn(|t| t.tool_call("vcs_commit", commit("feat: add m")))
        .turn(|t| t.text("done"))
        .turn(|t| t.tool_call("write", write("c.txt", "chat\n")))
        .turn(|t| t.tool_call("vcs_commit", commit("feat: add c")))
        .turn(|t| t.text("Wrote c.txt. cargo test: 3 passed."))
        .turn(|t| t.tool_call("write", write("d.txt", "chat\n")))
        .turn(|t| t.tool_call("vcs_commit", commit("feat: add d")))
        .turn(|t| t.text("Added d.txt."));
    let forge = github.forge();
    let (host, mut events) = host(llm, forge.clone(), data.path());
    let repo = host.block_on(host.clone_from_forge(FULL_NAME)).unwrap();
    let project = common::project(&host, &repo.name);
    let main = host.block_on(host.main_of(&repo.name)).unwrap();
    go_on(&host, &mut events, &main, "write m");
    let mains = project.blocking().trunk().unwrap();
    let origin = github.branch("main");

    let chat = host
        .block_on(host.start(
            "write c, please",
            &ModelChoice::default(),
            &repo.name,
        ))
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &chat.id);

    let error = host.block_on(host.prepare_pull_request(&main)).unwrap_err();
    assert!(error.to_string().contains("pushes to GitHub"), "{error}");

    let draft = host.block_on(host.prepare_pull_request(&chat.id)).unwrap();
    assert_eq!(draft.repo, FULL_NAME);
    assert_eq!(draft.base, "main");
    assert!(draft.mergeable);
    assert_eq!(draft.commits.len(), 1);
    assert_eq!(draft.commits[0].title, "feat: add c");
    assert_eq!((draft.commits[0].added, draft.commits[0].removed), (1, 0));
    assert!(
        draft.summary.contains("changed 1 files"),
        "{}",
        draft.summary
    );

    let (opened, head) = host
        .block_on(host.create_pull_request(
            &chat.id,
            &draft,
            "Write c",
            "The body",
            true,
            true,
            &["alice".into()],
        ))
        .unwrap();
    assert_eq!(opened.number, 7);
    assert_eq!(github.branch(&draft.head), head);
    // On GitHub's main, with the chat's file and not the main chat's.
    let parent = git(&github.bare, &["rev-parse", &format!("{head}^")]);
    assert_eq!(parent, origin);
    assert!(github.has(&head, "c.txt"));
    assert!(!github.has(&head, "m.txt"));
    let carries_main = git::output(
        &github.bare,
        &["merge-base", "--is-ancestor", &mains, &head],
    );
    assert!(!carries_main.status.success());
    assert_eq!(github.branch("main"), origin, "main was not pushed");
    // The forge is asked for the pull request alone, as drafted, with
    // its reviewers: the commits went with `git`.
    let pulls = forge.pulls();
    assert_eq!(pulls.len(), 1);
    let pull = &pulls[0];
    assert_eq!(pull.repo, FULL_NAME);
    assert_eq!(
        (pull.base.as_str(), pull.head.as_str()),
        ("main", &*draft.head)
    );
    assert_eq!(
        (pull.title.as_str(), pull.body.as_str()),
        ("Write c", "The body")
    );
    assert!(pull.draft);
    assert_eq!(pull.reviewers, ["alice"]);
    assert_eq!(forge.asked(), [format!("open_pull {FULL_NAME}")]);
    assert_eq!(
        host.block_on(host.pull_request_checks(&chat.id)).unwrap(),
        tau_ui_remote::pull_request::Checks::Passed
    );
    // Of the commit that went.
    assert_eq!(forge.asked()[1], format!("checks {FULL_NAME} {head}"));

    // A later commit goes on top of the branch.
    assert!(host.keeps_pushing(&chat.id));
    go_on(&host, &mut events, &chat.id, "add d");
    assert!(host.block_on(host.push_later_commits(&chat.id)).unwrap());
    let later = github.branch(&draft.head);
    assert_eq!(
        git(&github.bare, &["rev-parse", &format!("{later}^")]),
        head
    );
    assert!(github.has(&later, "d.txt"));
    assert!(!github.has(&later, "m.txt"));
    assert!(
        !host.block_on(host.push_later_commits(&chat.id)).unwrap(),
        "nothing new"
    );
}

/// A chat that changes what the main chat has not pushed cannot go on
/// GitHub's `main` alone: the pull request is refused, naming the file.
#[test]
fn a_chat_on_mains_unpushed_file_would_conflict() {
    let github = GitHub::new();
    let data = tempfile::tempdir().unwrap();
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("write", write("m.txt", "main\n")))
        .turn(|t| t.tool_call("vcs_commit", commit("feat: add m")))
        .turn(|t| t.text("done"))
        .turn(|t| t.tool_call("write", write("m.txt", "chat\n")))
        .turn(|t| t.tool_call("vcs_commit", commit("fix: change m")))
        .turn(|t| t.text("done"));
    let (host, mut events) = host(llm, github.forge(), data.path());
    let repo = host.block_on(host.clone_from_forge(FULL_NAME)).unwrap();
    let main = host.block_on(host.main_of(&repo.name)).unwrap();
    go_on(&host, &mut events, &main, "write m");
    let chat = host
        .block_on(host.start("change m", &ModelChoice::default(), &repo.name))
        .unwrap();
    until_end(&mut events);
    wait_until_done(&host, &chat.id);
    let error = host
        .block_on(host.prepare_pull_request(&chat.id))
        .unwrap_err();
    assert_eq!(error.to_string(), "would conflict on origin/main: m.txt");
}
