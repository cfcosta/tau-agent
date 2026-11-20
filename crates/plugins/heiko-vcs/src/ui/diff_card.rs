//! The `vcs_diff` and `vcs_show` cards, as view trees (ADR 0045). A
//! diff's header names its change and counts its files, and its body
//! lists them, each a row whose path opens its hunks under it. A
//! show's header is the change's id and first line; its body, on a
//! band, leads with the ids and state, then the message, author and
//! parents, then its files, each on a row with the lines it adds and
//! removes.

use heiko_ui_plugin::tree::{KeyValue, Kind, Node};
use serde_json::json;

use super::{
    Card,
    SET,
    change_diff::{ChangeDiff, FileDiff},
    change_log::Change,
    file_row,
    file_rows,
    first_line,
    quiet,
};

/// A diff's header: which change, and how many files.
pub fn files_summary(diff: &ChangeDiff) -> String {
    format!("{} · {}", diff.change.short_id(), diff.file_count())
}

/// A show's header: the change's short id and first line.
pub fn commit_summary(diff: &ChangeDiff) -> String {
    let change = &diff.change;
    format!("{} {}", change.short_id(), first_line(change))
}

/// An open diff: its files, those `open` names with their hunks.
pub fn files_body(
    card: &Card,
    diff: &ChangeDiff,
    open: &dyn Fn(&str) -> bool,
) -> Node {
    file_list(card, &diff.files, diff.truncated, &|_| false, open)
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

/// A show, on a band: the ids and state on a line, the message, the
/// author and parents, then the files.
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
    Node::Band { children: column }
}

/// A parent as the log names a change: its short id, bookmarks and
/// subject.
fn parent_line(parent: &Change) -> String {
    let mut words = vec![parent.short_id().to_owned()];
    words.extend(parent.info.bookmarks.iter().cloned());
    words.push(first_line(parent));
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

/// Each file on a row, its path a button that shows its hunks under it
/// or hides them (`open` says which show), then a note when the diff
/// was cut.
pub fn file_list(
    card: &Card,
    files: &[FileDiff],
    truncated: bool,
    conflicted: &dyn Fn(&str) -> bool,
    open: &dyn Fn(&str) -> bool,
) -> Node {
    let mut list = Vec::new();
    for file in files {
        let open = open(&file.path);
        let path = Node::Button {
            label: file.path.clone(),
            action: SET.into(),
            args: json!({
                "key": card.file_key(&file.path),
                "value": (!open).then_some(true),
            }),
            kind: Kind::Secondary,
            icon: None,
            size: Default::default(),
            wide: false,
            detail: None,
        };
        list.push(file_row(file, path, conflicted(&file.path)));
        if open {
            list.push(hunks(file));
        }
    }
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
