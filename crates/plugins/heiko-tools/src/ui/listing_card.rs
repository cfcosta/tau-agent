//! The `ls` card, as view trees (ADR 0045). Its header counts the
//! folders and files (the card's summary); its body is the names in
//! columns, the folders first, each in a tone that says what it is: a
//! folder, a file `@` changes (with the letter of how), one `.gitignore`
//! leaves out. The first few show until the card is opened.

use heiko_ui_plugin::tree::{Node, Tone};

use super::{
    listing::{ChangeKind, DirListing, Listed},
    more,
};

/// The entries a card shows before it is opened.
pub const PEEK: usize = 9;

/// The names, the folders then the files, in columns; none for an empty
/// directory. `key` is the UI state that opens the card.
pub fn body(
    listing: &DirListing,
    key: &str,
    open: bool,
    compact: bool,
) -> Option<Node> {
    let entries: Vec<&Listed> = listing.dirs().chain(listing.files()).collect();
    if entries.is_empty() {
        return None;
    }
    let columns = if compact { 2 } else { 3 };
    let rest = entries.len().saturating_sub(PEEK);
    let shown = if open {
        entries.len()
    } else {
        entries.len() - rest
    };
    let rows = entries[..shown]
        .chunks(columns)
        .map(|row| {
            let mut cells: Vec<Node> =
                row.iter().map(|entry| name(entry)).collect();
            cells.resize(columns, Node::text(""));
            cells
        })
        .collect();
    let mut pieces = vec![Node::Table {
        head: Vec::new(),
        rows,
        widths: vec![1.; columns],
    }];
    if rest > 0 {
        pieces.push(more(key, open, format!("+ {rest} more")));
    }
    if listing.truncated {
        pieces.push(Node::Toned {
            text: "cut at the limit".into(),
            tone: Tone::Neutral,
            mono: false,
        });
    }
    Some(Node::stack(pieces))
}

/// One entry's name: a folder's with its slash, a symlink's with where
/// it points, a changed file's with the letter of its change.
fn name(listed: &Listed) -> Node {
    let entry = &listed.entry;
    let mut text = entry.name.clone();
    if listed.is_dir() {
        text.push('/');
    }
    if let Some(target) = &entry.target {
        text.push_str(&format!(" → {target}"));
    }
    let tone = if entry.ignored {
        Some(Tone::Neutral)
    } else if let Some(kind) = listed.change {
        let (letter, tone) = change_look(kind);
        text.push(' ');
        text.push_str(letter);
        Some(tone)
    } else if listed.is_dir() {
        Some(Tone::Info)
    } else {
        None
    };
    match tone {
        Some(tone) => Node::Toned {
            text,
            tone,
            mono: true,
        },
        None => Node::Mono { text },
    }
}

fn change_look(kind: ChangeKind) -> (&'static str, Tone) {
    match kind {
        ChangeKind::Added => ("A", Tone::Good),
        ChangeKind::Modified => ("M", Tone::Warn),
        ChangeKind::Removed => ("D", Tone::Bad),
    }
}
