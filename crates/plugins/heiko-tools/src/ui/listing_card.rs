//! The `ls` card, as view trees (ADR 0045). Its header counts the
//! folders and files (the card's summary); its body is the names in
//! columns on a band, the folders first, each in a tone that says what
//! it is: a folder, a file `@` changes (with the letter of how), one
//! `.gitignore` leaves out. The first few show until the card is opened.

use heiko_ui_plugin::tree::{Node, Tone};

use super::{
    listing::{ChangeKind, DirListing, Listed},
    more,
};

/// The entries a card shows before it is opened.
pub const PEEK: usize = 9;

/// The room between two columns, in characters.
const GUTTER: usize = 3;

/// The names, the folders then the files, read across columns each as
/// wide as its longest name: three, or two on a compact window. None
/// for an empty directory. `key` is the UI state that opens the card.
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
    let names: Vec<(String, Option<Tone>)> =
        entries[..shown].iter().copied().map(name).collect();
    let column = |at: usize| {
        let cells = || names.iter().skip(at).step_by(columns);
        // Every column but the last is padded, so the next starts clear
        // of its longest name.
        let width = cells()
            .map(|(text, _)| text.chars().count())
            .max()
            .filter(|_| at + 1 < columns)
            .map_or(0, |widest| widest + GUTTER);
        Node::stack(cells().map(|(text, tone)| {
            let text = format!("{text:<width$}");
            match tone {
                Some(tone) => Node::Toned {
                    text,
                    tone: *tone,
                    mono: true,
                },
                None => Node::Mono { text },
            }
        }))
    };
    let mut pieces = vec![Node::row((0..columns.min(names.len())).map(column))];
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
    Some(Node::Band { children: pieces })
}

/// One entry's name and the tone it is set in: a folder's with its
/// slash, a symlink's with where it points, a changed file's with the
/// letter of its change.
fn name(listed: &Listed) -> (String, Option<Tone>) {
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
    (text, tone)
}

fn change_look(kind: ChangeKind) -> (&'static str, Tone) {
    match kind {
        ChangeKind::Added => ("A", Tone::Good),
        ChangeKind::Modified => ("M", Tone::Warn),
        ChangeKind::Removed => ("D", Tone::Bad),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn listing(files: usize) -> DirListing {
        let mut entries = vec![json!({ "name": "src", "kind": "dir" })];
        entries.extend((0..files).map(|n| {
            json!({ "name": format!("file-{n}.rs"), "kind": "file", "size": 1 })
        }));
        DirListing::parse(&json!({ "entries": entries, "truncated": false }))
            .expect("a listing")
    }

    /// The texts of each column of a body, and what follows the columns.
    fn columns(body: &Node) -> (Vec<Vec<String>>, usize) {
        let Node::Band { children } = body else {
            panic!("a band: {body:?}");
        };
        let Node::Row { children: columns } = &children[0] else {
            panic!("a row of columns: {:?}", children[0]);
        };
        let texts = columns
            .iter()
            .map(|column| match column {
                Node::Stack { children } => children
                    .iter()
                    .map(|cell| match cell {
                        Node::Mono { text } | Node::Toned { text, .. } => {
                            text.clone()
                        }
                        other => panic!("a name: {other:?}"),
                    })
                    .collect(),
                other => panic!("a column: {other:?}"),
            })
            .collect();
        (texts, children.len() - 1)
    }

    /// The names read across three columns, two on a compact window,
    /// with no table around them; a column's names are as wide as each
    /// other, so the next column lines up.
    #[test]
    fn names_read_across_plain_columns() {
        let listing = listing(4);
        let body = body(&listing, "key", false, false).expect("a body");
        let (wide, after) = columns(&body);
        assert_eq!(after, 0);
        assert_eq!(wide.len(), 3);
        assert_eq!(wide[0].len(), 2);
        assert_eq!(wide[0][0].trim_end(), "src/");
        assert_eq!(wide[0][1].trim_end(), "file-2.rs");
        assert_eq!(wide[1], ["file-0.rs   ", "file-3.rs   "]);
        assert_eq!(wide[0][0].len(), wide[0][1].len());
        // The last column is not padded.
        assert_eq!(wide[2], ["file-1.rs"]);
        let body = super::body(&listing, "key", false, true).expect("a body");
        assert_eq!(columns(&body).0.len(), 2);
    }

    /// Past the first few, a button shows the rest.
    #[test]
    fn a_long_listing_opens_to_every_name() {
        let long = listing(PEEK + 3);
        let count = |open| {
            let body = body(&long, "key", open, false).expect("a body");
            let (columns, after) = columns(&body);
            (columns.iter().map(Vec::len).sum::<usize>(), after)
        };
        assert_eq!(count(false), (PEEK, 1));
        assert_eq!(count(true), (PEEK + 4, 1));
        // A directory with one entry still has a body.
        assert!(body(&listing(0), "key", false, false).is_some());
    }
}
