//! The `ls` card. Closed, it names the directory, counts its folders
//! and files and what they weigh (the card's summary), and flags what `@` changes and what
//! `.gitignore` leaves out. Open, it lists the folders, then the files,
//! each row with a kind mark, the name, a size or entry count, its age
//! and how `@` changes it. Ignored entries stay in place, dimmed, with
//! only their name.

use gpui::{Div, Hsla, div, prelude::*, px};
use tau_ui_kit::{
    assets::Icon,
    components::{heading, icon, mono},
    theme::{Design as _, IconSize, Theme, Type, radius, sp, weight},
};

use super::listing::{ChangeKind, DirListing, Listed, age, count, size};
use crate::ls::EntryKind;

/// The header: the directory, then a chip for each kind of change in
/// `@` and one for ignored entries. What it holds follows, as the
/// card's summary.
pub fn summary(
    listing: &DirListing,
    path: &str,
    t: &Theme,
    compact: bool,
) -> Div {
    let path = if path.is_empty() { "." } else { path };
    let changes = [ChangeKind::Added, ChangeKind::Modified]
        .into_iter()
        .map(|kind| {
            let n = listing
                .entries
                .iter()
                .filter(|entry| entry.change == Some(kind))
                .count();
            (kind, n)
        })
        .filter(|(_, n)| *n > 0);
    div()
        .flex_1()
        .min_w(px(0.))
        .flex()
        .items_center()
        .gap(sp(1.5))
        .typeset(Type::CAPTION.mono())
        .child(
            div()
                .min_w(px(0.))
                .truncate()
                .text_color(t.text_soft)
                .child(path.to_owned()),
        )
        .child(div().flex_1())
        .children(changes.map(|(kind, n)| {
            let (letter, color) = change_look(kind, t);
            chip(format!("{letter} {n}"), color)
        }))
        .when(!compact && listing.ignored() > 0, |row| {
            row.child(chip(format!("{} ignored", listing.ignored()), t.dim))
        })
}

/// An open listing: the folders, then the files.
pub fn body(listing: &DirListing, t: &Theme, compact: bool) -> Div {
    let now = tau_ai::time::now_seconds() as i64;
    let dirs: Vec<&Listed> = listing.dirs().collect();
    let files: Vec<&Listed> = listing.files().collect();
    let section = |title: String, first: bool| {
        div()
            .flex()
            .items_center()
            .h(px(30.))
            .px(sp(3.))
            .when(!first, |row| row.border_t_1().border_color(t.raised))
            .child(heading(&title, t))
    };
    div()
        .flex()
        .flex_col()
        .pb(sp(1.5))
        .when(listing.entries.is_empty(), |list| {
            list.child(
                div()
                    .px(sp(3.))
                    .py(sp(2.))
                    .typeset(Type::CAPTION)
                    .text_color(t.dim)
                    .child("Empty directory."),
            )
        })
        .when(!dirs.is_empty(), |list| {
            list.child(section(format!("Folders · {}", dirs.len()), true))
                .children(dirs.iter().map(|dir| row(dir, now, t, compact)))
        })
        .when(!files.is_empty(), |list| {
            let title =
                format!("Files · {} · {}", files.len(), size(listing.bytes()));
            list.child(section(title, dirs.is_empty()))
                .children(files.iter().map(|file| row(file, now, t, compact)))
        })
        .when(listing.truncated, |list| {
            list.child(
                div()
                    .px(sp(3.))
                    .pt(sp(1.5))
                    .typeset(Type::CAPTION)
                    .text_color(t.dim)
                    .child(
                        "The listing was cut: entries past the limit are \
                         not shown.",
                    ),
            )
        })
}

/// One entry: its mark, name, size or count, age and change. An ignored
/// one keeps only its name, dimmed.
fn row(listed: &Listed, now: i64, t: &Theme, compact: bool) -> Div {
    let entry = &listed.entry;
    let ignored = entry.ignored;
    let name_color = if ignored {
        t.dim
    } else if entry.name.starts_with('.') {
        t.muted
    } else {
        t.text_soft
    };
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
    div()
        .flex()
        .items_center()
        .gap(sp(2.5))
        .h(px(30.))
        .px(sp(3.))
        .hover(|row| row.bg(t.raised))
        .when(ignored, |row| row.opacity(0.7))
        .child(mark(listed, t))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .typeset(Type::CAPTION.mono())
                .child(
                    div()
                        .min_w(px(0.))
                        .truncate()
                        .text_color(name_color)
                        .child(entry.name.clone()),
                )
                .when(listed.is_dir(), |name| {
                    name.child(
                        div().flex_shrink_0().text_color(t.dim).child("/"),
                    )
                })
                .when_some(entry.target.clone(), |name, target| {
                    name.child(
                        div()
                            .min_w(px(0.))
                            .truncate()
                            .pl(sp(1.5))
                            .text_color(t.dim)
                            .child(format!("→ {target}")),
                    )
                }),
        )
        .child(
            mono(measure.unwrap_or_default(), Type::MICRO, t.dim)
                .w(px(76.))
                .flex_shrink_0()
                .flex()
                .justify_end()
                .when(!ignored && !listed.is_dir(), |cell| {
                    cell.text_color(t.muted)
                }),
        )
        .when(!compact, |row| {
            let ago = entry
                .modified
                .filter(|_| !ignored)
                .map(|modified| age(modified, now));
            row.child(
                mono(ago.unwrap_or_default(), Type::MICRO, t.dim)
                    .w(px(36.))
                    .flex_shrink_0()
                    .flex()
                    .justify_end(),
            )
        })
        .child(div().w(px(18.)).flex_shrink_0().children(listed.change.map(
            |kind| {
                let (letter, color) = change_look(kind, t);
                badge(letter.to_owned(), color)
            },
        )))
}

/// A folder's icon, or a file's badge: the first letter of its type,
/// in its type's color.
fn mark(listed: &Listed, t: &Theme) -> Div {
    let entry = &listed.entry;
    if listed.is_dir() {
        let color = if entry.ignored {
            t.dim
        } else if listed.is_vcs() {
            t.change
        } else {
            t.blue
        };
        return div()
            .size(px(18.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .child(icon(Icon::Folder, IconSize::COMPACT, color));
    }
    if entry.kind == EntryKind::Symlink {
        return badge("→".to_owned(), t.dim);
    }
    let (letter, color) = file_look(&entry.name, t);
    badge(letter, if entry.ignored { t.dim } else { color })
}

/// A file's badge letter and color, from its extension, or from the
/// name after the dot for a dotfile such as `.envrc`.
fn file_look(name: &str, t: &Theme) -> (String, Hsla) {
    let kind = match name.rsplit_once('.') {
        Some((_, rest)) => rest,
        None => "",
    }
    .to_lowercase();
    let color = match kind.as_str() {
        "lock" => t.slate,
        "toml" | "yaml" | "yml" | "json" | "ini" | "conf" | "cfg" => t.accent,
        "md" | "txt" | "rst" | "adoc" => t.blue,
        "rs" | "ts" | "tsx" | "js" | "py" | "go" | "c" | "h" | "swift"
        | "kt" => t.marks[4],
        "nix" => t.marks[5],
        "sh" | "bash" | "zsh" | "fish" | "envrc" => t.green,
        _ => t.muted,
    };
    let letter = kind
        .chars()
        .next()
        .map_or("·".to_owned(), |c| c.to_uppercase().collect());
    (letter, color)
}

fn change_look(kind: ChangeKind, t: &Theme) -> (&'static str, Hsla) {
    match kind {
        ChangeKind::Added => ("A", t.green),
        ChangeKind::Modified => ("M", t.accent),
        ChangeKind::Removed => ("D", t.red),
    }
}

fn badge(text: String, color: Hsla) -> Div {
    mono(text, Type::MICRO, color)
        .size(px(18.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius::SMALL)
        .bg(color.opacity(0.14))
        .font_weight(weight::EMPHASIS)
}

fn chip(text: String, color: Hsla) -> Div {
    div()
        .flex_shrink_0()
        .px(sp(1.5))
        .rounded(radius::SMALL)
        .bg(color.opacity(0.14))
        .typeset(Type::MICRO)
        .text_color(color)
        .child(text)
}
