//! Runs' heads as the person sees them (`docs/reference/vcs.md`, "Run
//! heads"; ADR 0043): Git refs under `refs/tau/runs/`, never branches,
//! never bookmarks.
//!
//! - Against a model and the CLIs: runs in a project commit, end turns,
//!   commit and undo with the model's tools, keep their work as a
//!   sub-agent's is kept, are dropped or lose their heads, and tau
//!   starts again, or closes between an operation and its ref, while
//!   the person commits in a run's workspace and runs `jj git import`. After every step each run's head is
//!   the model's newest commit; `git for-each-ref refs/tau` has one ref
//!   per run with a head, at that commit; `git branch -a`, `git push
//!   --all` and `jj bookmark list --all-remotes` show none of them; and
//!   `jj log -r 'all()'` shows each open run's head.
//! - Metamorphic, for projects from before: runs' heads kept as
//!   `tau/<run>` bookmarks, made by tau's own code path or by the jj CLI,
//!   are the same heads once the project opens, and no bookmark is left;
//!   an open cut off between its steps is finished by the next.
//!
//! The jj CLI is the one of jj-lib's version (`TAU_TEST_JJ`); where it
//! is not installed, the checks that need it say they did not run.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

mod common;
#[path = "common/jj_cli.rs"]
mod jj_cli;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_agent::{
    plugin::Plugin as _,
    tool::{AgentTool, RunId, ToolCtx},
};
use tau_testing::{block_on_io as block_on, git::git};
use tau_vcs_host::{Head, Identity, ProjectRepo, UpdateFrom, Vcs, VcsPlugin};

const RUNS: usize = 3;

fn run_head(run: &str) -> Head {
    Head::Run(RunId(run.into()))
}

fn run_name(run: usize) -> String {
    format!("r{run}")
}

/// The jj CLI in `main`, with a home and config of its own and no
/// other variable of this process. Fails the test unless it succeeds.
fn jj(jj: &Path, home: &Path, main: &Path, args: &[&str]) -> String {
    std::fs::create_dir_all(home.join("config")).unwrap();
    let output = Command::new(jj)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("JJ_USER", "Person")
        .env("JJ_EMAIL", "person@localhost")
        .current_dir(main)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "jj {args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap()
}

/// A run's workspace's vcs, and its tools.
struct Tools {
    vcs: Vcs,
    tools: Vec<Arc<dyn AgentTool>>,
}

impl Tools {
    fn new(vcs: Vcs) -> Self {
        let tools = VcsPlugin::new(vcs.clone()).tools();
        Self { vcs, tools }
    }

    fn call(&self, name: &str, args: Value) -> Value {
        let tool = self.tools.iter().find(|tool| tool.name() == name).unwrap();
        block_on(tool.call(args, ToolCtx::detached()))
            .unwrap_or_else(|error| panic!("{name}: {error}"))
            .details
            .unwrap_or(Value::Null)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Open,
    /// Its workspace closed, its work kept: a kept sub-agent's.
    Kept,
    Dropped,
}

#[derive(Debug, Clone, hegel::PrettyPrintable)]
enum Step {
    /// The run's file written, and committed as a run's last turn is
    /// (`commit_all`).
    Commit(usize),
    /// The run's file written, and its turn ended with it uncommitted.
    EndTurn(usize),
    /// The run's file written, and committed with `vcs_commit`.
    ToolCommit(usize),
    /// The same, then undone with `vcs_undo`.
    CommitAndUndo(usize),
    /// The run's workspace closes and its work is kept; with `true`,
    /// right after tau closed between an operation and its ref write,
    /// so its ref is behind.
    Keep(usize, bool),
    /// The run's own commits abandoned, its workspace and head gone.
    Drop(usize),
    /// The run's head removed while its workspace lives: its next turn's
    /// end points it again.
    RemoveHead(usize),
    /// tau closed between an operation and the ref write after it: the
    /// run's ref is behind, at trunk.
    CutOff(usize),
    /// The person commits in the run's workspace with the jj CLI.
    PersonCommits(usize),
    /// tau closes and opens the project again.
    Restart,
    /// The person runs `jj git import`.
    Import,
}

#[hegel::composite]
fn step(tc: &TestCase) -> Step {
    let run = || tc.draw(gs::integers::<usize>().max_value(RUNS - 1));
    match tc.draw(gs::integers::<u8>().max_value(12)) {
        0 | 1 => Step::Commit(run()),
        2 => Step::EndTurn(run()),
        3 | 4 => Step::ToolCommit(run()),
        5 => Step::CommitAndUndo(run()),
        6 => Step::Keep(run(), tc.draw(gs::booleans())),
        7 => Step::Drop(run()),
        8 => Step::RemoveHead(run()),
        9 => Step::CutOff(run()),
        10 => Step::PersonCommits(run()),
        11 => Step::Restart,
        _ => Step::Import,
    }
}

struct Run {
    state: State,
    tools: Option<Tools>,
    /// The model's newest commit of the run, once a turn's end, a final
    /// commit or a landing claimed its head: none before.
    head: Option<String>,
    /// The run's newest commit, its `@`'s parent, as its tools last
    /// showed it: what its head becomes once claimed.
    newest: String,
    /// Its ref is behind its head until tau's next operation, since a
    /// cut-off or the person's commit.
    behind: bool,
}

struct World {
    home: tempfile::TempDir,
    project: ProjectRepo,
    trunk: String,
    runs: Vec<Run>,
    /// Writes so far, so every write changes a file.
    writes: usize,
    jj: Option<PathBuf>,
    /// A bare repository a `git push --dry-run` goes to.
    bare: PathBuf,
}

impl World {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let project = common::project(home.path());
        let trunk = project.trunk().unwrap();
        let runs = (0..RUNS)
            .map(|run| {
                let vcs =
                    project.add_workspace(&run_name(run), &trunk).unwrap();
                Run {
                    state: State::Open,
                    tools: Some(Tools::new(vcs)),
                    head: None,
                    newest: trunk.clone(),
                    behind: false,
                }
            })
            .collect();
        // The main chat on trunk, as the host has it before its first
        // turn, so `main/` has a branch checked out for `git branch`.
        let main = block_on(Vcs::open(project.main_dir(), Identity::default()))
            .unwrap();
        block_on(main.move_onto(
            trunk.clone(),
            project.trunk_name().unwrap(),
            true,
        ))
        .unwrap();
        let bare = home.path().join("bare.git");
        git(home.path(), &["init", "--quiet", "--bare", "bare.git"]);
        Self {
            home,
            project,
            trunk,
            runs,
            writes: 0,
            jj: jj_cli::matching_jj(),
            bare,
        }
    }

    fn main(&self) -> PathBuf {
        self.project.main_dir().to_owned()
    }

    fn write(&mut self, run: usize) {
        self.writes += 1;
        let dir = self.project.workspace_dir(&run_name(run));
        std::fs::write(dir.join("run.txt"), format!("{}\n", self.writes))
            .unwrap();
    }

    /// tau wrote an operation, and the runs' refs after it.
    fn settled(&mut self) {
        for r in &mut self.runs {
            r.behind = false;
        }
    }

    fn step(&mut self, tc: &TestCase, step: &Step) {
        let open =
            |world: &Self, run: usize| world.runs[run].state == State::Open;
        match *step {
            Step::Commit(run) if open(self, run) => {
                self.write(run);
                let vcs = &self.runs[run].tools.as_ref().unwrap().vcs;
                let committed = block_on(
                    vcs.commit_all("feat: run", run_head(&run_name(run))),
                )
                .unwrap();
                self.settled();
                let r = &mut self.runs[run];
                r.newest = committed.commit_id.clone();
                r.head = Some(committed.commit_id);
            }
            Step::EndTurn(run) if open(self, run) => {
                self.write(run);
                let vcs = &self.runs[run].tools.as_ref().unwrap().vcs;
                let turn =
                    block_on(vcs.end_turn(run_head(&run_name(run)), None))
                        .unwrap();
                self.settled();
                let r = &mut self.runs[run];
                assert_eq!(turn.head, r.newest, "a turn's end moved nothing");
                r.head = Some(turn.head);
            }
            Step::ToolCommit(run) if open(self, run) => {
                self.write(run);
                let tools = self.runs[run].tools.as_ref().unwrap();
                let details = tools
                    .call("vcs_commit", json!({ "message": "feat: tool" }));
                let id = details["committed"]["commit_id"].as_str().unwrap();
                let id = id.to_owned();
                self.settled();
                let r = &mut self.runs[run];
                r.newest = id.clone();
                // Once claimed, the head is `@`'s parent as it moves.
                if r.head.is_some() {
                    tc.event("a model's commit moves a claimed head");
                    r.head = Some(id.to_owned());
                }
            }
            Step::CommitAndUndo(run) if open(self, run) => {
                self.write(run);
                let tools = self.runs[run].tools.as_ref().unwrap();
                tools.call("vcs_commit", json!({ "message": "feat: undone" }));
                tools.call("vcs_undo", json!({}));
                self.settled();
                if self.runs[run].head.is_some() {
                    tc.event("an undo moves a claimed head back");
                }
            }
            Step::Keep(run, behind) if open(self, run) && self.clean(run) => {
                tc.event("a run's work kept");
                if behind && self.runs[run].head.is_some() {
                    tc.event("a run's work kept with its ref behind");
                    let name = run_head(&run_name(run)).to_string();
                    git(&self.main(), &["update-ref", &name, &self.trunk]);
                }
                self.project.forget_workspace(&run_name(run)).unwrap();
                self.settled();
                let r = &mut self.runs[run];
                r.tools = None;
                r.state = State::Kept;
            }
            Step::Drop(run) if self.runs[run].state != State::Dropped => {
                tc.event("a run dropped");
                let name = run_name(run);
                if let Some(head) = self.project.head(&run_head(&name)).unwrap()
                {
                    self.project.abandon_between(&self.trunk, &head).unwrap();
                }
                if let Some(wc) = self.project.workspace_head(&name).unwrap() {
                    self.project.abandon_between(&self.trunk, &wc).unwrap();
                }
                self.project.forget_workspace(&name).unwrap();
                self.project.remove_head(&run_head(&name)).unwrap();
                // Forgetting an open run's workspace is an operation;
                // a kept one's drop may abandon nothing.
                if self.runs[run].state == State::Open {
                    self.settled();
                }
                let r = &mut self.runs[run];
                r.tools = None;
                r.head = None;
                r.behind = false;
                r.state = State::Dropped;
            }
            Step::RemoveHead(run) if open(self, run) => {
                tc.event("a live run's head removed");
                self.project.remove_head(&run_head(&run_name(run))).unwrap();
                let r = &mut self.runs[run];
                r.head = None;
                r.behind = false;
            }
            Step::CutOff(run)
                if open(self, run) && self.runs[run].head.is_some() =>
            {
                tc.event("a ref left behind");
                let name = run_head(&run_name(run)).to_string();
                git(&self.main(), &["update-ref", &name, &self.trunk]);
                self.runs[run].behind = true;
            }
            Step::PersonCommits(run)
                if open(self, run) && self.jj.is_some() =>
            {
                tc.event("the person commits in a run's workspace");
                self.write(run);
                let dir = self.project.workspace_dir(&run_name(run));
                let person = self.home.path().join("person");
                let cli = self.jj.clone().unwrap();
                jj(&cli, &person, &dir, &["commit", "-m", "person's"]);
                let id = jj(
                    &cli,
                    &person,
                    &dir,
                    &[
                        "log",
                        "-r",
                        "@-",
                        "--no-graph",
                        "--ignore-working-copy",
                        "-T",
                        "commit_id",
                    ],
                );
                let r = &mut self.runs[run];
                r.newest = id.trim().to_owned();
                if r.head.is_some() {
                    r.head = Some(r.newest.clone());
                    r.behind = true;
                }
            }
            Step::Restart => {
                tc.event("a restart");
                let root = self.project.root().to_owned();
                self.project =
                    ProjectRepo::open(&root, Identity::default()).unwrap();
                for (run, r) in self.runs.iter_mut().enumerate() {
                    if r.state == State::Open {
                        let dir = self.project.workspace_dir(&run_name(run));
                        let vcs =
                            block_on(Vcs::open(&dir, Identity::default()))
                                .unwrap();
                        r.tools = Some(Tools::new(vcs));
                    }
                }
            }
            Step::Import => {
                let Some(cli) = self.jj.clone() else {
                    return;
                };
                tc.event("the person runs jj git import");
                jj(
                    &cli,
                    &self.home.path().join("person"),
                    &self.main(),
                    &["git", "import"],
                );
            }
            _ => {}
        }
    }

    /// Whether `run`'s `@` holds nothing: a kept sub-agent committed its
    /// work.
    fn clean(&self, run: usize) -> bool {
        let wc = self
            .project
            .workspace_head(&run_name(run))
            .unwrap()
            .unwrap();
        self.project
            .diff(&self.runs[run].newest, &wc)
            .unwrap()
            .is_empty()
    }

    /// What the person sees, against the model.
    fn check(&self) {
        let mut want = BTreeMap::new();
        for (run, r) in self.runs.iter().enumerate() {
            let name = run_name(run);
            let head = self.project.head(&run_head(&name)).unwrap();
            assert_eq!(head, r.head, "{name}'s head ({:?})", r.state);
            if r.state == State::Open {
                let wc = self.project.workspace_head(&name).unwrap().unwrap();
                assert_eq!(
                    self.project.parent_of(&wc).unwrap().as_ref(),
                    Some(&r.newest),
                    "{name}'s newest commit"
                );
            }
            if let Some(head) = &r.head {
                want.insert(run_head(&name).to_string(), head.clone());
            }
        }
        // A ref left behind is the workspace's again after tau's next
        // operation; until then the head is read from the workspace.
        let mut refs = common::git_refs(&self.project, "refs/tau/");
        for (run, r) in self.runs.iter().enumerate() {
            if r.behind {
                let name = run_head(&run_name(run)).to_string();
                refs.insert(name.clone(), want[&name].clone());
            }
        }
        assert_eq!(refs, want);
        assert_eq!(
            self.project.run_heads().unwrap(),
            want.keys().cloned().collect::<Vec<_>>()
        );
        common::assert_no_run_branches(&self.project);
        let branches = git(&self.main(), &["branch", "-a"]);
        assert!(!branches.contains("tau/"), "git branch -a: {branches}");
        // `git push --all` sends branches, and no run's head.
        let pushed = git(
            &self.main(),
            &[
                "push",
                "--all",
                "--dry-run",
                "--porcelain",
                self.bare.to_str().unwrap(),
            ],
        );
        let refs: Vec<&str> = pushed
            .lines()
            .filter(|line| line.contains("refs/"))
            .collect();
        assert!(!refs.is_empty(), "{pushed}");
        assert!(
            refs.iter().all(|line| !line.contains("tau/")),
            "git push --all sends a run's head: {pushed}"
        );
        let Some(cli) = &self.jj else {
            return;
        };
        let person = self.home.path().join("person");
        let bookmarks = jj(
            cli,
            &person,
            &self.main(),
            &[
                "bookmark",
                "list",
                "--all-remotes",
                "--ignore-working-copy",
                "-T",
                r#"name ++ "@" ++ remote ++ "\n""#,
            ],
        );
        assert!(
            !bookmarks.lines().any(|line| line.starts_with("tau/")),
            "jj bookmark list: {bookmarks}"
        );
        let visible = jj(
            cli,
            &person,
            &self.main(),
            &[
                "log",
                "-r",
                "all()",
                "--no-graph",
                "--ignore-working-copy",
                "-T",
                r#"commit_id ++ "\n""#,
            ],
        );
        for r in &self.runs {
            if r.state == State::Open
                && let Some(head) = &r.head
            {
                assert!(
                    visible.lines().any(|id| id == head),
                    "{head} is hidden"
                );
            }
        }
    }
}

/// Runs' heads against the model and as Git and the jj CLI show them:
/// see the module docs.
#[hegel::test(
    test_cases = 25,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn run_heads_are_refs_no_branch_or_bookmark_shows(tc: TestCase) {
    let steps: Vec<Step> = tc.draw(gs::vecs(step()).min_size(1).max_size(12));
    let mut world = World::new();
    if world.jj.is_none() {
        eprintln!("no jj CLI of jj-lib's version: compared with git only");
    }
    world.check();
    for step in &steps {
        world.step(&tc, step);
        world.check();
    }
}

/// How an old project's `tau/<run>` bookmarks were made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, hegel::PrettyPrintable)]
enum Made {
    /// By tau's own code path: a run's commit moving the bookmark.
    ByTau,
    /// By the jj CLI: `jj bookmark set`, at the run's newest commit.
    ByJj,
}

/// Where an earlier open was cut off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, hegel::PrettyPrintable)]
enum Cut {
    No,
    /// After writing the refs, at stale commits here: the bookmarks are
    /// still there.
    AfterRefs,
    /// After the export took Git's `refs/heads/tau/*` away, before the
    /// operation that removes the bookmarks.
    AfterExport,
}

/// A project from before runs' heads were refs opens with the same
/// heads, as refs, and no `tau/` bookmark, in one operation; an open cut
/// off between its steps is finished by the next; and a pull request's
/// branch, a `tau/` bookmark with a remote, stays a bookmark.
#[hegel::test(
    test_cases = 25,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn old_run_bookmarks_become_the_same_heads(tc: TestCase) {
    let cli = jj_cli::matching_jj();
    let made = if cli.is_some() {
        tc.draw(gs::sampled_from(vec![Made::ByTau, Made::ByJj]))
    } else {
        Made::ByTau
    };
    let cut = tc.draw(gs::sampled_from(vec![
        Cut::No,
        Cut::AfterRefs,
        Cut::AfterExport,
    ]));
    let runs = tc.draw(gs::integers::<usize>().min_value(1).max_value(RUNS));
    let kept: Vec<bool> = (0..runs).map(|_| tc.draw(gs::booleans())).collect();
    let pull_request = tc.draw(gs::booleans());

    let home = tempfile::tempdir().unwrap();
    let project = common::project(home.path());
    let trunk = project.trunk().unwrap();
    let main = project.main_dir().to_owned();
    let person = home.path().join("person");
    if pull_request {
        // A pull request's branch, fetched back: `tau/shared@origin`.
        git(&home.path().join("src"), &["branch", "tau/shared"]);
        project
            .update(UpdateFrom::Checkout(&home.path().join("src")))
            .unwrap();
        assert!(project.bookmark("tau/shared").unwrap().is_some());
    }
    let mut old = BTreeMap::new();
    for (run, kept) in kept.iter().enumerate() {
        let name = run_name(run);
        let vcs = project.add_workspace(&name, &trunk).unwrap();
        std::fs::write(project.workspace_dir(&name).join("run.txt"), &name)
            .unwrap();
        let bookmark = format!("tau/{name}");
        match made {
            Made::ByTau => {
                block_on(vcs.commit_all("feat: run", bookmark.as_str()))
                    .unwrap();
            }
            Made::ByJj => {
                let tools = Tools::new(vcs);
                let details =
                    tools.call("vcs_commit", json!({ "message": "feat: run" }));
                let id = details["committed"]["commit_id"].as_str().unwrap();
                jj(
                    cli.as_ref().unwrap(),
                    &person,
                    &main,
                    &[
                        "bookmark",
                        "set",
                        &bookmark,
                        "-r",
                        id,
                        "--ignore-working-copy",
                    ],
                );
            }
        }
        let target = project.bookmark(&bookmark).unwrap().unwrap();
        assert_eq!(
            git(&main, &["rev-parse", &format!("refs/heads/{bookmark}")]),
            target,
            "jj exported {bookmark}"
        );
        if *kept {
            project.forget_workspace(&name).unwrap();
        }
        old.insert(name, target);
    }
    match cut {
        Cut::No => {}
        Cut::AfterRefs => {
            for name in old.keys() {
                git(
                    &main,
                    &["update-ref", &format!("refs/tau/runs/{name}"), &trunk],
                );
            }
        }
        Cut::AfterExport => {
            for name in old.keys() {
                git(
                    &main,
                    &["update-ref", "-d", &format!("refs/heads/tau/{name}")],
                );
            }
        }
    }
    let repo_path = main.join(".jj").join("repo");
    let mut log = common::ops::OpLog::open(&repo_path);
    log.new_ops();

    let project =
        ProjectRepo::open(project.root(), Identity::default()).unwrap();
    let ops = log.new_ops();
    let descriptions: Vec<&str> = ops
        .iter()
        .map(|op| op.metadata().description.as_str())
        .collect();
    assert_eq!(descriptions, ["tau: move run heads to refs/tau"]);
    let mut want = BTreeMap::new();
    for (name, target) in &old {
        assert_eq!(
            project.head(&run_head(name)).unwrap().as_ref(),
            Some(target),
            "{name}'s head"
        );
        want.insert(run_head(name).to_string(), target.clone());
    }
    assert_eq!(common::git_refs(&project, "refs/tau/"), want);
    let branches: Vec<String> = common::git_refs(&project, "refs/heads/")
        .into_keys()
        .collect();
    let mut kept_branches = vec!["refs/heads/main".to_owned()];
    if pull_request {
        kept_branches.push("refs/heads/tau/shared".to_owned());
        assert!(project.bookmark("tau/shared").unwrap().is_some());
    }
    assert_eq!(branches, kept_branches, "Git's branches");
    let expected_bookmarks: Vec<String> = if pull_request {
        vec!["tau/shared".to_owned()]
    } else {
        Vec::new()
    };
    assert_eq!(project.bookmarks("tau/").unwrap(), expected_bookmarks);
    if let Some(cli) = &cli {
        let listed = jj(
            cli,
            &person,
            &main,
            &[
                "bookmark",
                "list",
                "--all-remotes",
                "--ignore-working-copy",
                "-T",
                r#"name ++ "@" ++ remote ++ "\n""#,
            ],
        );
        for name in old.keys() {
            assert!(
                !listed
                    .lines()
                    .any(|line| line.starts_with(&format!("tau/{name}@"))),
                "jj bookmark list: {listed}"
            );
        }
    }

    // Done once: opening again changes nothing.
    let project =
        ProjectRepo::open(project.root(), Identity::default()).unwrap();
    assert!(log.new_ops().is_empty(), "a second open moved heads again");
    // The runs go on, their heads moving and no bookmark coming back.
    for (run, kept) in kept.iter().enumerate() {
        let name = run_name(run);
        if *kept {
            continue;
        }
        let vcs = block_on(Vcs::open(
            project.workspace_dir(&name),
            Identity::default(),
        ))
        .unwrap();
        std::fs::write(project.workspace_dir(&name).join("run.txt"), "more\n")
            .unwrap();
        let committed =
            block_on(vcs.commit_all("feat: more", run_head(&name))).unwrap();
        assert_eq!(
            project.parent_of(&committed.commit_id).unwrap().as_ref(),
            Some(&old[&name])
        );
        assert_eq!(
            project.head(&run_head(&name)).unwrap(),
            Some(committed.commit_id.clone())
        );
        assert_eq!(
            common::git_refs(&project, "refs/tau/")
                [&run_head(&name).to_string()],
            committed.commit_id
        );
    }
    assert_eq!(project.bookmarks("tau/").unwrap(), expected_bookmarks);
}

/// A ref whose commit is gone names no head: the run's work was
/// dropped, and reading it brings nothing back.
#[test]
fn a_ref_to_an_abandoned_commit_names_no_head() {
    let home = tempfile::tempdir().unwrap();
    let project = common::project(home.path());
    let trunk = project.trunk().unwrap();
    let vcs = project.add_workspace("r0", &trunk).unwrap();
    std::fs::write(project.workspace_dir("r0").join("run.txt"), "x\n").unwrap();
    let committed =
        block_on(vcs.commit_all("feat: run", run_head("r0"))).unwrap();
    project.forget_workspace("r0").unwrap();
    assert_eq!(
        project.head(&run_head("r0")).unwrap(),
        Some(committed.commit_id.clone())
    );
    project
        .abandon_between(&trunk, &committed.commit_id)
        .unwrap();
    assert_eq!(project.head(&run_head("r0")).unwrap(), None);
    // The ref stays where it was, for whoever reports the drop.
    assert_eq!(
        common::git_refs(&project, "refs/tau/")[&run_head("r0").to_string()],
        committed.commit_id
    );
    assert_eq!(project.run_heads().unwrap(), [run_head("r0").to_string()]);
}

/// A ref left behind, as tau closing between an operation and its ref
/// write leaves it, does not name the head: while the run's workspace
/// lives its `@-` does, and forgetting the workspace, as a kept
/// sub-agent's is, writes the ref there first.
#[test]
fn a_ref_left_behind_is_written_before_the_workspace_goes() {
    let home = tempfile::tempdir().unwrap();
    let project = common::project(home.path());
    let trunk = project.trunk().unwrap();
    let vcs = project.add_workspace("r0", &trunk).unwrap();
    std::fs::write(project.workspace_dir("r0").join("run.txt"), "x\n").unwrap();
    let committed =
        block_on(vcs.commit_all("feat: run", run_head("r0"))).unwrap();
    let main = project.main_dir().to_owned();
    let name = run_head("r0").to_string();
    git(&main, &["update-ref", &name, &trunk]);
    assert_eq!(
        project.head(&run_head("r0")).unwrap(),
        Some(committed.commit_id.clone())
    );
    project.forget_workspace("r0").unwrap();
    assert_eq!(
        project.head(&run_head("r0")).unwrap(),
        Some(committed.commit_id.clone())
    );
    assert_eq!(
        common::git_refs(&project, "refs/tau/")[&name],
        committed.commit_id
    );
}
