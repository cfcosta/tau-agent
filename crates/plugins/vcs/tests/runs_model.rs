//! Runs in a project against a model (`docs/reference/vcs.md`,
//! "Projects", "Runs and turns", "Landing a child run", "Merging a run
//! into trunk"; ADR 0009, 0014): random sequences of runs, turns, edits
//! left uncommitted, forks, landings, drops, merges into trunk and
//! forgotten workspaces on a real project, checked after every step.
//!
//! The model is a graph of commits, each a parent and a file tree, the
//! runs that point into it, each with the tree of its `@`, and trunk.
//! What it holds to:
//! - a turn commits what it changed on top of the run's head, and a turn
//!   that changed nothing names the commit before it; the run's bookmark
//!   `tau/<run>` names its newest commit;
//! - a turn that ends as `RunWorkspace` ends one snapshots `@` and leaves
//!   it uncommitted, listing the paths changed since the last snapshot;
//! - a fork at a change's link starts on exactly that link's files; one
//!   at a snapshot starts on the snapshot's files, on its parent as it
//!   is now, with what the parent gained since merged in;
//! - runs are isolated: a run's files are its `@`'s tree, whatever the
//!   other runs do;
//! - landing moves the child's own changes (what its head has that the
//!   parent's lacks) onto the parent's head, in order, each keeping its
//!   change id, each tree rebased as jj rebases (`onto + old - base`,
//!   path by path, resolved by jj's trivial merge, else a conflict); a
//!   child already on the parent's head is not rewritten; the parent's
//!   files and bookmark follow, and its uncommitted work moves on top;
//!   the child closes;
//! - moving a top-level run onto trunk restacks its changes and its `@`
//!   the same way, reporting conflicts in `@` too; trunk moves forward
//!   to a merged run and never sideways;
//! - dropping abandons the child's own changes; whatever descended from
//!   a rewritten or abandoned commit follows it, as jj rebases it;
//! - `Project::current` moves each link to its change's commit now, and
//!   leaves a link to an abandoned change, or to a snapshot, where it
//!   was;
//! - `forget_workspace` deletes the directory and keeps the commits and
//!   the bookmark.
//!
//! File contents are one line or empty, so jj's line merge of a file
//! resolves exactly when its trivial merge of whole files does.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    process::Command,
};

use hegel::{TestCase, generators as gs};
use tau_testing::block_on;
use tau_vcs::{Identity, Link, Project, Vcs, VcsError};

const PATHS: [&str; 3] = ["a.txt", "c.txt", "dir/b.txt"];
const VALUES: [&str; 4] = ["", "one\n", "two\n", "three\n"];
const MAX_RUNS: usize = 6;

/// A file's contents; `None` is no file.
type Val = Option<&'static str>;

/// A path's value in a tree: jj's merge terms, counted (+1 for each
/// add, -1 for each remove, zeros dropped). One value counted once is a
/// resolved file; anything else is a conflict.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Term(BTreeMap<Val, i32>);

impl Term {
    fn resolved(value: Val) -> Self {
        Self(BTreeMap::from([(value, 1)]))
    }

    fn value(&self) -> Option<Val> {
        match self.0.iter().collect::<Vec<_>>().as_slice() {
            [(value, 1)] => Some(**value),
            _ => None,
        }
    }

    /// What jj writes to disk for a conflict: its sides merged with a
    /// missing file read as empty, when that resolves; `None` when the
    /// file gets conflict markers.
    fn materialized(&self) -> Option<Val> {
        let mut counts: BTreeMap<&str, i32> = BTreeMap::new();
        for (value, n) in &self.0 {
            *counts.entry(value.unwrap_or("")).or_default() += n;
        }
        counts.retain(|_, n| *n != 0);
        let positive: Vec<&str> = counts
            .iter()
            .filter(|(_, n)| **n > 0)
            .map(|(value, _)| *value)
            .collect();
        match (counts.len(), positive.as_slice()) {
            (1, [value]) | (2, [value]) => Some(Some(*value)),
            _ => None,
        }
    }

    /// `old`, rebased from `base` onto `onto`: jj's `onto + old - base`,
    /// resolved as `trivial_merge` does with `same-change = accept`.
    fn rebase(onto: &Term, base: &Term, old: &Term) -> Term {
        let mut counts = onto.0.clone();
        for (value, n) in &old.0 {
            *counts.entry(*value).or_default() += n;
        }
        for (value, n) in &base.0 {
            *counts.entry(*value).or_default() -= n;
        }
        counts.retain(|_, n| *n != 0);
        let positive: Vec<Val> = counts
            .iter()
            .filter(|(_, n)| **n > 0)
            .map(|(value, _)| *value)
            .collect();
        match (counts.len(), positive.as_slice()) {
            (1, [value]) | (2, [value]) => Term::resolved(*value),
            _ => Term(counts),
        }
    }
}

/// Path to value; a path missing is no file.
type Tree = BTreeMap<&'static str, Term>;

fn get(tree: &Tree, path: &'static str) -> Term {
    tree.get(path)
        .cloned()
        .unwrap_or_else(|| Term::resolved(None))
}

fn set(tree: &mut Tree, path: &'static str, term: Term) {
    if term == Term::resolved(None) {
        tree.remove(path);
    } else {
        tree.insert(path, term);
    }
}

fn rebase_tree(onto: &Tree, base: &Tree, old: &Tree) -> Tree {
    let mut tree = Tree::new();
    for path in PATHS {
        set(
            &mut tree,
            path,
            Term::rebase(&get(onto, path), &get(base, path), &get(old, path)),
        );
    }
    tree
}

fn conflicts(tree: &Tree) -> Vec<String> {
    tree.iter()
        .filter(|(_, term)| term.value().is_none())
        .map(|(path, _)| (*path).to_owned())
        .collect()
}

#[derive(Debug, Clone)]
struct Commit {
    /// Learned from the tools; the trunk's only once a link names it.
    change_id: Option<String>,
    commit_id: String,
    parent: Option<usize>,
    tree: Tree,
    abandoned: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Open,
    Landed,
    Dropped,
    Forgotten,
    Merged,
}

/// A run's link, and the commit it names: for a turn's snapshot, the
/// commit `@` stood on, with that commit's tree and `@`'s then.
#[derive(Debug, Clone)]
struct Linked {
    link: Link,
    at: usize,
    snapshot: Option<(Tree, Tree)>,
}

struct Run {
    name: String,
    parent: Option<usize>,
    /// The run's newest commit.
    head: usize,
    /// `@`'s tree: the files on disk, committed or not.
    wc: Tree,
    /// Its links.
    links: Vec<Linked>,
    /// The last turn's snapshot and its tree, which the next turn's
    /// paths are counted from, as `RunWorkspace` keeps it.
    since: Option<(String, Tree)>,
    turns: u32,
    state: State,
    /// A turn has set `tau/<name>`.
    bookmarked: bool,
    vcs: Vcs,
}

impl Run {
    fn bookmark(&self) -> String {
        format!("tau/{}", self.name)
    }
}

struct Machine {
    home: tempfile::TempDir,
    project: Project,
    commits: Vec<Commit>,
    runs: Vec<Run>,
    /// The commit trunk's bookmark names.
    trunk: usize,
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(["-c", "init.defaultBranch=main"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

impl Machine {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let src = home.path().join("src");
        std::fs::create_dir_all(src.join("dir")).unwrap();
        git(&src, &["init", "--quiet"]);
        std::fs::write(src.join("a.txt"), "one\n").unwrap();
        std::fs::write(src.join("dir/b.txt"), "two\n").unwrap();
        git(&src, &["add", "."]);
        git(&src, &["commit", "--quiet", "-m", "first"]);
        let project = Project::import(
            src.to_str().unwrap(),
            home.path().join("p"),
            Identity::default(),
        )
        .unwrap();
        let trunk = project.trunk().unwrap();
        let mut tree = Tree::new();
        set(&mut tree, "a.txt", Term::resolved(Some("one\n")));
        set(&mut tree, "dir/b.txt", Term::resolved(Some("two\n")));
        let mut machine = Self {
            home,
            project,
            commits: vec![Commit {
                change_id: None,
                commit_id: trunk,
                parent: None,
                tree,
                abandoned: false,
            }],
            runs: Vec::new(),
            trunk: 0,
        };
        // A first run, so no step waits for one.
        machine.start_root();
        machine
    }

    /// A new run, on trunk.
    fn start_root(&mut self) {
        let name = format!("r{}", self.runs.len());
        let trunk = self.project.trunk().unwrap();
        assert_eq!(trunk, self.commits[self.trunk].commit_id);
        let vcs = self.project.add_workspace(&name, &trunk).unwrap();
        self.runs.push(Run {
            name,
            parent: None,
            head: self.trunk,
            wc: self.commits[self.trunk].tree.clone(),
            links: Vec::new(),
            since: None,
            turns: 0,
            state: State::Open,
            bookmarked: false,
            vcs,
        });
    }

    fn open(&self) -> Vec<usize> {
        (0..self.runs.len())
            .filter(|&r| self.runs[r].state == State::Open)
            .collect()
    }

    fn has_open_children(&self, run: usize) -> bool {
        self.runs.iter().any(|other| {
            other.parent == Some(run) && other.state == State::Open
        })
    }

    fn ancestors(&self, mut at: usize) -> BTreeSet<usize> {
        let mut set = BTreeSet::new();
        loop {
            set.insert(at);
            match self.commits[at].parent {
                Some(parent) => at = parent,
                None => return set,
            }
        }
    }

    /// What `head` has that `keep` lacks, oldest first: a stack.
    fn own_changes(&self, head: usize, keep: usize) -> Vec<usize> {
        let kept = self.ancestors(keep);
        let mut changes = Vec::new();
        let mut at = head;
        while !kept.contains(&at) {
            changes.push(at);
            at = self.commits[at].parent.expect("the trunk is kept");
        }
        changes.reverse();
        changes
    }

    /// Where a commit is now: itself, or the nearest ancestor that was
    /// not abandoned.
    fn live(&self, mut at: usize) -> usize {
        while self.commits[at].abandoned {
            at = self.commits[at].parent.unwrap();
        }
        at
    }

    /// Rebases what descends from `changed` (commits already rewritten
    /// or abandoned, with their trees before in `before`), as jj's
    /// `rebase_descendants` does. Returns the commits it moved.
    fn follow(
        &mut self,
        changed: &BTreeSet<usize>,
        before: &[Tree],
    ) -> Vec<usize> {
        let mut changed = changed.clone();
        let mut moved = Vec::new();
        for at in 0..self.commits.len() {
            if changed.contains(&at) || self.commits[at].abandoned {
                continue;
            }
            let Some(parent) = self.commits[at].parent else {
                continue;
            };
            if !changed.contains(&parent) {
                continue;
            }
            let onto = self.live(parent);
            let tree = rebase_tree(
                &self.commits[onto].tree,
                &before[parent],
                &before[at],
            );
            self.commits[at].parent = Some(onto);
            self.commits[at].tree = tree;
            changed.insert(at);
            moved.push(at);
        }
        moved
    }

    /// Learns where each of `moved` is now, through its change id.
    fn learn(&mut self, moved: &[usize]) {
        let links: Vec<Link> =
            moved.iter().map(|&at| self.link_to(at)).collect();
        let now = self.project.current(links).unwrap();
        for (&at, link) in moved.iter().zip(now) {
            self.commits[at].commit_id = link.commit_id;
        }
    }

    fn link_to(&self, at: usize) -> Link {
        let commit = &self.commits[at];
        Link {
            turn: 0,
            workspace: String::new(),
            commit_id: commit.commit_id.clone(),
            change_id: commit.change_id.clone().expect("a known change"),
            changed: true,
            from: None,
            snapshot: false,
        }
    }

    fn disk(&self, run: usize) -> BTreeMap<&'static str, Vec<u8>> {
        let dir = self.project.workspace_dir(&self.runs[run].name);
        PATHS
            .iter()
            .filter_map(|path| {
                std::fs::read(dir.join(path))
                    .ok()
                    .map(|bytes| (*path, bytes))
            })
            .collect()
    }

    /// The files in `run`'s workspace are `@`'s tree: resolved files as
    /// they are, conflicts as jj materializes them (see
    /// `a_landed_conflict_shows_markers`).
    fn check_files(&self, run: usize) {
        let disk = self.disk(run);
        let tree = &self.runs[run].wc;
        for path in PATHS {
            let term = get(tree, path);
            let on_disk = disk.get(path);
            match term.value() {
                Some(value) => assert_eq!(
                    on_disk.map(Vec::as_slice),
                    value.map(str::as_bytes),
                    "{path} in {}",
                    self.runs[run].name
                ),
                None => match term.materialized() {
                    Some(value) => {
                        assert_eq!(
                            on_disk.map(Vec::as_slice).unwrap_or_default(),
                            value.unwrap_or_default().as_bytes(),
                            "conflicted {path} without markers"
                        );
                    }
                    None => {
                        let text =
                            String::from_utf8_lossy(on_disk.unwrap_or_else(
                                || panic!("conflicted {path} is missing"),
                            ));
                        assert!(text.contains("<<<<<<<"), "{path}: {text}");
                    }
                },
            }
        }
    }

    /// A commit without conflicts holds exactly its model tree.
    fn check_commit(&self, at: usize) {
        let commit = &self.commits[at];
        if !conflicts(&commit.tree).is_empty() {
            return;
        }
        for path in PATHS {
            let got = self.project.file_at(&commit.commit_id, path).unwrap();
            let want = get(&commit.tree, path).value().unwrap();
            assert_eq!(
                got.map(|(bytes, _)| bytes),
                want.map(|text| text.as_bytes().to_vec()),
                "{path} at {}",
                commit.commit_id
            );
        }
        let parent = commit.parent.map(|p| self.commits[p].commit_id.clone());
        assert_eq!(
            self.project.parent_of(&commit.commit_id).unwrap(),
            parent,
            "parent of {}",
            commit.commit_id
        );
    }

    /// Closes `run` as the host does after landing or dropping it.
    fn close(&mut self, run: usize, state: State) {
        let name = self.runs[run].name.clone();
        self.project.forget_workspace(&name).unwrap();
        self.project
            .remove_bookmark(&self.runs[run].bookmark())
            .unwrap();
        assert!(!self.project.workspace_dir(&name).exists());
        assert_eq!(
            self.project.bookmark(&self.runs[run].bookmark()).unwrap(),
            None
        );
        self.runs[run].state = state;
        self.runs[run].bookmarked = false;
    }

    /// Whether `run` has no uncommitted work: a run commits what it
    /// leaves when it finishes.
    fn clean(&self, run: usize) -> bool {
        let run = &self.runs[run];
        run.wc == self.commits[run.head].tree
    }

    /// A child that can land or be dropped, by the host's rule: open or
    /// forgotten, finished, with an open parent, and no children open or
    /// holding changes it does not have.
    fn closable(&self) -> Vec<usize> {
        (0..self.runs.len())
            .filter(|&r| {
                let run = &self.runs[r];
                matches!(run.state, State::Open | State::Forgotten)
                    && self.clean(r)
                    && run
                        .parent
                        .is_some_and(|p| self.runs[p].state == State::Open)
                    && !self.has_open_children(r)
                    && self.runs.iter().all(|child| {
                        child.parent != Some(r)
                            || child.state != State::Forgotten
                            || !child.bookmarked
                            || self
                                .ancestors(run.head)
                                .contains(&self.live(child.head))
                    })
            })
            .collect()
    }

    /// A top-level run the host lets merge into trunk
    /// (`Host::merge_plan`): open, and no child holding changes it does
    /// not have. The host also lets an idle child with a workspace stay
    /// open, which the merge leaves stale
    /// (`a_merge_leaves_an_open_fork_stale`); until that is decided, the
    /// model keeps no child open.
    fn mergeable(&self) -> Vec<usize> {
        (0..self.runs.len())
            .filter(|&r| {
                let run = &self.runs[r];
                run.parent.is_none()
                    && run.state == State::Open
                    && !self.has_open_children(r)
                    && self.runs.iter().all(|child| {
                        let has_bookmark = child.bookmarked
                            && matches!(
                                child.state,
                                State::Open | State::Forgotten
                            );
                        child.parent != Some(r)
                            || !has_bookmark
                            || run.bookmarked
                                && self
                                    .ancestors(self.live(run.head))
                                    .contains(&self.live(child.head))
                    })
            })
            .collect()
    }

    /// Restacks `chain` onto `onto` in the model, as jj rebases it, with
    /// the commits that descend from it. Returns whether it rewrote
    /// anything, and the descendants it dragged along.
    fn restack(&mut self, chain: &[usize], onto: usize) -> (bool, Vec<usize>) {
        let rewrite = chain
            .first()
            .is_some_and(|&root| self.commits[root].parent != Some(onto));
        if !rewrite {
            return (false, Vec::new());
        }
        let before: Vec<Tree> =
            self.commits.iter().map(|c| c.tree.clone()).collect();
        let mut onto = onto;
        for &at in chain {
            let base = self.commits[at].parent.unwrap();
            let tree = rebase_tree(
                &self.commits[onto].tree,
                &before[base],
                &before[at],
            );
            self.commits[at].parent = Some(onto);
            self.commits[at].tree = tree;
            onto = at;
        }
        let dragged = self.follow(&chain.iter().copied().collect(), &before);
        (true, dragged)
    }

    /// The paths jj holds in conflict at `commit` (a full hex id).
    fn jj_conflicts(&self, commit: &str) -> BTreeSet<String> {
        use jj_lib::{
            backend::CommitId,
            config::{ConfigLayer, ConfigSource, StackedConfig},
            default_backend_factories::{
                default_backend_factories,
                default_working_copy_factories,
            },
            repo::Repo as _,
            settings::UserSettings,
            workspace::Workspace,
        };
        let mut config = StackedConfig::with_defaults();
        let mut user = ConfigLayer::empty(ConfigSource::User);
        user.set_value("user.name", "model").unwrap();
        user.set_value("user.email", "model@localhost").unwrap();
        config.add_layer(user);
        let settings = UserSettings::from_config(config).unwrap();
        let workspace = Workspace::load(
            &settings,
            &self.home.path().join("p").join("main"),
            &default_backend_factories(),
            &default_working_copy_factories(),
        )
        .unwrap();
        let repo =
            pollster::block_on(workspace.repo_loader().load_at_head()).unwrap();
        let id = CommitId::try_from_hex(commit).unwrap();
        repo.store()
            .get_commit(&id)
            .unwrap()
            .tree()
            .conflicts()
            .map(|(path, _)| path.as_internal_file_string().to_owned())
            .collect()
    }

    /// The commit `run`'s working copy stands on, as jj has it.
    fn wc_parent(&self, run: usize) -> Option<String> {
        let wc = self
            .project
            .workspace_head(&self.runs[run].name)
            .unwrap()
            .unwrap();
        self.project.parent_of(&wc).unwrap()
    }
}

/// Writes `edits` to `dir` and returns `wc` with them.
fn write_edits(
    tc: &TestCase,
    dir: &Path,
    wc: &Tree,
    edits: &[(&'static str, Val)],
) -> Tree {
    let mut tree = wc.clone();
    for (path, value) in edits {
        let file = dir.join(path);
        match value {
            Some(text) => {
                std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                std::fs::write(&file, text).unwrap();
            }
            None => match std::fs::remove_file(&file) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => panic!("{e}"),
            },
        }
        if conflicts(&tree).contains(&(*path).to_owned()) {
            tc.event("an edit resolves a conflict");
        }
        set(&mut tree, path, Term::resolved(*value));
    }
    tree
}

/// Settles what a snapshot made of conflicts jj wrote without markers.
/// One the edits left alone may be taken as resolved, and one rewritten
/// with the very text jj wrote stays a conflict: an open question
/// (`a_turn_keeps_a_conflict_it_did_not_touch`). Either reading is
/// accepted, as `conflicted`, the paths jj holds in conflict now, tells.
fn settle(
    tc: &TestCase,
    was: &Tree,
    tree: &mut Tree,
    conflicted: &BTreeSet<String>,
) {
    for path in PATHS {
        let old = get(was, path);
        let Some(value) = old.materialized().filter(|_| old.value().is_none())
        else {
            continue;
        };
        let now = get(tree, path);
        let kept = conflicted.contains(path);
        if now == old && !kept {
            tc.event("an untouched conflict resolves itself");
            set(tree, path, Term::resolved(value));
        } else if now == Term::resolved(value) && kept {
            tc.event("writing a conflict's own text keeps it");
            set(tree, path, old);
        }
    }
}

/// The paths a snapshot lists as changed from `base` are those the model
/// changed, and maybe conflicts it holds as they were: after a few
/// restacks, jj lists an untouched conflict that stays a conflict,
/// perhaps written as another form of the same merge.
fn check_paths(listed: &[String], base: &Tree, tree: &Tree, what: &str) {
    for path in PATHS {
        let (old, new) = (get(base, path), get(tree, path));
        let is_listed = listed.iter().any(|p| p == path);
        if old != new {
            assert!(is_listed, "{what}: {path} changed, but is not listed");
        } else if is_listed {
            assert!(
                new.value().is_none(),
                "{what}: {path} is listed, but did not change"
            );
        }
    }
}

impl Machine {
    /// A turn in `run`: `edits` to its files, then a commit of all of
    /// `@`, as a run's model makes one.
    fn do_turn(
        &mut self,
        tc: &TestCase,
        run: usize,
        edits: &[(&'static str, Val)],
    ) {
        let dir = self.project.workspace_dir(&self.runs[run].name);
        let head = self.runs[run].head;
        let was = self.runs[run].wc.clone();
        let mut tree = write_edits(tc, &dir, &was, edits);
        self.runs[run].turns += 1;
        let r = &self.runs[run];
        let turn = block_on(r.vcs.commit_all(
            format!("tau: run {} turn {}", r.name, r.turns),
            r.bookmark(),
        ))
        .unwrap();
        let conflicted = self.jj_conflicts(&turn.commit_id);
        settle(tc, &was, &mut tree, &conflicted);
        self.runs[run].wc = tree.clone();
        check_paths(&turn.paths, &self.commits[head].tree, &tree, "the turn");

        let at = if !turn.changed {
            tc.event("a turn changes nothing");
            assert_eq!(tree, self.commits[head].tree, "a turn did not commit");
            assert!(turn.paths.is_empty());
            // It names the commit before it.
            let commit = &mut self.commits[head];
            assert_eq!(turn.commit_id, commit.commit_id);
            match &commit.change_id {
                Some(id) => assert_eq!(&turn.change_id, id),
                None => commit.change_id = Some(turn.change_id.clone()),
            }
            head
        } else {
            if tree == self.commits[head].tree {
                tc.event("a turn rewrites a conflict alone");
            }
            assert!(
                self.commits
                    .iter()
                    .all(|c| c.change_id.as_ref() != Some(&turn.change_id)),
                "a turn's commit reused a change id"
            );
            self.commits.push(Commit {
                change_id: Some(turn.change_id.clone()),
                commit_id: turn.commit_id.clone(),
                parent: Some(head),
                tree,
                abandoned: false,
            });
            self.commits.len() - 1
        };
        let r = &mut self.runs[run];
        r.head = at;
        r.bookmarked = true;
        r.links.push(Linked {
            link: Link {
                turn: r.turns,
                workspace: r.name.clone(),
                commit_id: turn.commit_id,
                change_id: turn.change_id,
                changed: turn.changed,
                from: None,
                snapshot: false,
            },
            at,
            snapshot: None,
        });
        self.check_commit(at);
    }

    /// A turn in `run` that ends as `RunWorkspace` ends one (ADR 0014):
    /// `edits` to its files, then a snapshot of `@`, left uncommitted.
    fn do_snapshot(
        &mut self,
        tc: &TestCase,
        run: usize,
        edits: &[(&'static str, Val)],
    ) {
        let dir = self.project.workspace_dir(&self.runs[run].name);
        let head = self.runs[run].head;
        let was = self.runs[run].wc.clone();
        let mut tree = write_edits(tc, &dir, &was, edits);
        let r = &self.runs[run];
        let since = r.since.clone();
        let snapshot =
            block_on(r.vcs.end_turn(
                r.bookmark(),
                since.as_ref().map(|(id, _)| id.clone()),
            ))
            .unwrap();
        // The paths count from the turn before's snapshot, else from the
        // run's head.
        let base = since
            .map(|(_, tree)| tree)
            .unwrap_or_else(|| self.commits[head].tree.clone());
        let conflicted = self.jj_conflicts(&snapshot.commit_id);
        settle(tc, &was, &mut tree, &conflicted);
        check_paths(&snapshot.paths, &base, &tree, "the snapshot");
        assert_eq!(snapshot.head, self.commits[head].commit_id);
        if tree != self.commits[head].tree {
            tc.event("a turn leaves work uncommitted");
        }

        let head_tree = self.commits[head].tree.clone();
        let r = &mut self.runs[run];
        r.wc = tree.clone();
        r.turns += 1;
        r.bookmarked = true;
        r.since = Some((snapshot.commit_id.clone(), tree.clone()));
        r.links.push(Linked {
            link: Link {
                turn: r.turns,
                workspace: r.name.clone(),
                commit_id: snapshot.commit_id,
                change_id: snapshot.change_id,
                changed: !snapshot.paths.is_empty(),
                from: None,
                snapshot: true,
            },
            at: head,
            snapshot: Some((head_tree, tree)),
        });
    }

    /// The tree a fork at `link` starts with in `@`: a change's own, or
    /// a snapshot's files on its parent as it is now, what a landing
    /// brought to the parent since merged in.
    fn fork_tree(&self, linked: &Linked) -> Tree {
        match &linked.snapshot {
            None => self.commits[linked.at].tree.clone(),
            Some((then, wc)) => {
                rebase_tree(&self.commits[linked.at].tree, then, wc)
            }
        }
    }

    /// A fork of `parent` at its `k`th link.
    fn do_fork(&mut self, tc: &TestCase, parent: usize, k: usize) {
        let count = self.runs[parent].links.len();
        tc.event(if k + 1 == count {
            "a fork at the latest link"
        } else {
            "a fork at an earlier link"
        });
        let linked = self.runs[parent].links[k].clone();
        let link = linked.link.clone();
        // A change's link follows its change, which a move onto trunk
        // may have rewritten; a snapshot's is that very commit.
        let now = self.project.current([link.clone()]).unwrap().remove(0);
        let want = if link.snapshot {
            link.commit_id.clone()
        } else {
            self.commits[linked.at].commit_id.clone()
        };
        assert_eq!(now.commit_id, want, "the link to fork at");
        let name = format!("r{}", self.runs.len());
        let vcs = if link.snapshot {
            if self.commits[linked.at].tree
                != linked.snapshot.as_ref().unwrap().0
            {
                tc.event("a fork at a snapshot whose parent's files moved");
            }
            self.project
                .add_workspace_from_snapshot(&name, &link.commit_id)
                .unwrap()
        } else {
            self.project.add_workspace(&name, &now.commit_id).unwrap()
        };
        let links = self.runs[parent].links[..=k].to_vec();
        self.runs.push(Run {
            name,
            parent: Some(parent),
            head: linked.at,
            wc: self.fork_tree(&linked),
            links,
            since: link.snapshot.then(|| {
                (link.commit_id.clone(), linked.snapshot.clone().unwrap().1)
            }),
            turns: link.turn,
            state: State::Open,
            bookmarked: false,
            vcs,
        });
        // The fork starts on exactly that turn's files.
        let fork = self.runs.len() - 1;
        self.check_files(fork);
        assert_eq!(
            self.wc_parent(fork).as_ref(),
            Some(&self.commits[linked.at].commit_id),
            "the fork's working copy is not on its link's commit"
        );
    }

    /// Lands `child` on its parent, previewed first, then closes it.
    fn do_land(&mut self, tc: &TestCase, child: usize) {
        let parent = self.runs[child].parent.unwrap();
        let child_head = self
            .project
            .bookmark(&self.runs[child].bookmark())
            .unwrap()
            .expect("a child's bookmark");
        assert_eq!(child_head, self.commits[self.runs[child].head].commit_id);
        let p = &self.runs[parent];
        let files = self.disk(parent);
        let bookmark = self.project.bookmark(&p.bookmark()).unwrap();
        let wc = self.wc_parent(parent);

        let preview =
            block_on(p.vcs.land(&child_head, p.bookmark(), false)).unwrap();
        let p = &self.runs[parent];
        assert_eq!(self.disk(parent), files, "a preview changed files");
        assert_eq!(self.project.bookmark(&p.bookmark()).unwrap(), bookmark);
        // A preview snapshots `@`, which may rewrite it; it stays put.
        assert_eq!(self.wc_parent(parent), wc);

        let landing =
            block_on(p.vcs.land(&child_head, p.bookmark(), true)).unwrap();

        // The model's landing.
        if !self.clean(parent) {
            tc.event("a landing under uncommitted work");
        }
        let old_head = self.runs[parent].head;
        let old_head_tree = self.commits[old_head].tree.clone();
        let moving = self.own_changes(self.runs[child].head, old_head);
        let old_ids: Vec<String> = moving
            .iter()
            .map(|&at| self.commits[at].commit_id.clone())
            .collect();
        let (rewrite, dragged) = self.restack(&moving, old_head);
        match (moving.is_empty(), rewrite) {
            (true, _) => tc.event("a landing with nothing to land"),
            (false, false) => tc.event("a landing that rewrites nothing"),
            (false, true) => tc.event("a landing that restacks"),
        }
        if !dragged.is_empty() {
            tc.event("a landing drags a forgotten fork along");
        }

        // Its changes, newest first, keep their change ids.
        let ids: Vec<String> = landing
            .changes
            .iter()
            .map(|change| change.change_id.clone())
            .collect();
        let want: Vec<String> = moving
            .iter()
            .rev()
            .map(|&at| self.commits[at].change_id.clone().unwrap())
            .collect();
        assert_eq!(ids, want, "the landed changes");
        assert_eq!(
            preview
                .changes
                .iter()
                .map(|c| &c.change_id)
                .collect::<Vec<_>>(),
            ids.iter().collect::<Vec<_>>(),
            "the preview's changes"
        );
        assert_eq!(
            preview.conflicts, landing.conflicts,
            "the preview's conflicts"
        );
        for (change, &at) in landing.changes.iter().zip(moving.iter().rev()) {
            self.commits[at].commit_id = change.commit_id.clone();
            let conflicted = !conflicts(&self.commits[at].tree).is_empty();
            assert_eq!(change.conflict, conflicted, "conflict flag of {at}");
        }
        if !rewrite {
            let now: Vec<String> = moving
                .iter()
                .map(|&at| self.commits[at].commit_id.clone())
                .collect();
            assert_eq!(now, old_ids, "a child on the parent's head moved");
        }
        self.learn(&dragged);
        if let Some(&last) = moving.last() {
            self.runs[parent].head = last;
        }
        let head = self.runs[parent].head;
        // The parent's uncommitted work moves on top, as jj rebases it.
        let p = &mut self.runs[parent];
        p.wc = rebase_tree(&self.commits[head].tree, &old_head_tree, &p.wc);
        let want_conflicts = conflicts(&self.commits[head].tree);
        if !want_conflicts.is_empty() {
            tc.event("a landing conflicts");
        }
        assert_eq!(
            landing.conflicts, want_conflicts,
            "the landing's conflicts"
        );
        assert_eq!(landing.head, self.commits[head].commit_id);
        for &at in moving.iter().chain(&dragged) {
            self.check_commit(at);
        }
        if !moving.is_empty() {
            assert_eq!(
                self.project
                    .bookmark(&self.runs[parent].bookmark())
                    .unwrap(),
                Some(landing.head.clone()),
                "the parent's bookmark"
            );
            self.runs[parent].bookmarked = true;
        }

        // The host links the landed changes in the parent.
        let from = self.runs[child].name.clone();
        let turn = self.runs[parent].turns;
        let workspace = self.runs[parent].name.clone();
        for change in landing.changes.iter().rev() {
            let at = moving
                .iter()
                .copied()
                .find(|&at| {
                    self.commits[at].change_id.as_ref()
                        == Some(&change.change_id)
                })
                .unwrap();
            self.runs[parent].links.push(Linked {
                link: Link {
                    turn,
                    workspace: workspace.clone(),
                    commit_id: change.commit_id.clone(),
                    change_id: change.change_id.clone(),
                    changed: true,
                    from: Some(from.clone()),
                    snapshot: false,
                },
                at,
                snapshot: None,
            });
        }
        self.close(child, State::Landed);
    }

    /// Moves top-level `run`'s changes, up to `@`, onto trunk, previewed
    /// first. With `merge`, the host's merge goes on: trunk moves
    /// forward to the run and the run closes, unless nothing moved or
    /// something conflicts, when the run stays open to resolve it.
    fn do_move(&mut self, tc: &TestCase, run: usize, merge: bool) {
        let trunk = self.trunk;
        let head = self.runs[run].head;
        let trunk_id = self.commits[trunk].commit_id.clone();
        // Trunk only moves forward.
        if !self.ancestors(head).contains(&trunk) {
            tc.event("trunk has moved on past a run");
            let head_id = self.commits[head].commit_id.clone();
            let error = self.project.fast_forward_trunk(&head_id).unwrap_err();
            assert!(
                matches!(error, VcsError::NotFastForward(_)),
                "a sideways move of trunk: {error}"
            );
            assert_eq!(self.project.trunk().unwrap(), trunk_id);
        }

        let r = &self.runs[run];
        let files = self.disk(run);
        let bookmark = self.project.bookmark(&r.bookmark()).unwrap();
        let wc = self.wc_parent(run);
        let preview =
            block_on(r.vcs.move_onto(trunk_id.clone(), r.bookmark(), false))
                .unwrap();
        let r = &self.runs[run];
        assert_eq!(self.disk(run), files, "a preview changed files");
        assert_eq!(self.project.bookmark(&r.bookmark()).unwrap(), bookmark);
        assert_eq!(self.wc_parent(run), wc, "a preview moved @");
        let moved =
            block_on(r.vcs.move_onto(trunk_id, r.bookmark(), true)).unwrap();

        // The model's move.
        let chain = self.own_changes(head, trunk);
        let old_ids: Vec<String> = chain
            .iter()
            .map(|&at| self.commits[at].commit_id.clone())
            .collect();
        let old_head_tree = self.commits[head].tree.clone();
        let (rewrite, dragged) = self.restack(&chain, trunk);
        match (chain.is_empty(), rewrite) {
            (true, _) if head == trunk => {
                tc.event("a move with nothing to move")
            }
            (true, _) => tc.event("a move of @ alone onto trunk"),
            (false, false) => tc.event("a move that rewrites nothing"),
            (false, true) => tc.event("a move that restacks"),
        }
        if !dragged.is_empty() {
            tc.event("a move drags a forgotten fork along");
        }
        let ids: Vec<String> = moved
            .changes
            .iter()
            .map(|change| change.change_id.clone())
            .collect();
        let want: Vec<String> = chain
            .iter()
            .rev()
            .map(|&at| self.commits[at].change_id.clone().unwrap())
            .collect();
        assert_eq!(ids, want, "the moved changes");
        assert_eq!(
            preview
                .changes
                .iter()
                .map(|c| &c.change_id)
                .collect::<Vec<_>>(),
            ids.iter().collect::<Vec<_>>(),
            "the preview's changes"
        );
        assert_eq!(
            preview.conflicts, moved.conflicts,
            "the preview's conflicts"
        );
        for (change, &at) in moved.changes.iter().zip(chain.iter().rev()) {
            self.commits[at].commit_id = change.commit_id.clone();
            let conflicted = !conflicts(&self.commits[at].tree).is_empty();
            assert_eq!(change.conflict, conflicted, "conflict flag of {at}");
        }
        if !rewrite {
            let now: Vec<String> = chain
                .iter()
                .map(|&at| self.commits[at].commit_id.clone())
                .collect();
            assert_eq!(now, old_ids, "a run on trunk's head moved");
        }
        self.learn(&dragged);
        let new_head = chain.last().copied().unwrap_or(trunk);
        let r = &mut self.runs[run];
        r.head = new_head;
        r.bookmarked = true;
        r.wc = rebase_tree(&self.commits[new_head].tree, &old_head_tree, &r.wc);

        // The conflicts in the new head, then any more in `@`.
        let mut want = conflicts(&self.commits[new_head].tree);
        for path in conflicts(&self.runs[run].wc) {
            if !want.contains(&path) {
                tc.event("a move conflicts in @ alone");
                want.push(path);
            }
        }
        if !want.is_empty() {
            tc.event("a move conflicts");
        }
        assert_eq!(moved.conflicts, want, "the move's conflicts");
        assert_eq!(moved.head, self.commits[new_head].commit_id);
        for &at in chain.iter().chain(&dragged) {
            self.check_commit(at);
        }
        assert_eq!(
            self.project.bookmark(&self.runs[run].bookmark()).unwrap(),
            Some(moved.head.clone()),
            "the run's bookmark"
        );

        if !merge {
            return;
        }
        if moved.changes.is_empty() {
            tc.event("a merge with nothing to merge");
            return;
        }
        if !moved.conflicts.is_empty() {
            tc.event("a merge waits on its conflicts");
            return;
        }
        let into = self.project.fast_forward_trunk(&moved.head).unwrap();
        assert_eq!(into, "main");
        assert_eq!(self.project.trunk().unwrap(), moved.head);
        tc.event("a merge into trunk");
        if !self.runs[run].links.iter().all(|l| l.snapshot.is_none()) {
            tc.event("a merge of a run with snapshots");
        }
        self.trunk = new_head;
        self.close(run, State::Merged);
    }
}

#[hegel::state_machine]
impl Machine {
    /// A new run, on trunk: two open at most, so most steps go to
    /// children.
    #[rule]
    fn new_run(&mut self, tc: TestCase) {
        let roots = self
            .runs
            .iter()
            .filter(|r| r.parent.is_none() && r.state == State::Open)
            .count();
        tc.assume(self.runs.len() < MAX_RUNS && roots < 2);
        self.start_root();
    }

    /// A turn: edits in one run's files, then the checkpoint
    /// `RunWorkspace` makes at `TurnEnd`.
    #[rule(weight = 6)]
    fn turn(&mut self, tc: TestCase) {
        let open = self.open();
        tc.assume(!open.is_empty());
        let run = tc.draw(gs::sampled_from(open));
        let edits = tc.draw(edits());
        self.do_turn(&tc, run, &edits);
    }

    /// A turn as `RunWorkspace` ends one: edits in one run's files,
    /// snapshotted and left uncommitted.
    #[rule(weight = 3)]
    fn snapshot_turn(&mut self, tc: TestCase) {
        let open = self.open();
        tc.assume(!open.is_empty());
        let run = tc.draw(gs::sampled_from(open));
        let edits = tc.draw(edits());
        self.do_snapshot(&tc, run, &edits);
    }

    /// Edits in one run's files, with no tool run after them.
    #[rule(weight = 2)]
    fn write(&mut self, tc: TestCase) {
        let open = self.open();
        tc.assume(!open.is_empty());
        let run = tc.draw(gs::sampled_from(open));
        let edits = tc.draw(edits());
        let dir = self.project.workspace_dir(&self.runs[run].name);
        self.runs[run].wc = write_edits(&tc, &dir, &self.runs[run].wc, &edits);
    }

    /// A fork at one of a run's links, as `RunWorkspace` starts one: on
    /// the commit `Project::current` gives a change's link, or on a
    /// snapshot's files, inheriting the links up to it.
    #[rule(weight = 3)]
    fn fork(&mut self, tc: TestCase) {
        tc.assume(self.runs.len() < MAX_RUNS);
        let candidates: Vec<usize> = self
            .open()
            .into_iter()
            .filter(|&r| !self.runs[r].links.is_empty())
            .collect();
        tc.assume(!candidates.is_empty());
        let parent = tc.draw(gs::sampled_from(candidates));
        let count = self.runs[parent].links.len();
        let k = tc.draw(gs::integers::<usize>().max_value(count - 1));
        self.do_fork(&tc, parent, k);
    }

    /// Merges a finished top-level run into trunk, as `Host::merge` and
    /// `Host::finish_merge` do.
    #[rule(weight = 2)]
    fn merge(&mut self, tc: TestCase) {
        let candidates: Vec<usize> = self
            .mergeable()
            .into_iter()
            .filter(|&r| self.clean(r))
            .collect();
        tc.assume(!candidates.is_empty());
        let run = tc.draw(gs::sampled_from(candidates));
        self.do_move(&tc, run, true);
    }

    /// Moves a top-level run onto trunk with its work in `@`, as the
    /// host brings a main chat up to trunk.
    #[rule]
    fn move_onto_trunk(&mut self, tc: TestCase) {
        let candidates = self.mergeable();
        tc.assume(!candidates.is_empty());
        let run = tc.draw(gs::sampled_from(candidates));
        self.do_move(&tc, run, false);
    }

    /// Lands a child on its parent, previewed first, then closes it.
    #[rule(weight = 3)]
    fn land(&mut self, tc: TestCase) {
        let candidates: Vec<usize> = self
            .closable()
            .into_iter()
            .filter(|&r| self.runs[r].bookmarked)
            .collect();
        tc.assume(!candidates.is_empty());
        let child = tc.draw(gs::sampled_from(candidates));
        self.do_land(&tc, child);
    }

    /// Drops a child: its own changes are abandoned, as `Host::drop_child`
    /// does, and it closes.
    #[rule(weight = 2)]
    fn drop_child(&mut self, tc: TestCase) {
        let candidates = self.closable();
        tc.assume(!candidates.is_empty());
        let child = tc.draw(gs::sampled_from(candidates));
        tc.event("a drop");
        let parent = self.runs[child].parent.unwrap();
        let keep = self.runs[parent].head;
        if self.runs[child].bookmarked {
            let head = self
                .project
                .bookmark(&self.runs[child].bookmark())
                .unwrap()
                .unwrap();
            let count = self
                .project
                .abandon_between(&self.commits[keep].commit_id, &head)
                .unwrap();
            let own = self.own_changes(self.runs[child].head, keep);
            assert_eq!(count, own.len(), "abandoned changes");
            if own.is_empty() {
                tc.event("a drop with nothing to abandon");
            }
            let before: Vec<Tree> =
                self.commits.iter().map(|c| c.tree.clone()).collect();
            for &at in &own {
                self.commits[at].abandoned = true;
            }
            let dragged = self.follow(&own.iter().copied().collect(), &before);
            if !dragged.is_empty() {
                tc.event("a drop drags a forgotten fork along");
            }
            self.learn(&dragged);
            for &at in &dragged {
                self.check_commit(at);
            }
        }
        self.close(child, State::Dropped);
    }

    /// Forgets a fork's workspace, as "keep branch" does for the others:
    /// its directory goes, its commits and bookmark stay.
    #[rule]
    fn forget(&mut self, tc: TestCase) {
        let candidates: Vec<usize> = self
            .open()
            .into_iter()
            .filter(|&r| {
                self.runs[r].parent.is_some()
                    && !self.has_open_children(r)
                    && self.clean(r)
            })
            .collect();
        tc.assume(!candidates.is_empty());
        let run = tc.draw(gs::sampled_from(candidates));
        tc.event("a workspace forgotten");
        let name = self.runs[run].name.clone();
        self.project.forget_workspace(&name).unwrap();
        assert!(!self.project.workspace_dir(&name).exists());
        self.runs[run].state = State::Forgotten;
    }

    #[invariant(always_run)]
    fn the_project_is_the_model(&self, tc: TestCase) {
        tc.event_value("runs", self.runs.len() as f64);
        let mut open: Vec<String> = self
            .open()
            .into_iter()
            .map(|r| self.runs[r].name.clone())
            .collect();
        open.sort();
        assert_eq!(self.project.workspaces().unwrap(), open, "workspaces");

        for (r, run) in self.runs.iter().enumerate() {
            let head = &self.commits[self.live(run.head)];
            if run.state == State::Open {
                self.check_files(r);
                let wc =
                    self.project.workspace_head(&run.name).unwrap().unwrap();
                assert_eq!(
                    self.project.parent_of(&wc).unwrap().as_ref(),
                    Some(&head.commit_id),
                    "{}'s working copy is not on its head",
                    run.name
                );
            }
            let bookmark = self.project.bookmark(&run.bookmark()).unwrap();
            let want = match run.state {
                State::Open | State::Forgotten if run.bookmarked => {
                    Some(head.commit_id.clone())
                }
                _ => None,
            };
            assert_eq!(bookmark, want, "{}'s bookmark", run.bookmark());
        }

        // Every link, of every run, is where its change is now; one to
        // an abandoned change stays where it was, and a snapshot is that
        // very commit.
        let links: Vec<&Linked> =
            self.runs.iter().flat_map(|run| &run.links).collect();
        let now = self
            .project
            .current(links.iter().map(|linked| linked.link.clone()))
            .unwrap();
        for (Linked { link, at, .. }, now) in links.into_iter().zip(now) {
            let commit = &self.commits[*at];
            let want = if commit.abandoned || link.snapshot {
                link.commit_id.clone()
            } else {
                commit.commit_id.clone()
            };
            assert_eq!(now.commit_id, want, "link {link:?}");
            assert_eq!(now.change_id, link.change_id);
        }
    }
}

/// Runs, forks, landings and drops against the model: see the module
/// docs. Each case imports a project and runs up to 40 steps.
#[hegel::test(
    test_cases = 30,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn runs_behave_like_the_model(tc: TestCase) {
    hegel::stateful::machine(Machine::new()).steps(40).run(tc);
}

#[hegel::test(
    profile = "nightly_slow",
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
#[ignore = "nightly"]
fn runs_behave_like_the_model_nightly(tc: TestCase) {
    hegel::stateful::machine(Machine::new()).steps(50).run(tc);
}

/// Files written or deleted, in order.
type Edits = Vec<(&'static str, Val)>;

/// A turn's edits.
#[hegel::composite]
fn edits(tc: &TestCase) -> Edits {
    tc.draw(
        gs::vecs(gs::tuples!(
            gs::sampled_from(PATHS.to_vec()),
            gs::optional(gs::sampled_from(VALUES.to_vec()))
        ))
        .max_size(3),
    )
}

/// Landing, head on: a parent's turns, children forked at drawn links,
/// turns on every side, the children landing one after another in a
/// drawn order, and a last parent turn on top, against the same model.
/// The state machine reaches these steps only now and then.
#[hegel::test(
    test_cases = 30,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn children_land_like_the_model(tc: TestCase) {
    let mut m = Machine::new();
    for edits in tc.draw(gs::vecs(edits()).min_size(1).max_size(3)) {
        m.do_turn(&tc, 0, &edits);
    }
    let children = tc.draw(gs::integers::<usize>().min_value(1).max_value(2));
    for _ in 0..children {
        let count = m.runs[0].links.len();
        let k = tc.draw(gs::integers::<usize>().max_value(count - 1));
        m.do_fork(&tc, 0, k);
    }
    let kids: Vec<usize> = (1..=children).collect();
    // Turns in a drawn order: the parent's and its children's.
    let turns: Vec<(usize, Vec<(&'static str, Val)>)> = tc.draw(
        gs::vecs(gs::tuples!(
            gs::integers::<usize>().max_value(children),
            edits()
        ))
        .max_size(6),
    );
    for (run, edits) in turns {
        m.do_turn(&tc, run, &edits);
    }
    for &kid in &kids {
        if !m.runs[kid].bookmarked {
            m.do_turn(&tc, kid, &[]);
        }
    }
    for kid in tc.draw(gs::permutations(kids)) {
        m.do_land(&tc, kid);
        m.the_project_is_the_model(tc.clone());
    }
    let last = tc.draw(edits());
    m.do_turn(&tc, 0, &last);
    m.the_project_is_the_model(tc.clone());
}

/// Merging, head on: two top-level runs take turns, committed or left
/// in `@`; the first merges into trunk, so trunk moves on past the
/// second, which then moves onto trunk with whatever `@` holds, and
/// maybe merges too. A fork at one of its links after that starts on a
/// snapshot whose parent the move rewrote. Against the same model; the
/// state machine reaches these steps only now and then.
#[hegel::test(
    test_cases = 30,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn runs_merge_like_the_model(tc: TestCase) {
    let mut m = Machine::new();
    m.start_root();
    for run in [0, 1] {
        m.do_turn(&tc, run, &tc.draw(edits()));
        m.do_snapshot(&tc, run, &tc.draw(edits()));
    }
    let steps: Vec<(usize, bool, Edits)> = tc.draw(
        gs::vecs(gs::tuples!(
            gs::integers::<usize>().max_value(1),
            gs::booleans(),
            edits()
        ))
        .max_size(4),
    );
    for (run, commit, edits) in steps {
        if commit {
            m.do_turn(&tc, run, &edits);
        } else {
            m.do_snapshot(&tc, run, &edits);
        }
    }
    if !m.clean(0) {
        m.do_turn(&tc, 0, &[]);
    }
    m.do_move(&tc, 0, true);
    m.the_project_is_the_model(tc.clone());

    let merge = tc.draw(gs::booleans()) && m.clean(1);
    m.do_move(&tc, 1, merge);
    m.the_project_is_the_model(tc.clone());
    if m.runs[1].state == State::Open {
        let count = m.runs[1].links.len();
        let k = tc.draw(gs::integers::<usize>().max_value(count - 1));
        m.do_fork(&tc, 1, k);
        let fork = m.runs.len() - 1;
        m.do_turn(&tc, fork, &tc.draw(edits()));
        m.the_project_is_the_model(tc.clone());
    }
}

#[test]
fn rebasing_a_path_follows_jjs_trivial_merge() {
    let r = |v: &'static str| Term::resolved(Some(v));
    let none = Term::resolved(None);
    // One side changed: the change wins.
    assert_eq!(Term::rebase(&r("a"), &r("a"), &r("b")), r("b"));
    assert_eq!(Term::rebase(&r("b"), &r("a"), &r("a")), r("b"));
    // Both made the same change.
    assert_eq!(Term::rebase(&r("b"), &r("a"), &r("b")), r("b"));
    // Both changed it differently, or one deleted what the other changed.
    assert!(Term::rebase(&r("b"), &r("a"), &r("c")).value().is_none());
    assert!(Term::rebase(&none, &r("a"), &r("c")).value().is_none());
    // A conflict rebased onto one of its sides' bases resolves.
    let conflict = Term::rebase(&r("b"), &r("a"), &r("c"));
    assert_eq!(Term::rebase(&r("a"), &r("b"), &conflict), r("c"));
}

// Questions the model found, pinned as examples.

/// A landing reports `c.txt` as conflicted when the parent added it
/// empty and the child added it with text. jj writes the child's text
/// with no markers (it reads the missing base as empty, and that merges
/// cleanly), yet the tree keeps the conflict: `vcs_status` tells the
/// model to "edit the markers out" of a file that has none, and every
/// later commit carries the conflict until something rewrites the file.
///
/// Open question: ADR 0009 says the parent's model "edits the markers
/// out like any other file", which assumes markers. Should the landing
/// resolve conflicts that materialize cleanly, write markers anyway, or
/// the tools tell the model to rewrite such files? Until that is
/// decided, this expects markers and is ignored.
#[test]
#[ignore = "open question: a landed conflict that jj materializes without markers"]
fn a_landed_conflict_shows_markers() {
    let m = Machine::new();
    let trunk = m.commits[0].commit_id.clone();
    let parent = m.project.add_workspace("p", &trunk).unwrap();
    let child = m.project.add_workspace("c", &trunk).unwrap();
    let dir = m.project.workspace_dir("p");
    std::fs::write(dir.join("c.txt"), "").unwrap();
    block_on(parent.commit_all("p1", "tau/p")).unwrap();
    std::fs::write(m.project.workspace_dir("c").join("c.txt"), "two\n")
        .unwrap();
    let head = block_on(child.commit_all("c1", "tau/c")).unwrap();
    let landing =
        block_on(parent.land(&head.commit_id, "tau/p", true)).unwrap();
    assert_eq!(landing.conflicts, ["c.txt"]);
    let text = std::fs::read_to_string(dir.join("c.txt")).unwrap();
    assert!(text.contains("<<<<<<<"), "c.txt holds {text:?}");
}

/// The same question, other side: a landing leaves `dir/b.txt` in
/// conflict (the parent made it empty, the child deleted it), and jj
/// writes it empty, without markers. The parent's next turn touches
/// only `a.txt`, yet its commit resolves `dir/b.txt` to the empty file:
/// the conflict is decided by a turn that never looked at it. In
/// `a_landed_conflict_shows_markers` the same kind of conflict is kept
/// instead; which one happens depends on jj's working-copy state. And
/// a turn that writes exactly the text jj wrote for such a conflict
/// keeps the conflict (the file matches its materialized form), so the
/// right answer cannot resolve it: only different text does.
#[test]
#[ignore = "open question: a landed conflict that jj materializes without markers"]
fn a_turn_keeps_a_conflict_it_did_not_touch() {
    let m = Machine::new();
    let trunk = m.commits[0].commit_id.clone();
    let parent = m.project.add_workspace("p", &trunk).unwrap();
    let dir = m.project.workspace_dir("p");
    std::fs::write(dir.join("a.txt"), "").unwrap();
    let turn = block_on(parent.commit_all("p1", "tau/p")).unwrap();
    let child = m.project.add_workspace("c", &turn.commit_id).unwrap();
    let child_dir = m.project.workspace_dir("c");
    std::fs::write(dir.join("dir/b.txt"), "").unwrap();
    std::fs::remove_file(dir.join("a.txt")).unwrap();
    block_on(parent.commit_all("p2", "tau/p")).unwrap();
    std::fs::write(child_dir.join("a.txt"), "one\n").unwrap();
    block_on(child.commit_all("c1", "tau/c")).unwrap();
    std::fs::remove_file(child_dir.join("dir/b.txt")).unwrap();
    let head = block_on(child.commit_all("c2", "tau/c")).unwrap();
    let landing =
        block_on(parent.land(&head.commit_id, "tau/p", true)).unwrap();
    assert_eq!(landing.conflicts, ["a.txt", "dir/b.txt"]);
    std::fs::remove_file(dir.join("a.txt")).unwrap();
    let turn = block_on(parent.commit_all("p3", "tau/p")).unwrap();
    assert_eq!(turn.paths, ["a.txt"], "the turn only deleted a.txt");
}

/// Merging a top-level run whose idle fork still has a workspace: the
/// host allows it (`Host::merge_plan` refuses only children holding
/// changes the run lacks), trunk has moved on, so the merge rewrites
/// the run's commits, and jj rebases the fork's `@` along with them.
/// The fork's workspace is then stale: its next turn fails with "The
/// working copy is stale", and its parent is closed.
///
/// Open question: should the host refuse the merge while a child has a
/// workspace, or the merge bring the children's workspaces along? A
/// landing can do the same to an idle grandchild. Until that is
/// decided, this expects the fork to go on and is ignored.
#[test]
#[ignore = "open question: a merge rewrites the commits under an idle fork's @"]
fn a_merge_leaves_an_open_fork_stale() {
    let m = Machine::new();
    let trunk = m.commits[0].commit_id.clone();
    let run = m.project.add_workspace("r", &trunk).unwrap();
    let dir = m.project.workspace_dir("r");
    std::fs::write(dir.join("a.txt"), "two\n").unwrap();
    let turn = block_on(run.commit_all("r1", "tau/r")).unwrap();
    // A fork at that turn, idle, with nothing of its own.
    let fork = m.project.add_workspace("f", &turn.commit_id).unwrap();
    // Trunk moves on.
    let other = m.project.add_workspace("o", &trunk).unwrap();
    std::fs::write(m.project.workspace_dir("o").join("c.txt"), "one\n")
        .unwrap();
    let moved = block_on(other.commit_all("o1", "tau/o")).unwrap();
    m.project.fast_forward_trunk(&moved.commit_id).unwrap();
    // The run merges onto it.
    let merged =
        block_on(run.move_onto(moved.commit_id.clone(), "tau/r", true))
            .unwrap();
    assert!(merged.conflicts.is_empty());
    m.project.fast_forward_trunk(&merged.head).unwrap();
    // The fork's next turn.
    block_on(fork.end_turn("tau/f", None)).unwrap();
}
