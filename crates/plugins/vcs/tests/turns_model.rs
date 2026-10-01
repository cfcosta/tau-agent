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
//!   since the turn before, or since the run started for the first;
//! - a run that would stop with changes in `@` is held once; what is left
//!   at the end of a normal run or one at a limit is committed with the
//!   model's message, and a failed run keeps it in `@`;
//! - the run's bookmark names its newest commit, and its stack is the
//!   model's;
//! - a fork at a turn starts on exactly that turn's files, on that turn's
//!   parent, and the two runs never see each other's edits.

use std::{
    collections::BTreeMap,
    path::Path,
    process::Command,
    sync::{Arc, Mutex},
};

use hegel::{TestCase, generators as gs};
use serde_json::json;
use tau_agent::{
    agent::{Agent, Checkpoint},
    limits::Limits,
};
use tau_store::Store;
use tau_testing::scripted::ScriptedModel;
use tau_tools::{path::Root, plugin::CodingTools};
use tau_vcs::{
    Identity,
    Link,
    Project,
    RunWorkspace,
    VcsPlugin,
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

fn git(dir: &Path, args: &[&str]) {
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
}

/// A project whose trunk holds `a.txt`: `one`.
fn project(home: &Path) -> Project {
    let src = home.join("src");
    std::fs::create_dir_all(&src).unwrap();
    git(&src, &["init", "--quiet"]);
    std::fs::write(src.join("a.txt"), "one\n").unwrap();
    git(&src, &["add", "a.txt"]);
    git(&src, &["commit", "--quiet", "-m", "first"]);
    Project::import(src.to_str().unwrap(), home.join("p"), Identity::default())
        .unwrap()
}

fn agent(llm: ScriptedModel, workspace: &RunWorkspace) -> Agent {
    Agent::new(llm)
        .name("coder")
        .plugin(CodingTools::new(Root::new(workspace.dir())))
        .plugin(VcsPlugin::new(workspace.vcs().clone()))
        .plugin(workspace.clone())
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

fn links(entries: &[(i64, String)]) -> Vec<(i64, Link)> {
    entries
        .iter()
        .filter_map(|(seq, body)| Link::parse(body).map(|link| (*seq, link)))
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
            parent_change: self.stack.last().cloned(),
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
    /// `@`'s parent, unless it is trunk.
    parent_change: Option<Change>,
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
        at.push(model.at_turn());
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
            at.push(model.at_turn());
            if dirty {
                llm = llm.turn(|t| t.text("still done"));
                at.push(model.at_turn());
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
            at.push(model.at_turn());
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
        let mut runner = agent(llm.clone(), &first);
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
        let mut before = trunk_tree();
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
            let paths = changed_paths(&before, &want.wc.tree);
            assert_eq!(heard[n], paths, "turn {turn}'s paths");
            assert_eq!(link.changed, !paths.is_empty(), "turn {turn} changed");
            before = want.wc.tree.clone();
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
        // A fork follows its turn's parent by change id. Once `vcs_undo`
        // took that change back into `@`, the fork lands on what the run
        // did with it since, or on the run's own `@`: see
        // `a_fork_whose_parent_was_undone_shares_the_runs_working_copy`.
        if let Some(parent) = &base.parent_change
            && !model
                .stack
                .iter()
                .any(|c| c.id == parent.id && c.tree == parent.tree)
        {
            tc.event("the fork's parent was undone since");
            return;
        }
        let mut want = base.wc.tree.clone();
        want.extend(fork_writes.iter().copied());
        let dirty = want != base.parent;
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
        let forked = agent(fork_llm, &fork)
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
            base.wc.tree,
            "the fork starts on turn {}'s files",
            k + 1
        );
        let fork_parent =
            project.parent_of(&own[0].commit_id).unwrap().unwrap();
        assert_eq!(
            at_commit(&project, &fork_parent),
            base.parent,
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
/// `vcs_undo` starts on the run's own working copy, and the run's later
/// edits reach it.
///
/// Turn 1 commits (`vcs_commit`), so its snapshot is a new `@` on the
/// commit `X`. Turn 2 undoes the commit: `X` is `@` again, under the same
/// change id, and the described commit is hidden.
/// `Project::add_workspace_from_snapshot` follows the snapshot's parent
/// by change id (`visible` in `src/project.rs`), finds the run's `@`, and
/// starts the fork's change on it. When the run snapshots an edit, jj
/// rewrites `@` and rebases its descendants, the fork's change among
/// them, so the fork's next tool call moves its files onto the run's
/// edit. With edits after the undo that the run then committed, the fork
/// starts with those later files merged in instead.
///
/// Fix options:
/// - follow the parent's change only to a commit no workspace has as its
///   working copy, and otherwise start the fork on the parent's own
///   parent with the turn's files, uncommitted (the undone commit's
///   description is lost to the fork);
/// - follow the parent only when the commit it moved to descends from a
///   rewrite of it, not from an undo (the operation log tells them
///   apart), and otherwise start on the snapshot's parent as it was,
///   which jj then shows as a divergent change;
/// - store, with each snapshot link, the parent's commit id and follow
///   the change only through restacks the run's own links record.
///
/// The first keeps forks apart with no divergence, and is the
/// recommendation.
#[test]
#[ignore = "bug: a fork follows its turn's parent into the run's own @ after vcs_undo"]
fn a_fork_whose_parent_was_undone_shares_the_runs_working_copy() {
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
        let outcome = agent(llm.clone(), &first)
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
        agent(fork_llm, &fork)
            .fork(&Checkpoint::at(outcome.run.clone(), linked[0].0))
            .start("go on", &store)
            .outcome()
            .await
            .unwrap();

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
