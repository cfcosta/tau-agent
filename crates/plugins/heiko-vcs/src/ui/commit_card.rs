//! The `vcs_commit` card, as a view tree (ADR 0045), drawn as the
//! stack the commit made, each change a square on the line that joins
//! them: the new working copy on top, the new change under it (the
//! filled square), and the parent below. Each has its files on a band
//! under it, so a commit that took only some paths shows what went into
//! the change and what stayed in `@`.

use heiko_ui_plugin::tree::Node;
use serde::Deserialize;
use serde_json::Value;

use super::{
    bookmarks,
    change_diff::{FileDiff, files_of},
    change_log::Change,
    diff_card::message,
    file_rows,
    first_line,
    node,
    quiet,
    short_id,
};
use crate::{ChangeInfo, FileChange};

/// What a `vcs_commit` result shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// The change the commit made.
    pub change: Change,
    /// The change it sits on, when it has one.
    pub parent: Option<Change>,
    /// The new `@` on top of it.
    pub working_copy: Change,
    /// What the change holds against its parent.
    pub files: Vec<FileDiff>,
    pub truncated: bool,
    /// What stays in `@`, uncommitted.
    pub left: Vec<FileDiff>,
    pub left_truncated: bool,
}

#[derive(Deserialize)]
struct Details {
    committed: ChangeInfo,
    working_copy: ChangeInfo,
    parent: Option<ChangeInfo>,
    files: Vec<FileChange>,
    diff: String,
    truncated: bool,
    #[serde(default)]
    left_files: Vec<FileChange>,
    #[serde(default)]
    left_diff: String,
    #[serde(default)]
    left_truncated: bool,
}

impl Commit {
    /// Reads a `vcs_commit` result's details. `None` when they are not
    /// that shape.
    pub fn parse(details: &Value) -> Option<Self> {
        let details = Details::deserialize(details).ok()?;
        Some(Self {
            change: Change::new(details.committed),
            parent: details.parent.map(Change::new),
            working_copy: Change::new(details.working_copy),
            files: files_of(&details.diff, &details.files, details.truncated),
            truncated: details.truncated,
            left: files_of(
                &details.left_diff,
                &details.left_files,
                details.left_truncated,
            ),
            left_truncated: details.left_truncated,
        })
    }

    /// The closed card's count: `2 files`, or `1 of 3 files committed`
    /// when some stayed in `@`.
    pub fn label(&self) -> String {
        let files = |count: usize| match count {
            1 => "1 file".to_owned(),
            count => format!("{count} files"),
        };
        if self.left.is_empty() {
            files(self.files.len())
        } else {
            format!(
                "{} of {} committed",
                self.files.len(),
                files(self.files.len() + self.left.len())
            )
        }
    }
}

/// The card's head: the change's first line.
pub fn summary(commit: &Commit) -> Node {
    Node::Mono {
        text: first_line(&commit.change),
    }
}

/// A change's row on the line, and `under` it on a band, when there is
/// any: its message, its files.
fn step(row: Vec<Node>, under: Vec<Node>) -> Node {
    let row = Node::row(row);
    if under.is_empty() {
        row
    } else {
        Node::stack([row, Node::Band { children: under }])
    }
}

/// The stack the commit made, its squares joined by the line: `@` on
/// top with what stayed in it, the new change with its message and
/// files, and the parent below.
pub fn body(commit: &Commit) -> Node {
    let change = &commit.change;
    let working_copy = match commit.left.len() {
        0 => vec![
            node(false),
            quiet("(empty)"),
            Node::text("new working copy"),
        ],
        left => vec![
            node(false),
            short_id(&commit.working_copy),
            Node::text(match left {
                1 => "working copy · still holds 1 file".to_owned(),
                n => format!("working copy · still holds {n} files"),
            }),
        ],
    };
    let mut steps = vec![step(
        working_copy,
        file_rows(&commit.left, commit.left_truncated, &|_| false),
    )];
    let mut committed =
        vec![node(true), short_id(change), Node::text(first_line(change))];
    committed.push(Node::Spacer);
    committed.extend(bookmarks(change));
    let mut holds = message(change);
    holds.extend(file_rows(&commit.files, commit.truncated, &|_| false));
    steps.push(step(committed, holds));
    if let Some(parent) = &commit.parent {
        let mut row = vec![
            node(false),
            short_id(parent),
            Node::text(first_line(parent)),
        ];
        row.push(Node::Spacer);
        row.extend(bookmarks(parent));
        steps.push(step(row, Vec::new()));
    }
    Node::Rail { children: steps }
}
