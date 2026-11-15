//! The `vcs_log` card, as a view tree (ADR 0045): the working copy, the
//! stack and trunk, newest first, each change a square on the line that
//! joins them, with its id, first line, states and bookmarks. A change's id picks it, and
//! the one picked shows its whole description and ids; which is picked
//! is the plugin's UI state, under [`super::pick_key`].

use heiko_ui_plugin::tree::{KeyValue, Kind, Node};
use serde_json::json;

use super::{
    Card,
    SET,
    bookmarks,
    change_log::{Change, ChangeLog},
    first_line,
    flags,
    node,
    pick,
    quiet,
    short_id,
};

/// The log: every change, newest first, its square on the line that
/// joins them, with the detail of the change picked on a band under its
/// row.
pub fn body(card: &Card, log: &ChangeLog, picked: Option<&str>) -> Node {
    let rows: Vec<Node> = log
        .changes()
        .map(|change| pickable(card, change, picked))
        .collect();
    let mut column = Vec::new();
    if rows.is_empty() {
        column.push(Node::Empty {
            text: "No changes yet.".into(),
        });
    } else {
        column.push(Node::Rail { children: rows });
    }
    if log.more {
        column.push(quiet("older changes are past the call's limit"));
    }
    Node::stack(column)
}

/// One change on the line: its square, filled when it is `on`, `id`,
/// its first line, its states and its bookmarks.
fn change_row(change: &Change, on: bool, id: Node) -> Node {
    let mut row = vec![node(on), id, Node::text(first_line(change))];
    row.extend(flags(change));
    row.push(Node::Spacer);
    row.extend(bookmarks(change));
    Node::row(row)
}

/// A change's row, its id a button that picks it or puts it back, and
/// the change's detail on a band under it while it is the one picked.
fn pickable(card: &Card, change: &Change, picked: Option<&str>) -> Node {
    let id = change.info.change_id.as_str();
    let button = Node::Button {
        label: change.short_id().to_owned(),
        action: SET.into(),
        args: json!({ "key": card.pick_key(), "value": pick(picked, id) }),
        kind: Kind::Secondary,
        icon: None,
        size: Default::default(),
        wide: false,
        detail: None,
    };
    let row = change_row(change, change.info.working_copy, button);
    if picked == Some(id) {
        Node::stack([
            row,
            Node::Band {
                children: vec![detail(change)],
            },
        ])
    } else {
        row
    }
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
    Node::stack([
        Node::text(if description.is_empty() {
            "(no description set)"
        } else {
            description
        }),
        Node::Rows { rows },
    ])
}

/// Changes as they sit on a stack, newest first, without picking: a
/// landing's preview and its card in the parent's chat. The newest, the
/// tip that lands, has the filled square, and the line joins them.
pub fn stack(changes: &[Change]) -> Node {
    Node::Rail {
        children: changes
            .iter()
            .enumerate()
            .map(|(n, change)| change_row(change, n == 0, short_id(change)))
            .collect(),
    }
}
