//! Turns, the model's vcs tools and forks through the real plugins
//! (`docs/reference/vcs.md`, "Runs and turns"): an agent with
//! `RunWorkspace`, `VcsPlugin` and the coding tools runs drawn turns
//! that write files and call `vcs_commit`, `vcs_new`, `vcs_restore` and
//! `vcs_undo`, then ends normally, at a turn limit, or failing. A fork
//! at a drawn turn continues from it with `Checkpoint::at`, as the host
//! forks.
//!
//! The model is a stack of changes, each an abstract change id, a
//! description and a file tree, with `@` on top, and the tools'
//! operations for `vcs_undo` (as `tests/model.rs` has them). What it
//! holds to:
//! - each turn links a snapshot of `@` as the turn left it: its files,
//!   its parent's files and its change id;
//! - `changed`, and the paths observers hear, are what the turn changed
//!   since the turn before, or since the run started for the first: the
//!   run's own undos and commits are the turn's;
//! - a run that would stop with changes in `@` is held once; what is left
//!   at the end of a normal run or one at a limit is committed with the
//!   model's message, and a failed run keeps it in `@`;
//! - the run's bookmark names its newest commit, and its stack is the
//!   model's;
//! - a fork at a turn starts on exactly that turn's files, on that turn's
//!   parent, and the two runs never see each other's edits.

mod common;

use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex},
};

use common::{coder, links, project};
use hegel::{TestCase, generators as gs};
use serde_json::json;
use tau_agent::{agent::Checkpoint, limits::Limits};
use tau_store::Store;
use tau_testing::scripted::ScriptedModel;
use tau_vcs::{
    Identity,
    Link,
    Project,
    RunWorkspace,
    run_workspace::{PLUGIN, bookmark},
};

const PATHS: [&str; 3] = ["a.txt", "b.txt", "dir/c.txt"];
const VALUES: [&str; 3] = ["", "one\n", "two\n"];
/// What `vcs_restore` names: a file, or everything.
const RESTORE: [&str; 4] = ["a.txt", "b.txt", "dir/c.txt", "."];

type Tree = BTreeMap<&'static str, &'static str>;

fn trunk_tree() -> Tree {
    Tree::from([("a.txt", "one\n")])
}

fn value(text: &[u8]) -> &'static str {
    VALUES
        .iter()
        .find(|v| v.as_bytes() == text)
        .unwrap_or_else(|| panic!("unexpected contents {text:?}"))
}

fn files(dir: &Path) -> Tree {
    PATHS
        .iter()
        .filter_map(|path| {
            let bytes = std::fs::read(dir.join(path)).ok()?;
            Some((*path, value(&bytes)))
        })
        .collect()
}

fn at_commit(project: &Project, commit: &str) -> Tree {
    PATHS
        .iter()
        .filter_map(|path| {
            let (bytes, _) = project.file_at(commit, path).unwrap()?;
            Some((*path, value(&bytes)))
        })
        .collect()
}

/// The paths whose contents differ between two trees.
fn changed_paths(from: &Tree, to: &Tree) -> Vec<String> {
    PATHS
        .iter()
        .filter(|path| from.get(*path) != to.get(*path))
        .map(|path| (*path).to_owned())
        .collect()
}

/// What a turn's model does, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Write(&'static str, &'static str),
    Commit,
    New,
    Restore(&'static str),
    Undo,
}
hegel::pretty_print_as_debug!(Action);

/// How the run ends after its drawn turns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    /// The model answers; held once if `@` has changes.
    Answer,
    /// The turn limit stops it after the drawn turns.
    Limit,
    /// The connection drops on the turn after them.
    Fail,
}
hegel::pretty_print_as_debug!(End);

#[hegel::composite]
fn action(tc: &TestCase) -> Action {
    match tc.draw(gs::integers::<u8>().max_value(9)) {
        0..=3 => Action::Write(
            tc.draw(gs::sampled_from(PATHS.to_vec())),
            tc.draw(gs::sampled_from(VALUES.to_vec())),
        ),
        4 | 5 => Action::Commit,
        6 => Action::New,
        7 => Action::Restore(tc.draw(gs::sampled_from(RESTORE.to_vec()))),
        _ => Action::Undo,
    }
}

/// A turn's actions. Writes run side by side when no vcs tool is in the
/// batch, so such a turn writes each path once.
#[hegel::composite]
fn turn(tc: &TestCase) -> Vec<Action> {
    let mut actions: Vec<Action> = tc.draw(gs::vecs(action()).max_size(4));
    if actions.iter().all(|a| matches!(a, Action::Write(..))) {
        let mut seen = Vec::new();
        actions.retain(|a| match a {
            Action::Write(path, _) if seen.contains(path) => false,
            Action::Write(path, _) => {
                seen.push(*path);
                true
            }
            _ => true,
        });
    }
    actions
}

/// One change: an abstract change id, a description and its files.
#[derive(Debug, Clone, PartialEq)]
struct Change {
    id: usize,
    desc: String,
    tree: Tree,
}

/// A tool operation, for `vcs_undo`: the model before it, and the files
/// right after it.
#[derive(Debug, Clone)]
struct Op {
    stack: Vec<Change>,
    wc: Change,
    after: Tree,
}

/// The run's changes above trunk, `@` on top, and the tools' operations.
#[derive(Debug, Clone)]
struct Model {
    stack: Vec<Change>,
    wc: Change,
    ops: Vec<Op>,
    next: usize,
    commits: usize,
}

impl Model {
    fn new() -> Self {
        Self {
            stack: Vec::new(),
            wc: Change {
                id: 0,
                desc: String::new(),
                tree: trunk_tree(),
            },
            ops: Vec::new(),
            next: 1,
            commits: 0,
        }
    }

    fn parent_tree(&self) -> Tree {
        self.stack
            .last()
            .map(|c| c.tree.clone())
            .unwrap_or_else(trunk_tree)
    }

    fn at_turn(&self) -> AtTurn {
        AtTurn {
            wc: self.wc.clone(),
            parent: self.parent_tree(),
            stack: self.stack.clone(),
            from: None,
        }
    }

    fn dirty(&self) -> bool {
        self.wc.tree != self.parent_tree()
    }

    fn fresh(&mut self, tree: Tree) -> Change {
        self.next += 1;
        Change {
            id: self.next - 1,
            desc: String::new(),
            tree,
        }
    }

    /// Applies one action, and returns the tool call that makes it.
    fn apply(&mut self, action: Action) -> (&'static str, serde_json::Value) {
        let before = (self.stack.clone(), self.wc.clone());
        let record = |model: &mut Self| {
            model.ops.push(Op {
                stack: before.0.clone(),
                wc: before.1.clone(),
                after: model.wc.tree.clone(),
            });
        };
        match action {
            Action::Write(path, content) => {
                self.wc.tree.insert(path, content);
                ("write", json!({ "path": path, "content": content }))
            }
            Action::Commit => {
                self.commits += 1;
                let message = format!("feat: commit {}", self.commits);
                self.wc.desc = message.clone();
                let fresh = self.fresh(self.wc.tree.clone());
                let old = std::mem::replace(&mut self.wc, fresh);
                self.stack.push(old);
                record(self);
                ("vcs_commit", json!({ "message": message }))
            }
            Action::New => {
                let fresh = self.fresh(self.wc.tree.clone());
                let old = std::mem::replace(&mut self.wc, fresh);
                self.stack.push(old);
                record(self);
                ("vcs_new", json!({}))
            }
            Action::Restore(name) => {
                let source = self.parent_tree();
                let mut tree = self.wc.tree.clone();
                for path in PATHS {
                    if name == "." || name == path {
                        match source.get(path) {
                            Some(v) => tree.insert(path, v),
                            None => tree.remove(path),
                        };
                    }
                }
                let changed = tree != self.wc.tree;
                self.wc.tree = tree;
                if changed {
                    record(self);
                }
                ("vcs_restore", json!({ "paths": [name] }))
            }
            Action::Undo => {
                if let Some(op) = self.ops.pop() {
                    // Edits made since the operation stay.
                    let now = self.wc.tree.clone();
                    let mut tree = op.wc.tree.clone();
                    for path in PATHS {
                        if op.after.get(path) != now.get(path) {
                            match now.get(path) {
                                Some(v) => tree.insert(path, v),
                                None => tree.remove(path),
                            };
                        }
                    }
                    self.stack = op.stack;
                    self.wc = Change { tree, ..op.wc };
                }
                ("vcs_undo", json!({}))
            }
        }
    }
}

/// A turn's model state at its end.
#[derive(Debug, Clone)]
struct AtTurn {
    wc: Change,
    parent: Tree,
    /// The changes under `@`, oldest first.
    stack: Vec<Change>,
    /// What the turn's paths count from: the turn before's files, or
    /// trunk's for the first. Nothing lands on or catches up this run, so
    /// no move of its head is left out.
    from: Option<Tree>,
}

/// Records the turn that just ended, with what its paths count from.
fn push_turn(model: &Model, at: &mut Vec<AtTurn>) {
    let mut turn = model.at_turn();
    turn.from = Some(match at.last() {
        None => trunk_tree(),
        Some(before) => before.wc.tree.clone(),
    });
    at.push(turn);
}

/// One path of a three-way merge, `None` when it conflicts.
fn merge(
    now: Option<&'static str>,
    then: Option<&'static str>,
    turn: Option<&'static str>,
) -> Option<Option<&'static str>> {
    if then == turn {
        Some(now)
    } else if then == now || now == turn {
        Some(turn)
    } else {
        None
    }
}

/// Where a fork at the turn `at` starts, by the reference: the tree of
/// its parent as it is now, and its files, the turn's work merged onto
/// that parent. The turn's parent is followed by change id, except into
/// the run's `@` (an undo took it back), where the fork goes one change
/// down. `None` when the merge conflicts.
fn fork_base(model: &Model, at: &AtTurn) -> Option<(Tree, Tree)> {
    let tree_of = |stack: &[Change]| {
        stack
            .last()
            .map(|c| c.tree.clone())
            .unwrap_or_else(trunk_tree)
    };
    let (then, now) = match at.stack.last() {
        None => (trunk_tree(), trunk_tree()),
        Some(parent) if parent.id == model.wc.id => (
            tree_of(&at.stack[..at.stack.len() - 1]),
            model.parent_tree(),
        ),
        Some(parent) => {
            let now = model
                .stack
                .iter()
                .find(|c| c.id == parent.id)
                .map(|c| c.tree.clone())
                .unwrap_or_else(|| parent.tree.clone());
            (parent.tree.clone(), now)
        }
    };
    let mut files = Tree::new();
    for path in PATHS {
        let value = |tree: &Tree| tree.get(path).copied();
        if let Some(value) =
            merge(value(&now), value(&then), value(&at.wc.tree))?
        {
            files.insert(path, value);
        }
    }
    Some((now, files))
}

/// Drives the drawn turns through the model, and scripts them.
fn plan(
    turns: &[Vec<Action>],
    end: End,
) -> (ScriptedModel, Vec<AtTurn>, Model) {
    let mut model = Model::new();
    let mut llm = ScriptedModel::new();
    let mut at = Vec::new();
    for actions in turns {
        let calls: Vec<_> =
            actions.iter().map(|action| model.apply(*action)).collect();
        llm = llm.turn(move |mut t| {
            if calls.is_empty() {
                return t.tool_call("read", json!({ "path": "a.txt" }));
            }
            for (name, args) in calls {
                t = t.tool_call(name, args);
            }
            t
        });
        push_turn(&model, &mut at);
    }
    let dirty = model.dirty();
    let leftover = |model: &mut Model, llm: ScriptedModel| {
        model.wc.desc = "feat: leftover".to_owned();
        let fresh = model.fresh(model.wc.tree.clone());
        let old = std::mem::replace(&mut model.wc, fresh);
        model.stack.push(old);
        llm.turn(|t| t.text("feat: leftover"))
    };
    match end {
        End::Answer => {
            llm = llm.turn(|t| t.text("done"));
            push_turn(&model, &mut at);
            if dirty {
                llm = llm.turn(|t| t.text("still done"));
                push_turn(&model, &mut at);
                llm = leftover(&mut model, llm);
            }
        }
        End::Limit => {
            if dirty {
                llm = leftover(&mut model, llm);
            }
        }
        End::Fail => {
            llm = llm.turn(|t| t.dropped());
            push_turn(&model, &mut at);
        }
    }
    (llm, at, model)
}

/// Every turn's link, observer call and the run's final stack follow the
/// model; a fork at any turn starts on that turn's files and parent, and
/// stays apart from the run it came from.
#[hegel::test(
    test_cases = 40,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn turns_and_forks_follow_the_model(tc: TestCase) {
    let turns: Vec<Vec<Action>> =
        tc.draw(gs::vecs(turn()).min_size(1).max_size(4));
    let end =
        tc.draw(gs::sampled_from(vec![End::Answer, End::Limit, End::Fail]));
    let fork_writes: Vec<(&'static str, &'static str)> = tc.draw(
        gs::vecs(gs::tuples!(
            gs::sampled_from(PATHS.to_vec()),
            gs::sampled_from(VALUES.to_vec())
        ))
        .max_size(2),
    );
    let after_fork: &'static str = tc.draw(gs::sampled_from(PATHS.to_vec()));

    let (llm, at, model) = plan(&turns, end);
    for actions in &turns {
        for action in actions {
            tc.event(match action {
                Action::Write(..) => "write",
                Action::Commit => "vcs_commit",
                Action::New => "vcs_new",
                Action::Restore(_) => "vcs_restore",
                Action::Undo => "vcs_undo",
            });
        }
    }

    let home = tempfile::tempdir().unwrap();
    let project = project(home.path());
    let trunk = project.trunk().unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let store = Store::memory().await.unwrap();
        let heard: Arc<Mutex<Vec<Vec<String>>>> = Arc::default();
        let first =
            RunWorkspace::new(project.clone(), "first", Identity::default())
                .unwrap()
                .on_turn({
                    let heard = heard.clone();
                    move |turn| heard.lock().unwrap().push(turn.paths.clone())
                });
        let mut runner = coder(llm.clone(), &first, true);
        if end == End::Limit {
            runner = runner.limits(Limits {
                max_turns: Some(turns.len() as u32),
                ..Limits::default()
            });
        }
        let outcome = runner.run("work", &store).await.unwrap();
        assert_eq!(
            llm.remaining(),
            0,
            "the run stopped with {:?}",
            outcome.stop
        );

        // One link a turn, each a snapshot of `@` as the model has it.
        let entries =
            store.plugin_entries(&outcome.run.0, PLUGIN).await.unwrap();
        let linked = links(&entries);
        assert_eq!(
            linked.len(),
            at.len(),
            "one link per turn: {entries:?}, the run stopped with {:?}",
            outcome.stop
        );
        let heard = heard.lock().unwrap().clone();
        for (n, ((_, link), want)) in linked.iter().zip(&at).enumerate() {
            let turn = n + 1;
            assert!(link.snapshot, "turn {turn} links a snapshot");
            assert_eq!(link.turn as usize, turn);
            assert_eq!(
                at_commit(&project, &link.commit_id),
                want.wc.tree,
                "turn {turn}'s files"
            );
            let parent = project.parent_of(&link.commit_id).unwrap().unwrap();
            assert_eq!(
                at_commit(&project, &parent),
                want.parent,
                "turn {turn}'s parent"
            );
            let from = want.from.as_ref().unwrap();
            if n > 0
                && fork_base(&model, &at[n - 1]).map(|(_, f)| f).as_ref()
                    != Some(from)
            {
                tc.event("a turn rewrote the change under the turn before");
            }
            let paths = changed_paths(from, &want.wc.tree);
            assert_eq!(heard[n], paths, "turn {turn}'s paths");
            assert_eq!(link.changed, !paths.is_empty(), "turn {turn} changed");
        }
        // Links share a change id exactly when the model's `@` was one
        // change.
        for (i, (_, a)) in linked.iter().enumerate() {
            for (j, (_, b)) in linked.iter().enumerate() {
                assert_eq!(
                    a.change_id == b.change_id,
                    at[i].wc.id == at[j].wc.id,
                    "turns {} and {} share a change",
                    i + 1,
                    j + 1
                );
            }
        }

        // The run's stack and `@`.
        let head = project.bookmark(&bookmark(&outcome.run)).unwrap().unwrap();
        let stack: Vec<(String, Tree)> = project
            .stack(&head)
            .unwrap()
            .into_iter()
            .map(|c| {
                (
                    c.description.trim().to_owned(),
                    at_commit(&project, &c.commit_id),
                )
            })
            .collect();
        let want: Vec<(String, Tree)> = model
            .stack
            .iter()
            .map(|c| (c.desc.clone(), c.tree.clone()))
            .collect();
        assert_eq!(stack, want, "the run's stack");
        if want.is_empty() {
            assert_eq!(head, trunk);
        }
        assert_eq!(files(&first.dir()), model.wc.tree, "the run's files");
        tc.event(match (end, model.stack.last().map(|c| c.desc.as_str())) {
            (End::Fail, _) if model.dirty() => "failed with work in @",
            (_, Some("feat: leftover")) => "leftover committed at the end",
            _ => "nothing left over",
        });

        // A fork at a drawn turn.
        let k = tc.draw(gs::integers::<usize>().max_value(linked.len() - 1));
        let (seq, _) = linked[k].clone();
        let base = &at[k];
        if at[k].stack.last().is_some_and(|p| p.id == model.wc.id) {
            tc.event("the fork's parent is the run's @ again");
        }
        let Some((parent_now, start)) = fork_base(&model, base) else {
            // jj would hold the conflict; the model has no terms for it.
            tc.event("the fork's start conflicts");
            return;
        };
        if parent_now != base.parent {
            tc.event("the fork's parent changed since its turn");
        }
        let mut want = start.clone();
        want.extend(fork_writes.iter().copied());
        let dirty = want != parent_now;
        let mut fork_llm = ScriptedModel::new()
            .turn(|t| t.tool_call("read", json!({ "path": "a.txt" })));
        if !fork_writes.is_empty() {
            let writes = fork_writes.clone();
            fork_llm = fork_llm.turn(move |mut t| {
                for (path, content) in writes {
                    t = t.tool_call(
                        "write",
                        json!({ "path": path, "content": content }),
                    );
                }
                t
            });
        }
        fork_llm = fork_llm.turn(|t| t.text("done"));
        if dirty {
            fork_llm = fork_llm
                .turn(|t| t.text("still done"))
                .turn(|t| t.text("feat: fork"));
        }
        let fork =
            RunWorkspace::new(project.clone(), "fork", Identity::default())
                .unwrap();
        let forked = coder(fork_llm, &fork, true)
            .fork(&Checkpoint::at(outcome.run.clone(), seq))
            .start("go on", &store)
            .outcome()
            .await
            .unwrap();
        let own: Vec<Link> =
            links(&store.plugin_entries(&forked.run.0, PLUGIN).await.unwrap())
                .into_iter()
                .map(|(_, link)| link)
                .filter(|link| link.workspace == "fork")
                .collect();
        assert_eq!(
            at_commit(&project, &own[0].commit_id),
            start,
            "the fork starts on turn {}'s files",
            k + 1
        );
        let fork_parent =
            project.parent_of(&own[0].commit_id).unwrap().unwrap();
        assert_eq!(
            at_commit(&project, &fork_parent),
            parent_now,
            "the fork starts on turn {}'s parent",
            k + 1
        );
        assert!(!own[0].changed, "the fork's reading turn changed nothing");
        assert_eq!(files(&fork.dir()), want, "the fork's files");
        assert_eq!(files(&first.dir()), model.wc.tree, "the run's files");

        // Later edits in the run stay out of the fork, and the other way.
        let fork_files = files(&fork.dir());
        let old = model.wc.tree.get(after_fork).copied();
        let new = if old == Some("two\n") {
            "one\n"
        } else {
            "two\n"
        };
        let path = first.dir().join(after_fork);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, new).unwrap();
        first.vcs().working_copy().await.unwrap();
        fork.vcs().working_copy().await.unwrap();
        assert_eq!(
            files(&fork.dir()),
            fork_files,
            "the run's edit reached the fork"
        );
        let mut run_files = model.wc.tree.clone();
        run_files.insert(after_fork, new);
        assert_eq!(files(&first.dir()), run_files);
    });
}

/// A fork at a turn whose parent change the run took back with
/// `vcs_undo` does not stand on the run's working copy.
///
/// Turn 1 commits, so its snapshot is a new `@` on the commit `X`. Turn 2
/// undoes the commit: `X` is the run's `@` again, under the same change
/// id. Following `X` by change id would start the fork on the run's `@`,
/// and jj would rebase the fork onto every edit the run makes. The fork
/// starts on `X`'s parent instead, with the turn's files.
#[test]
fn a_fork_whose_parent_was_undone_stays_apart() {
    let home = tempfile::tempdir().unwrap();
    let project = project(home.path());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let store = Store::memory().await.unwrap();
        let first =
            RunWorkspace::new(project.clone(), "first", Identity::default())
                .unwrap();
        let llm = ScriptedModel::new()
            .turn(|t| {
                t.tool_call("vcs_commit", json!({ "message": "feat: empty" }))
            })
            .turn(|t| t.tool_call("vcs_undo", json!({})))
            .turn(|t| t.text("done"));
        let outcome = coder(llm.clone(), &first, true)
            .run("commit, then undo", &store)
            .await
            .unwrap();
        llm.assert_exhausted();
        let linked =
            links(&store.plugin_entries(&outcome.run.0, PLUGIN).await.unwrap());
        let fork =
            RunWorkspace::new(project.clone(), "fork", Identity::default())
                .unwrap();
        let fork_llm = ScriptedModel::new().turn(|t| t.text("forked"));
        coder(fork_llm, &fork, true)
            .fork(&Checkpoint::at(outcome.run.clone(), linked[0].0))
            .start("go on", &store)
            .outcome()
            .await
            .unwrap();

        let wc = project.workspace_head("fork").unwrap().unwrap();
        assert_eq!(
            project.parent_of(&wc).unwrap(),
            Some(project.trunk().unwrap()),
            "the fork starts on trunk"
        );
        std::fs::write(first.dir().join("a.txt"), "two\n").unwrap();
        first.vcs().working_copy().await.unwrap();
        fork.vcs().working_copy().await.unwrap();
        assert_eq!(
            files(&fork.dir()),
            trunk_tree(),
            "the run's edit reached the fork"
        );
    });
}

/// A turn that undoes the commit the turn before made, edits, and commits
/// again changed the file it edited: the run rewriting its own commit is
/// the turn's work, not a catch-up's.
#[test]
fn a_turn_that_recommits_an_undone_commit_keeps_its_paths() {
    let home = tempfile::tempdir().unwrap();
    let project = project(home.path());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let store = Store::memory().await.unwrap();
        let heard: Arc<Mutex<Vec<Vec<String>>>> = Arc::default();
        let first =
            RunWorkspace::new(project.clone(), "first", Identity::default())
                .unwrap()
                .on_turn({
                    let heard = heard.clone();
                    move |turn| heard.lock().unwrap().push(turn.paths.clone())
                });
        let llm = ScriptedModel::new()
            .turn(|t| {
                t.tool_call("vcs_commit", json!({ "message": "feat: x" }))
            })
            .turn(|t| {
                t.tool_call("vcs_undo", json!({}))
                    .tool_call(
                        "write",
                        json!({ "path": "a.txt", "content": "" }),
                    )
                    .tool_call("vcs_commit", json!({ "message": "feat: x" }))
            })
            .turn(|t| t.text("done"));
        coder(llm.clone(), &first, true)
            .run("commit, undo, write, commit", &store)
            .await
            .unwrap();
        llm.assert_exhausted();
        assert_eq!(heard.lock().unwrap()[1], ["a.txt"]);
    });
}
