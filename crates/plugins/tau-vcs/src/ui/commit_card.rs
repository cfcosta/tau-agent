//! The `vcs_commit` card, as a view tree (ADR 0045), drawn as the
//! stack the commit made: the working copy on top, the new change in
//! the middle, and the parent below. Closed, the change's subject;
//! open, each with its files, so a commit that took only some paths
//! shows what went into the change and what stayed in `@`.

use serde::Deserialize;
use serde_json::Value;
use tau_ui_plugin::tree::{Node, Tone};

use super::{
    Card,
    bookmarks,
    change_diff::{FileDiff, files_of},
    change_log::Change,
    diff_card::{file_list, message},
    kind_badge,
    short_id,
    subject,
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

/// The closed card's head: the stack the commit made in small, the
/// change's type and subject, and the bookmarks on it.
pub fn summary(commit: &Commit) -> Node {
    let change = &commit.change;
    let graph = if commit.parent.is_some() {
        "○─●─◌"
    } else {
        "●─◌"
    };
    let mut row = vec![Node::Mono { text: graph.into() }];
    row.extend(kind_badge(change));
    row.push(Node::text(subject(change)));
    row.push(Node::Spacer);
    row.extend(bookmarks(change));
    Node::row(row)
}

/// The open card: `@` on top with what stayed in it, the new change
/// with its message and files, and the parent below.
pub fn body(card: &Card, commit: &Commit) -> Node {
    let change = &commit.change;
    let left = commit.left.len();
    let mut working = vec![Node::row([
        Node::Badge {
            text: "@".into(),
            tone: Tone::Warn,
        },
        short_id(&commit.working_copy),
        Node::text(match left {
            0 => "working copy · empty".to_owned(),
            1 => "working copy · still holds 1 file".to_owned(),
            n => format!("working copy · still holds {n} files"),
        }),
    ])];
    if left > 0 {
        working.push(file_list(
            card,
            &commit.left,
            commit.left_truncated,
            &|_| false,
        ));
    }
    let mut head: Vec<Node> = kind_badge(change).into_iter().collect();
    head.push(Node::Heading {
        text: subject(change),
    });
    head.push(Node::Spacer);
    head.push(short_id(change));
    head.extend(bookmarks(change));
    let mut committed = vec![Node::row(head)];
    committed.extend(message(change));
    committed.push(Node::Card {
        children: vec![file_list(
            card,
            &commit.files,
            commit.truncated,
            &|_| false,
        )],
    });
    let mut column =
        vec![Node::stack(working), Node::Rule, Node::stack(committed)];
    if let Some(parent) = &commit.parent {
        let mut row = vec![Node::Mono {
            text: parent.short_id().to_owned(),
        }];
        row.extend(bookmarks(parent));
        row.push(Node::text(
            parent
                .info
                .description
                .lines()
                .next()
                .unwrap_or("(no description set)")
                .to_owned(),
        ));
        column.push(Node::Rule);
        column.push(Node::row(row));
    }
    Node::stack(column)
}
