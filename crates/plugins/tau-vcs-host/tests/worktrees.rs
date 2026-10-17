//! Run workspaces are Git worktrees of the project's Git store
//! (`docs/reference/vcs.md`, "Projects"; ADR 0043, decision 11), made
//! and removed without `git`, and checked here against the git CLI and
//! the jj CLI of jj-lib's version.
//!
//! - Over random sequences of runs coming and going (on trunk, or on
//!   the main chat's own commits, forks included, some with long
//!   sub-agent names), file edits and tools in them, the main chat's
//!   commits and catch-ups with upstream (which leave the runs on the
//!   main chat's commits stale), the person's `git worktree prune`, and
//!   the person's `jj` in a run's workspace, against a model of each
//!   workspace's `@-` and files:
//!   - `git worktree list --porcelain` lists the main checkout and
//!     exactly the live runs' workspaces, each detached at the `@-` the
//!     model has for it: the one the workspace's own last tool left, so
//!     an operation elsewhere moves no other workspace's `HEAD`, and a
//!     stale one keeps its until its files move;
//!   - jj's view records that same `HEAD` for the workspace, so the jj
//!     CLI's `HEAD` import in it moves nothing;
//!   - in each, `git status --porcelain` names exactly the paths the
//!     model says `@` changes, `git ls-files` lists `@`'s new files
//!     (intent to add) with `@-`'s, and `git rev-parse --show-toplevel`
//!     is the workspace;
//!   - after a forget the worktree is gone and `git fsck` is clean;
//!   - the jj CLI says each run's workspace is colocated, `jj status`
//!     works there, its `@-` is the model's, and tau's next tool there
//!     works.
//! - A worktree cut off at each step of its making, or pruned by the
//!   person after, is made whole again by the next open or the next
//!   use of the workspace, with one worktree folder per workspace.
//! - A run workspace from before runs had worktrees, made by the jj CLI
//!   with no worktree, gains one when the project opens.
//!
//! Where the jj CLI is not installed, the steps that need it are
//! skipped and the tests say so. Each world lives in a temporary
//! directory and reads no real config.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

#[path = "common/jj_cli.rs"]
mod jj_cli;

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

use hegel::{TestCase, generators as gs};
use jj_lib::{
    default_backend_factories::{
        default_backend_factories,
        default_working_copy_factories,
    },
    object_id::ObjectId as _,
    workspace::Workspace,
};
use pollster::block_on;
use serde_json::{Value, json};
use tau_agent::{
    plugin::Plugin as _,
    tool::{AgentTool, ToolCtx},
};
use tau_ai::message::InputBlock;
use tau_testing::{block_on_io, git::git};
use tau_vcs_host::{Identity, JjEnv, ProjectRepo, UpdateFrom, Vcs, VcsPlugin};

/// The paths runs edit. The main chat and upstream write files of
/// their own, so these are the same in every commit but a run's.
const PATHS: [&str; 3] = ["a.txt", "b.txt", "dir/c.txt"];
/// Each of a different size: Git's index keeps a file's size and time,
/// and jj-lib's index reset copies them from the index before, so an
/// edit that keeps a file's size within one tick of the clock that
/// stamps files is one `git status` does not see, in jj's colocated
/// workspaces as in tau's.
const VALUES: [Option<&str>; 3] = [Some("one\n"), Some("two!\n"), None];

/// A workspace's files under [`PATHS`].
type Files = BTreeMap<String, String>;

/// Trunk's files under [`PATHS`].
fn trunk_files() -> Files {
    Files::from([("a.txt".to_owned(), "one\n".to_owned())])
}

/// A source repository, and the config every project and the jj CLI
/// read here.
struct World {
    _dir: tempfile::TempDir,
    root: PathBuf,
    identity: Identity,
    jj: Option<PathBuf>,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        for sub in ["home", "config", "src"] {
            std::fs::create_dir_all(root.join(sub)).unwrap();
        }
        let src = root.join("src");
        git(&src, &["init", "--quiet"]);
        std::fs::write(src.join("a.txt"), "one\n").unwrap();
        git(&src, &["add", "."]);
        git(&src, &["commit", "--quiet", "-m", "first"]);
        let identity = Identity {
            name: "Tau".into(),
            email: "tau@localhost".into(),
            env: JjEnv {
                config_dir: Some(root.join("config")),
                home_dir: Some(root.join("home")),
                system_dir: None,
                hostname: Some("desk".into()),
                username: Some("person".into()),
                vars: HashMap::new(),
            },
        };
        Self {
            _dir: dir,
            root,
            identity,
            jj: jj_cli::matching_jj(),
        }
    }

    fn src(&self) -> PathBuf {
        self.root.join("src")
    }

    /// The project imported from the source, with its main chat caught
    /// up with trunk as the host does before its first turn.
    fn import(&self) -> (ProjectRepo, Tools) {
        let project = ProjectRepo::import(
            self.src().to_str().unwrap(),
            self.root.join("p"),
            self.identity.clone(),
        )
        .unwrap();
        let main = Tools::new(
            block_on_io(Vcs::open(project.main_dir(), self.identity.clone()))
                .unwrap(),
        );
        let trunk = project.trunk().unwrap();
        block_on_io(main.vcs.move_onto(
            trunk,
            project.trunk_name().unwrap(),
            true,
        ))
        .unwrap();
        (project, main)
    }

    fn open(&self) -> ProjectRepo {
        ProjectRepo::open(self.root.join("p"), self.identity.clone()).unwrap()
    }

    /// The jj CLI in `dir`, with this world's home and config and no
    /// other variable of this process; it must succeed.
    fn jj(&self, jj: &Path, dir: &Path, args: &[&str]) -> String {
        let output = Command::new(jj)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("JJ_USER", "Person")
            .env("JJ_EMAIL", "person@localhost")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "jj {args:?}: {output:?}");
        String::from_utf8(output.stdout).unwrap()
    }
}

/// One workspace's handle and its tools.
struct Tools {
    vcs: Vcs,
    tools: Vec<Arc<dyn AgentTool>>,
}

impl Tools {
    fn new(vcs: Vcs) -> Self {
        let tools = VcsPlugin::new(vcs.clone()).tools();
        Self { vcs, tools }
    }

    fn ok(&self, name: &str, args: Value) -> Value {
        let tool = self
            .tools
            .iter()
            .find(|tool| tool.name() == name)
            .unwrap_or_else(|| panic!("no tool {name}"));
        let shown = args.to_string();
        let output = block_on_io(tool.call(args, ToolCtx::detached()))
            .unwrap_or_else(|err| panic!("{name} {shown} failed: {err}"));
        assert!(matches!(output.content[0], InputBlock::Text(_)));
        output.details.unwrap_or(Value::Null)
    }

    /// `@-`'s commit id, from `vcs_status`.
    fn parent(&self) -> String {
        let status = self.ok("vcs_status", json!({}));
        status["parents"][0]["commit_id"]
            .as_str()
            .unwrap()
            .to_owned()
    }
}

/// `edits` written into `dir`, and into `files`.
fn write(dir: &Path, files: &mut Files, edits: &[(&str, Option<&str>)]) {
    for (path, value) in edits {
        let at = dir.join(path);
        match value {
            Some(text) => {
                std::fs::create_dir_all(at.parent().unwrap()).unwrap();
                std::fs::write(&at, text).unwrap();
                files.insert((*path).to_owned(), (*text).to_owned());
            }
            None => {
                let _ = std::fs::remove_file(&at);
                files.remove(*path);
            }
        }
    }
}

#[hegel::composite]
fn edits(tc: &TestCase) -> Vec<(&'static str, Option<&'static str>)> {
    tc.draw(
        gs::vecs(gs::tuples!(
            gs::sampled_from(PATHS.to_vec()),
            gs::sampled_from(VALUES.to_vec())
        ))
        .max_size(3),
    )
}

/// A run's workspace name: a chat's, a sub-agent's (`<caller>-sub-<12
/// hex>`), or a sub-agent's of a long-named caller, longer than a
/// worktree id may be.
#[hegel::composite]
fn run_name(tc: &TestCase) -> String {
    let hex = |tc: &TestCase, n: usize| -> String {
        tc.draw(gs::from_regex(&format!("[0-9a-f]{{{n}}}")).fullmatch(true))
    };
    let caller = format!("chat-{}", hex(tc, 4));
    match tc.draw(gs::integers::<u8>().max_value(2)) {
        0 => caller,
        1 => format!("{caller}-sub-{}", hex(tc, 12)),
        _ => format!(
            "{caller}-{}-sub-{}-sub-{}",
            hex(tc, 32),
            hex(tc, 12),
            hex(tc, 12)
        ),
    }
}

/// One live run, as the model has it.
struct Run {
    tools: Tools,
    /// What Git's `HEAD` in its workspace should be: `@-` as its own
    /// last tool left it.
    head: String,
    /// `@-`'s files and the files on disk, under [`PATHS`].
    base: Files,
    disk: Files,
    /// It stands on the main chat's own commits, which a catch-up
    /// rebases.
    on_main: bool,
    /// A catch-up rebased it; its next tool moves its files.
    stale: bool,
}

/// What `git worktree list --porcelain` in `dir` says: each worktree's
/// path, its `HEAD`, and its other lines.
fn worktree_list(dir: &Path) -> BTreeMap<PathBuf, (String, Vec<String>)> {
    let text = git(dir, &["worktree", "list", "--porcelain"]);
    let mut found = BTreeMap::new();
    for block in text.split("\n\n") {
        let mut path = None;
        let mut head = String::new();
        let mut rest = Vec::new();
        for line in block.lines() {
            if let Some(at) = line.strip_prefix("worktree ") {
                path = Some(PathBuf::from(at));
            } else if let Some(at) = line.strip_prefix("HEAD ") {
                head = at.to_owned();
            } else {
                rest.push(line.to_owned());
            }
        }
        found.insert(path.expect("a worktree line"), (head, rest));
    }
    found
}

/// The paths `git status --porcelain` in `dir` names.
fn status_paths(dir: &Path) -> BTreeSet<String> {
    // Not trimmed: the first line may start with a space.
    let output = tau_testing::git::output(
        dir,
        &[
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--no-renames",
        ],
    );
    assert!(output.status.success(), "git status: {output:?}");
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| line[3..].to_owned())
        .collect()
}

/// What `git ls-files` in `dir` lists of [`PATHS`].
fn listed(dir: &Path) -> BTreeSet<String> {
    git(dir, &["ls-files"])
        .lines()
        .filter(|path| PATHS.contains(path))
        .map(str::to_owned)
        .collect()
}

/// The paths whose files differ between `a` and `b`.
fn differ(a: &Files, b: &Files) -> BTreeSet<String> {
    a.keys()
        .chain(b.keys())
        .filter(|path| a.get(*path) != b.get(*path))
        .cloned()
        .collect()
}

/// The `HEAD` jj's view records for each workspace, read with jj-lib in
/// `main`.
fn view_heads(world: &World, main: &Path) -> BTreeMap<String, String> {
    let settings = tau_vcs_host::jj_settings(
        &world.identity,
        main,
        &main.join(".jj").join("repo"),
    )
    .unwrap();
    let workspace = Workspace::load(
        &settings,
        main,
        &default_backend_factories(),
        &default_working_copy_factories(),
    )
    .unwrap();
    let repo = block_on(workspace.repo_loader().load_at_head()).unwrap();
    repo.view()
        .wc_commit_ids()
        .keys()
        .filter_map(|name| {
            let head = repo.view().git_head(name).as_normal()?.hex();
            Some((name.as_str().to_owned(), head))
        })
        .collect()
}

/// `git fsck` in `dir` finds nothing wrong.
fn assert_fsck_clean(dir: &Path, what: &str) {
    let output = tau_testing::git::output(dir, &["fsck", "--no-dangling"]);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "{what}: git fsck: {text}");
    assert!(
        !text.contains("error") && !text.contains("broken"),
        "{what}: git fsck: {text}"
    );
}

/// The worktree folders in `main`'s Git store that name `dir`.
fn admin_dirs_naming(main: &Path, dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(main.join(".git").join("worktrees"))
    else {
        return Vec::new();
    };
    entries
        .map(|entry| entry.unwrap().path())
        .filter(|admin| {
            std::fs::read_to_string(admin.join("gitdir"))
                .is_ok_and(|text| Path::new(text.trim()) == dir.join(".git"))
        })
        .collect()
}

struct Machine {
    world: World,
    project: ProjectRepo,
    main: Tools,
    /// The main chat's `@-`, as its last tool left it.
    main_head: String,
    /// The main chat's commits so far.
    main_commits: usize,
    upstream: usize,
    runs: BTreeMap<String, Run>,
}

impl Machine {
    fn new() -> Self {
        let world = World::new();
        let (project, main) = world.import();
        let main_head = main.parent();
        Self {
            world,
            project,
            main,
            main_head,
            main_commits: 0,
            upstream: 0,
            runs: BTreeMap::new(),
        }
    }

    fn dir(&self, name: &str) -> PathBuf {
        self.project.workspace_dir(name)
    }

    /// One of the live runs, by name.
    fn pick(&self, tc: &TestCase) -> Option<String> {
        if self.runs.is_empty() {
            return None;
        }
        let names: Vec<String> = self.runs.keys().cloned().collect();
        Some(tc.draw(gs::sampled_from(names)))
    }

    /// Whether `commit` is upstream's, not one of the main chat's own.
    fn is_upstream(&self, commit: &str) -> bool {
        tau_testing::git::output(
            &self.world.src(),
            &["cat-file", "-e", &format!("{commit}^{{commit}}")],
        )
        .status
        .success()
    }

    fn do_add_run(&mut self, tc: &TestCase) {
        if self.runs.len() >= 4 {
            return;
        }
        let name = tc.draw(run_name());
        if self.runs.contains_key(&name) {
            return;
        }
        let base = if tc.draw(gs::booleans()) {
            self.main_head.clone()
        } else {
            self.project.trunk().unwrap()
        };
        let vcs = self.project.add_workspace(&name, &base).unwrap();
        let on_main = !self.is_upstream(&base);
        if on_main {
            tc.event("a run on the main chat's commits");
        }
        if name.len() > 64 {
            tc.event("a run with a long name");
        }
        self.runs.insert(
            name,
            Run {
                tools: Tools::new(vcs),
                head: base,
                base: trunk_files(),
                disk: trunk_files(),
                on_main,
                stale: false,
            },
        );
    }

    /// A fork of a fresh run, from its `@` as its last snapshot has it.
    fn do_fork(&mut self, tc: &TestCase) {
        let Some(parent) = self.pick(tc) else {
            return;
        };
        let name = tc.draw(run_name());
        if self.runs.contains_key(&name)
            || self.runs.len() >= 4
            || self.runs[&parent].stale
        {
            return;
        }
        let from = &self.runs[&parent];
        let status = from.tools.ok("vcs_status", json!({}));
        let snapshot = status["working_copy"]["commit_id"].as_str().unwrap();
        let vcs = self
            .project
            .add_workspace_from_snapshot(&name, snapshot)
            .unwrap();
        tc.event("a fork");
        let run = Run {
            tools: Tools::new(vcs),
            head: from.head.clone(),
            base: from.base.clone(),
            disk: from.disk.clone(),
            on_main: from.on_main,
            stale: false,
        };
        self.runs.insert(name, run);
    }

    /// Edits in a run, then one of its tools.
    fn do_edit(&mut self, tc: &TestCase) {
        let Some(name) = self.pick(tc) else {
            return;
        };
        let edits = tc.draw(edits());
        let tool = tc.draw(gs::sampled_from(vec![
            "status", "commit", "new", "describe",
        ]));
        let dir = self.dir(&name);
        let run = self.runs.get_mut(&name).unwrap();
        write(&dir, &mut run.disk, &edits);
        if run.stale {
            tc.event("a stale run's files move");
        }
        match tool {
            "commit" if run.base != run.disk => {
                run.tools.ok("vcs_commit", json!({ "message": "work" }));
                run.base = run.disk.clone();
            }
            "new" => {
                run.tools.ok("vcs_new", json!({}));
                run.base = run.disk.clone();
            }
            "describe" => {
                run.tools.ok("vcs_describe", json!({ "message": "wip" }));
            }
            _ => {
                run.tools.ok("vcs_status", json!({}));
            }
        }
        run.head = run.tools.parent();
        run.stale = false;
    }

    /// The main chat commits a file of its own.
    fn do_main_commit(&mut self) {
        let path = format!("main-{}.txt", self.main_commits);
        std::fs::write(self.project.main_dir().join(path), "main\n").unwrap();
        self.main.ok("vcs_commit", json!({ "message": "main" }));
        self.main_commits += 1;
        self.main_head = self.main.parent();
    }

    /// Upstream moves, the project updates, and the main chat catches
    /// up, rebasing its own commits and the runs on them.
    fn do_catch_up(&mut self, tc: &TestCase) {
        let src = self.world.src();
        std::fs::write(src.join(format!("up-{}.txt", self.upstream)), "up\n")
            .unwrap();
        self.upstream += 1;
        git(&src, &["add", "-A"]);
        git(&src, &["commit", "--quiet", "-m", "upstream"]);
        self.project.update(UpdateFrom::Checkout(&src)).unwrap();
        let trunk = self.project.trunk().unwrap();
        let name = self.project.trunk_name().unwrap();
        block_on_io(self.main.vcs.move_onto(trunk, name, true)).unwrap();
        self.main_head = self.main.parent();
        for run in self.runs.values_mut() {
            if run.on_main {
                run.stale = true;
                tc.event("a catch-up leaves a run stale");
            }
        }
    }

    fn do_forget(&mut self, tc: &TestCase) {
        let Some(name) = self.pick(tc) else {
            return;
        };
        let dir = self.dir(&name);
        self.project.forget_workspace(&name).unwrap();
        self.runs.remove(&name);
        let main = self.project.main_dir().to_owned();
        assert!(!dir.exists(), "{name}'s workspace stays");
        assert_eq!(
            admin_dirs_naming(&main, &dir),
            Vec::<PathBuf>::new(),
            "{name}'s worktree stays"
        );
        assert_fsck_clean(&main, "after a forget");
    }

    fn do_prune(&mut self) {
        git(self.project.main_dir(), &["worktree", "prune"]);
    }

    /// The person edits files in a run's workspace and runs `jj` there.
    fn do_person_jj(&mut self, tc: &TestCase) {
        let Some(jj) = self.world.jj.clone() else {
            return;
        };
        let Some(name) = self.pick(tc) else {
            return;
        };
        if self.runs[&name].stale {
            return;
        }
        let edits = tc.draw(edits());
        let dir = self.dir(&name);
        let run = self.runs.get_mut(&name).unwrap();
        write(&dir, &mut run.disk, &edits);
        let said = self.world.jj(&jj, &dir, &["git", "colocation", "status"]);
        assert!(
            said.contains("is currently colocated with Git"),
            "{name}: {said}"
        );
        self.world.jj(&jj, &dir, &["status"]);
        let parent = self.world.jj(
            &jj,
            &dir,
            &["log", "--no-graph", "-r", "@-", "-T", "commit_id"],
        );
        assert_eq!(parent.trim(), run.head, "{name}: the jj CLI's @-");
        tc.event("the person runs jj in a run's workspace");
    }

    /// See the module docs: Git's worktrees, `HEAD`s, status and index
    /// against the model.
    fn check(&self) {
        let main = self.project.main_dir().to_owned();
        let mut expected = BTreeMap::from([(
            main.clone(),
            (self.main_head.clone(), vec!["detached".to_owned()]),
        )]);
        for (name, run) in &self.runs {
            expected.insert(
                self.dir(name),
                (run.head.clone(), vec!["detached".to_owned()]),
            );
        }
        assert_eq!(worktree_list(&main), expected, "git worktree list");
        let recorded = view_heads(&self.world, &main);
        assert_eq!(
            recorded.get(self.project.main_workspace()),
            Some(&self.main_head),
            "the main chat's HEAD in jj's view"
        );
        assert!(status_paths(&main).is_empty(), "git status in main/");
        for (name, run) in &self.runs {
            let dir = self.dir(name);
            assert_eq!(
                recorded.get(name),
                Some(&run.head),
                "{name}'s HEAD in jj's view"
            );
            assert_eq!(
                git(&dir, &["rev-parse", "--show-toplevel"]),
                dir.to_str().unwrap(),
                "{name}'s top level"
            );
            assert_eq!(
                status_paths(&dir),
                differ(&run.base, &run.disk),
                "git status in {name}"
            );
            let index: BTreeSet<String> =
                run.base.keys().chain(run.disk.keys()).cloned().collect();
            assert_eq!(listed(&dir), index, "git ls-files in {name}");
            // As jj-cli's `maybe_add_gitignore` writes it, for a Git that
            // does not leave `.jj` out on its own.
            assert_eq!(
                std::fs::read_to_string(dir.join(".jj").join(".gitignore"))
                    .ok()
                    .as_deref(),
                Some("/*\n"),
                "{name}'s .jj/.gitignore"
            );
        }
    }
}

#[hegel::state_machine]
impl Machine {
    #[rule(weight = 2)]
    fn add_run(&mut self, tc: TestCase) {
        self.do_add_run(&tc);
    }

    #[rule]
    fn fork(&mut self, tc: TestCase) {
        self.do_fork(&tc);
    }

    #[rule(weight = 4)]
    fn edit(&mut self, tc: TestCase) {
        self.do_edit(&tc);
    }

    #[rule]
    fn main_commit(&mut self, _tc: TestCase) {
        self.do_main_commit();
    }

    #[rule]
    fn catch_up(&mut self, tc: TestCase) {
        self.do_catch_up(&tc);
    }

    #[rule]
    fn forget(&mut self, tc: TestCase) {
        self.do_forget(&tc);
    }

    #[rule]
    fn prune(&mut self, _tc: TestCase) {
        self.do_prune();
    }

    #[rule]
    fn person_jj(&mut self, tc: TestCase) {
        self.do_person_jj(&tc);
    }

    #[invariant(always_run)]
    fn git_sees_the_model(&mut self, _tc: TestCase) {
        self.check();
    }
}

/// Git's view of the runs' workspaces against the model: see the module
/// docs.
#[hegel::test(
    test_cases = 30,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn runs_are_worktrees_git_and_jj_agree_on(tc: TestCase) {
    if jj_cli::matching_jj().is_none() {
        eprintln!("no jj of jj-lib's version: the person's jj steps skip");
    }
    hegel::stateful::machine(Machine::new()).steps(25).run(tc);
}

/// Where making a worktree stops: after each of its steps, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, hegel::PrettyPrintable)]
enum Cut {
    /// Nothing made, as in a workspace from before worktrees, or one
    /// whose worktree the person removed.
    Nothing,
    /// The worktree's folder, empty.
    Folder,
    /// Its `gitdir`.
    Gitdir,
    /// Its `commondir`.
    Commondir,
    /// Its `HEAD`, jj's unborn placeholder.
    Head,
    /// The workspace's `.git`: the worktree is whole, and the jj
    /// workspace not made yet.
    Gitlink,
    /// The workspace's `.git`, cut off empty.
    EmptyGitlink,
}

/// The files a cut at `cut` leaves of the worktree folder `admin`, for
/// the workspace at `dir`, as tau writes them, in its order.
fn write_cut(admin: &Path, dir: &Path, cut: Cut) {
    let step = |at: Cut| cut as u8 >= at as u8;
    if !step(Cut::Folder) {
        return;
    }
    std::fs::create_dir_all(admin).unwrap();
    if step(Cut::Gitdir) {
        let gitdir = format!("{}\n", dir.join(".git").display());
        std::fs::write(admin.join("gitdir"), gitdir).unwrap();
    }
    if step(Cut::Commondir) {
        std::fs::write(admin.join("commondir"), "../..\n").unwrap();
    }
    if step(Cut::Head) {
        std::fs::write(admin.join("HEAD"), "ref: refs/jj/root\n").unwrap();
    }
    if cut == Cut::EmptyGitlink {
        std::fs::write(dir.join(".git"), "").unwrap();
    } else if step(Cut::Gitlink) {
        let link = format!("gitdir: {}\n", admin.display());
        std::fs::write(dir.join(".git"), link).unwrap();
    }
}

/// Run `name`'s whole worktree, cut back to what a cut at `cut` leaves:
/// its folder emptied and written again up to that step.
fn cut_worktree(dir: &Path, cut: Cut) {
    let link = std::fs::read_to_string(dir.join(".git")).unwrap();
    let admin = PathBuf::from(link.strip_prefix("gitdir: ").unwrap().trim());
    std::fs::remove_dir_all(&admin).unwrap();
    std::fs::remove_file(dir.join(".git")).unwrap();
    write_cut(&admin, dir, cut);
}

/// Every folder of `main`'s Git store's worktrees.
fn admin_dirs(main: &Path) -> usize {
    std::fs::read_dir(main.join(".git").join("worktrees"))
        .map(Iterator::count)
        .unwrap_or(0)
}

/// After a repair: one worktree folder per run, Git and the jj CLI see
/// the model, `git fsck` is clean, and run `name`'s next tool works.
fn check_repaired(m: &mut Machine, tc: &TestCase, name: &str, cut: Cut) {
    let main = m.project.main_dir().to_owned();
    let dir = m.dir(name);
    assert_eq!(
        admin_dirs_naming(&main, &dir).len(),
        1,
        "{name}'s worktree folders after {cut:?}"
    );
    assert_eq!(admin_dirs(&main), m.runs.len(), "worktree folders, {cut:?}");
    m.check();
    assert_fsck_clean(&main, "after a repair");
    m.do_person_jj(tc);
    m.check();
    let run = m.runs.get_mut(name).unwrap();
    run.tools.ok("vcs_status", json!({}));
    m.check();
}

/// The worktree of a run's workspace, cut off at any step of its
/// making (or gone, as before worktrees), and pruned by the person
/// after or not, is whole again after the next open or the next use of
/// the workspace: see [`check_repaired`].
#[hegel::test(
    test_cases = 40,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn a_worktree_cut_off_is_made_whole(tc: TestCase) {
    let mut m = Machine::new();
    let runs = tc.draw(gs::integers::<usize>().min_value(1).max_value(3));
    for _ in 0..runs {
        m.do_add_run(&tc);
    }
    if m.runs.is_empty() {
        return;
    }
    for _ in 0..tc.draw(gs::integers::<usize>().max_value(2)) {
        m.do_edit(&tc);
    }
    let name = m.pick(&tc).unwrap();
    let dir = m.dir(&name);
    let cut = tc.draw(gs::sampled_from(vec![
        Cut::Nothing,
        Cut::Folder,
        Cut::Gitdir,
        Cut::Commondir,
        Cut::Head,
        Cut::EmptyGitlink,
    ]));
    cut_worktree(&dir, cut);
    if tc.draw(gs::booleans()) {
        m.do_prune();
        tc.event("the person prunes after the cut");
    }
    if tc.draw(gs::booleans()) {
        m.project = m.world.open();
    } else {
        let base = m.project.trunk().unwrap();
        let vcs = m.project.add_workspace(&name, &base).unwrap();
        m.runs.get_mut(&name).unwrap().tools = Tools::new(vcs);
        tc.event("the next use makes it whole");
    }
    check_repaired(&mut m, &tc, &name, cut);
}

/// Making a run's workspace cut off in its worktree, before the jj
/// workspace (at any step, then pruned by the person or not), is
/// finished by the next `add_workspace`: see [`check_repaired`].
#[hegel::test(
    test_cases = 30,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn a_run_cut_off_while_made_is_finished(tc: TestCase) {
    let mut m = Machine::new();
    m.do_add_run(&tc);
    let name = format!(
        "chat-{}",
        tc.draw(gs::from_regex("[0-9a-z]{6}").fullmatch(true))
    );
    let dir = m.dir(&name);
    std::fs::create_dir_all(&dir).unwrap();
    let cut = tc.draw(gs::sampled_from(vec![
        Cut::Folder,
        Cut::Gitdir,
        Cut::Commondir,
        Cut::Head,
        Cut::Gitlink,
        Cut::EmptyGitlink,
    ]));
    let main = m.project.main_dir().to_owned();
    write_cut(&main.join(".git").join("worktrees").join(&name), &dir, cut);
    if tc.draw(gs::booleans()) {
        m.do_prune();
        tc.event("the person prunes after the cut");
    }
    let trunk = m.project.trunk().unwrap();
    let vcs = m.project.add_workspace(&name, &trunk).unwrap();
    m.runs.insert(
        name.clone(),
        Run {
            tools: Tools::new(vcs),
            head: trunk,
            base: trunk_files(),
            disk: trunk_files(),
            on_main: false,
            stale: false,
        },
    );
    check_repaired(&mut m, &tc, &name, cut);
}

/// A run's workspace made before runs had worktrees, as jj makes a
/// workspace that is not one (`jj workspace add --no-colocate`), with
/// work left in it, gains its worktree when the project opens: `HEAD`
/// at its `@-`, the work in `git status`, and the jj CLI calls it
/// colocated.
#[test]
fn a_run_from_before_worktrees_gains_one() {
    let world = World::new();
    let Some(jj) = world.jj.clone() else {
        eprintln!("no jj of jj-lib's version: not compared");
        return;
    };
    let (project, _main) = world.import();
    let dir = project.workspace_dir("old");
    std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
    let trunk = project.trunk().unwrap();
    world.jj(
        &jj,
        project.main_dir(),
        &[
            "workspace",
            "add",
            "--no-colocate",
            "--name",
            "old",
            "-r",
            &trunk,
            dir.to_str().unwrap(),
        ],
    );
    assert!(!dir.join(".git").exists());
    std::fs::write(dir.join("b.txt"), "two!\n").unwrap();
    std::fs::write(dir.join("a.txt"), "two!\n").unwrap();
    world.jj(&jj, &dir, &["status"]);
    let project = world.open();
    assert_eq!(project.workspaces().unwrap(), vec!["old".to_owned()]);
    let main = project.main_dir().to_owned();
    let list = worktree_list(&main);
    assert_eq!(list[&dir], (trunk.clone(), vec!["detached".to_owned()]));
    assert_eq!(view_heads(&world, &main).get("old"), Some(&trunk));
    assert_eq!(
        status_paths(&dir),
        BTreeSet::from(["a.txt".to_owned(), "b.txt".to_owned()])
    );
    assert_eq!(
        listed(&dir),
        BTreeSet::from(["a.txt".to_owned(), "b.txt".to_owned()])
    );
    let said = world.jj(&jj, &dir, &["git", "colocation", "status"]);
    assert!(said.contains("is currently colocated with Git"), "{said}");
    world.jj(&jj, &dir, &["status"]);
    let parent = world.jj(
        &jj,
        &dir,
        &["log", "--no-graph", "-r", "@-", "-T", "commit_id"],
    );
    assert_eq!(parent.trim(), trunk);
}
