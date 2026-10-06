//! The vcs tools in a chat's workspace while other workspaces act on
//! the same repository, against a model (`docs/reference/vcs.md`,
//! "Tools", "Scoping rules", "A repository's main chat"; ADR 0015).
//!
//! A project has a main chat, committing on trunk, and a chat that
//! started on one of the main chat's commits. Random sequences of the
//! chat's file edits and tool calls run on it, and between them the
//! other workspaces act: upstream moves and the main chat catches up
//! (`ProjectRepo::update`, `Vcs::move_onto`), restacking the commits the
//! chat stands on; the main chat commits; other chats come, land on the
//! main chat or are dropped, and go; an idle chat snapshots, ends turns
//! and describes. Checked after every step against a model of the chat:
//! its stack of changes above the main chat's commit, each a description
//! and a file tree, its `@`, and the files on disk.
//!
//! What the model holds to:
//! - a catch-up rebases the chat's changes and `@` as jj rebases them
//!   (`onto + old - base`, path by path), keeping their change ids and
//!   descriptions; the chat's next tool moves its files there, with
//!   what was edited since the last snapshot merged on top, and a
//!   clash becomes a conflict with markers;
//! - `vcs_status`, `vcs_diff`, `vcs_log` and `vcs_show` report the
//!   rewritten state; the write tools keep working on it;
//! - `vcs_undo` undoes only the chat's own tool operations: it passes
//!   over snapshots and turn ends of any workspace, and refuses when any
//!   other operation is newer, a catch-up's included;
//! - a commit id names the same commit forever: shown or restored from
//!   after a rewrite hid it, it is what it was; a change id an undo
//!   abandoned is refused as hidden;
//! - a turn's end lists the paths changed since the turn before's
//!   snapshot, rebased onto its parent as that is now: what a catch-up
//!   brought is not the turn's.
//!
//! File contents are one line, never empty, so jj's line merge of a file
//! resolves exactly when its trivial merge of whole files does, and
//! every conflict jj writes has markers.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

mod common;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use common::merge::*;
use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_agent::{
    plugin::Plugin,
    tool::{AgentTool, ToolCtx},
};
use tau_ai::message::InputBlock;
use tau_testing::{block_on, git::git};
use tau_vcs_host::{
    DEFAULT_WORKSPACE,
    Identity,
    ProjectRepo,
    UpdateFrom,
    Vcs,
    VcsPlugin,
};

/// One workspace's handle and its tools.
struct Workspace {
    vcs: Vcs,
    tools: Vec<Arc<dyn AgentTool>>,
}

impl Workspace {
    fn new(vcs: Vcs) -> Self {
        let tools = VcsPlugin::new(vcs.clone()).tools();
        Self { vcs, tools }
    }

    /// Calls `name`; the text and details, or the error's text.
    fn call(&self, name: &str, args: Value) -> Result<(String, Value), String> {
        let tool = self
            .tools
            .iter()
            .find(|tool| tool.name() == name)
            .unwrap_or_else(|| panic!("no tool {name}"));
        block_on(tool.call(args, ToolCtx::detached()))
            .map(|output| {
                let text = match &output.content[0] {
                    InputBlock::Text(text) => text.text.clone(),
                    other => panic!("expected text, got {other:?}"),
                };
                (text, output.details.unwrap_or(Value::Null))
            })
            .map_err(|err| err.to_string())
    }

    fn ok(&self, name: &str, args: Value) -> (String, Value) {
        let shown = args.to_string();
        self.call(name, args)
            .unwrap_or_else(|err| panic!("{name} {shown} failed: {err}"))
    }
}

/// A project imported from a checkout holding `a.txt`, and its main
/// chat, caught up with trunk as the host has it before its first turn.
fn project() -> (tempfile::TempDir, ProjectRepo, Workspace) {
    let home = tempfile::tempdir().unwrap();
    let project = common::project(home.path());
    let dir = project.workspace_dir(DEFAULT_WORKSPACE);
    let vcs = tau_testing::block_on_io(tau_vcs_host::Vcs::open(
        &dir,
        Identity::default(),
    ))
    .unwrap();
    let trunk = project.trunk().unwrap();
    let name = project.trunk_name().unwrap();
    block_on(vcs.move_onto(trunk, name, true)).unwrap();
    (home, project, Workspace::new(vcs))
}

fn assert_refused(result: Result<(String, Value), String>) {
    let err = result.expect_err("undo of an operation the tools did not make");
    assert!(
        err.starts_with("The last operation was not made by the vcs tools"),
        "{err}"
    );
}

const PATHS: [&str; 3] = ["a.txt", "b.txt", "dir/c.txt"];
const VALUES: [&str; 3] = ["one\n", "two\n", "three\n"];
/// What `restore` can name: files, a directory, everything.
const RESTORE_PATHS: [&str; 5] = ["a.txt", "b.txt", "dir/c.txt", "dir", "."];
/// The most chats that come and go in one case.
const MAX_CHATS: usize = 3;

/// `(path, kind)` for every path that differs from `from` to `to`, in
/// path order, as the tools list changes.
fn changes(from: &Tree, to: &Tree) -> Vec<(String, String)> {
    PATHS
        .iter()
        .filter_map(|&path| {
            let (old, new) = (get(from, path), get(to, path));
            if old == new {
                return None;
            }
            let absent = Term::resolved(None);
            let kind = if old == absent {
                "added"
            } else if new == absent {
                "removed"
            } else {
                "modified"
            };
            Some((path.to_owned(), kind.to_owned()))
        })
        .collect()
}

fn listed(details: &Value, key: &str) -> Vec<(String, String)> {
    details[key]
        .as_array()
        .unwrap_or_else(|| panic!("no `{key}` in {details}"))
        .iter()
        .map(|change| {
            (
                change["path"].as_str().unwrap().to_owned(),
                change["kind"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// The changed paths a tool listed are the model's. A conflict the
/// model holds as it was may be listed too: after a few rebases jj can
/// write the same merge in another form (see `runs_model.rs`).
fn check_changes(got: &[(String, String)], from: &Tree, to: &Tree, what: &str) {
    let want = changes(from, to);
    for (path, kind) in &want {
        assert!(
            got.iter().any(|(p, k)| p == path && k == kind),
            "{what}: {path} {kind} is not listed in {got:?}"
        );
    }
    for (path, _) in got {
        if !want.iter().any(|(p, _)| p == path) {
            let path = PATHS.iter().find(|p| **p == path).unwrap();
            assert!(
                get(to, path).value().is_none(),
                "{what}: {path} is listed in {got:?}, but did not change"
            );
        }
    }
}

/// A description as jj stores it: trailing whitespace trimmed, one final
/// newline unless empty.
fn stored(message: &str) -> String {
    let trimmed = message.trim_end();
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}\n")
    }
}

/// Whether `path` is `name` or under it (`.` is everything).
fn under(path: &str, name: &str) -> bool {
    name == "." || path == name || path.starts_with(&format!("{name}/"))
}

/// Files written or deleted, in order.
type Edits = Vec<(&'static str, Val)>;

#[hegel::composite]
fn edits(tc: &TestCase, min: usize) -> Edits {
    tc.draw(
        gs::vecs(gs::tuples!(
            gs::sampled_from(PATHS.to_vec()),
            gs::optional(gs::sampled_from(VALUES.to_vec()))
        ))
        .min_size(min)
        .max_size(3),
    )
}

/// Writes `edits` to `dir` and returns `tree` with them.
fn write_edits(dir: &Path, tree: &Tree, edits: &[(&'static str, Val)]) -> Tree {
    let mut tree = tree.clone();
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
        set(&mut tree, path, Term::resolved(*value));
    }
    tree
}

/// The files in `dir` are `tree`: resolved files as they are, a
/// conflict with jj's markers.
fn check_files(dir: &Path, tree: &Tree, what: &str) {
    for path in PATHS {
        let on_disk = std::fs::read(dir.join(path)).ok();
        match get(tree, path).value() {
            Some(value) => assert_eq!(
                on_disk.as_deref(),
                value.map(str::as_bytes),
                "{path} in {what}"
            ),
            None => {
                let text =
                    String::from_utf8_lossy(on_disk.as_deref().unwrap_or_else(
                        || panic!("conflicted {path} is missing in {what}"),
                    ))
                    .into_owned();
                assert!(text.contains("<<<<<<<"), "{path} in {what}: {text}");
            }
        }
    }
}

/// A description a model might write: several lines, trailing space,
/// or nothing at all.
#[hegel::composite]
fn message(tc: &TestCase) -> String {
    tc.draw(gs::sampled_from(vec![
        "Fix the parser".to_owned(),
        "Add a test\n\nWith a body.".to_owned(),
        "trailing space   \n\n".to_owned(),
        String::new(),
        "   ".to_owned(),
    ]))
}

/// One of the chat's changes: its description as jj stores it, its
/// files, and its change id.
#[derive(Debug, Clone, PartialEq)]
struct Change {
    description: String,
    tree: Tree,
    change_id: String,
}

/// What the chat's `vcs_undo` finds, newest last.
#[derive(Debug, Clone)]
enum Op {
    /// The chat's own tool operation, and the model as it was before.
    Tool {
        stack: Vec<Change>,
        wc: Change,
        /// The files right after the operation: edits made since are
        /// the ones undo must keep.
        after: Tree,
    },
    /// Anything else that undo may not pass over: another workspace's
    /// operation, or the host's in the chat's.
    Other,
}

/// A turn's snapshot, as the next turn counts its paths from it.
#[derive(Debug, Clone)]
struct Since {
    commit_id: String,
    /// `@`'s files then.
    tree: Tree,
}

/// A commit the chat's tools showed: what its id names forever.
#[derive(Debug, Clone)]
struct Seen {
    change_id: String,
    description: String,
    tree: Tree,
    parent: Tree,
}

struct Machine {
    home: tempfile::TempDir,
    project: ProjectRepo,
    main: Workspace,
    chat: Workspace,
    /// A chat that only snapshots, ends turns and describes.
    idle: Workspace,
    /// The source's files, as trunk's upstream commits have them.
    upstream: Tree,
    /// How many commits upstream has.
    upstream_commits: usize,
    /// The main chat's own commits above upstream, oldest first: their
    /// trees and change ids. Its `@` is always empty on the newest.
    main_chain: Vec<(Tree, String)>,
    /// The main chat's commit the chat stands on.
    base: usize,
    /// The chat's changes above `base`, oldest first.
    stack: Vec<Change>,
    /// The chat's `@`; its tree is the files on disk.
    wc: Change,
    /// `@`'s tree at the last snapshot or checkout, as jj has it.
    seen: Tree,
    /// `@`'s tree as another workspace's operation rewrote it, until the
    /// chat's next tool brings the files there.
    stale: Option<Tree>,
    ops: Vec<Op>,
    /// Snapshots or turn ends of other workspaces since the chat's
    /// newest operation: undo passes over them.
    passed: bool,
    /// The last turn's snapshot.
    since: Option<Since>,
    /// How the main chat's catch-ups moved the chat's newest commit since
    /// that snapshot, oldest first: its tree before and after each.
    moved_since: Vec<(Tree, Tree)>,
    /// Change ids an undo abandoned.
    dead: BTreeSet<String>,
    /// Every commit of the chat's stack a tool showed, by commit id.
    shown: BTreeMap<String, Seen>,
    /// Chats that came and went.
    chats: usize,
    /// The main chat's last turn's snapshot, its tree, the main chat's
    /// commit it stood on, and that commit's tree then.
    main_since: Option<(String, Tree, usize, Tree)>,
    /// The main chat's commits landed since that snapshot.
    main_landed: Vec<usize>,
}

impl Machine {
    fn new() -> Self {
        let (home, project, main) = project();
        let mut upstream = Tree::new();
        set(&mut upstream, "a.txt", Term::resolved(Some("one\n")));
        // A chat on upstream's commit, which no catch-up rewrites.
        let trunk = project.trunk().unwrap();
        let idle =
            Workspace::new(project.add_workspace("idle", &trunk).unwrap());
        // The main chat's first commit, which the chat starts on.
        let main_dir = project.workspace_dir(DEFAULT_WORKSPACE);
        let first =
            write_edits(&main_dir, &upstream, &[("b.txt", Some("one\n"))]);
        let (_, committed) =
            main.ok("vcs_commit", json!({ "message": "main 0" }));
        let head = committed["committed"]["commit_id"].as_str().unwrap();
        let main_change = committed["committed"]["change_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let trunk_name = project.trunk_name().unwrap();
        let turn = block_on(main.vcs.end_turn(trunk_name, None)).unwrap();
        let chat = Workspace::new(project.add_workspace("chat", head).unwrap());
        let (_, status) = chat.ok("vcs_status", json!({}));
        let wc_id = status["working_copy"]["change_id"]
            .as_str()
            .unwrap()
            .to_owned();
        Self {
            home,
            project,
            main,
            chat,
            idle,
            upstream,
            upstream_commits: 1,
            main_chain: vec![(first.clone(), main_change)],
            base: 0,
            stack: Vec::new(),
            wc: Change {
                description: String::new(),
                tree: first.clone(),
                change_id: wc_id,
            },
            seen: first.clone(),
            stale: None,
            ops: Vec::new(),
            passed: false,
            since: None,
            moved_since: Vec::new(),
            dead: BTreeSet::new(),
            shown: BTreeMap::new(),
            chats: 0,
            main_since: Some((turn.commit_id, first.clone(), 0, first.clone())),
            main_landed: Vec::new(),
        }
    }

    fn dir(&self, name: &str) -> PathBuf {
        self.project.workspace_dir(name)
    }

    fn src(&self) -> PathBuf {
        self.home.path().join("src")
    }

    fn base_tree(&self) -> Tree {
        self.main_chain[self.base].0.clone()
    }

    /// The tree of `@`'s parent.
    fn parent_tree(&self) -> Tree {
        self.stack
            .last()
            .map(|change| change.tree.clone())
            .unwrap_or_else(|| self.base_tree())
    }

    /// The tree of the `at`th change's parent.
    fn tree_under(&self, at: usize) -> Tree {
        if at == 0 {
            self.base_tree()
        } else {
            self.stack[at - 1].tree.clone()
        }
    }

    /// What the chat's next tool does first: when another workspace's
    /// operation rewrote `@`, the files move to the rewritten tree, with
    /// what was edited since the last snapshot merged on top
    /// (`session::snapshot_locked`). Then `@` is the files.
    fn freshen(&mut self, tc: &TestCase) {
        if let Some(stale) = self.stale.take() {
            if stale != self.seen {
                tc.event("a stale chat catches up");
                if self.wc.tree != self.seen {
                    tc.event("edits made while stale merge onto the rewrite");
                }
            }
            let merged = rebase_tree(&stale, &self.seen, &self.wc.tree);
            if conflicts(&merged).len() > conflicts(&stale).len() {
                tc.event("an edit made while stale conflicts");
            }
            self.wc.tree = merged;
        }
        self.seen = self.wc.tree.clone();
    }

    /// Before a write tool: the model as undoing the tool gives it back.
    fn before_tool(&mut self, tc: &TestCase) -> (Vec<Change>, Change) {
        self.freshen(tc);
        (self.stack.clone(), self.wc.clone())
    }

    fn after_tool(&mut self, before: (Vec<Change>, Change)) {
        let (stack, wc) = before;
        self.ops.push(Op::Tool {
            stack,
            wc,
            after: self.wc.tree.clone(),
        });
        self.passed = false;
    }

    /// An operation undo may not pass over.
    fn other(&mut self) {
        self.ops.push(Op::Other);
        self.passed = false;
    }

    /// `@`'s change id from a tool's `working_copy` details is the
    /// model's.
    fn check_wc(&self, details: &Value) {
        assert_eq!(
            details["working_copy"]["change_id"],
            json!(self.wc.change_id),
            "@'s change id"
        );
    }

    /// The chat's changes and their commit ids now, from `vcs_log`, with
    /// what each commit id names noted.
    fn commit_ids(&mut self) -> Vec<String> {
        let (_, log) = self.chat.ok("vcs_log", json!({ "limit": 100 }));
        let rows = log["changes"].as_array().unwrap();
        let mut ids = Vec::new();
        for (at, change) in self.stack.iter().enumerate() {
            let row = &rows[self.stack.len() - at];
            assert_eq!(row["change_id"], json!(change.change_id));
            ids.push(row["commit_id"].as_str().unwrap().to_owned());
        }
        for (at, id) in ids.iter().enumerate() {
            self.note(id, at);
        }
        ids
    }

    /// Notes that commit `id` is the `at`th change as the model has it
    /// now, or checks it against what was noted before.
    fn note(&mut self, id: &str, at: usize) {
        let change = &self.stack[at];
        let seen = Seen {
            change_id: change.change_id.clone(),
            description: change.description.clone(),
            tree: change.tree.clone(),
            parent: self.tree_under(at),
        };
        match self.shown.get(id) {
            None => {
                self.shown.insert(id.to_owned(), seen);
            }
            Some(old) => {
                assert_eq!(old.change_id, seen.change_id, "commit {id}");
                assert_eq!(old.description, seen.description, "commit {id}");
                assert_eq!(old.tree, seen.tree, "commit {id}'s tree");
            }
        }
    }

    /// The main chat's catch-up after upstream moved from `old`: its
    /// commits go onto the new upstream, and the chat's changes and `@`
    /// follow the one it stands on.
    fn rebase_all(&mut self, old: &Tree) {
        let old_base = self.base_tree();
        let mut onto = self.upstream.clone();
        let mut base = old.clone();
        for (tree, _) in &mut self.main_chain {
            let new = rebase_tree(&onto, &base, tree);
            base = std::mem::replace(tree, new.clone());
            onto = new;
        }
        let mut onto = self.base_tree();
        let mut base = old_base;
        for change in &mut self.stack {
            let new = rebase_tree(&onto, &base, &change.tree);
            base = std::mem::replace(&mut change.tree, new.clone());
            onto = new;
        }
        // jj's `@`: the last snapshot, or what rewrote it before.
        let wc = self.stale.take().unwrap_or_else(|| self.seen.clone());
        self.stale = Some(rebase_tree(&onto, &base, &wc));
    }
}

impl Machine {
    /// The chat's agent edits files, as `write` or `edit` would.
    fn do_write(&mut self, tc: &TestCase) {
        let edits = tc.draw(edits(1));
        let dir = self.dir("chat");
        self.wc.tree = write_edits(&dir, &self.wc.tree, &edits);
    }

    fn do_describe(&mut self, tc: &TestCase) {
        let message = tc.draw(message());
        let before = self.before_tool(tc);
        let (_, details) =
            self.chat.ok("vcs_describe", json!({ "message": message }));
        self.wc.description = stored(&message);
        self.check_wc(&details);
        self.after_tool(before);
    }

    fn do_commit(&mut self, tc: &TestCase) {
        let message = tc.draw(message());
        if message.trim().is_empty() {
            // Refused before the snapshot.
            let err = self
                .chat
                .call("vcs_commit", json!({ "message": message }))
                .unwrap_err();
            assert_eq!(err, "The description must not be empty");
            return;
        }
        let before = self.before_tool(tc);
        let (_, details) =
            self.chat.ok("vcs_commit", json!({ "message": message }));
        assert_eq!(
            details["committed"]["change_id"],
            json!(self.wc.change_id),
            "commit gave @ a new change id"
        );
        self.wc.description = stored(&message);
        let id = details["working_copy"]["change_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let tree = self.wc.tree.clone();
        let committed = std::mem::replace(
            &mut self.wc,
            Change {
                description: String::new(),
                tree,
                change_id: id,
            },
        );
        self.stack.push(committed);
        self.after_tool(before);
    }

    fn do_new_change(&mut self, tc: &TestCase) {
        let message = tc.draw(gs::optional(message()));
        let before = self.before_tool(tc);
        let args = match &message {
            Some(message) => json!({ "message": message }),
            None => json!({}),
        };
        let (_, details) = self.chat.ok("vcs_new", args);
        let id = details["working_copy"]["change_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let tree = self.wc.tree.clone();
        let old = std::mem::replace(
            &mut self.wc,
            Change {
                description: stored(message.as_deref().unwrap_or("")),
                tree,
                change_id: id,
            },
        );
        self.stack.push(old);
        self.after_tool(before);
    }

    /// Restores paths from `@`'s parent, from a change of the stack by
    /// its change id or its commit id now, or from a commit a tool
    /// showed before, which a rewrite may have hidden since.
    fn do_restore(&mut self, tc: &TestCase) {
        let paths: Vec<&str> = tc.draw(
            gs::vecs(gs::sampled_from(RESTORE_PATHS.to_vec()))
                .min_size(1)
                .max_size(2),
        );
        let kind = tc.draw(gs::integers::<u8>().max_value(3));
        self.freshen(tc);
        let ids = self.commit_ids();
        let (from, source) = match kind {
            1 | 2 if !self.stack.is_empty() => {
                let at = tc.draw(
                    gs::integers::<usize>().max_value(self.stack.len() - 1),
                );
                let id = if kind == 1 {
                    self.stack[at].change_id.clone()
                } else {
                    ids[at].clone()
                };
                (Some(id), self.stack[at].tree.clone())
            }
            3 if !self.shown.is_empty() => {
                let id = tc.draw(gs::sampled_from(
                    self.shown.keys().cloned().collect::<Vec<_>>(),
                ));
                if !ids.contains(&id) {
                    tc.event("restore from a commit a rewrite hid");
                }
                let tree = self.shown[&id].tree.clone();
                (Some(id), tree)
            }
            _ => (None, self.parent_tree()),
        };
        let before = self.before_tool(tc);
        let mut args = json!({ "paths": paths });
        if let Some(from) = &from {
            args["from"] = json!(from);
        }
        let (_, details) = self.chat.ok("vcs_restore", args);
        let old = self.wc.tree.clone();
        let mut new = old.clone();
        for path in PATHS {
            if paths.iter().any(|name| under(path, name)) {
                set(&mut new, path, get(&source, path));
            }
        }
        check_changes(&listed(&details, "restored"), &old, &new, "restored");
        self.wc.tree = new;
        self.seen = self.wc.tree.clone();
        self.check_wc(&details);
        // Restoring what already matches writes no operation.
        if !listed(&details, "restored").is_empty() {
            self.after_tool(before);
        }
    }

    /// Undoes the chat's newest tool operation, or is refused.
    fn do_undo(&mut self, tc: &TestCase) {
        let stale = self.stale.as_ref().is_some_and(|t| t != &self.seen);
        let result = self.chat.call("vcs_undo", json!({}));
        self.freshen(tc);
        match self.ops.last() {
            None | Some(Op::Other) => {
                tc.event("undo refused");
                if stale {
                    tc.event("undo right after the stale update is refused");
                }
                assert_refused(result);
            }
            Some(Op::Tool { .. }) => {
                let Some(Op::Tool { stack, wc, after }) = self.ops.pop() else {
                    unreachable!()
                };
                tc.event("undo");
                if self.passed {
                    tc.event("undo passes over snapshots and turn ends");
                }
                let (_, details) =
                    result.unwrap_or_else(|err| panic!("undo failed: {err}"));
                // Edits made since the operation stay.
                let now = self.wc.tree.clone();
                let mut tree = wc.tree.clone();
                for (path, _) in changes(&after, &now) {
                    let path = PATHS.iter().find(|p| **p == path).unwrap();
                    set(&mut tree, path, get(&now, path));
                }
                let ids = |stack: &[Change], wc: &Change| -> BTreeSet<String> {
                    stack
                        .iter()
                        .chain([wc])
                        .map(|change| change.change_id.clone())
                        .collect()
                };
                let gone = ids(&self.stack, &self.wc);
                let kept = ids(&stack, &wc);
                self.dead.extend(gone.difference(&kept).cloned());
                self.stack = stack;
                self.wc = Change { tree, ..wc };
                self.seen = self.wc.tree.clone();
                self.check_wc(&details);
            }
        }
        self.passed = false;
    }

    /// The host commits what `@` holds, as at the end of a run: the
    /// host's operation, which undo does not pass over.
    fn do_commit_all(&mut self, tc: &TestCase) {
        self.freshen(tc);
        let turn =
            block_on(self.chat.vcs.commit_all("tau: the end", "tau/chat"))
                .unwrap();
        if self.wc.tree == self.parent_tree() {
            assert!(!turn.changed, "nothing to commit, yet it committed");
            return;
        }
        assert!(turn.changed);
        assert_eq!(turn.change_id, self.wc.change_id);
        if self.wc.description.is_empty() {
            self.wc.description = stored("tau: the end");
        }
        let (_, status) = self.chat.ok("vcs_status", json!({}));
        let id = status["working_copy"]["change_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let tree = self.wc.tree.clone();
        let committed = std::mem::replace(
            &mut self.wc,
            Change {
                description: String::new(),
                tree,
                change_id: id,
            },
        );
        self.stack.push(committed);
        self.other();
    }

    /// The host ends one of the chat's turns: a snapshot of `@`, listing
    /// the paths changed since the turn before's. Undo passes over it.
    fn do_end_turn(&mut self, tc: &TestCase) {
        self.freshen(tc);
        let since = self.since.clone();
        let turn =
            block_on(self.chat.vcs.end_turn(
                "tau/chat",
                since.as_ref().map(|s| s.commit_id.clone()),
            ))
            .unwrap();
        // From the turn before's snapshot, with each tagged catch-up's
        // move of the chat's newest commit since replayed on it: what a
        // catch-up brought is not the turn's
        // (`a_turns_paths_leave_out_what_a_catch_up_brought`), and nothing
        // else is followed: the chat's own commits, undos and recommits
        // are the turn's work.
        let moves = std::mem::take(&mut self.moved_since);
        let base = match since {
            None => self.parent_tree(),
            Some(since) => {
                let mut base = since.tree;
                for (before, after) in moves {
                    if before != after {
                        tc.event(
                            "a turn counts from a snapshot a catch-up moved",
                        );
                    }
                    base = rebase_tree(&after, &before, &base);
                }
                base
            }
        };
        let got: Vec<(String, String)> = turn
            .paths
            .iter()
            .map(|path| {
                let kind = changes(&base, &self.wc.tree)
                    .into_iter()
                    .find(|(p, _)| p == path)
                    .map(|(_, kind)| kind)
                    .unwrap_or_default();
                (path.clone(), kind)
            })
            .collect();
        check_changes(&got, &base, &self.wc.tree, "the turn's paths");
        assert_eq!(turn.change_id, self.wc.change_id);
        self.since = Some(Since {
            commit_id: turn.commit_id,
            tree: self.wc.tree.clone(),
        });
        self.passed = true;
    }

    /// `vcs_show` and `vcs_diff` of a change of the stack, by change id:
    /// the change as it is now.
    fn do_show(&mut self, tc: &TestCase) {
        tc.assume(!self.stack.is_empty());
        let at =
            tc.draw(gs::integers::<usize>().max_value(self.stack.len() - 1));
        self.freshen(tc);
        let change = self.stack[at].clone();
        let parent = self.tree_under(at);
        let (_, details) = self
            .chat
            .ok("vcs_show", json!({ "change": change.change_id }));
        assert_eq!(details["change"]["change_id"], json!(change.change_id));
        assert_eq!(details["change"]["description"], json!(change.description));
        assert_eq!(details["change"]["divergent"], json!(false));
        assert_eq!(details["change"]["working_copy"], json!(false));
        check_changes(
            &listed(&details, "files"),
            &parent,
            &change.tree,
            "show",
        );
        let id = details["change"]["commit_id"].as_str().unwrap().to_owned();
        self.note(&id, at);
        let (_, diff) = self
            .chat
            .ok("vcs_diff", json!({ "change": change.change_id }));
        assert_eq!(diff["change"]["commit_id"], json!(id));
        check_changes(&listed(&diff, "files"), &parent, &change.tree, "diff");
    }

    /// `vcs_show` of a commit a tool showed before, by its commit id:
    /// what it was then, though a rewrite may have hidden it.
    fn do_show_seen(&mut self, tc: &TestCase) {
        tc.assume(!self.shown.is_empty());
        let id = tc.draw(gs::sampled_from(
            self.shown.keys().cloned().collect::<Vec<_>>(),
        ));
        self.freshen(tc);
        if !self.commit_ids().contains(&id) {
            tc.event("show a commit a rewrite hid");
        }
        let seen = self.shown[&id].clone();
        let (_, details) = self.chat.ok("vcs_show", json!({ "change": id }));
        assert_eq!(details["change"]["commit_id"], json!(id));
        assert_eq!(details["change"]["change_id"], json!(seen.change_id));
        assert_eq!(details["change"]["description"], json!(seen.description));
        check_changes(
            &listed(&details, "files"),
            &seen.parent,
            &seen.tree,
            "show of an old commit",
        );
    }

    /// A change an undo abandoned is refused by its change id.
    fn do_show_dead(&mut self, tc: &TestCase) {
        tc.assume(!self.dead.is_empty());
        let id = tc.draw(gs::sampled_from(
            self.dead.iter().cloned().collect::<Vec<_>>(),
        ));
        self.freshen(tc);
        tc.event("show an abandoned change");
        let err = self
            .chat
            .call("vcs_show", json!({ "change": id }))
            .unwrap_err();
        assert_eq!(err, format!("Change `{id}` is hidden (abandoned)"));
    }

    /// Upstream moves, an update brings it in, and the main chat catches
    /// up: its commits, the chat's base among them, go on top, and the
    /// chat's changes and `@` follow. Maybe after edits in the chat that
    /// no tool has seen yet.
    fn do_upstream_and_catch_up(&mut self, tc: &TestCase) {
        let unseen = tc.draw(edits(0));
        let upstream = tc.draw(edits(1));
        let dir = self.dir("chat");
        self.wc.tree = write_edits(&dir, &self.wc.tree, &unseen);
        let src = self.src();
        let old = self.upstream.clone();
        self.upstream = write_edits(&src, &old, &upstream);
        git(&src, &["add", "-A"]);
        git(
            &src,
            &["commit", "--quiet", "--allow-empty", "-m", "upstream"],
        );
        self.upstream_commits += 1;
        self.project.update(UpdateFrom::Checkout(&src)).unwrap();
        let trunk = self.project.trunk().unwrap();
        let name = self.project.trunk_name().unwrap();
        let moved =
            block_on(self.main.vcs.move_onto(trunk, name, true)).unwrap();
        let head_before = self.parent_tree();
        self.rebase_all(&old);
        // The catch-up is tagged: the chat's next turn leaves out what it
        // brought to the chat's newest commit.
        self.moved_since.push((head_before, self.parent_tree()));
        self.other();
        tc.event("a catch-up rewrites the chat");
        if self.stack.iter().any(|c| !conflicts(&c.tree).is_empty())
            || !conflicts(self.stale.as_ref().unwrap()).is_empty()
        {
            tc.event("a catch-up conflicts in the chat");
        }
        let head = &self.main_chain.last().unwrap().0;
        assert_eq!(
            moved.conflicts,
            conflicts(head),
            "the catch-up's conflicts"
        );
        check_files(&self.dir(DEFAULT_WORKSPACE), head, "the main chat");
        // The chat's files stay until its next tool.
        check_files(&dir, &self.wc.tree, "the stale chat");
    }

    /// The main chat commits work of its own on trunk.
    fn do_main_commit(&mut self, tc: &TestCase) {
        let edits = tc.draw(edits(1));
        let dir = self.dir(DEFAULT_WORKSPACE);
        let head = self.main_chain.last().unwrap().0.clone();
        let tree = write_edits(&dir, &head, &edits);
        let (_, details) = self.main.ok(
            "vcs_commit",
            json!({ "message": format!("main {}", self.main_chain.len()) }),
        );
        let id = details["committed"]["change_id"].as_str().unwrap();
        self.main_chain.push((tree, id.to_owned()));
        self.other();
        // The turn's end moves trunk to the main chat's newest commit.
        let name = self.project.trunk_name().unwrap();
        let since = self.main_since.take();
        let turn = block_on(
            self.main
                .vcs
                .end_turn(name, since.as_ref().map(|(id, ..)| id.clone())),
        )
        .unwrap();
        // The turn's paths count from the snapshot before, rebased onto
        // its parent as that is now, with what landed since on top: the
        // main chat's own commit alone.
        let (_, then, at, parent_then) = since.unwrap();
        let mut base = rebase_tree(&self.main_chain[at].0, &parent_then, &then);
        if !self.main_landed.is_empty() {
            tc.event("a main chat turn leaves out what landed");
        }
        for at in std::mem::take(&mut self.main_landed) {
            let (tree, _) = &self.main_chain[at];
            base = rebase_tree(tree, &self.main_chain[at - 1].0, &base);
        }
        let head = self.main_chain.last().unwrap().0.clone();
        let got: Vec<(String, String)> = turn
            .paths
            .iter()
            .map(|path| {
                let kind = changes(&base, &head)
                    .into_iter()
                    .find(|(p, _)| p == path)
                    .map(|(_, kind)| kind)
                    .unwrap_or_default();
                (path.clone(), kind)
            })
            .collect();
        check_changes(&got, &base, &head, "the main chat's turn");
        let at = self.main_chain.len() - 1;
        self.main_since = Some((turn.commit_id, head.clone(), at, head));
    }

    /// Another chat starts on trunk, commits, and lands on the main chat
    /// or is dropped; then its workspace goes.
    fn do_another_chat(&mut self, tc: &TestCase) {
        tc.assume(self.chats < MAX_CHATS);
        let edits = tc.draw(edits(1));
        let land = tc.draw(gs::booleans());
        let name = format!("other{}", self.chats);
        self.chats += 1;
        let trunk = self.project.trunk().unwrap();
        let other =
            Workspace::new(self.project.add_workspace(&name, &trunk).unwrap());
        let head = self.main_chain.last().unwrap().0.clone();
        let tree = write_edits(&self.dir(&name), &head, &edits);
        let (_, details) = other
            .ok("vcs_commit", json!({ "message": format!("{name}'s work") }));
        let committed = &details["committed"];
        let commit = committed["commit_id"].as_str().unwrap();
        if land {
            tc.event("another chat lands");
            let name = self.project.trunk_name().unwrap();
            let landing =
                block_on(self.main.vcs.land(commit, name, true)).unwrap();
            assert_eq!(landing.head, commit, "a landing on trunk's head moved");
            let id = committed["change_id"].as_str().unwrap().to_owned();
            self.main_chain.push((tree, id));
            self.main_landed.push(self.main_chain.len() - 1);
        } else {
            tc.event("another chat is dropped");
            assert_eq!(
                self.project.abandon_between(&trunk, commit).unwrap(),
                1
            );
        }
        self.project.forget_workspace(&name).unwrap();
        self.other();
    }

    /// The idle chat snapshots edits, as any of its tools does first,
    /// or ends a turn. Neither is an operation undo stops at.
    fn do_idle_snapshot(&mut self, tc: &TestCase) {
        let edits = tc.draw(edits(1));
        let end_turn = tc.draw(gs::booleans());
        write_edits(&self.dir("idle"), &Tree::new(), &edits);
        if end_turn {
            block_on(self.idle.vcs.end_turn("tau/idle", None)).unwrap();
        } else {
            self.idle.ok("vcs_status", json!({}));
        }
        self.passed = true;
    }

    /// The idle chat describes its `@`: another workspace's tool.
    fn do_idle_describe(&mut self, tc: &TestCase) {
        let message = tc.draw(message());
        self.idle.ok("vcs_describe", json!({ "message": message }));
        self.other();
    }
}

#[hegel::state_machine]
impl Machine {
    #[rule(weight = 4)]
    fn write(&mut self, tc: TestCase) {
        self.do_write(&tc);
    }

    #[rule]
    fn describe(&mut self, tc: TestCase) {
        self.do_describe(&tc);
    }

    #[rule(weight = 2)]
    fn commit(&mut self, tc: TestCase) {
        self.do_commit(&tc);
    }

    #[rule]
    fn new_change(&mut self, tc: TestCase) {
        self.do_new_change(&tc);
    }

    #[rule(weight = 2)]
    fn restore(&mut self, tc: TestCase) {
        self.do_restore(&tc);
    }

    #[rule(weight = 3)]
    fn undo(&mut self, tc: TestCase) {
        self.do_undo(&tc);
    }

    #[rule]
    fn commit_all(&mut self, tc: TestCase) {
        self.do_commit_all(&tc);
    }

    #[rule(weight = 2)]
    fn end_turn(&mut self, tc: TestCase) {
        self.do_end_turn(&tc);
    }

    #[rule]
    fn show(&mut self, tc: TestCase) {
        self.do_show(&tc);
    }

    #[rule]
    fn show_seen(&mut self, tc: TestCase) {
        self.do_show_seen(&tc);
    }

    #[rule]
    fn show_dead(&mut self, tc: TestCase) {
        self.do_show_dead(&tc);
    }

    #[rule(weight = 3)]
    fn upstream_and_catch_up(&mut self, tc: TestCase) {
        self.do_upstream_and_catch_up(&tc);
    }

    #[rule]
    fn main_commit(&mut self, tc: TestCase) {
        self.do_main_commit(&tc);
    }

    #[rule]
    fn another_chat(&mut self, tc: TestCase) {
        self.do_another_chat(&tc);
    }

    #[rule(weight = 2)]
    fn idle_snapshot(&mut self, tc: TestCase) {
        self.do_idle_snapshot(&tc);
    }

    #[rule]
    fn idle_describe(&mut self, tc: TestCase) {
        self.do_idle_describe(&tc);
    }

    /// After every step the chat's files are the model's, and, unless a
    /// rewrite waits for the chat's next tool, so is what its tools see.
    #[invariant(always_run)]
    fn the_chat_is_the_model(&mut self, tc: TestCase) {
        check_files(&self.dir("chat"), &self.wc.tree, "the chat");
        if self.stale.is_some() {
            return;
        }
        let parent = self.parent_tree();
        let (_, status) = self.chat.ok("vcs_status", json!({}));
        self.seen = self.wc.tree.clone();
        check_changes(
            &listed(&status, "changes"),
            &parent,
            &self.wc.tree,
            "status",
        );
        assert_eq!(
            status["conflicts"],
            json!(conflicts(&self.wc.tree)),
            "status conflicts"
        );
        let wc = &status["working_copy"];
        assert_eq!(wc["change_id"], json!(self.wc.change_id), "@'s change id");
        assert_eq!(wc["description"], json!(self.wc.description));
        if conflicts(&parent).is_empty() && conflicts(&self.wc.tree).is_empty()
        {
            assert_eq!(
                wc["empty"],
                json!(parent == self.wc.tree),
                "@'s empty flag"
            );
        }
        let parent_id = self
            .stack
            .last()
            .map(|change| change.change_id.clone())
            .unwrap_or_else(|| self.main_chain[self.base].1.clone());
        assert_eq!(status["parents"][0]["change_id"], json!(parent_id));

        let (_, log) = self.chat.ok("vcs_log", json!({ "limit": 100 }));
        let rows = log["changes"].as_array().unwrap();
        assert_eq!(
            rows.len(),
            1 + self.stack.len() + self.base + 1 + self.upstream_commits,
            "log rows: {log}"
        );
        let mine = std::iter::once(&self.wc).chain(self.stack.iter().rev());
        for (row, change) in rows.iter().zip(mine) {
            assert_eq!(row["change_id"], json!(change.change_id));
            assert_eq!(row["description"], json!(change.description));
            assert_eq!(row["divergent"], json!(false));
        }
        assert_eq!(
            rows[1 + self.stack.len()]["change_id"],
            json!(self.main_chain[self.base].1),
            "the chat's base"
        );
        let ids: Vec<String> = (0..self.stack.len())
            .map(|at| {
                rows[self.stack.len() - at]["commit_id"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        for (at, id) in ids.iter().enumerate() {
            self.note(id, at);
        }
        tc.event_value("stack", self.stack.len() as f64);
    }
}

/// The chat's tools against the model while other workspaces act: see
/// the module docs. Each case imports a project and runs up to 30 steps.
#[hegel::test(
    test_cases = 30,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn the_tools_hold_while_other_workspaces_act(tc: TestCase) {
    hegel::stateful::machine(Machine::new()).steps(30).run(tc);
}

#[hegel::test(
    profile = "nightly_slow",
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
#[ignore = "nightly"]
fn the_tools_hold_while_other_workspaces_act_nightly(tc: TestCase) {
    hegel::stateful::machine(Machine::new()).steps(60).run(tc);
}

/// The chat's tools that write: what `vcs_undo` may undo.
const TOOLS: [&str; 4] = ["describe", "commit", "new", "restore"];

/// What happens between a chat's tool and its `vcs_undo`: in other
/// workspaces, or the host's in the chat's.
const BETWEEN: [&str; 7] = [
    "idle snapshot",
    "idle describe",
    "main commit",
    "catch-up",
    "another chat",
    "end turn",
    "commit all",
];

/// Undo, head on: the chat's tool, then a drawn run of what other
/// workspaces and the host do, then `vcs_undo` twice. Undo passes over
/// snapshots and turn ends alone, refuses at anything else, and right
/// after a catch-up refuses with the files moved. Against the same
/// model; the state machine reaches these orders only now and then.
#[hegel::test(
    test_cases = 30,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn undo_stops_at_what_others_did(tc: TestCase) {
    let mut m = Machine::new();
    m.do_write(&tc);
    match tc.draw(gs::sampled_from(TOOLS.to_vec())) {
        "describe" => m.do_describe(&tc),
        "commit" => m.do_commit(&tc),
        "new" => m.do_new_change(&tc),
        _ => m.do_restore(&tc),
    }
    m.the_chat_is_the_model(tc.clone());
    // Half the time only what undo passes over.
    let kinds = if tc.draw(gs::booleans()) {
        vec!["idle snapshot", "end turn"]
    } else {
        BETWEEN.to_vec()
    };
    let between =
        tc.draw(gs::vecs(gs::sampled_from(kinds)).min_size(1).max_size(3));
    for what in between {
        if tc.draw(gs::booleans()) {
            m.do_write(&tc);
        }
        match what {
            "idle snapshot" => m.do_idle_snapshot(&tc),
            "idle describe" => m.do_idle_describe(&tc),
            "main commit" => m.do_main_commit(&tc),
            "catch-up" => m.do_upstream_and_catch_up(&tc),
            "another chat" => m.do_another_chat(&tc),
            "end turn" => m.do_end_turn(&tc),
            _ => m.do_commit_all(&tc),
        }
        m.the_chat_is_the_model(tc.clone());
    }
    m.do_undo(&tc);
    m.the_chat_is_the_model(tc.clone());
    m.do_undo(&tc);
    m.the_chat_is_the_model(tc.clone());
}

// Regressions, pinned as they happened.

/// The main chat's `vcs_undo` before any tool of its own refuses: the
/// newest operation in its workspace is the host's catch-up with trunk.
/// It used to undo it, tagged as a tool's (`move_onto`): `@` went back
/// to the root commit, off trunk, and a catch-up that had restacked the
/// commits chats stand on would have been taken back under them.
#[test]
fn undo_refuses_the_hosts_catch_up() {
    let (_home, project, main) = project();
    let trunk = project.trunk().unwrap();
    assert_refused(main.call("vcs_undo", json!({})));
    let (_, status) = main.ok("vcs_status", json!({}));
    assert_eq!(status["parents"][0]["commit_id"], json!(trunk));
}

/// The same for a landing on the main chat: undoing it would hide the
/// landed changes its links name.
#[test]
fn undo_refuses_a_landing() {
    let (_home, project, main) = project();
    let trunk = project.trunk().unwrap();
    let chat = Workspace::new(project.add_workspace("c", &trunk).unwrap());
    std::fs::write(project.workspace_dir("c").join("b.txt"), "b\n").unwrap();
    let (_, committed) = chat.ok("vcs_commit", json!({ "message": "Add b" }));
    let head = committed["committed"]["commit_id"].as_str().unwrap();
    let name = project.trunk_name().unwrap();
    let landing = block_on(main.vcs.land(head, name, true)).unwrap();
    assert_refused(main.call("vcs_undo", json!({})));
    assert_eq!(project.trunk().unwrap(), landing.head);
}

/// A project, its main chat with a commit of its own on trunk, and a
/// chat started on that commit, with the chat's directory.
fn a_chat_on_main() -> (
    tempfile::TempDir,
    ProjectRepo,
    Workspace,
    Workspace,
    PathBuf,
) {
    let (home, project, main) = project();
    let name = project.trunk_name().unwrap();
    std::fs::write(
        project.workspace_dir(DEFAULT_WORKSPACE).join("b.txt"),
        "b\n",
    )
    .unwrap();
    let (_, committed) = main.ok("vcs_commit", json!({ "message": "Add b" }));
    block_on(main.vcs.end_turn(name, None)).unwrap();
    let head = committed["committed"]["commit_id"].as_str().unwrap();
    let chat = Workspace::new(project.add_workspace("chat", head).unwrap());
    let dir = project.workspace_dir("chat");
    (home, project, main, chat, dir)
}

/// Upstream writes `a.txt`, an update brings it in, and the main chat
/// catches up, restacking the commit the chat stands on.
fn upstream_writes_a(home: &Path, project: &ProjectRepo, main: &Workspace) {
    let src = home.join("src");
    std::fs::write(src.join("a.txt"), "three\n").unwrap();
    git(&src, &["commit", "--quiet", "-am", "upstream"]);
    project.update(UpdateFrom::Checkout(&src)).unwrap();
    let trunk = project.trunk().unwrap();
    let name = project.trunk_name().unwrap();
    block_on(main.vcs.move_onto(trunk, name, true)).unwrap();
}

/// A chat whose `@` holds a conflict in `a.txt`: upstream changed it,
/// and the chat changed it too before its next tool.
fn a_chat_in_conflict() -> (tempfile::TempDir, ProjectRepo, Workspace, PathBuf)
{
    let (home, project, main, chat, dir) = a_chat_on_main();
    upstream_writes_a(home.path(), &project, &main);
    std::fs::write(dir.join("a.txt"), "two\n").unwrap();
    let (_, status) = chat.ok("vcs_status", json!({}));
    assert_eq!(status["conflicts"], json!(["a.txt"]));
    (home, project, chat, dir)
}

/// A tool that fails after its snapshot keeps the snapshot: the next
/// tool sees the files as they are. A `vcs_undo` the catch-up made
/// refuse used to leave the working copy's record behind, so the next
/// tool took the snapshot's own changes for edits made since and applied
/// them again: `a.txt`, resolved by hand, came back as a conflict.
#[test]
fn a_refused_tool_keeps_its_snapshot() {
    let (_home, _project, chat, dir) = a_chat_in_conflict();
    std::fs::write(dir.join("a.txt"), "resolved\n").unwrap();
    assert_refused(chat.call("vcs_undo", json!({})));
    let (_, status) = chat.ok("vcs_status", json!({}));
    assert_eq!(status["conflicts"], json!([]));
    assert_eq!(
        std::fs::read_to_string(dir.join("a.txt")).unwrap(),
        "resolved\n"
    );
}

/// A turn lists only what it changed: what a catch-up brought between
/// two turns, by restacking the commit the chat stands on, is not the
/// turn's. It used to be: `end_turn` diffed `@` against the turn
/// before's snapshot as it was, so upstream's `a.txt` showed up as the
/// turn's, and memory marked notes about it stale as this run's.
#[test]
fn a_turns_paths_leave_out_what_a_catch_up_brought() {
    let (home, project, main, chat, dir) = a_chat_on_main();
    let first = block_on(chat.vcs.end_turn("tau/chat", None)).unwrap();
    upstream_writes_a(home.path(), &project, &main);
    std::fs::write(dir.join("c.txt"), "c\n").unwrap();
    let second =
        block_on(chat.vcs.end_turn("tau/chat", Some(first.commit_id))).unwrap();
    assert_eq!(second.paths, ["c.txt"], "the turn only wrote c.txt");
}

/// The same for a landing: the main chat's turn after a chat landed on
/// it lists only its own edits. The landed changes are recorded by
/// their own links. It used to list the landed files as the turn's.
#[test]
fn a_turns_paths_leave_out_what_landed() {
    let (_home, project, main, chat, dir) = a_chat_on_main();
    let name = project.trunk_name().unwrap();
    let first = block_on(main.vcs.end_turn(name.clone(), None)).unwrap();
    std::fs::write(dir.join("d.txt"), "d\n").unwrap();
    let (_, committed) = chat.ok("vcs_commit", json!({ "message": "Add d" }));
    let head = committed["committed"]["commit_id"].as_str().unwrap();
    block_on(main.vcs.land(head, name.clone(), true)).unwrap();
    let main_dir = project.workspace_dir(DEFAULT_WORKSPACE);
    std::fs::write(main_dir.join("c.txt"), "c\n").unwrap();
    let second =
        block_on(main.vcs.end_turn(name, Some(first.commit_id))).unwrap();
    assert_eq!(second.paths, ["c.txt"], "the turn only wrote c.txt");
}

/// An undone tool stays undone when an update brings in Git's refs. A
/// `vcs_undo` used to take back the view's record of Git's refs with
/// the rest of the operation, so the next export found the run's
/// bookmark, back on `@`, already where the record said and left Git's
/// branch on the commit the undone `vcs_describe` made. The update's
/// import took that for a move made in Git: the described commit came
/// back under the bookmark, a divergent twin of `@`, and after the
/// catch-up every tool refused the chat as stale.
#[test]
fn an_undone_describe_stays_undone_after_an_update() {
    let (home, project, main, chat, _dir) = a_chat_on_main();
    // The run's bookmark on `@`: committed, then the commit undone.
    chat.ok("vcs_commit", json!({ "message": "Fix the parser" }));
    block_on(chat.vcs.commit_all("tau: the end", "tau/chat")).unwrap();
    chat.ok("vcs_undo", json!({}));
    // The describe moves the bookmark with `@`; its undo moves it back.
    chat.ok("vcs_describe", json!({ "message": "Fix the parser" }));
    chat.ok("vcs_undo", json!({}));
    upstream_writes_a(home.path(), &project, &main);
    let (_, details) =
        chat.ok("vcs_describe", json!({ "message": "Add a test" }));
    assert_eq!(details["working_copy"]["divergent"], json!(false));
    let (_, log) = chat.ok("vcs_log", json!({ "limit": 100 }));
    let rows = log["changes"].as_array().unwrap();
    assert_eq!(rows[0]["description"], json!("Add a test\n"));
    assert!(
        rows.iter().all(|row| row["divergent"] == json!(false)),
        "{log}"
    );
}

/// An undo keeps what was edited since its operation, and only that. A
/// conflict a snapshot since wrote in another form, without a change to
/// the file, was taken for an edit: undoing a `vcs_restore` that
/// brought back the conflict of `@`'s parent kept the conflict.
#[test]
fn an_undo_takes_back_a_restored_conflict() {
    let (home, project, main, chat, dir) = a_chat_on_main();
    // Deleted in the chat, changed or added upstream: the catch-up
    // leaves `@` in conflict at a.txt, and `@`'s parent at b.txt, with
    // `@` holding upstream's b.txt.
    std::fs::remove_file(dir.join("a.txt")).unwrap();
    std::fs::remove_file(dir.join("b.txt")).unwrap();
    let src = home.path().join("src");
    std::fs::write(src.join("a.txt"), "two\n").unwrap();
    std::fs::write(src.join("b.txt"), "two\n").unwrap();
    git(&src, &["add", "-A"]);
    git(&src, &["commit", "--quiet", "-m", "upstream"]);
    project.update(UpdateFrom::Checkout(&src)).unwrap();
    let trunk = project.trunk().unwrap();
    let name = project.trunk_name().unwrap();
    block_on(main.vcs.move_onto(trunk, name, true)).unwrap();
    let b = || std::fs::read_to_string(dir.join("b.txt")).unwrap();
    let (_, status) = chat.ok("vcs_status", json!({}));
    assert_eq!(status["conflicts"], json!(["a.txt"]));
    assert_eq!(b(), "two\n");
    chat.ok("vcs_restore", json!({ "paths": ["b.txt"] }));
    assert!(b().contains("<<<<<<<"), "{}", b());
    // An edit since, and a snapshot of it.
    std::fs::remove_file(dir.join("a.txt")).unwrap();
    chat.ok("vcs_status", json!({}));
    chat.ok("vcs_undo", json!({}));
    assert_eq!(b(), "two\n", "the restore is undone");
    assert!(!dir.join("a.txt").exists(), "the edit since stays");
}

/// `@` that holds its parent's conflict as a restore from the parent
/// wrote it, in another form than the parent's, changes nothing, and
/// the host's commit at a run's end commits nothing. It used to commit
/// it: jj's emptiness compares the trees as written.
#[test]
fn a_restored_conflict_is_nothing_to_commit() {
    let (home, project, main, chat, dir) = a_chat_on_main();
    let src = home.path().join("src");
    let catch_up = |path: &str| {
        std::fs::create_dir_all(src.join(path).parent().unwrap()).unwrap();
        std::fs::write(src.join(path), "two\n").unwrap();
        git(&src, &["add", "-A"]);
        git(&src, &["commit", "--quiet", "-m", "upstream"]);
        project.update(UpdateFrom::Checkout(&src)).unwrap();
        let trunk = project.trunk().unwrap();
        let name = project.trunk_name().unwrap();
        block_on(main.vcs.move_onto(trunk, name, true)).unwrap();
    };
    // a.txt deleted in the chat and changed upstream: `@` is in
    // conflict there when the chat commits.
    std::fs::remove_file(dir.join("a.txt")).unwrap();
    std::fs::create_dir_all(dir.join("dir")).unwrap();
    std::fs::write(dir.join("dir/c.txt"), "one\n").unwrap();
    catch_up("a.txt");
    chat.ok("vcs_commit", json!({ "message": "Fix the parser" }));
    // dir/c.txt added both in the chat's commit and upstream, and
    // deleted in `@`: the commit is in conflict there, `@` has
    // upstream's.
    std::fs::remove_file(dir.join("dir/c.txt")).unwrap();
    catch_up("dir/c.txt");
    chat.ok("vcs_restore", json!({ "paths": ["dir/c.txt"] }));
    let (_, status) = chat.ok("vcs_status", json!({}));
    assert_eq!(status["changes"], json!([]), "{status}");
    let turn =
        block_on(chat.vcs.commit_all("tau: the end", "tau/chat")).unwrap();
    assert!(!turn.changed, "nothing to commit, yet it committed");
}

/// A catch-up made by another process while a chat's tool snapshots
/// forks the operation log: both start from the same operation, and the
/// next load merges them, leaving the chat's `@` divergent. Inside one
/// process the repository's lock keeps them apart. Across processes the
/// tools refuse with the stale error, as the reference says, rather than
/// pick one copy of `@`: either may hold the chat's work.
#[test]
fn a_catch_up_from_another_process_leaves_the_chat_stale() {
    use jj_lib::{backend::CommitId, repo::Repo as _, rewrite::rebase_commit};
    let (home, project, _main, chat, dir) = a_chat_on_main();
    let src = home.path().join("src");
    std::fs::write(src.join("a.txt"), "three\n").unwrap();
    git(&src, &["commit", "--quiet", "-am", "upstream"]);
    project.update(UpdateFrom::Checkout(&src)).unwrap();
    // The repository as both the chat's tool and the catch-up find it.
    let repo = jj_repo(home.path());
    std::fs::write(dir.join("c.txt"), "c\n").unwrap();
    chat.ok("vcs_status", json!({}));
    // Meanwhile the main chat's catch-up, from the same operation, as
    // `Vcs::move_onto` makes it.
    let id = |hex: &str| CommitId::try_from_hex(hex).unwrap();
    let head = project.workspace_head(DEFAULT_WORKSPACE).unwrap().unwrap();
    let head = project.parent_of(&head).unwrap().unwrap();
    let trunk = project.trunk().unwrap();
    let mut tx = repo.start_transaction();
    let commit = repo.store().get_commit(&id(&head)).unwrap();
    pollster::block_on(rebase_commit(tx.repo_mut(), commit, vec![id(&trunk)]))
        .unwrap();
    pollster::block_on(tx.repo_mut().rebase_descendants()).unwrap();
    pollster::block_on(tx.commit("tau vcs: move_onto")).unwrap();

    let error = chat.call("vcs_status", json!({})).unwrap_err();
    assert!(error.contains("The working copy is stale"), "{error}");
}

/// The repository at its newest operation, loaded through jj-lib.
fn jj_repo(home: &Path) -> std::sync::Arc<jj_lib::repo::ReadonlyRepo> {
    use jj_lib::{
        config::{ConfigLayer, ConfigSource, StackedConfig},
        default_backend_factories::{
            default_backend_factories,
            default_working_copy_factories,
        },
        settings::UserSettings,
        workspace::Workspace as JjWorkspace,
    };
    let mut config = StackedConfig::with_defaults();
    let mut user = ConfigLayer::empty(ConfigSource::User);
    user.set_value("user.name", "host").unwrap();
    user.set_value("user.email", "host@localhost").unwrap();
    config.add_layer(user);
    let settings = UserSettings::from_config(config).unwrap();
    let workspace = JjWorkspace::load(
        &settings,
        &home.join("p").join("main"),
        &default_backend_factories(),
        &default_working_copy_factories(),
    )
    .unwrap();
    pollster::block_on(workspace.repo_loader().load_at_head()).unwrap()
}
