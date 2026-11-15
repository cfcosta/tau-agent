//! The `vcs_diff` and `vcs_show` cards, as view trees (ADR 0045). A
//! diff's header names its change and counts its files, and its body
//! lists them, each a section that opens to its hunks as bands. A
//! show's header is the change's id and first line; its body leads with
//! the ids and state, then the message, author and parents, then its
//! files, each on a row with the lines it adds and removes.

use heiko_ui_plugin::tree::{KeyValue, Node};

use super::{
    Card,
    change_diff::{ChangeDiff, FileDiff},
    change_log::Change,
    file_rows,
    first_line,
    quiet,
};

/// A diff's header: which change, and how many files.
pub fn files_summary(diff: &ChangeDiff) -> Node {
    Node::Mono {
        text: format!("{} · {}", diff.change.short_id(), diff.file_count()),
    }
}

/// A show's header: the change's short id and first line.
pub fn commit_summary(diff: &ChangeDiff) -> Node {
    let change = &diff.change;
    Node::Mono {
        text: format!("{} {}", change.short_id(), first_line(change)),
    }
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

/// A show: the ids and state on a line, the message, the author and
/// parents, then the files.
pub fn commit_body(diff: &ChangeDiff) -> Node {
    let change = &diff.change;
    let info = &change.info;
    let mono = |text: &str| Node::Mono { text: text.into() };
    let mut ids = vec![
        quiet("change"),
        mono(change.short_id()),
        quiet("commit"),
        mono(&info.commit_id[..info.commit_id.len().min(7)]),
        quiet(change.state().join(" · ")),
    ];
    ids.extend(info.bookmarks.iter().map(|name| Node::Toned {
        text: name.clone(),
        tone: heiko_ui_plugin::tree::Tone::Good,
        mono: true,
    }));
    let mut column = vec![Node::row(ids)];
    column.extend(message(change));
    let field = |key: &str, value: String| KeyValue {
        key: key.into(),
        value,
    };
    let mut rows = Vec::new();
    if let Some(author) = &diff.author {
        rows.push(field("author", author.name.clone()));
    }
    rows.extend(
        diff.parents
            .iter()
            .map(|parent| field("parent", parent_line(parent))),
    );
    if !rows.is_empty() {
        column.push(Node::Rows { rows });
    }
    column.extend(file_rows(&diff.files, diff.truncated, &|_| false));
    Node::stack(column)
}

/// A parent as the log names a change: its short id, bookmarks and
/// subject.
fn parent_line(parent: &Change) -> String {
    let mut words = vec![parent.short_id().to_owned()];
    words.extend(parent.info.bookmarks.iter().cloned());
    words.push(first_line(parent));
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
        list.push(quiet("cut at 50 KB: files past the cut are not listed"));
    }
    Node::stack(list)
}
