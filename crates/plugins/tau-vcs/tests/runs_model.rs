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
//!   it uncommitted, listing the paths changed since the last snapshot,
//!   with what each landing and catch-up since brought to the run's
//!   newest commit applied on top;
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

mod common;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use common::merge::*;
use hegel::{TestCase, generators as gs};
use tau_testing::{block_on, git::git};
use tau_vcs::{DEFAULT_WORKSPACE, Identity, Link, Project, UpdateFrom, Vcs};

const PATHS: [&str; 3] = ["a.txt", "c.txt", "dir/b.txt"];
const VALUES: [&str; 4] = ["", "one\n", "two\n", "three\n"];
const MAX_RUNS: usize = 6;

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
    /// Its workspace: `default` for the main chat.
    name: String,
    /// The bookmark its commits move: trunk's for the main chat.
    bookmark: String,
    parent: Option<usize>,
    /// The run's newest commit.
    head: usize,
    /// The files on disk, committed or not.
    wc: Tree,
    /// `@`'s tree at the last snapshot or checkout, as jj has it.
    seen: Tree,
    /// `@`'s tree as another workspace's operation rewrote it, until
    /// this workspace's next tool brings the files there.
    stale: Option<Tree>,
    /// Its links.
    links: Vec<Linked>,
    /// The last turn's snapshot and its tree, which the next turn's
    /// paths are counted from, as `RunWorkspace` keeps it: a fork's `@`
    /// as it started, until its first turn.
    since: Option<(String, Tree)>,
    /// How the run's newest commit moved under it since that snapshot,
    /// oldest first, by a landing or a catch-up: its tree before and
    /// after. A turn's paths leave out what each brought.
    moves: Vec<(Tree, Tree)>,
    turns: u32,
    state: State,
    /// A turn has set `tau/<name>`.
    bookmarked: bool,
    vcs: Vcs,
}

struct Machine {
    home: tempfile::TempDir,
    project: Project,
    commits: Vec<Commit>,
    /// The main chat first, then its chats.
    runs: Vec<Run>,
    /// The commit trunk's bookmark names.
    trunk: usize,
    /// The source checkout's `HEAD`, as the project last imported it.
    upstream: usize,
}

/// The main chat.
const MAIN: usize = 0;

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
            upstream: 0,
        };
        // The repository's main chat, in jj's own workspace, committing
        // on trunk (ADR 0015). The workspace starts empty, on jj's root
        // commit: the host's catch-up before its first turn moves it onto
        // trunk.
        let dir = machine.project.workspace_dir(DEFAULT_WORKSPACE);
        let vcs = Vcs::open(&dir, Identity::default()).unwrap();
        let trunk = machine.commits[0].commit_id.clone();
        let name = machine.project.trunk_name().unwrap();
        let moved = block_on(vcs.move_onto(trunk.clone(), name, true)).unwrap();
        assert_eq!(moved.head, trunk);
        let tree = machine.commits[0].tree.clone();
        machine.runs.push(Run {
            name: DEFAULT_WORKSPACE.to_owned(),
            bookmark: machine.project.trunk_name().unwrap(),
            parent: None,
            head: 0,
            wc: tree.clone(),
            seen: tree,
            stale: None,
            links: Vec::new(),
            moves: Vec::new(),
            since: None,
            turns: 0,
            state: State::Open,
            bookmarked: true,
            vcs,
        });
        machine
    }

    fn src(&self) -> std::path::PathBuf {
        self.home.path().join("src")
    }

    fn open(&self) -> Vec<usize> {
        (0..self.runs.len())
            .filter(|&r| self.runs[r].state == State::Open)
            .collect()
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
    /// `a_markerless_landed_conflict_can_be_explicitly_accepted`).
    fn check_files(&self, run: usize) {
        let disk = self.disk(run);
        let r = &self.runs[run];
        let tree = &r.wc;
        // Pending writes have not reached the repository. A stale workspace
        // has the rewritten repository tree but still the old disk bytes.
        let stored = r.stale.as_ref().unwrap_or(&r.seen);
        let id = self.project.workspace_head(&r.name).unwrap().unwrap();
        assert_eq!(
            self.project.conflicts(&id).unwrap(),
            conflicts(stored),
            "stored conflicts in {}",
            r.name
        );
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
            .remove_bookmark(&self.runs[run].bookmark.clone())
            .unwrap();
        assert!(!self.project.workspace_dir(&name).exists());
        assert_eq!(
            self.project
                .bookmark(&self.runs[run].bookmark.clone())
                .unwrap(),
            None
        );
        self.runs[run].state = state;
        self.runs[run].bookmarked = false;
    }

    /// Whether `run` has no uncommitted work: a run commits what it
    /// leaves when it finishes.
    fn clean(&self, run: usize) -> bool {
        let run = &self.runs[run];
        run.wc == run.seen
            && run.stale.as_ref().unwrap_or(&run.seen)
                == &self.commits[run.head].tree
    }

    /// A chat that can land on the main chat or be dropped: open or
    /// forgotten, and finished. Runs nest one level (ADR 0016), so no
    /// chat has runs under it.
    fn closable(&self) -> Vec<usize> {
        (0..self.runs.len())
            .filter(|&r| {
                r != MAIN
                    && matches!(
                        self.runs[r].state,
                        State::Open | State::Forgotten
                    )
                    && self.clean(r)
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
        if get(&tree, path).value().is_none() {
            tc.event("a pending edit touches a conflict");
        }
        set(&mut tree, path, Term::resolved(*value));
    }
    tree
}

/// Snapshot intent comes from final bytes, not transient writes. Deleting
/// and recreating a file before a native snapshot is not explicit acceptance.
/// This prediction uses only the last observed reference tree and edits.
fn retain_same_materializations(
    tc: &TestCase,
    seen: &Tree,
    candidate: &mut Tree,
) {
    for path in PATHS {
        let old = get(seen, path);
        if old.value().is_none()
            && let Some(value) = old.materialized()
            && get(candidate, path).value() == Some(value)
        {
            tc.event("same final materialized bytes keep the conflict");
            set(candidate, path, old);
        }
    }
}

/// Listed paths follow the independent tree model exactly. Cancelling
/// redundant native merge terms is not a change to an untouched conflict.
fn check_paths(listed: &[String], base: &Tree, tree: &Tree, what: &str) {
    let mut expected: Vec<String> = PATHS
        .iter()
        .copied()
        .filter(|path| get(base, path) != get(tree, path))
        .map(str::to_owned)
        .collect();
    expected.sort_unstable();
    assert_eq!(listed, expected, "{what}: changed paths");
}

impl Machine {
    /// `edits` to `run`'s files, then what its next tool finds there:
    /// when another workspace's operation rewrote its `@`, the files move
    /// to the rewritten tree first, with what was edited since the last
    /// snapshot merged on top, as a rebase would
    /// (`session::snapshot_locked`). Predicts the tree independently:
    /// untouched and same-byte conflicts keep their original terms;
    /// no observed SUT conflict set changes the expected tree.
    fn edit_then_freshen(
        &mut self,
        tc: &TestCase,
        run: usize,
        edits: &[(&'static str, Val)],
    ) -> Tree {
        let dir = self.project.workspace_dir(&self.runs[run].name);
        let r = &mut self.runs[run];
        let mut tree = write_edits(tc, &dir, &r.wc.clone(), edits);
        retain_same_materializations(tc, &r.seen, &mut tree);
        let Some(stale) = r.stale.take() else {
            return tree;
        };
        tc.event("a stale workspace catches up");
        if tree != r.seen {
            tc.event("edits on a stale workspace merge onto the rewrite");
        }
        // jj now sees the rewritten tree.
        rebase_tree(&stale, &r.seen, &tree)
    }

    /// A turn in `run`: `edits` to its files, then a commit of all of
    /// `@`, as a run's model makes one.
    fn do_turn(
        &mut self,
        tc: &TestCase,
        run: usize,
        edits: &[(&'static str, Val)],
    ) {
        let head = self.runs[run].head;
        let tree = self.edit_then_freshen(tc, run, edits);
        self.runs[run].turns += 1;
        let r = &self.runs[run];
        let turn = block_on(r.vcs.commit_all(
            format!("tau: run {} turn {}", r.name, r.turns),
            r.bookmark.clone(),
        ))
        .unwrap();
        self.runs[run].wc = tree.clone();
        self.runs[run].seen = tree.clone();
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
        if run == MAIN {
            // The main chat commits on trunk.
            self.trunk = at;
        }
        let r = &mut self.runs[run];
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
        let head = self.runs[run].head;
        let tree = self.edit_then_freshen(tc, run, edits);
        let r = &self.runs[run];
        let since = r.since.clone();
        let snapshot = block_on(r.vcs.end_turn(
            r.bookmark.clone(),
            since.as_ref().map(|(id, ..)| id.clone()),
        ))
        .unwrap();
        // The paths count from the turn before's snapshot with what each
        // landing and catch-up since brought applied, else from the
        // run's head.
        let base = match since {
            Some((_, tree)) => {
                let mut base = tree;
                for (from, to) in &self.runs[run].moves {
                    if from != to {
                        tc.event("a turn leaves out what landed or caught up");
                    }
                    base = rebase_tree(to, from, &base);
                }
                base
            }
            None => self.commits[head].tree.clone(),
        };
        check_paths(&snapshot.paths, &base, &tree, "the snapshot");
        assert_eq!(snapshot.head, self.commits[head].commit_id);
        if tree != self.commits[head].tree {
            tc.event("a turn leaves work uncommitted");
        }

        let head_tree = self.commits[head].tree.clone();
        let r = &mut self.runs[run];
        r.wc = tree.clone();
        r.seen = tree.clone();
        r.turns += 1;
        r.bookmarked = true;
        r.moves.clear();
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
        let name_of_fork = name.clone();
        self.runs.push(Run {
            bookmark: format!("tau/{name}"),
            name,
            parent: Some(parent),
            head: linked.at,
            wc: self.fork_tree(&linked),
            seen: self.fork_tree(&linked),
            stale: None,
            links,
            moves: Vec::new(),
            // `RunWorkspace` counts a fork's first turn from its `@` as
            // it started.
            since: Some((
                self.project.workspace_head(&name_of_fork).unwrap().unwrap(),
                self.fork_tree(&linked),
            )),
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

    /// A chat started before the main chat has a turn: on trunk, as the
    /// host starts one.
    fn do_start(&mut self, tc: &TestCase) {
        tc.event("a chat starts on trunk");
        let name = format!("r{}", self.runs.len());
        let trunk = self.commits[self.trunk].commit_id.clone();
        let vcs = self.project.add_workspace(&name, &trunk).unwrap();
        self.runs.push(Run {
            bookmark: format!("tau/{name}"),
            name,
            parent: Some(MAIN),
            head: self.trunk,
            wc: self.commits[self.trunk].tree.clone(),
            seen: self.commits[self.trunk].tree.clone(),
            stale: None,
            links: Vec::new(),
            moves: Vec::new(),
            since: None,
            turns: 0,
            state: State::Open,
            bookmarked: false,
            vcs,
        });
        self.check_files(self.runs.len() - 1);
    }

    /// Lands `child` on its parent, previewed first, then closes it.
    fn do_land(&mut self, tc: &TestCase, child: usize) {
        let parent = self.runs[child].parent.unwrap();
        let child_head = self
            .project
            .bookmark(&self.runs[child].bookmark.clone())
            .unwrap()
            .expect("a child's bookmark");
        assert_eq!(child_head, self.commits[self.runs[child].head].commit_id);
        let p = &self.runs[parent];
        let files = self.disk(parent);
        let bookmark = self.project.bookmark(&p.bookmark.clone()).unwrap();
        let wc = self.wc_parent(parent);

        let preview =
            block_on(p.vcs.land(&child_head, p.bookmark.clone(), false))
                .unwrap();
        let p = &self.runs[parent];
        assert_eq!(self.disk(parent), files, "a preview changed files");
        assert_eq!(
            self.project.bookmark(&p.bookmark.clone()).unwrap(),
            bookmark
        );
        // A preview snapshots `@`, which may rewrite it; it stays put.
        assert_eq!(self.wc_parent(parent), wc);

        let landing =
            block_on(p.vcs.land(&child_head, p.bookmark.clone(), true))
                .unwrap();

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
        p.seen = p.wc.clone();
        if parent == MAIN {
            // Landing on the main chat moves trunk.
            self.trunk = head;
        }
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
                    .bookmark(&self.runs[parent].bookmark.clone())
                    .unwrap(),
                Some(landing.head.clone()),
                "the parent's bookmark"
            );
            self.runs[parent].bookmarked = true;
        }

        let new_head_tree = self.commits[self.runs[parent].head].tree.clone();
        self.runs[parent]
            .moves
            .push((old_head_tree.clone(), new_head_tree));
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

    /// The main chat catches up with trunk, as the host has it before
    /// each of its turns and before a chat lands on it: its changes, up
    /// to `@`, move onto trunk's head, previewed first, and trunk follows
    /// them. Whatever stands on the commits it rewrote follows too: a
    /// chat's `@` moves with them, and its files on its next tool.
    fn do_catch_up(&mut self, tc: &TestCase) {
        let trunk = self.trunk;
        let head = self.runs[MAIN].head;
        let trunk_id = self.commits[trunk].commit_id.clone();
        let r = &mut self.runs[MAIN];
        retain_same_materializations(tc, &r.seen, &mut r.wc);
        let r = &self.runs[MAIN];
        let files = self.disk(MAIN);
        let wc = self.wc_parent(MAIN);
        let preview = block_on(r.vcs.move_onto(
            trunk_id.clone(),
            r.bookmark.clone(),
            false,
        ))
        .unwrap();
        assert_eq!(self.disk(MAIN), files, "a preview changed files");
        assert_eq!(
            self.project.trunk().unwrap(),
            trunk_id,
            "a preview moved trunk"
        );
        assert_eq!(self.wc_parent(MAIN), wc, "a preview moved @");
        // The preview snapshots without deciding untouched conflicts.
        // The reference tree does not depend on the observed conflict set.
        let r = &mut self.runs[MAIN];
        r.seen = r.wc.clone();
        let r = &self.runs[MAIN];
        let moved =
            block_on(r.vcs.move_onto(trunk_id, r.bookmark.clone(), true))
                .unwrap();

        // The model's catch-up.
        let chain = self.own_changes(head, trunk);
        let old_ids: Vec<String> = chain
            .iter()
            .map(|&at| self.commits[at].commit_id.clone())
            .collect();
        let before: Vec<Tree> =
            self.commits.iter().map(|c| c.tree.clone()).collect();
        let (rewrite, dragged) = self.restack(&chain, trunk);
        match (chain.is_empty(), rewrite) {
            (true, _) if head == trunk => {
                tc.event("a catch-up with nothing to move")
            }
            (true, _) => tc.event("a catch-up moves @ alone onto trunk"),
            (false, false) => tc.event("a catch-up that rewrites nothing"),
            (false, true) => tc.event("a catch-up that restacks"),
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
            assert_eq!(now, old_ids, "a main chat on trunk's head moved");
        }
        self.learn(&dragged);
        let new_head = chain.last().copied().unwrap_or(trunk);
        let r = &mut self.runs[MAIN];
        r.moves
            .push((before[head].clone(), self.commits[new_head].tree.clone()));
        r.head = new_head;
        r.wc = rebase_tree(&self.commits[new_head].tree, &before[head], &r.wc);
        r.seen = r.wc.clone();
        self.trunk = new_head;

        // The chats on what it rewrote: their `@` moved with it.
        let rewritten: BTreeSet<usize> = if rewrite {
            chain.iter().chain(&dragged).copied().collect()
        } else {
            BTreeSet::new()
        };
        for run in 1..self.runs.len() {
            let r = &self.runs[run];
            if r.state != State::Open || !rewritten.contains(&r.head) {
                continue;
            }
            tc.event("a catch-up moves a chat's @");
            let moved =
                (before[r.head].clone(), self.commits[r.head].tree.clone());
            // jj's `@`: the last snapshot, or what rewrote it before.
            let repo_wc = r.stale.clone().unwrap_or_else(|| r.seen.clone());
            let stale = rebase_tree(
                &self.commits[r.head].tree,
                &before[r.head],
                &repo_wc,
            );
            self.runs[run].stale = Some(stale);
            self.runs[run].moves.push(moved);
        }

        // The conflicts in the new head, then any more in `@`.
        let mut want = conflicts(&self.commits[new_head].tree);
        for path in conflicts(&self.runs[MAIN].wc) {
            if !want.contains(&path) {
                tc.event("a catch-up conflicts in @ alone");
                want.push(path);
            }
        }
        if !want.is_empty() {
            tc.event("a catch-up conflicts");
        }
        assert_eq!(moved.conflicts, want, "the catch-up's conflicts");
        assert_eq!(moved.head, self.commits[new_head].commit_id);
        for &at in chain.iter().chain(&dragged) {
            self.check_commit(at);
        }
        assert_eq!(
            self.project.trunk().unwrap(),
            moved.head,
            "trunk follows the main chat"
        );
    }

    /// The source moves on: `edits` committed in the user's checkout,
    /// then brought in with `Project::update`. Trunk takes the source's
    /// branch, even when the main chat has moved it too.
    fn do_upstream(&mut self, tc: &TestCase, edits: &[(&'static str, Val)]) {
        let src = self.src();
        let tree =
            write_edits(tc, &src, &self.commits[self.upstream].tree, edits);
        git(&src, &["add", "-A"]);
        git(
            &src,
            &["commit", "--quiet", "--allow-empty", "-m", "upstream"],
        );
        let id = git(&src, &["rev-parse", "HEAD"]);
        self.commits.push(Commit {
            change_id: None,
            commit_id: id.clone(),
            parent: Some(self.upstream),
            tree,
            abandoned: false,
        });
        let at = self.commits.len() - 1;
        if self.trunk != self.upstream {
            tc.event("an update under the main chat's commits");
        }
        let before = self.commits[self.trunk].commit_id.clone();
        let updated = self.project.update(UpdateFrom::Checkout(&src)).unwrap();
        assert_eq!((updated.before, updated.after), (before, id));
        self.upstream = at;
        self.trunk = at;
        self.check_commit(at);
    }
}

#[hegel::state_machine]
impl Machine {
    /// A turn: edits in one run's files, then a commit of all of them.
    /// The main chat catches up with trunk first, as the host has it.
    #[rule(weight = 6)]
    fn turn(&mut self, tc: TestCase) {
        let open = self.open();
        let run = tc.draw(gs::sampled_from(open));
        let edits = tc.draw(edits());
        if run == MAIN {
            self.do_catch_up(&tc);
        }
        self.do_turn(&tc, run, &edits);
    }

    /// A turn as `RunWorkspace` ends one: edits in one run's files,
    /// snapshotted and left uncommitted.
    #[rule(weight = 3)]
    fn snapshot_turn(&mut self, tc: TestCase) {
        let open = self.open();
        let run = tc.draw(gs::sampled_from(open));
        let edits = tc.draw(edits());
        if run == MAIN {
            self.do_catch_up(&tc);
        }
        self.do_snapshot(&tc, run, &edits);
    }

    /// Edits in one run's files, with no tool run after them.
    #[rule(weight = 2)]
    fn write(&mut self, tc: TestCase) {
        let open = self.open();
        let run = tc.draw(gs::sampled_from(open));
        let edits = tc.draw(edits());
        let dir = self.project.workspace_dir(&self.runs[run].name);
        let r = &mut self.runs[run];
        r.wc = write_edits(&tc, &dir, &r.wc, &edits);
    }

    /// A chat of the main chat, the only run that forks (ADR 0016): at
    /// one of its links, as `RunWorkspace` starts one, or on trunk before
    /// it has any.
    #[rule(weight = 3)]
    fn fork(&mut self, tc: TestCase) {
        tc.assume(self.runs.len() < MAX_RUNS);
        let count = self.runs[MAIN].links.len();
        if count == 0 {
            self.do_start(&tc);
            return;
        }
        let k = tc.draw(gs::integers::<usize>().max_value(count - 1));
        self.do_fork(&tc, MAIN, k);
    }

    /// New commits in the source, brought in by an update.
    #[rule(weight = 2)]
    fn upstream(&mut self, tc: TestCase) {
        let edits = tc.draw(edits());
        self.do_upstream(&tc, &edits);
    }

    /// Lands a chat on the main chat, previewed first, then closes it.
    /// The main chat catches up with trunk first, as the host has it.
    #[rule(weight = 3)]
    fn land(&mut self, tc: TestCase) {
        let candidates: Vec<usize> = self
            .closable()
            .into_iter()
            .filter(|&r| self.runs[r].bookmarked)
            .collect();
        tc.assume(!candidates.is_empty());
        let child = tc.draw(gs::sampled_from(candidates));
        self.do_catch_up(&tc);
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
                .bookmark(&self.runs[child].bookmark.clone())
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
            .filter(|&r| r != MAIN && self.clean(r))
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
        // The main chat works in jj's own workspace, which is not a run's.
        let mut open: Vec<String> = self
            .open()
            .into_iter()
            .filter(|&r| r != MAIN)
            .map(|r| self.runs[r].name.clone())
            .collect();
        open.sort();
        assert_eq!(self.project.workspaces().unwrap(), open, "workspaces");
        let trunk = &self.commits[self.trunk].commit_id;
        assert_eq!(&self.project.trunk().unwrap(), trunk, "trunk");

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
            let bookmark =
                self.project.bookmark(&run.bookmark.clone()).unwrap();
            // The main chat's bookmark is trunk's, which an update moves
            // without it.
            let want = match run.state {
                _ if r == MAIN => Some(trunk.clone()),
                State::Open | State::Forgotten if run.bookmarked => {
                    Some(head.commit_id.clone())
                }
                _ => None,
            };
            assert_eq!(bookmark, want, "{}'s bookmark", run.bookmark.clone());
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
    tc.draw(some_edits(0))
}

/// At least `min` edits.
#[hegel::composite]
fn some_edits(tc: &TestCase, min: usize) -> Edits {
    tc.draw(
        gs::vecs(gs::tuples!(
            gs::sampled_from(PATHS.to_vec()),
            gs::optional(gs::sampled_from(VALUES.to_vec()))
        ))
        .min_size(min)
        .max_size(3),
    )
}

/// Landing, head on: the main chat's turns, chats forked at drawn links,
/// turns on every side, the chats landing one after another in a drawn
/// order, and a last main chat turn on top, against the same model. The
/// state machine reaches these steps only now and then.
#[hegel::test(
    test_cases = 30,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn chats_land_like_the_model(tc: TestCase) {
    let mut m = Machine::new();
    for edits in tc.draw(gs::vecs(edits()).min_size(1).max_size(3)) {
        m.do_turn(&tc, MAIN, &edits);
    }
    let chats = tc.draw(gs::integers::<usize>().min_value(1).max_value(2));
    for _ in 0..chats {
        let count = m.runs[MAIN].links.len();
        let k = tc.draw(gs::integers::<usize>().max_value(count - 1));
        m.do_fork(&tc, MAIN, k);
    }
    let ids: Vec<usize> = (1..=chats).collect();
    // Turns in a drawn order: the main chat's and its chats'.
    let turns: Vec<(usize, Edits)> = tc.draw(
        gs::vecs(gs::tuples!(
            gs::integers::<usize>().max_value(chats),
            edits()
        ))
        .max_size(6),
    );
    for (run, edits) in turns {
        m.do_turn(&tc, run, &edits);
    }
    for &chat in &ids {
        if !m.runs[chat].bookmarked {
            m.do_turn(&tc, chat, &[]);
        }
    }
    for chat in tc.draw(gs::permutations(ids)) {
        m.do_catch_up(&tc);
        m.do_land(&tc, chat);
        m.the_project_is_the_model(tc.clone());
    }
    let last = tc.draw(edits());
    m.do_turn(&tc, MAIN, &last);
    m.the_project_is_the_model(tc.clone());
}

/// Updating, head on: the main chat takes turns, committed or left in
/// `@`; chats fork it and work; the source moves on and an update brings
/// it in, so trunk moves without the main chat; its next turn catches
/// up, restacking its commits and the chats standing on them. Each chat
/// then works on, maybe after edits made while it was stale, and lands.
/// A fork at an older link starts on a snapshot whose parent the
/// catch-up rewrote. Against the same model; the state machine reaches
/// these steps only now and then.
#[hegel::test(
    test_cases = 30,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn chats_follow_an_update_like_the_model(tc: TestCase) {
    let mut m = Machine::new();
    // A commit first, so the update has the main chat's own commits to
    // go under.
    m.do_turn(&tc, MAIN, &tc.draw(some_edits(1)));
    let main_turns: Vec<(bool, Edits)> =
        tc.draw(gs::vecs(gs::tuples!(gs::booleans(), edits())).max_size(3));
    for (commit, edits) in main_turns {
        if commit {
            m.do_turn(&tc, MAIN, &edits);
        } else {
            m.do_snapshot(&tc, MAIN, &edits);
        }
    }
    let chats = tc.draw(gs::integers::<usize>().min_value(1).max_value(2));
    for _ in 0..chats {
        let count = m.runs[MAIN].links.len();
        let k = tc.draw(gs::integers::<usize>().max_value(count - 1));
        m.do_fork(&tc, MAIN, k);
        let chat = m.runs.len() - 1;
        if tc.draw(gs::booleans()) {
            m.do_turn(&tc, chat, &tc.draw(edits()));
        }
    }
    m.do_upstream(&tc, &tc.draw(some_edits(1)));
    m.the_project_is_the_model(tc.clone());
    m.do_catch_up(&tc);
    m.do_snapshot(&tc, MAIN, &tc.draw(edits()));
    m.the_project_is_the_model(tc.clone());

    let ids: Vec<usize> = (1..=chats).collect();
    for &chat in &ids {
        // Edits made before the chat's next tool runs.
        if tc.draw(gs::booleans()) {
            let edits = tc.draw(edits());
            let dir = m.project.workspace_dir(&m.runs[chat].name);
            let r = &mut m.runs[chat];
            r.wc = write_edits(&tc, &dir, &r.wc, &edits);
        }
        m.do_turn(&tc, chat, &tc.draw(edits()));
        m.the_project_is_the_model(tc.clone());
    }
    if m.runs.len() < MAX_RUNS {
        // At a snapshot, when there is one: its parent may be a commit the
        // catch-up rewrote.
        let snapshots: Vec<usize> = (0..m.runs[MAIN].links.len())
            .filter(|&k| m.runs[MAIN].links[k].link.snapshot)
            .collect();
        let k = if snapshots.is_empty() {
            let count = m.runs[MAIN].links.len();
            tc.draw(gs::integers::<usize>().max_value(count - 1))
        } else {
            tc.draw(gs::sampled_from(snapshots))
        };
        m.do_fork(&tc, MAIN, k);
        m.the_project_is_the_model(tc.clone());
    }
    for chat in tc.draw(gs::permutations(ids)) {
        m.do_catch_up(&tc);
        m.do_land(&tc, chat);
        m.the_project_is_the_model(tc.clone());
    }
}

/// Retain the minimized native history where catch-up reordered two
/// markerless conflict addends without editing or deciding the file.
#[test]
fn a_catchup_snapshot_does_not_count_reordered_conflict_terms() {
    hegel::Hegel::new(|tc| {
        let mut m = Machine::new();
        m.do_turn(&tc, MAIN, &[("a.txt", None)]);
        m.do_snapshot(&tc, MAIN, &[("dir/b.txt", Some(""))]);
        m.do_fork(&tc, MAIN, 0);
        m.do_upstream(&tc, &[("dir/b.txt", None)]);
        m.the_project_is_the_model(tc.clone());
        m.do_catch_up(&tc);
        m.do_snapshot(&tc, MAIN, &[]);
        m.the_project_is_the_model(tc);
    })
    .settings(hegel::Settings::new().test_cases(1))
    .run();
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

/// Keep jj's markerless materialization and structured conflict until
/// an explicit acceptance, rather than inventing markers or deciding
/// file presence from unchanged bytes.
#[test]
fn a_markerless_landed_conflict_can_be_explicitly_accepted() {
    use serde_json::json;
    use tau_agent::{plugin::Plugin as _, tool::ToolCtx};
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
    assert_eq!(text, "two\n");
    for transient_delete in [false, true] {
        if transient_delete {
            std::fs::remove_file(dir.join("c.txt")).unwrap();
        }
        std::fs::write(dir.join("c.txt"), "two\n").unwrap();
        let copy = block_on(parent.working_copy()).unwrap();
        assert!(copy.paths.is_empty(), "same final bytes are not acceptance");
        let wc = m.project.workspace_head("p").unwrap().unwrap();
        assert_eq!(m.project.conflicts(&wc).unwrap(), ["c.txt"]);
    }
    let tool = tau_vcs::VcsPlugin::new(parent.clone())
        .tools()
        .into_iter()
        .find(|tool| tool.name() == "vcs_resolve")
        .unwrap();
    block_on(tool.call(json!({"paths": ["c.txt"]}), ToolCtx::detached()))
        .unwrap();
    let turn =
        block_on(parent.commit_all("accept markerless contents", "tau/p"))
            .unwrap();
    assert_eq!(turn.paths, ["c.txt"]);
    assert!(m.project.conflicts(&turn.commit_id).unwrap().is_empty());
    assert_eq!(std::fs::read(dir.join("c.txt")).unwrap(), b"two\n");
}

/// Resolving a.txt cannot decide dir/b.txt's empty-v-deleted conflict.
/// Its native markerless materialization is not an implicit approval.
#[test]
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
    assert_eq!(m.project.conflicts(&turn.commit_id).unwrap(), ["dir/b.txt"]);
}
