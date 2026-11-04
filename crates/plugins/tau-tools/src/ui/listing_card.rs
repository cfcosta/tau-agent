//! The `ls` card, as view trees (ADR 0045). Closed, it names the
//! directory, counts its folders and files and what they weigh (the
//! card's summary), and flags what `@` changes and what `.gitignore`
//! leaves out. Open, it lists the folders, then the files, each row with
//! a kind mark, the name, a size or entry count, the day it last changed
//! and how `@` changes it. Ignored entries stay in place, with only their
//! name.

use tau_ui_plugin::tree::{Node, Tone};

use super::listing::{ChangeKind, DirListing, Listed, count, size};
use crate::details::EntryKind;

/// The header: the directory, then a badge for each kind of change in
/// `@` and one for ignored entries. What it holds follows, as the
/// card's summary.
pub fn summary(listing: &DirListing, path: &str, compact: bool) -> Node {
    let path = if path.is_empty() { "." } else { path };
    let mut row = vec![Node::Mono { text: path.into() }, Node::Spacer];
    for kind in [ChangeKind::Added, ChangeKind::Modified] {
        let n = listing
            .entries
            .iter()
            .filter(|entry| entry.change == Some(kind))
            .count();
        if n > 0 {
            let (letter, tone) = change_look(kind);
            row.push(Node::Badge {
                text: format!("{letter} {n}"),
                tone,
            });
        }
    }
    if !compact && listing.ignored() > 0 {
        row.push(Node::Badge {
            text: format!("{} ignored", listing.ignored()),
            tone: Tone::Neutral,
        });
    }
    Node::row(row)
}

/// An open listing: the folders, then the files.
pub fn body(listing: &DirListing, compact: bool) -> Node {
    let dirs: Vec<&Listed> = listing.dirs().collect();
    let files: Vec<&Listed> = listing.files().collect();
    let mut pieces = Vec::new();
    if listing.entries.is_empty() {
        pieces.push(Node::Empty {
            text: "Empty directory.".into(),
        });
    }
    if !dirs.is_empty() {
        pieces.push(Node::Heading {
            text: format!("Folders · {}", dirs.len()),
        });
        pieces.push(table(&dirs, compact));
    }
    if !files.is_empty() {
        pieces.push(Node::Heading {
            text: format!(
                "Files · {} · {}",
                files.len(),
                size(listing.bytes())
            ),
        });
        pieces.push(table(&files, compact));
    }
    if listing.truncated {
        pieces.push(Node::text(
            "The listing was cut: entries past the limit are not shown.",
        ));
    }
    Node::stack(pieces)
}

/// Rows of entries, the name given the room; the day each changed is
/// left out on a narrow window.
fn table(entries: &[&Listed], compact: bool) -> Node {
    let widths = if compact {
        vec![0.5, 6., 1.5, 0.5]
    } else {
        vec![0.5, 6., 1.5, 1.75, 0.5]
    };
    Node::Table {
        head: Vec::new(),
        rows: entries.iter().map(|entry| row(entry, compact)).collect(),
        widths,
    }
}

/// One entry: its mark, name, size or count, the day it changed and its
/// change. An ignored one keeps only its name.
fn row(listed: &Listed, compact: bool) -> Vec<Node> {
    let entry = &listed.entry;
    let ignored = entry.ignored;
    let measure = if ignored {
        Some("ignored".to_owned())
    } else if listed.is_vcs() {
        Some("vcs".to_owned())
    } else if listed.is_dir() {
        entry.items.map(|n| match n {
            0 => "empty".to_owned(),
            n => count(n as usize, "item"),
        })
    } else {
        entry.size.map(size)
    };
    let mut name = entry.name.clone();
    if listed.is_dir() {
        name.push('/');
    }
    if let Some(target) = &entry.target {
        name.push_str(&format!("  → {target}"));
    }
    // An ignored entry matters less: dimmed.
    let name = if ignored {
        Node::Toned {
            text: name,
            tone: Tone::Neutral,
            mono: true,
        }
    } else {
        Node::Mono { text: name }
    };
    let mut cells = vec![
        mark(listed),
        name,
        Node::Mono {
            text: measure.unwrap_or_default(),
        },
    ];
    if !compact {
        // The day, not the time since: a view does not read the clock.
        let day = entry.modified.filter(|_| !ignored).map(|modified| {
            tau_ai::time::date(modified.max(0) as u64 / 86_400)
        });
        cells.push(Node::Mono {
            text: day.unwrap_or_default(),
        });
    }
    cells.push(match listed.change {
        Some(kind) => {
            let (letter, tone) = change_look(kind);
            Node::Badge {
                text: letter.into(),
                tone,
            }
        }
        None => Node::text(""),
    });
    cells
}

/// A folder's icon, or a file's badge: the first letter of its type,
/// in its type's tone.
fn mark(listed: &Listed) -> Node {
    let entry = &listed.entry;
    if listed.is_dir() {
        let tone = if entry.ignored {
            Tone::Neutral
        } else if listed.is_vcs() {
            Tone::Warn
        } else {
            Tone::Info
        };
        return Node::Icon {
            name: "folder".into(),
            tone,
        };
    }
    if entry.kind == EntryKind::Symlink {
        return Node::Badge {
            text: "→".into(),
            tone: Tone::Neutral,
        };
    }
    let (letter, tone) = file_look(&entry.name);
    Node::Badge {
        text: letter,
        tone: if entry.ignored { Tone::Neutral } else { tone },
    }
}

/// A file's badge letter and tone, from its extension, or from the name
/// after the dot for a dotfile such as `.envrc`.
fn file_look(name: &str) -> (String, Tone) {
    let kind = match name.rsplit_once('.') {
        Some((_, rest)) => rest,
        None => "",
    }
    .to_lowercase();
    let tone = match kind.as_str() {
        "toml" | "yaml" | "yml" | "json" | "ini" | "conf" | "cfg" => Tone::Warn,
        "md" | "txt" | "rst" | "adoc" | "rs" | "ts" | "tsx" | "js" | "py"
        | "go" | "c" | "h" | "swift" | "kt" | "nix" => Tone::Info,
        "sh" | "bash" | "zsh" | "fish" | "envrc" => Tone::Good,
        _ => Tone::Neutral,
    };
    let letter = kind
        .chars()
        .next()
        .map_or("·".to_owned(), |c| c.to_uppercase().collect());
    (letter, tone)
}

fn change_look(kind: ChangeKind) -> (&'static str, Tone) {
    match kind {
        ChangeKind::Added => ("A", Tone::Good),
        ChangeKind::Modified => ("M", Tone::Warn),
        ChangeKind::Removed => ("D", Tone::Bad),
    }
}
