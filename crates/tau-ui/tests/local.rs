//! Local repositories through the host (ADR 0043, `docs/reference/vcs.md`,
//! "Local repositories in the host"), before any interface: a folder on
//! this computer, a Git repository built here with the git CLI.
//!
//! - adding it lists it with the trunk jj's `trunk()` names, else the
//!   checked-out branch, the main chat's workspace on it, and its pushes
//!   going to GitHub only when `origin` is there and tau's app can push;
//!   nothing tau writes lands in the person's folder but `.jj` (their
//!   files, branches, tags, `HEAD`, index and config are as before, a
//!   chat's run included), and a chat works in a Git worktree under the
//!   runs' directory the listing names (model, metamorphic);
//! - an add that is refused, or whose setup is stopped at any step,
//!   lists nothing and leaves the folder as it was (fault injection);
//! - started again, tau lists it as it was, with its main chat and its
//!   trunk, and fetches nothing for it.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    collections::BTreeMap,
    ops::ControlFlow,
    path::{Path, PathBuf},
    time::Duration,
};

use gpui::{TestAppContext, VisualTestContext};
use hegel::{TestCase, generators as gs};
use serde_json::json;
use tau_agent::{event::RunEvent, tool::RunId};
use tau_github_host::Token;
use tau_testing::{git::git, scripted::ScriptedModel};
use tau_ui::{
    accounts::Credentials,
    host::{Host, HostConfig, NotAdded},
};
use tau_ui_plugin::vcs::SetupStep;
use tau_ui_remote::{Workspace, catalog::Catalog, models::ModelChoice};
use tau_vcs_host::{
    Identity,
    JjEnv,
    Layout,
    Project,
    ProjectRepo,
    local::{Refusal, State, Step},
};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::support;

const WAIT: Duration = Duration::from_secs(60);
/// The GitHub repository an `origin` on GitHub names.
const ON_GITHUB: &str = "octo/hello";

/// Where `origin` is.
#[derive(Debug, Clone, hegel::PrettyPrintable)]
enum Origin {
    None,
    /// A bare repository here, with `branch` pushed to it, and
    /// `origin/HEAD` naming it or not.
    Bare {
        pushed: Branch,
        head: bool,
    },
    /// GitHub, over SSH, where tau's app can push or not.
    GitHub {
        push: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, hegel::PrettyPrintable)]
enum Branch {
    Main,
    Develop,
}

impl Branch {
    fn name(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Develop => "develop",
        }
    }
}

#[derive(Debug, Clone, hegel::PrettyPrintable)]
struct World {
    origin: Origin,
    checked_out: Branch,
}

#[hegel::composite]
fn branch(tc: &TestCase) -> Branch {
    if tc.draw(gs::booleans()) {
        Branch::Main
    } else {
        Branch::Develop
    }
}

#[hegel::composite]
fn world(tc: &TestCase) -> World {
    let origin = match tc.draw(gs::integers::<u8>().max_value(2)) {
        0 => Origin::None,
        1 => Origin::Bare {
            pushed: tc.draw(branch()),
            head: tc.draw(gs::booleans()),
        },
        _ => Origin::GitHub {
            push: tc.draw(gs::booleans()),
        },
    };
    World {
        origin,
        checked_out: tc.draw(branch()),
    }
}

impl World {
    /// The trunk adding must choose: `origin/HEAD`'s branch, through
    /// the `trunk()` alias; jj's own `trunk()`, which names `main@origin`;
    /// else the checked-out branch. The two branches are on commits of
    /// their own.
    fn trunk(&self) -> &'static str {
        match self.origin {
            Origin::Bare { pushed, head: true } => pushed.name(),
            Origin::Bare {
                pushed: Branch::Main,
                head: false,
            } => "main",
            _ => self.checked_out.name(),
        }
    }

    /// Where pushes must go.
    fn push(&self) -> serde_json::Value {
        match self.origin {
            Origin::GitHub { push: true } => json!({ "forge": ON_GITHUB }),
            _ => json!("nowhere"),
        }
    }
}

/// What outlives a host: the person's folder, tau's store, list and
/// directories, and a fake GitHub.
struct Disk {
    _dir: tempfile::TempDir,
    root: PathBuf,
    folder: PathBuf,
    bare: PathBuf,
    github: String,
}

impl Disk {
    fn new(world: &World) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let folder = root.join("hello");
        std::fs::create_dir_all(&folder).unwrap();
        git(&folder, &["init", "--quiet"]);
        std::fs::write(folder.join("README.md"), "hello\n").unwrap();
        std::fs::write(folder.join(".gitignore"), "*.log\n").unwrap();
        git(&folder, &["add", "README.md", ".gitignore"]);
        git(&folder, &["commit", "--quiet", "-m", "first"]);
        git(&folder, &["checkout", "--quiet", "-b", "develop"]);
        std::fs::write(folder.join("DEVELOP.md"), "next\n").unwrap();
        git(&folder, &["add", "DEVELOP.md"]);
        git(&folder, &["commit", "--quiet", "-m", "second"]);
        git(&folder, &["checkout", "--quiet", world.checked_out.name()]);
        // The person's own work, which stays theirs: a change, an
        // untracked file and an ignored one.
        std::fs::write(folder.join("README.md"), "hello, edited\n").unwrap();
        std::fs::write(folder.join("notes.txt"), "mine\n").unwrap();
        std::fs::write(folder.join("build.log"), "ignored\n").unwrap();
        let bare = root.join("origin.git");
        match &world.origin {
            Origin::None => {}
            Origin::Bare { pushed, head } => {
                std::fs::create_dir_all(&bare).unwrap();
                git(&bare, &["init", "--quiet", "--bare"]);
                let url = bare.to_str().unwrap();
                git(&folder, &["remote", "add", "origin", url]);
                git(&folder, &["push", "--quiet", "origin", pushed.name()]);
                if *head {
                    let target =
                        format!("refs/remotes/origin/{}", pushed.name());
                    git(
                        &folder,
                        &["symbolic-ref", "refs/remotes/origin/HEAD", &target],
                    );
                }
            }
            Origin::GitHub { .. } => {
                let url = format!("git@github.com:{ON_GITHUB}.git");
                git(&folder, &["remote", "add", "origin", &url]);
            }
        }
        let push = matches!(world.origin, Origin::GitHub { push: true });
        let (github, _) = support::fake(move |line| match line {
            "GET /repos/octo/hello" => (
                200,
                json!({ "permissions": { "pull": true, "push": push } }),
            ),
            _ => (404, json!({})),
        });
        for dir in ["home", "config/jj"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        let disk = Self {
            _dir: dir,
            root,
            folder,
            bare,
            github,
        };
        tau_testing::block_on_io(
            Token {
                token: "ghu_token".into(),
                user: "octo".into(),
                expires_at: None,
            }
            .save(&disk.credentials().dir),
        )
        .unwrap();
        disk
    }

    /// tau's identity under the person's jj config directory and home,
    /// both empty: with a config directory, a setup writes the
    /// repository's `trunk()` alias, as the jj CLI does, and the app
    /// always has one.
    fn identity(&self) -> Identity {
        let config = self.root.join("config");
        Identity {
            env: JjEnv {
                config_dir: Some(config.clone()),
                home_dir: Some(self.root.join("home")),
                vars: [(
                    "XDG_CONFIG_HOME".to_owned(),
                    config.display().to_string(),
                )]
                .into(),
                ..JjEnv::default()
            },
            ..Identity::default()
        }
    }

    fn credentials(&self) -> Credentials {
        Credentials::new(self.root.join("config"))
    }

    fn repo_list(&self) -> PathBuf {
        self.root.join("repos.json")
    }

    /// A host started on what is on disk, as tau starts: it opens what
    /// the list holds.
    fn start(&self, llm: ScriptedModel) -> (Host, UnboundedReceiver<RunEvent>) {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let store = runtime
            .block_on(tau_store_sqlite::open(self.root.join("runs.db")))
            .unwrap();
        let config = HostConfig {
            credentials: self.credentials(),
            model: Some("gpt-6-luna".into()),
            store: self.root.join("runs.db"),
            repos: self.root.join("data/repos"),
            settings: self.root.join("models.json"),
            repo_list: self.repo_list(),
            home: None,
            services: tau_ui_plugin::Services::default().offer(
                tau_ui_plugin::services::names::IDENTITY,
                self.identity(),
            ),
        };
        let agent = tau_agent::agent::Agent::new(llm).name("coder");
        let (host, events) = Host::with_agent(runtime, agent, store, config);
        let host = host
            .with_plugin_settings(
                tau_memory::NAME,
                json!({ "after_each_run": false }),
            )
            .with_forge(std::sync::Arc::new(tau_github_host::GitHub::new(
                self.credentials().dir,
                &tau_github_host::Endpoints {
                    web: self.github.clone(),
                    api: self.github.clone(),
                },
            )))
            .with_listed_repos()
            .unwrap();
        (host, events)
    }

    /// What `repos.json` keeps of the repositories listed.
    /// The project of the one repository listed, a local one, opened
    /// again where its listing says its workspaces are.
    fn project(&self) -> Project {
        let listed = self.listed();
        let local = &listed[0]["local"];
        let dir = |key: &str| PathBuf::from(local[key].as_str().unwrap());
        let layout =
            Layout::local(dir("folder"), dir("main_dir"), dir("runs_dir"));
        ProjectRepo::open_local(layout, self.identity())
            .map(Project::from)
            .unwrap()
    }

    fn listed(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.repo_list())
            .ok()
            .map(|text| {
                let list: serde_json::Value =
                    serde_json::from_str(&text).unwrap();
                list["repos"].as_array().cloned().unwrap_or_default()
            })
            .unwrap_or_default()
    }

    /// tau's directories for local repositories.
    fn local_dirs(&self) -> PathBuf {
        self.root.join("data/repos/local")
    }
}

/// The person's Git, as they see it: `HEAD`, the index's bytes, their
/// branches and tags, `.git/config`, and their files. The index is read
/// first: no git command here refreshes it.
#[derive(Debug, PartialEq, Eq)]
struct GitView {
    head: Vec<u8>,
    index: Vec<u8>,
    refs: String,
    config: Vec<u8>,
    files: BTreeMap<PathBuf, Vec<u8>>,
}

fn git_view(folder: &Path) -> GitView {
    let dot_git = folder.join(".git");
    GitView {
        index: std::fs::read(dot_git.join("index")).unwrap(),
        head: std::fs::read(dot_git.join("HEAD")).unwrap(),
        refs: git(
            folder,
            &[
                "for-each-ref",
                "--format=%(refname) %(objectname) %(symref)",
                "refs/heads",
                "refs/tags",
                "refs/remotes",
            ],
        ),
        config: std::fs::read(dot_git.join("config")).unwrap(),
        files: tree(folder, &[".git", ".jj"]),
    }
}

/// Every file and folder under `dir` but those in `skip`, with its
/// bytes (a folder's are empty, a link's its target).
fn tree(dir: &Path, skip: &[&str]) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![dir.to_owned()];
    while let Some(next) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(dir).unwrap().to_owned();
            if skip.iter().any(|skip| relative.starts_with(skip)) {
                continue;
            }
            let kind = std::fs::symlink_metadata(&path).unwrap().file_type();
            if kind.is_dir() {
                files.insert(relative, Vec::new());
                pending.push(path);
            } else if kind.is_symlink() {
                let target = std::fs::read_link(&path).unwrap();
                files.insert(
                    relative,
                    target.into_os_string().into_encoded_bytes(),
                );
            } else {
                files.insert(relative, std::fs::read(&path).unwrap());
            }
        }
    }
    files
}

fn go_on(_: SetupStep) -> ControlFlow<()> {
    ControlFlow::Continue(())
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
    panic!("the run did not end");
}

fn wait_until_done(host: &Host, run: &RunId) {
    let deadline = std::time::Instant::now() + WAIT;
    while host.is_running(run) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!host.is_running(run));
}

/// A chat's script: it writes `file` and commits it.
fn writes(llm: ScriptedModel, file: &str) -> ScriptedModel {
    let write = json!({ "path": file, "content": "x\n" });
    let commit = json!({ "message": format!("feat: {file}") });
    llm.turn(|t| t.tool_call("write", write))
        .turn(|t| t.tool_call("vcs_commit", commit))
        .turn(|t| t.text("done"))
}

/// A chat in `repo` that writes and commits `file`; its workspace.
fn chat(
    host: &Host,
    events: &mut UnboundedReceiver<RunEvent>,
    repo: &str,
    file: &str,
) -> (RunId, PathBuf) {
    let view = host
        .block_on(host.start(
            format!("write {file}"),
            &ModelChoice::default(),
            repo,
        ))
        .unwrap();
    until_end(events);
    wait_until_done(host, &view.id);
    let dir = host.block_on(host.workspace(&view.id)).unwrap();
    (view.id, dir)
}

/// The commit `rev` names in the Git repository at `dir`.
fn rev(dir: &Path, rev: &str) -> String {
    git(dir, &["rev-parse", rev])
}

/// Adding a folder lists it with its trunk and where its pushes go, puts
/// the main chat's workspace on trunk, and leaves the person's Git as it
/// was; a chat in it works in a Git worktree under the runs' directory,
/// and leaves the folder as it was too.
#[hegel::test(
    test_cases = 10,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn adding_a_folder_lists_it_on_trunk_beside_the_persons_work(tc: TestCase) {
    let world = tc.draw(world());
    let disk = Disk::new(&world);
    let before = git_view(&disk.folder);
    let llm = writes(ScriptedModel::new(), "chat.txt");
    let (host, mut events) = disk.start(llm.clone());
    let repo = host.block_on(host.add_local(&disk.folder, go_on)).unwrap();
    assert_eq!(repo.name, "hello");
    assert_eq!(repo.path, disk.folder.display().to_string());
    assert!(disk.folder.join(".jj").is_dir());
    assert_eq!(git_view(&disk.folder), before);

    // Listed with what opens it again, and where its pushes go.
    let listed = disk.listed();
    assert_eq!(listed.len(), 1, "{listed:?}");
    let local = &listed[0]["local"];
    assert_eq!(local["folder"], json!(disk.folder));
    assert_eq!(local["push"], world.push(), "{local}");
    assert_eq!(local["links"], json!([]));
    assert_eq!(local["main_workspace"], json!("tau-main"));
    let id = listed[0]["id"].as_str().unwrap();
    let dir = disk.local_dirs().join(id);
    let runs_dir = PathBuf::from(local["runs_dir"].as_str().unwrap());
    let main_dir = PathBuf::from(local["main_dir"].as_str().unwrap());
    assert_eq!(runs_dir, dir.join("runs/hello"));
    assert_eq!(main_dir, dir.join("main"));
    let state = State::read(&disk.folder).unwrap().unwrap();
    assert_eq!(state.runs_dir, "hello");
    assert_eq!(state.trunk.as_deref(), Some(world.trunk()));

    // The main chat's workspace is on trunk: its `HEAD` is `@-`.
    let trunk = rev(&disk.folder, world.trunk());
    let project = disk.project();
    assert_eq!(project.blocking().trunk_name().unwrap(), world.trunk());
    assert_eq!(project.blocking().trunk().unwrap(), trunk);
    assert_eq!(rev(&main_dir, "HEAD"), trunk);
    assert_eq!(
        std::fs::read(main_dir.join("README.md")).unwrap(),
        b"hello\n"
    );

    // A chat works in a worktree of the folder's Git under the runs'
    // directory, and what it writes stays there.
    let (_, workspace) = chat(&host, &mut events, "hello", "chat.txt");
    assert_eq!(workspace.parent(), Some(runs_dir.as_path()));
    assert!(workspace.join("chat.txt").is_file());
    let common = git(&workspace, &["rev-parse", "--git-common-dir"]);
    assert_eq!(
        std::fs::canonicalize(&common).unwrap(),
        disk.folder.join(".git")
    );
    assert_eq!(git_view(&disk.folder), before);
    // What tau keeps for it is in its directory.
    assert!(dir.join("artifacts").is_dir());
    llm.assert_exhausted();

    // Added again, it is listed already.
    let again = host.block_on(host.add_local(&disk.folder, go_on));
    assert!(
        matches!(&again, Err(NotAdded::Listed { name, .. }) if name == "hello"),
        "{again:?}"
    );
    // Its pushes go nowhere, or wait for pushing without `origin`.
    let pushed = host.block_on(host.push_main("hello", false));
    assert!(pushed.is_err(), "{pushed:?}");
}

/// Where an add is made to fail.
#[derive(Debug, Clone, Copy, hegel::PrettyPrintable)]
enum Cut {
    /// Inspecting refuses the folder: it is inside the repository.
    Refused,
    /// The setup is stopped before this step.
    Stopped(usize),
}

/// An add that fails, refused or stopped at any step of the setup,
/// lists nothing, leaves the folder as it was, `.git` and all (but for
/// the objects the setup wrote), and leaves nothing in tau's
/// directories; adding again then works.
#[hegel::test(
    test_cases = 10,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn a_failed_add_lists_nothing_and_leaves_the_folder(tc: TestCase) {
    let world = tc.draw(world());
    let cut = if tc.draw(gs::integers::<u8>().max_value(4)) == 0 {
        Cut::Refused
    } else {
        Cut::Stopped(
            tc.draw(gs::integers::<usize>().max_value(Step::ALL.len() - 1)),
        )
    };
    let disk = Disk::new(&world);
    std::fs::create_dir_all(disk.folder.join("docs")).unwrap();
    let everything = || tree(&disk.folder, &[".git/objects"]);
    let before = everything();
    let (host, _events) = disk.start(ScriptedModel::new());
    let added = match cut {
        Cut::Refused => {
            host.block_on(host.add_local(&disk.folder.join("docs"), go_on))
        }
        Cut::Stopped(k) => {
            host.block_on(host.add_local(&disk.folder, move |step| {
                if step.at == k {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            }))
        }
    };
    match cut {
        Cut::Refused => assert!(
            matches!(
                &added,
                Err(NotAdded::Refused(error)) if matches!(
                    error.downcast_ref::<Refusal>(),
                    Some(Refusal::Inside { root }) if *root == disk.folder
                )
            ),
            "{added:?}"
        ),
        Cut::Stopped(_) => {
            assert!(matches!(&added, Err(NotAdded::Failed(_))), "{added:?}")
        }
    }
    assert_eq!(everything(), before);
    assert!(disk.listed().is_empty(), "{:?}", disk.listed());
    assert!(host.block_on(host.catalog()).repos.is_empty());
    assert!(tree(&disk.local_dirs(), &[]).is_empty());
    host.block_on(host.add_local(&disk.folder, go_on)).unwrap();
    assert_eq!(disk.listed().len(), 1);
}

/// Runs GPUI until `done` holds.
fn until(
    cx: &mut VisualTestContext,
    what: &str,
    mut done: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Started again, tau lists the local repository as it was: its main
/// chat, its trunk, its chat's workspace. It fetches nothing for it as
/// it starts, though `origin` moved, and says nothing of an update.
#[gpui::test]
fn a_restart_keeps_a_local_repository_and_fetches_nothing(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let world = World {
        origin: Origin::Bare {
            pushed: Branch::Main,
            head: true,
        },
        checked_out: Branch::Develop,
    };
    let disk = Disk::new(&world);
    let llm = writes(ScriptedModel::new(), "kept.txt");
    let (host, mut events) = disk.start(llm.clone());
    let added = host.block_on(host.add_local(&disk.folder, go_on)).unwrap();
    let (_, kept_dir) = chat(&host, &mut events, "hello", "kept.txt");
    llm.assert_exhausted();
    drop(host);
    let listed = disk.listed();

    // Someone pushes to `origin` while tau is closed.
    let elsewhere = disk.root.join("elsewhere");
    git(
        &disk.root,
        &["clone", "--quiet", disk.bare.to_str().unwrap(), "elsewhere"],
    );
    git(
        &elsewhere,
        &["commit", "--quiet", "--allow-empty", "-m", "theirs"],
    );
    git(&elsewhere, &["push", "--quiet", "origin", "main"]);
    let before = git_view(&disk.folder);

    let (host, events) = disk.start(ScriptedModel::new());
    cx.update(tau_ui_remote::init);
    let window = cx.add_window(|window, cx| {
        Workspace::new("tau", Vec::new(), Catalog::default(), window, cx)
    });
    let workspace = window.root(cx).unwrap();
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    let host = cx.update(|_, cx| host.attach(&workspace, events, cx));
    until(&mut cx, "the host's catalog", |cx| {
        workspace.read_with(cx, |ws, _| !ws.catalog().repos.is_empty())
    });
    until(&mut cx, "the host to settle", |_| !host.busy());

    assert_eq!(disk.listed(), listed);
    let catalog = host.block_on(host.catalog());
    assert_eq!(catalog.repos.len(), 1);
    assert_eq!(catalog.repos[0].id, added.id);
    assert_eq!(catalog.repos[0].main, added.main);
    assert_eq!(catalog.update, None);
    let project = disk.project();
    assert!(project.blocking().layout().local);
    assert_eq!(project.blocking().trunk_name().unwrap(), "main");
    assert_eq!(project.root(), disk.folder);
    // The open chat keeps its workspace, and nothing was fetched.
    assert!(kept_dir.join("kept.txt").is_file());
    let name = kept_dir.file_name().unwrap().to_str().unwrap().to_owned();
    assert!(project.blocking().workspaces().unwrap().contains(&name));
    assert_eq!(git_view(&disk.folder), before);
}
