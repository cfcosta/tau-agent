//! The `vcs_diff` and `vcs_show` cards, as view trees (ADR 0045).
//! Closed, a diff sums up its files and lines; a show, its subject.
//! Open, a diff lists its files, each a section that opens to its
//! hunks, and a show leads with the message, ids, author and parents,
//! then lists its files the same way.

use tau_ui_plugin::tree::{KeyValue, Node, Tone};

use super::{
    Card,
    bookmarks,
    change_diff::{ChangeDiff, FileDiff},
    change_log::Change,
    kind_badge,
    short_id,
    subject,
};

/// A diff's header: which change, and how many files.
pub fn files_summary(diff: &ChangeDiff) -> Node {
    Node::row([
        short_id(&diff.change),
        Node::text("·"),
        Node::text(diff.file_count()),
    ])
}

/// A show's header: the change's type, subject and short id.
pub fn commit_summary(diff: &ChangeDiff) -> Node {
    let change = &diff.change;
    let mut row: Vec<Node> = kind_badge(change).into_iter().collect();
    row.extend([Node::text(subject(change)), Node::Spacer, short_id(change)]);
    Node::row(row)
}

/// An open diff: its files.
pub fn files_body(card: &Card, diff: &ChangeDiff) -> Node {
    file_list(card, &diff.files, diff.truncated, &|_| false)
}

/// The paragraphs of a description after its first line, if any.
pub(super) fn message(change: &Change) -> Vec<Node> {
    change
        .info
        .description
        .trim()
        .split_once('\n')
        .map(|(_, body)| body.trim())
        .filter(|body| !body.is_empty())
        .map(|body| {
            body.split("\n\n")
                .map(|paragraph| Node::text(paragraph.trim()))
                .collect()
        })
        .unwrap_or_default()
}

/// An open show: the message, then the ids, author and parents, then
/// the files.
pub fn commit_body(card: &Card, diff: &ChangeDiff) -> Node {
    let change = &diff.change;
    let info = &change.info;
    let mut head: Vec<Node> = Vec::new();
    if let Some(scope) = &change.scope {
        head.push(Node::Mono {
            text: scope.clone(),
        });
    }
    head.extend(kind_badge(change));
    head.extend(bookmarks(change));
    head.push(Node::Spacer);
    head.push(Node::Badge {
        text: change.state().join(" · "),
        tone: Tone::Neutral,
    });
    let mut column = vec![
        Node::row(head),
        Node::Heading {
            text: subject(change),
        },
    ];
    column.extend(message(change));
    let field = |key: &str, value: String| KeyValue {
        key: key.into(),
        value,
    };
    let mut rows = vec![
        field("change", info.change_id.clone()),
        field("commit", info.commit_id.clone()),
    ];
    if let Some(author) = &diff.author {
        rows.push(field("author", author.name.clone()));
    }
    rows.extend(
        diff.parents
            .iter()
            .map(|parent| field("parent", parent_line(parent))),
    );
    column.push(Node::Rows { rows });
    if !diff.files.is_empty() {
        column.push(Node::Rule);
        column.push(Node::Heading {
            text: format!("Files · {}", diff.file_count()),
        });
    }
    column.push(file_list(card, &diff.files, diff.truncated, &|_| false));
    Node::stack(column)
}

/// A parent as the log names a change: its short id, bookmarks and
/// subject.
fn parent_line(parent: &Change) -> String {
    let mut words = vec![parent.short_id().to_owned()];
    words.extend(parent.info.bookmarks.iter().cloned());
    words.push(subject(parent));
    words.join(" · ")
}

/// A file's section title: its kind (`jj status`'s letter), path, what it changes and what to
/// know of it.
fn file_title(file: &FileDiff, conflicted: bool) -> String {
    let mut words = vec![format!("{} {}", file.kind.letter(), file.path)];
    if file.added > 0 || file.removed > 0 {
        words.push(format!("+{} −{}", file.added, file.removed));
    }
    if conflicted {
        words.push("conflict".into());
    }
    if file.binary {
        words.push("binary".into());
    }
    words.join(" · ")
}

/// A file's hunks, colored by its language, or why there are none.
fn hunks(file: &FileDiff) -> Node {
    if file.binary {
        Node::text("A binary file.")
    } else if file.hunks.is_empty() {
        Node::text("No lines changed.")
    } else {
        Node::Diff {
            text: file.unified(),
            path: Some(file.path.clone()),
        }
    }
}

/// Each file as a section that opens to its hunks, then a note when
/// the diff was cut.
pub fn file_list(
    card: &Card,
    files: &[FileDiff],
    truncated: bool,
    conflicted: &dyn Fn(&str) -> bool,
) -> Node {
    let mut list: Vec<Node> = files
        .iter()
        .map(|file| Node::Section {
            id: card.file_id(&file.path),
            title: file_title(file, conflicted(&file.path)),
            open: false,
            children: vec![hunks(file)],
        })
        .collect();
    if files.is_empty() {
        list.push(Node::Empty {
            text: "No changes.".into(),
        });
    }
    if truncated {
        list.push(Node::text(
            "The diff was cut at 50 KB: files past the cut are not listed.",
        ));
    }
    Node::stack(list)
}
