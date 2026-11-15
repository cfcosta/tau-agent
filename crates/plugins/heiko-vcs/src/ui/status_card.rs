//! The `vcs_status` card, as a view tree (ADR 0045). Its header names
//! `@` and its parents and counts the files `@` changes; its body is
//! those files, each on a row with its kind and the lines it adds and
//! removes, after the files that hold conflict markers and the new ones
//! left out for their size.

use heiko_ui_kit::theme;
use heiko_ui_plugin::tree::{Ink, Node, Tone};

use super::{change_status::ChangeStatus, file_rows, quiet};

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

/// The header: `@ ourtwkkx on onvkmqwo`.
pub fn summary(status: &ChangeStatus) -> Node {
    let parents = status
        .parents
        .iter()
        .map(|parent| parent.short_id().to_owned())
        .collect::<Vec<_>>()
        .join(" + ");
    let mut text = format!("@ {}", status.working_copy.short_id());
    if !parents.is_empty() {
        text.push_str(&format!(" on {parents}"));
    }
    Node::Mono { text }
}

/// What `@` holds: the conflicts its files do not list, the files left
/// out, then the files it changes. None when it holds nothing.
pub fn body(status: &ChangeStatus) -> Option<Node> {
    let letter = |text: &str, tone| Node::Toned {
        text: text.into(),
        tone,
        mono: true,
    };
    let mut rows: Vec<Node> = status
        .conflicts
        .iter()
        .filter(|path| !status.files.iter().any(|file| &file.path == *path))
        .map(|path| {
            Node::row([
                letter("!", Tone::Bad),
                Node::Mono { text: path.clone() },
                Node::Spacer,
                Node::Stamp {
                    label: "conflict".into(),
                    tone: Ink::Bad,
                },
            ])
        })
        .collect();
    rows.extend(status.too_large.iter().map(|file| {
        Node::row([
            letter("?", Tone::Warn),
            Node::Mono {
                text: file.path.clone(),
            },
            Node::Spacer,
            quiet(format!(
                "left out · {} over {}",
                crate::byte_size(file.size),
                crate::byte_size(file.max_size)
            )),
        ])
    }));
    rows.extend(file_rows(&status.files, status.truncated, &|path| {
        status.is_conflicted(path)
    }));
    (!rows.is_empty()).then(|| Node::stack(rows))
}
