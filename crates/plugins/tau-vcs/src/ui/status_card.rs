//! The `vcs_status` card, as a view tree (ADR 0045). Closed, it names
//! `@` and its parents, counts the files it adds, modifies and deletes,
//! and flags conflicts and files left out. Open, it shows `@` over its
//! parents, the conflicts and left-out files in cards of their own,
//! then the files, each opening to its hunks as in the diff card.

use tau_ui_kit::theme;
use tau_ui_plugin::tree::{Node, Tone};

use super::{
    Card,
    bookmarks,
    change_log::Change,
    change_status::ChangeStatus,
    count,
    diff_card,
    flags,
    kind_badge,
    short_id,
    subject,
};
use crate::ChangeKind;

/// The card's edge: red while a file holds conflict markers, amber
/// while a file is left out, else none of its own.
pub fn edge(status: &ChangeStatus) -> Option<theme::Tone> {
    if !status.conflicts.is_empty() {
        Some(theme::Tone::Danger)
    } else if !status.too_large.is_empty() {
        Some(theme::Tone::Warn)
    } else {
        None
    }
}

fn kind_tone(kind: ChangeKind) -> Tone {
    match kind {
        ChangeKind::Added => Tone::Good,
        ChangeKind::Modified => Tone::Warn,
        ChangeKind::Removed => Tone::Bad,
    }
}

/// The header: `@ ourtwkkx on onvkmqwo`, then a badge for conflicts and
/// for files left out, then one for each kind of file.
pub fn summary(status: &ChangeStatus, compact: bool) -> Node {
    let parents = status
        .parents
        .iter()
        .map(|parent| parent.short_id().to_owned())
        .collect::<Vec<_>>()
        .join(" + ");
    let mut row = vec![Node::Mono {
        text: format!("@ {}", status.working_copy.short_id()),
    }];
    if !parents.is_empty() {
        row.push(Node::text("on"));
        row.push(Node::Mono { text: parents });
    }
    row.push(Node::Spacer);
    if !status.conflicts.is_empty() {
        row.push(Node::Badge {
            text: count(status.conflicts.len(), "conflict"),
            tone: Tone::Bad,
        });
    }
    if !status.too_large.is_empty() {
        row.push(Node::Badge {
            text: format!("{} left out", status.too_large.len()),
            tone: Tone::Warn,
        });
    }
    if !compact {
        row.extend(status.counts().into_iter().map(|(kind, n)| Node::Badge {
            text: format!("{} {n}", kind.letter()),
            tone: kind_tone(kind),
        }));
    }
    Node::row(row)
}

/// An open status: `@`, its parents, the conflicts and left-out files,
/// then what `@` changes.
pub fn body(card: &Card, status: &ChangeStatus) -> Node {
    let mut column = vec![working_copy_row(&status.working_copy)];
    column.extend(
        status
            .parents
            .iter()
            .map(|parent| parent_row(parent, card.compact)),
    );
    if !status.conflicts.is_empty() {
        column.push(notice(
            Tone::Bad,
            &count(status.conflicts.len(), "conflict"),
            "edit the markers out of these files",
            status
                .conflicts
                .iter()
                .map(|path| path_row(path, "!", Tone::Bad, None))
                .collect(),
        ));
    }
    if let Some(first) = status.too_large.first() {
        column.push(notice(
            Tone::Warn,
            "Left out of @",
            &format!(
                "new files over {} are not snapshotted",
                crate::byte_size(first.max_size)
            ),
            status
                .too_large
                .iter()
                .map(|file| {
                    let size = crate::byte_size(file.size);
                    path_row(&file.path, "?", Tone::Warn, Some(size))
                })
                .collect(),
        ));
    }
    if status.files.is_empty() {
        let tip = status
            .parents
            .first()
            .filter(|parent| !parent.info.bookmarks.is_empty())
            .map(|parent| {
                format!(", the tip of {}", parent.info.bookmarks.join(", "))
            })
            .unwrap_or_default();
        column.push(Node::row([
            Node::Icon {
                name: "check".into(),
                tone: Tone::Good,
            },
            Node::text(format!(
                "@ is empty: edits made now become a new change on {}{tip}.",
                status
                    .parents
                    .first()
                    .map_or("its parent", |parent| parent.short_id())
            )),
        ]));
    } else {
        column.push(Node::Rule);
        column.push(Node::Heading {
            text: format!("In @ · {}", status.summary()),
        });
        column.push(diff_card::file_list(
            card,
            &status.files,
            status.truncated,
            &|path| status.is_conflicted(path),
        ));
    }
    Node::stack(column)
}

/// `@`, pinned on top: its type and subject once described, else what
/// it is in words.
fn working_copy_row(change: &Change) -> Node {
    let mut row = vec![Node::Badge {
        text: "@".into(),
        tone: Tone::Warn,
    }];
    if change.subject.is_empty() {
        let mut words = vec!["Working copy"];
        words.extend(
            change
                .state()
                .into_iter()
                .filter(|word| !["working copy", "mutable"].contains(word)),
        );
        words.push("no description yet");
        row.push(Node::text(words.join(" · ")));
    } else {
        row.extend(kind_badge(change));
        row.push(Node::text(change.subject.clone()));
        row.extend(flags(change));
    }
    row.push(Node::Spacer);
    row.extend(bookmarks(change));
    row.push(short_id(change));
    Node::row(row)
}

/// A parent, as the log draws a change.
fn parent_row(parent: &Change, compact: bool) -> Node {
    let mut row = vec![Node::Mono { text: "@-".into() }];
    if !compact {
        row.extend(kind_badge(parent));
    }
    row.push(Node::text(subject(parent)));
    row.push(Node::Spacer);
    if parent.info.immutable {
        row.push(Node::Icon {
            name: "lock".into(),
            tone: Tone::Neutral,
        });
    }
    row.extend(bookmarks(parent));
    row.push(short_id(parent));
    Node::row(row)
}

/// A card for what needs the model's attention: a title, a line on
/// what to do, then the paths.
fn notice(tone: Tone, title: &str, hint: &str, rows: Vec<Node>) -> Node {
    let mut children = vec![Node::row([
        Node::Icon {
            name: "warning".into(),
            tone,
        },
        Node::Badge {
            text: title.to_owned(),
            tone,
        },
        Node::text(hint),
    ])];
    children.extend(rows);
    Node::Card { children }
}

/// A path in a notice: a badge, the path, and what the notice says of
/// it.
fn path_row(
    path: &str,
    letter: &str,
    tone: Tone,
    note: Option<String>,
) -> Node {
    let mut row = vec![
        Node::Badge {
            text: letter.to_owned(),
            tone,
        },
        Node::Mono {
            text: path.to_owned(),
        },
    ];
    if let Some(note) = note {
        row.push(Node::Spacer);
        row.push(Node::Mono { text: note });
    }
    Node::row(row)
}
