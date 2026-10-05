//! Importing and updating a project against its source
//! (`docs/reference/vcs.md`, "Projects"): a Git repository is built by
//! drawn `git` commands (commits, branches made, moved, deleted and
//! checked out, a detached `HEAD`, packed refs and objects), imported,
//! changed again and updated from. The oracle is `git` itself: the
//! source's branches and `HEAD` as `git` reports them, with trunk on
//! the branch `HEAD` named at the import.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::collections::BTreeMap;

use hegel::{Generator as _, TestCase, generators as gs};
use tau_testing::{block_on, git::git};
use tau_vcs::{Identity, ProjectRepo, UpdateFrom};

const BRANCHES: [&str; 5] = ["main", "master", "trunk", "feature", "feat/x"];
/// jj's root commit, in a Git-backed repository.
const ROOT: &str = "0000000000000000000000000000000000000000";

/// Something done to the source.
#[derive(Debug, Clone, hegel::PrettyPrintable)]
enum Op {
    /// A commit on whatever `HEAD` is, writing `f.txt`.
    Commit(String),
    /// A branch at `HEAD`.
    Branch(String),
    Checkout(String),
    Detach,
    Delete(String),
    /// Moves a branch to another's commit.
    Move(String, String),
    PackRefs,
    Gc,
}

#[hegel::composite]
fn op(tc: &TestCase) -> Op {
    let branch = || gs::sampled_from(BRANCHES.to_vec()).map(String::from);
    match tc.draw(gs::integers::<u8>().max_value(9)) {
        0..=3 => Op::Commit(tc.draw(
            gs::sampled_from(vec!["one", "two", "three"]).map(String::from),
        )),
        4 => Op::Branch(tc.draw(branch())),
        5 => Op::Checkout(tc.draw(branch())),
        6 => Op::Detach,
        7 => Op::Delete(tc.draw(branch())),
        8 => Op::Move(tc.draw(branch()), tc.draw(branch())),
        _ => {
            if tc.draw(gs::booleans()) {
                Op::PackRefs
            } else {
                Op::Gc
            }
        }
    }
}

/// The source's state as `git` reports it.
struct Source {
    dir: std::path::PathBuf,
    commits: usize,
}

impl Source {
    fn branches(&self) -> BTreeMap<String, String> {
        git(
            &self.dir,
            &[
                "for-each-ref",
                "--format=%(refname:short) %(objectname)",
                "refs/heads/",
            ],
        )
        .lines()
        .map(|line| {
            let (name, id) = line.split_once(' ').unwrap();
            (name.to_owned(), id.to_owned())
        })
        .collect()
    }

    /// The branch `HEAD` names, if it names one (it may not exist yet).
    fn head_branch(&self) -> Option<String> {
        let text = std::fs::read_to_string(self.dir.join(".git/HEAD")).unwrap();
        text.trim()
            .strip_prefix("ref: refs/heads/")
            .map(str::to_owned)
    }

    fn has_head(&self) -> bool {
        tau_testing::git::output(
            &self.dir,
            &["rev-parse", "--verify", "--quiet", "HEAD"],
        )
        .status
        .success()
    }

    /// Does `op`, when it makes sense in the source as it is.
    fn apply(&mut self, tc: &TestCase, op: &Op) {
        let branches = self.branches();
        let current = self.head_branch();
        let dir = self.dir.clone();
        match op {
            Op::Commit(text) => {
                self.commits += 1;
                std::fs::write(
                    dir.join("f.txt"),
                    format!("{text} {}\n", self.commits),
                )
                .unwrap();
                git(&dir, &["add", "-A"]);
                git(&dir, &["commit", "--quiet", "-m", text]);
                if current.is_none() {
                    tc.event("a commit on a detached HEAD");
                }
            }
            Op::Branch(name)
                if !branches.contains_key(name) && self.has_head() =>
            {
                git(&dir, &["branch", name]);
            }
            Op::Checkout(name) if branches.contains_key(name) => {
                git(&dir, &["checkout", "--quiet", name]);
            }
            Op::Detach if self.has_head() => {
                git(&dir, &["checkout", "--quiet", "--detach"]);
            }
            Op::Delete(name)
                if branches.contains_key(name)
                    && current.as_ref() != Some(name) =>
            {
                git(&dir, &["branch", "--quiet", "-D", name]);
            }
            Op::Move(name, to)
                if branches.contains_key(name)
                    && branches.contains_key(to)
                    && current.as_ref() != Some(name) =>
            {
                git(&dir, &["branch", "--quiet", "-f", name, to]);
            }
            Op::PackRefs => {
                git(&dir, &["pack-refs", "--all"]);
            }
            Op::Gc => {
                git(&dir, &["gc", "--quiet"]);
            }
            _ => {}
        }
    }

    /// The trunk `vcs.md` promises: the branch `head` names (the
    /// source's `HEAD` at the import), else `main`, `master` or `trunk`,
    /// else the root commit.
    fn trunk(&self, head: Option<String>) -> String {
        let branches = self.branches();
        head.into_iter()
            .chain(["main", "master", "trunk"].map(String::from))
            .find_map(|name| branches.get(&name).cloned())
            .unwrap_or_else(|| ROOT.to_owned())
    }
}

/// The project holds the source's branches as bookmarks, on the same
/// commits, and its trunk is the one the reference promises, on the
/// branch `head` names.
fn check(project: &ProjectRepo, source: &Source, head: Option<String>) {
    let branches = source.branches();
    let bookmarks: BTreeMap<String, String> = project
        .bookmarks("")
        .unwrap()
        .into_iter()
        .filter(|name| !name.starts_with("tau/"))
        .map(|name| {
            let id = project.bookmark(&name).unwrap().expect("a bookmark");
            (name, id)
        })
        .collect();
    assert_eq!(
        bookmarks, branches,
        "bookmarks against the source's branches"
    );
    assert_eq!(
        project.trunk().unwrap(),
        source.trunk(head.clone()),
        "trunk"
    );
    assert_eq!(project.default_branch(), head);
    // Every branch's files are readable at its commit.
    for id in branches.values() {
        let want = git(&source.dir, &["show", &format!("{id}:f.txt")]);
        let (got, _) = project.file_at(id, "f.txt").unwrap().unwrap();
        assert_eq!(String::from_utf8(got).unwrap().trim(), want);
    }
}

/// Every branch of the source becomes a bookmark on the same commit, the
/// trunk follows the rule in the reference, and an update brings in what
/// changed at the source (new commits, moved and deleted branches, a
/// new `HEAD`) while a run's workspace, commit and bookmark stay as they
/// were.
#[hegel::test(
    test_cases = 30,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn a_project_mirrors_its_source(tc: TestCase) {
    let initial = tc.draw(gs::sampled_from(BRANCHES.to_vec()));
    let before: Vec<Op> = tc.draw(gs::vecs(op()).max_size(8));
    let after: Vec<Op> = tc.draw(gs::vecs(op()).max_size(8));

    let home = tempfile::tempdir().unwrap();
    let mut source = Source {
        dir: home.path().join("src"),
        commits: 0,
    };
    std::fs::create_dir_all(&source.dir).unwrap();
    git(&source.dir, &["init", "--quiet", "-b", initial]);
    for op in &before {
        source.apply(&tc, op);
    }
    if source.branches().is_empty() {
        tc.event("an import with no branches");
    }
    if source.head_branch().is_none() {
        tc.event("an import with a detached HEAD");
    } else if source.trunk(source.head_branch()) != ROOT
        && !source
            .branches()
            .contains_key(&source.head_branch().unwrap())
    {
        tc.event("HEAD names a branch that is gone");
    }

    let project = tau_vcs::ProjectRepo::open_or_import(
        source.dir.to_str().unwrap(),
        home.path().join("p"),
        Identity::default(),
    )
    .unwrap();
    // The branch trunk follows: the one `HEAD` named at the import. An
    // update leaves it, whatever the checkout has checked out since.
    let head = source.head_branch();
    check(&project, &source, head.clone());

    // A run works meanwhile, on the trunk as it was.
    let trunk = project.trunk().unwrap();
    let vcs = project.add_workspace("run", &trunk).unwrap();
    let run_dir = project.workspace_dir("run");
    std::fs::write(run_dir.join("run.txt"), "run\n").unwrap();
    let turn =
        block_on(vcs.commit_all("tau: run 1 turn 1", "tau/run")).unwrap();
    let wc = project.workspace_head("run").unwrap();

    for op in &after {
        source.apply(&tc, op);
    }
    let updated = project.update(UpdateFrom::Checkout(&source.dir)).unwrap();
    assert_eq!(updated.before, trunk);
    let want = source.trunk(head.clone());
    assert_eq!(updated.after, want);
    assert_eq!(updated.changed(), trunk != want);
    if updated.changed() {
        tc.event("an update moves the trunk");
    }
    if source.head_branch() != head {
        tc.event("an update from a checkout on another branch");
    }
    check(&project, &source, head.clone());

    // The run is as it was.
    assert_eq!(
        project.bookmark("tau/run").unwrap(),
        Some(turn.commit_id.clone())
    );
    assert_eq!(project.workspace_head("run").unwrap(), wc);
    assert_eq!(project.workspaces().unwrap(), ["run"]);
    assert_eq!(
        project.parent_of(&turn.commit_id).unwrap().as_deref(),
        (trunk != ROOT).then_some(trunk.as_str())
    );
    assert_eq!(
        project.file_at(&turn.commit_id, "run.txt").unwrap(),
        Some((b"run\n".to_vec(), false))
    );
    assert_eq!(
        std::fs::read_to_string(run_dir.join("run.txt")).unwrap(),
        "run\n"
    );
    // It can go on working.
    std::fs::write(run_dir.join("run.txt"), "more\n").unwrap();
    let next =
        block_on(vcs.commit_all("tau: run 1 turn 2", "tau/run")).unwrap();
    assert!(next.changed);
    assert_eq!(
        project.parent_of(&next.commit_id).unwrap(),
        Some(turn.commit_id)
    );

    // Opening again finds the same project, updated.
    let again = tau_vcs::ProjectRepo::open_or_import(
        "unused",
        home.path().join("p"),
        Identity::default(),
    )
    .unwrap();
    check(&again, &source, head);
}
