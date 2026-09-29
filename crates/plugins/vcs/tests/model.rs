//! The vcs tools against a model (`docs/reference/vcs.md`): random
//! sequences of file edits, tool calls and turn checkpoints on a real jj
//! repository, checked after every step against a stack of changes, each
//! a description and a file tree.
//!
//! The model is what the reference promises:
//! - every tool snapshots first, so `@`'s tree is always the files on
//!   disk;
//! - `describe` rewrites `@`'s description and keeps its change id;
//!   `commit` describes `@` and starts an empty change on top; `new`
//!   starts one without describing; files never move;
//! - `restore` makes the named paths (and everything under them) match
//!   the source, deleting what the source lacks;
//! - `undo` reverts the tools' newest operation in this workspace, keeps
//!   file edits made since, and refuses anything the tools did not make;
//! - a turn checkpoint commits `@` when it changed anything.
//!
//! Every diff the tools show is also applied to the parent's file, and
//! must give back the child's.

use std::{collections::BTreeMap, sync::Arc};

use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_agent::{
    plugin::Plugin,
    tool::{AgentTool, RunId, ToolCtx, ToolUpdates},
};
use tau_ai::message::InputBlock;
use tau_testing::block_on;
use tau_vcs::{Identity, Vcs, VcsPlugin};
use tokio_util::sync::CancellationToken;

/// The files a test touches: nested, so a directory can be restored as
/// a whole, and one name that is not ASCII.
const PATHS: [&str; 5] =
    ["a.txt", "dir/b.txt", "dir/sub/c.txt", "é.txt", "blob.bin"];

/// What `restore` can name: files, a directory, everything.
const RESTORE_PATHS: [&str; 7] = [
    "a.txt",
    "dir/b.txt",
    "dir/sub/c.txt",
    "é.txt",
    "dir",
    "dir/sub",
    ".",
];

/// A file tree: path to contents.
type Tree = BTreeMap<String, Vec<u8>>;

/// One change: its description as jj stores it, its files, and its
/// change id once a tool has shown it.
#[derive(Debug, Clone, PartialEq)]
struct Change {
    description: String,
    tree: Tree,
    change_id: Option<String>,
}

impl Change {
    fn empty(tree: Tree) -> Self {
        Self {
            description: String::new(),
            tree,
            change_id: None,
        }
    }
}

/// What `undo` would find, newest last.
#[derive(Debug, Clone)]
enum Op {
    /// A tool's operation, and the model as it was before it.
    Tool {
        stack: Vec<Change>,
        wc: Change,
        /// The files right after the operation: edits made since are
        /// the ones undo must keep.
        after: Tree,
    },
    /// A turn checkpoint: the host's, not the tools'.
    Checkpoint,
}

struct Machine {
    dir: tempfile::TempDir,
    vcs: Vcs,
    tools: Vec<Arc<dyn AgentTool>>,
    /// Committed changes above the root, oldest first.
    stack: Vec<Change>,
    /// The working-copy change. Its tree is the files on disk as of the
    /// last tool call; [`Machine::disk`] is the files now.
    wc: Change,
    ops: Vec<Op>,
}

fn ctx() -> ToolCtx {
    let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
    ToolCtx::new(
        CancellationToken::new(),
        ToolUpdates::for_tests("call_1", sender),
        RunId("run_1".into()),
    )
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

/// `(path, kind)` for every path that differs from `from` to `to`, in
/// path order, as the tools list changes.
fn changes(from: &Tree, to: &Tree) -> Vec<(String, &'static str)> {
    let mut paths: Vec<&String> = from.keys().chain(to.keys()).collect();
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter_map(|path| match (from.get(path), to.get(path)) {
            (None, Some(_)) => Some((path.clone(), "added")),
            (Some(_), None) => Some((path.clone(), "removed")),
            (Some(a), Some(b)) if a != b => Some((path.clone(), "modified")),
            _ => None,
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

fn owned(changes: Vec<(String, &'static str)>) -> Vec<(String, String)> {
    changes
        .into_iter()
        .map(|(path, kind)| (path, kind.to_owned()))
        .collect()
}

/// Text: lines of a small vocabulary, so diffs have context, maybe
/// without a final newline, maybe empty. Or a few binary bytes.
#[hegel::composite]
fn contents(tc: &TestCase) -> Vec<u8> {
    if tc.draw(gs::weighted_booleans(0.1)) {
        let mut bytes = vec![0u8, 1, 2];
        bytes.extend(tc.draw(gs::binary().max_size(6)));
        return bytes;
    }
    let lines = tc.draw(
        gs::vecs(gs::sampled_from(vec!["alpha", "beta", "gamma", "delta"]))
            .max_size(6),
    );
    let mut text = lines.join("\n");
    if !lines.is_empty() && tc.draw(gs::booleans()) {
        text.push('\n');
    }
    text.into_bytes()
}

/// A description a model might write: several lines, trailing space,
/// or nothing at all.
#[hegel::composite]
fn message(tc: &TestCase) -> String {
    tc.draw(hegel::one_of!(
        gs::sampled_from(vec![
            "Fix the parser".to_owned(),
            "Add a test\n\nWith a body.".to_owned(),
            "trailing space   \n\n".to_owned(),
            String::new(),
            "   ".to_owned(),
        ]),
        gs::text().max_size(20),
    ))
}

impl Machine {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let vcs = Vcs::init(dir.path(), Identity::default()).unwrap();
        let tools = VcsPlugin::new(vcs.clone()).tools();
        let mut machine = Self {
            dir,
            vcs,
            tools,
            stack: Vec::new(),
            wc: Change::empty(Tree::new()),
            ops: Vec::new(),
        };
        let (_, status) = machine.ok("vcs_status", json!({}));
        machine.wc.change_id = Some(change_id(&status["working_copy"]));
        machine
    }

    /// The files on disk now.
    fn disk(&self) -> Tree {
        let mut tree = Tree::new();
        for path in PATHS {
            if let Ok(bytes) = std::fs::read(self.dir.path().join(path)) {
                tree.insert(path.to_owned(), bytes);
            }
        }
        tree
    }

    fn parent_tree(&self) -> Tree {
        self.stack
            .last()
            .map(|c| c.tree.clone())
            .unwrap_or_default()
    }

    fn call(&self, name: &str, args: Value) -> Result<(String, Value), String> {
        let tool = self
            .tools
            .iter()
            .find(|tool| tool.name() == name)
            .unwrap_or_else(|| panic!("no tool {name}"));
        block_on(tool.call(args, ctx()))
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

    /// Before a tool call: `@` takes the files on disk, as its snapshot
    /// does, in an operation of its own. Returns the model after that
    /// snapshot and before the call: what undoing the call goes back to.
    fn before_tool(&mut self) -> (Vec<Change>, Change) {
        self.wc.tree = self.disk();
        (self.stack.clone(), self.wc.clone())
    }

    fn after_tool(&mut self, before: (Vec<Change>, Change)) {
        let (stack, wc) = before;
        self.ops.push(Op::Tool {
            stack,
            wc,
            after: self.disk(),
        });
    }

    /// Reads `@`'s change id from a tool's `working_copy` details.
    fn learn_wc(&mut self, details: &Value) {
        let id = change_id(&details["working_copy"]);
        if let Some(known) = &self.wc.change_id {
            assert_eq!(&id, known, "@ changed its change id");
        }
        self.wc.change_id = Some(id);
    }

    /// The model's changes, newest first, as `vcs_log` lists them.
    fn log_rows(&self) -> Vec<&Change> {
        std::iter::once(&self.wc)
            .chain(self.stack.iter().rev())
            .collect()
    }
}

fn change_id(info: &Value) -> String {
    info["change_id"]
        .as_str()
        .unwrap_or_else(|| panic!("no change id in {info}"))
        .to_owned()
}

#[hegel::state_machine]
impl Machine {
    /// The model's agent edits a file, as `write` or `edit` would.
    #[rule(weight = 4)]
    fn write_file(&mut self, tc: TestCase) {
        let path = tc.draw(gs::sampled_from(PATHS.to_vec()));
        let bytes = tc.draw(contents());
        let file = self.dir.path().join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, bytes).unwrap();
    }

    /// Rewrites a file with other text of the same size: jj notices
    /// changes by size and modification time, so this is the edit it
    /// could miss.
    #[rule]
    fn rewrite_same_size(&mut self, tc: TestCase) {
        let disk = self.disk();
        let present: Vec<String> = disk.keys().cloned().collect();
        tc.assume(!present.is_empty());
        let path = tc.draw(gs::sampled_from(present));
        let old = &disk[&path];
        tc.assume(!old.is_empty());
        // Same length, different bytes: flip the first byte's case bit,
        // or bump it.
        let mut new = old.clone();
        new[0] = if new[0].is_ascii_alphabetic() {
            new[0] ^ 0x20
        } else {
            new[0].wrapping_add(1)
        };
        std::fs::write(self.dir.path().join(&path), new).unwrap();
    }

    #[rule]
    fn delete_file(&mut self, tc: TestCase) {
        let present: Vec<String> = self.disk().keys().cloned().collect();
        tc.assume(!present.is_empty());
        let path = tc.draw(gs::sampled_from(present));
        std::fs::remove_file(self.dir.path().join(path)).unwrap();
    }

    #[rule]
    fn describe(&mut self, tc: TestCase) {
        let message = tc.draw(message());
        let before = self.before_tool();
        let (_, details) =
            self.ok("vcs_describe", json!({ "message": message }));
        self.wc.description = stored(&message);
        self.learn_wc(&details);
        self.after_tool(before);
    }

    #[rule(weight = 2)]
    fn commit(&mut self, tc: TestCase) {
        let message = tc.draw(message());
        if message.trim().is_empty() {
            let err = self
                .call("vcs_commit", json!({ "message": message }))
                .unwrap_err();
            assert_eq!(err, "The description must not be empty");
            return;
        }
        let before = self.before_tool();
        let (_, details) = self.ok("vcs_commit", json!({ "message": message }));
        // The working copy is what was committed: same change id.
        assert_eq!(
            change_id(&details["committed"]),
            self.wc.change_id.clone().unwrap(),
            "commit gave @ a new change id"
        );
        self.wc.description = stored(&message);
        let files = self.disk();
        let committed = std::mem::replace(&mut self.wc, Change::empty(files));
        self.stack.push(committed);
        let id = change_id(&details["working_copy"]);
        assert_ne!(Some(&id), self.stack.last().unwrap().change_id.as_ref());
        self.wc.change_id = Some(id);
        self.after_tool(before);
    }

    #[rule]
    fn new_change(&mut self, tc: TestCase) {
        let message = tc.draw(gs::optional(message()));
        let before = self.before_tool();
        let args = match &message {
            Some(message) => json!({ "message": message }),
            None => json!({}),
        };
        let (_, details) = self.ok("vcs_new", args);
        let files = self.disk();
        let old = std::mem::replace(&mut self.wc, Change::empty(files));
        self.stack.push(old);
        self.wc.description = stored(message.as_deref().unwrap_or(""));
        self.wc.change_id = Some(change_id(&details["working_copy"]));
        self.after_tool(before);
    }

    /// Restores paths from `@`'s parent, or from a change in the stack.
    #[rule(weight = 2)]
    fn restore(&mut self, tc: TestCase) {
        let paths: Vec<&str> = tc.draw(
            gs::vecs(gs::sampled_from(RESTORE_PATHS.to_vec()))
                .min_size(1)
                .max_size(2),
        );
        let from = if self.stack.is_empty() || tc.draw(gs::booleans()) {
            None
        } else {
            let at = tc
                .draw(gs::integers::<usize>().max_value(self.stack.len() - 1));
            Some(at)
        };
        let before = self.before_tool();
        let source = match from {
            Some(at) => self.stack[at].tree.clone(),
            None => self.parent_tree(),
        };
        let mut args = json!({ "paths": paths });
        if let Some(at) = from {
            args["from"] = json!(self.stack[at].change_id.clone().unwrap());
        }
        let (_, details) = self.ok("vcs_restore", args);

        let old = self.wc.tree.clone();
        let mut new = old.clone();
        new.retain(|path, _| !paths.iter().any(|name| under(path, name)));
        for (path, bytes) in &source {
            if paths.iter().any(|name| under(path, name)) {
                new.insert(path.clone(), bytes.clone());
            }
        }
        // `restored` lists the paths that changed, in @ versus before.
        let restored: Vec<String> =
            changes(&old, &new).into_iter().map(|(p, _)| p).collect();
        let got: Vec<String> = listed(&details, "restored")
            .into_iter()
            .map(|(p, _)| p)
            .collect();
        assert_eq!(got, restored, "restored paths");
        self.wc.tree = new;
        assert_eq!(self.disk(), self.wc.tree, "restore left the files wrong");
        self.learn_wc(&details);
        // Restoring what already matches writes no operation.
        if !restored.is_empty() {
            self.after_tool(before);
        }
    }

    /// Undoes the tools' newest operation, or is refused.
    #[rule(weight = 2)]
    fn undo(&mut self, _tc: TestCase) {
        let result = self.call("vcs_undo", json!({}));
        match self.ops.pop() {
            None | Some(Op::Checkpoint) => {
                let err = result
                    .expect_err("undo of an operation the tools did not make");
                assert!(
                    err.starts_with(
                        "The last operation was not made by the vcs tools"
                    ),
                    "{err}"
                );
                // The refusal changes nothing; a checkpoint stays on top.
                if !self.stack.is_empty() || !self.ops.is_empty() {
                    self.ops.push(Op::Checkpoint);
                }
                self.wc.tree = self.disk();
            }
            Some(Op::Tool { stack, wc, after }) => {
                let (_, details) =
                    result.unwrap_or_else(|err| panic!("undo failed: {err}"));
                // Edits made since the operation stay.
                let now = self.disk();
                let mut tree = wc.tree.clone();
                for (path, _) in changes(&after, &now) {
                    match now.get(&path) {
                        Some(bytes) => tree.insert(path, bytes.clone()),
                        None => tree.remove(&path),
                    };
                }
                self.stack = stack;
                self.wc = Change { tree, ..wc };
                assert_eq!(
                    self.disk(),
                    self.wc.tree,
                    "undo left the files wrong"
                );
                self.learn_wc(&details);
            }
        }
    }

    /// The host ends a turn, as `RunWorkspace` does after `TurnEnd`.
    #[rule]
    fn checkpoint(&mut self, _tc: TestCase) {
        self.wc.tree = self.disk();
        let turn = block_on(self.vcs.checkpoint("tau: run 1 turn 1", "tau/1"))
            .unwrap();
        let parent = self.parent_tree();
        if self.wc.tree == parent {
            assert!(!turn.changed);
            assert!(turn.paths.is_empty());
            let parent_id = self.stack.last().and_then(|c| c.change_id.clone());
            if let Some(id) = parent_id {
                assert_eq!(
                    turn.change_id, id,
                    "an unchanged turn links its parent"
                );
            }
            return;
        }
        assert!(turn.changed);
        let expected: Vec<String> = changes(&parent, &self.wc.tree)
            .into_iter()
            .map(|(p, _)| p)
            .collect();
        assert_eq!(turn.paths, expected, "the turn's paths");
        assert_eq!(Some(&turn.change_id), self.wc.change_id.as_ref());
        if self.wc.description.is_empty() {
            self.wc.description = stored("tau: run 1 turn 1");
        }
        let files = self.disk();
        let committed = std::mem::replace(&mut self.wc, Change::empty(files));
        self.stack.push(committed);
        self.ops.push(Op::Checkpoint);
        // The new @'s id: learn it on the next look.
        let (_, status) = self.ok("vcs_status", json!({}));
        self.wc.change_id = Some(change_id(&status["working_copy"]));
    }

    /// `vcs_show` of a change in the stack: its description, and the
    /// files it changes against its parent.
    #[rule]
    fn show(&mut self, tc: TestCase) {
        tc.assume(!self.stack.is_empty());
        let at =
            tc.draw(gs::integers::<usize>().max_value(self.stack.len() - 1));
        let change = &self.stack[at];
        let id = change.change_id.clone().unwrap();
        // A unique prefix names it too.
        let len =
            tc.draw(gs::integers::<usize>().min_value(8).max_value(id.len()));
        let (_, details) = self.ok("vcs_show", json!({ "change": &id[..len] }));
        assert_eq!(details["change"]["change_id"], json!(id));
        assert_eq!(details["change"]["description"], json!(change.description));
        let parent = if at == 0 {
            Tree::new()
        } else {
            self.stack[at - 1].tree.clone()
        };
        assert_eq!(
            listed(&details, "files"),
            owned(changes(&parent, &change.tree))
        );
        check_patches(&details["diff"], &parent, &change.tree);
        self.wc.tree = self.disk();
    }

    /// `vcs_diff` limited to some paths lists exactly those changes.
    #[rule]
    fn diff_paths(&mut self, tc: TestCase) {
        let paths: Vec<&str> = tc.draw(
            gs::vecs(gs::sampled_from(RESTORE_PATHS.to_vec()))
                .min_size(1)
                .max_size(2),
        );
        let (_, details) = self.ok("vcs_diff", json!({ "paths": paths }));
        self.wc.tree = self.disk();
        let want: Vec<(String, &'static str)> =
            changes(&self.parent_tree(), &self.wc.tree)
                .into_iter()
                .filter(|(path, _)| paths.iter().any(|name| under(path, name)))
                .collect();
        assert_eq!(listed(&details, "files"), owned(want));
    }

    /// After every step, the tools see what the model holds.
    #[invariant(always_run)]
    fn the_tools_see_the_model(&self, _tc: TestCase) {
        let disk = self.disk();
        let parent = self.parent_tree();
        let (_, status) = self.ok("vcs_status", json!({}));
        assert_eq!(
            listed(&status, "changes"),
            owned(changes(&parent, &disk)),
            "status changes"
        );
        let wc = &status["working_copy"];
        assert_eq!(
            wc["description"],
            json!(self.wc.description),
            "@'s description"
        );
        assert_eq!(wc["empty"], json!(disk == parent), "@'s empty flag");
        if let Some(id) = &self.wc.change_id {
            assert_eq!(&change_id(wc), id, "@'s change id");
        }
        check_patches(&status["diff"], &parent, &disk);

        let (_, log) = self.ok("vcs_log", json!({ "limit": 100 }));
        let rows = log["changes"].as_array().unwrap();
        let model = self.log_rows();
        assert_eq!(rows.len(), model.len(), "log rows: {log}");
        for (row, change) in rows.iter().zip(&model) {
            assert_eq!(row["description"], json!(change.description));
            if let Some(id) = &change.change_id {
                assert_eq!(&change_id(row), id);
            }
        }
        for (at, change) in self.stack.iter().enumerate() {
            let parent = if at == 0 {
                Tree::new()
            } else {
                self.stack[at - 1].tree.clone()
            };
            let row = &rows[self.stack.len() - at];
            assert_eq!(
                row["empty"],
                json!(change.tree == parent),
                "empty flag of {row}"
            );
        }
    }
}

/// Applies each file's diff in `diff` to `from`, and checks it gives
/// `to`: the diff text says exactly what changed. Binary files say so.
fn check_patches(diff: &Value, from: &Tree, to: &Tree) {
    let text = diff.as_str().unwrap_or_else(|| panic!("no diff: {diff}"));
    let mut sections: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("diff --git a/") {
            let path = rest.split(" b/").next().unwrap().to_owned();
            sections.insert(path.clone(), Vec::new());
            current = Some(path);
        } else if let Some(path) = &current {
            sections.get_mut(path).unwrap().push(line);
        }
    }
    let changed: Vec<String> =
        changes(from, to).into_iter().map(|(p, _)| p).collect();
    let shown: Vec<String> = sections.keys().cloned().collect();
    assert_eq!(shown, changed, "files in the diff text:\n{text}");
    for (path, lines) in sections {
        let before = from.get(&path).cloned().unwrap_or_default();
        let after = to.get(&path).cloned().unwrap_or_default();
        let binary = |bytes: &[u8]| bytes.contains(&0);
        if binary(&before) || binary(&after) {
            assert!(
                lines.iter().any(|l| l.starts_with("Binary files")),
                "{path} is binary:\n{text}"
            );
            continue;
        }
        // Text that is not UTF-8 (no NUL, so not binary to jj) shows
        // lossily, as it does in the tools.
        let patched = apply(&String::from_utf8_lossy(&before), &lines)
            .unwrap_or_else(|why| panic!("{path}: {why}\n{text}"));
        assert_eq!(
            patched,
            String::from_utf8_lossy(&after),
            "{path}'s diff does not rebuild it:\n{text}"
        );
    }
}

/// Applies one file's unified-diff lines to `old`. Lines are kept with
/// whether each ends in a newline, which `\ No newline at end of file`
/// turns off for the line before it.
fn apply(old: &str, lines: &[&str]) -> Result<String, String> {
    let old: Vec<(&str, bool)> = old
        .split_inclusive('\n')
        .map(|line| match line.strip_suffix('\n') {
            Some(text) => (text, true),
            None => (line, false),
        })
        .collect();
    let mut out: Vec<(String, bool)> = Vec::new();
    let mut at = 0; // the next old line to copy
    let mut i = lines
        .iter()
        .position(|line| line.starts_with("@@"))
        .unwrap_or(lines.len());
    while i < lines.len() {
        // `@@ -start[,count] +...`: a count of 0 inserts after `start`.
        let range = lines[i]
            .trim_start_matches("@@ -")
            .split(' ')
            .next()
            .ok_or("bad hunk header")?;
        let mut numbers = range.split(',').map(str::parse::<usize>);
        let start = numbers
            .next()
            .ok_or("no start")?
            .map_err(|e| e.to_string())?;
        let count = numbers
            .next()
            .transpose()
            .map_err(|e| e.to_string())?
            .unwrap_or(1);
        let first = if count == 0 { start } else { start - 1 };
        while at < first {
            let (text, newline) = old.get(at).ok_or("hunk past the end")?;
            out.push(((*text).to_owned(), *newline));
            at += 1;
        }
        i += 1;
        // Whether the last line handled is in the output.
        let mut last_in_out = false;
        while i < lines.len() && !lines[i].starts_with("@@") {
            let line = lines[i];
            if line.starts_with("\\ ") {
                if last_in_out {
                    out.last_mut().ok_or("no line before the marker")?.1 =
                        false;
                }
            } else if let Some(text) = line.strip_prefix('+') {
                out.push((text.to_owned(), true));
                last_in_out = true;
            } else if let Some(text) = line.strip_prefix('-') {
                let (old_text, _) =
                    old.get(at).ok_or("removed past the end")?;
                if *old_text != text {
                    return Err(format!(
                        "removed {text:?}, file has {old_text:?}"
                    ));
                }
                at += 1;
                last_in_out = false;
            } else {
                let text = line.strip_prefix(' ').unwrap_or(line);
                let (old_text, newline) =
                    old.get(at).ok_or("context past the end")?;
                if *old_text != text {
                    return Err(format!(
                        "context {text:?}, file has {old_text:?}"
                    ));
                }
                out.push(((*old_text).to_owned(), *newline));
                at += 1;
                last_in_out = true;
            }
            i += 1;
        }
    }
    for (text, newline) in &old[at..] {
        out.push(((*text).to_owned(), *newline));
    }
    Ok(out
        .into_iter()
        .map(|(text, newline)| if newline { text + "\n" } else { text })
        .collect())
}

/// The tools, and a turn checkpoint, against the model: see the module
/// docs. Each case makes a jj repository and runs up to 30 steps.
#[hegel::test(test_cases = 40)]
fn the_tools_behave_like_the_model(tc: TestCase) {
    hegel::stateful::machine(Machine::new()).steps(30).run(tc);
}

#[hegel::test(profile = "nightly_slow")]
#[ignore = "nightly"]
fn the_tools_behave_like_the_model_nightly(tc: TestCase) {
    hegel::stateful::machine(Machine::new()).steps(60).run(tc);
}

// Regressions the model found, pinned as they happened.

/// A repository and its tools, for the examples below.
fn fresh() -> Machine {
    Machine::new()
}

/// `vcs_undo` right after a turn's checkpoint refuses: the checkpoint
/// is the host's, and undoing it would hide the commit the turn links
/// to. It used to undo it.
#[test]
fn undo_refuses_a_turn_checkpoint() {
    let machine = fresh();
    std::fs::write(machine.dir.path().join("a.txt"), "a\n").unwrap();
    let turn =
        block_on(machine.vcs.checkpoint("tau: run 1 turn 1", "tau/1")).unwrap();
    assert!(turn.changed);
    let err = machine.call("vcs_undo", json!({})).unwrap_err();
    assert!(
        err.starts_with("The last operation was not made by the vcs tools"),
        "{err}"
    );
}

/// Undoing `vcs_new` after editing a file goes back to the change it
/// left, with the edit. It used to report the undo and keep the new
/// change.
#[test]
fn undo_of_new_after_an_edit_goes_back_with_the_edit() {
    let machine = fresh();
    let (_, before) = machine.ok("vcs_status", json!({}));
    machine.ok("vcs_new", json!({}));
    std::fs::write(machine.dir.path().join("a.txt"), "edited\n").unwrap();
    let (_, undone) = machine.ok("vcs_undo", json!({}));
    assert_eq!(
        undone["working_copy"]["change_id"],
        before["working_copy"]["change_id"]
    );
    let (_, status) = machine.ok("vcs_status", json!({}));
    assert_eq!(
        status["changes"],
        json!([{"path": "a.txt", "kind": "added"}])
    );
    assert_eq!(
        machine.ok("vcs_log", json!({})).1["changes"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

/// Undoing `vcs_describe` after editing a file takes the description
/// back and keeps the edit, without leaving the old commit as a
/// divergent twin.
#[test]
fn undo_of_describe_after_an_edit_takes_the_description_back() {
    let machine = fresh();
    machine.ok("vcs_describe", json!({ "message": "Wrong" }));
    std::fs::write(machine.dir.path().join("a.txt"), "edited\n").unwrap();
    let (_, undone) = machine.ok("vcs_undo", json!({}));
    let wc = &undone["working_copy"];
    assert_eq!(wc["description"], json!(""));
    assert_eq!(wc["divergent"], json!(false));
    let (_, status) = machine.ok("vcs_status", json!({}));
    assert_eq!(
        status["changes"],
        json!([{"path": "a.txt", "kind": "added"}])
    );
}

/// Describing again, as undone, within the same second: Git keeps whole
/// seconds, so the rewrite used to be the very commit the undo hid, and
/// jj refused it as already existing.
#[test]
fn a_describe_undone_and_redone_at_once_works() {
    let machine = fresh();
    for _ in 0..3 {
        machine.ok("vcs_describe", json!({ "message": "Same" }));
        machine.ok("vcs_undo", json!({}));
    }
    let (_, done) = machine.ok("vcs_describe", json!({ "message": "Same" }));
    assert_eq!(done["working_copy"]["description"], json!("Same\n"));
}
