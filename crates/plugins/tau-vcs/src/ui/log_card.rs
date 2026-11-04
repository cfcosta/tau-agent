//! The `vcs_log` card, as a view tree (ADR 0045): the working copy and
//! the stack over trunk, each in runs of one commit scope. A change
//! picked in it shows its whole description and ids; which is picked is
//! the plugin's UI state, under [`super::pick_key`].

use serde_json::json;
use tau_ui_plugin::tree::{KeyValue, Kind, Node, Tone};

use super::{
    Card,
    SET,
    bookmarks,
    change_log::{Change, ChangeLog, ScopeRun},
    count,
    flags,
    kind_badge,
    pick,
    short_id as short_id_node,
    subject,
};

/// An open log: the working copy, the stack, then trunk, with the
/// detail of the change picked under its row.
pub fn body(card: &Card, log: &ChangeLog, picked: Option<&str>) -> Node {
    let mut column = vec![Node::Heading {
        text: "Stack · mutable".into(),
    }];
    if let Some(change) = &log.working_copy {
        column.extend(pickable(card, change, picked, working_copy_row(change)));
    }
    column.extend(scope_runs(&log.stack, card, picked));
    if log.trunk_len() > 0 {
        column.push(Node::Rule);
        column.push(Node::row([
            Node::Heading {
                text: "Trunk · immutable".into(),
            },
            Node::Icon {
                name: "lock".into(),
                tone: Tone::Neutral,
            },
        ]));
        column.extend(scope_runs(&log.trunk, card, picked));
    }
    if log.more {
        let shown = log.changes().count();
        column.push(Node::text(format!(
            "The newest {shown} changes. Older ones are past the call's \
             limit."
        )));
    }
    Node::stack(column)
}

/// `@`, pinned above the stack: what it holds, in words.
fn working_copy_row(change: &Change) -> Vec<Node> {
    let described = !change.subject.is_empty();
    let mut words = vec!["Working copy"];
    if change.info.empty {
        words.push("empty");
    }
    if change.info.conflict {
        words.push("conflict");
    }
    if change.info.divergent {
        words.push("divergent");
    }
    words.push(if described {
        &change.subject
    } else {
        "no description yet"
    });
    let mut row = vec![
        Node::Badge {
            text: "@".into(),
            tone: Tone::Warn,
        },
        Node::text(words.join(" · ")),
        Node::Spacer,
    ];
    row.extend(bookmarks(change));
    row.push(short_id_node(change));
    row
}

/// Each run's scope, then its changes.
fn scope_runs(
    runs: &[ScopeRun],
    card: &Card,
    picked: Option<&str>,
) -> Vec<Node> {
    let mut column = Vec::new();
    for run in runs {
        column.push(Node::row([
            Node::Mono {
                text: run.scope.clone().unwrap_or_else(|| "no scope".into()),
            },
            Node::text(count(run.changes.len(), "change")),
        ]));
        for change in &run.changes {
            column.extend(pickable(
                card,
                change,
                picked,
                change_row(change, card.compact),
            ));
        }
    }
    column
}

/// One change of a scope's run: its type, subject, states, bookmarks
/// and short id.
fn change_row(change: &Change, compact: bool) -> Vec<Node> {
    let mut row = Vec::new();
    if !compact {
        row.push(kind_badge(change).unwrap_or_else(|| Node::Badge {
            text: "—".into(),
            tone: Tone::Neutral,
        }));
    }
    row.push(Node::text(subject(change)));
    row.extend(flags(change));
    row.push(Node::Spacer);
    row.extend(bookmarks(change));
    row.push(short_id_node(change));
    row
}

/// A row behind a button that picks its change, or puts it back, and
/// the change's detail under it while it is the one picked.
fn pickable(
    card: &Card,
    change: &Change,
    picked: Option<&str>,
    row: Vec<Node>,
) -> Vec<Node> {
    let id = change.info.change_id.as_str();
    let chosen = picked == Some(id);
    let button = Node::Button {
        label: String::new(),
        action: SET.into(),
        args: json!({ "key": card.pick_key(), "value": pick(picked, id) }),
        kind: Kind::Secondary,
        icon: Some(if chosen { "down" } else { "chevron" }.into()),
    };
    let mut nodes = vec![Node::row(std::iter::once(button).chain(row))];
    if chosen {
        nodes.push(detail(change));
    }
    nodes
}

/// The picked change: its whole description, ids and state.
pub fn detail(change: &Change) -> Node {
    let info = &change.info;
    let description = info.description.trim();
    let field = |key: &str, value: String| KeyValue {
        key: key.into(),
        value,
    };
    let mut rows = vec![
        field("change", info.change_id.clone()),
        field("commit", info.commit_id.clone()),
        field("state", change.state().join(" · ")),
    ];
    if !info.bookmarks.is_empty() {
        rows.push(field("bookmarks", info.bookmarks.join(", ")));
    }
    Node::Card {
        children: vec![
            Node::text(if description.is_empty() {
                "(no description set)"
            } else {
                description
            }),
            Node::Rows { rows },
        ],
    }
}

/// Changes as they sit on a stack, newest first, without picking: a
/// landing's preview and its card in the parent's chat.
pub fn stack(changes: &[Change], compact: bool) -> Node {
    Node::stack(
        changes
            .iter()
            .map(|change| Node::row(change_row(change, compact))),
    )
}
