//! The `vcs_status` card. Closed, it names `@` and its parents, counts
//! the files it adds, modifies and deletes, and flags conflicts and
//! files left out. Open, it shows `@` over its parents, the conflicts
//! and left-out files in boxes of their own, then the files, each
//! opening to its hunks as in the diff card.

use gpui::{Div, Hsla, div, prelude::*, rems};
use tau_ui_kit::{
    assets::Icon,
    components::{heading, icon, mono},
    theme::{Design as _, IconSize, Theme, Tone, Type, radius, sp, weight},
};

use super::{
    Card,
    change_log::Change,
    change_status::ChangeStatus,
    diff_card,
    log_card,
};
use crate::ChangeKind;

/// The card's edge: red while a file holds conflict markers, amber
/// while a file is left out, else none of its own.
pub fn edge(status: &ChangeStatus) -> Option<Tone> {
    if !status.conflicts.is_empty() {
        Some(Tone::Danger)
    } else if !status.too_large.is_empty() {
        Some(Tone::Warn)
    } else {
        None
    }
}

/// The header: `@ ourtwkkx on onvkmqwo`, then a chip for conflicts and
/// for files left out, then a badge for each kind of file.
pub fn summary(status: &ChangeStatus, t: &Theme, compact: bool) -> Div {
    let parents = status
        .parents
        .iter()
        .map(|parent| parent.short_id().to_owned())
        .collect::<Vec<_>>()
        .join(" + ");
    let chip = |text: String, color: Hsla| {
        div()
            .flex_shrink_0()
            .px(sp(1.5))
            .rounded(radius::SMALL)
            .bg(color.opacity(0.14))
            .typeset(Type::MICRO)
            .text_color(color)
            .child(text)
    };
    div()
        .flex_1()
        .min_w(rems(0.))
        .flex()
        .items_center()
        .gap(sp(1.5))
        .typeset(Type::CAPTION.mono())
        .child(div().flex_shrink_0().text_color(t.accent).child("@"))
        .child(log_card::short_id(&status.working_copy, t))
        .when(!parents.is_empty(), |row| {
            row.child(div().flex_shrink_0().text_color(t.dim).child("on"))
                .child(
                    div()
                        .min_w(rems(0.))
                        .truncate()
                        .text_color(t.change)
                        .child(parents),
                )
        })
        .child(div().flex_1())
        .when(!status.conflicts.is_empty(), |row| {
            row.child(chip(count(status.conflicts.len(), "conflict"), t.red))
        })
        .when(!status.too_large.is_empty(), |row| {
            row.child(chip(
                format!("{} left out", status.too_large.len()),
                t.accent,
            ))
        })
        .when(!compact, |row| {
            row.children(status.counts().into_iter().map(|(kind, n)| {
                let (letter, color) = kind_letter(kind, t);
                chip(format!("{letter} {n}"), color)
            }))
        })
}

/// An open status: `@`, its parents, the conflicts and left-out files,
/// then what `@` changes.
pub fn body(
    card: &Card<'_>,
    status: &ChangeStatus,
    t: &Theme,
    compact: bool,
) -> Div {
    let working_copy = &status.working_copy;
    let parents: Vec<Div> = status
        .parents
        .iter()
        .map(|parent| parent_row(parent, t, compact))
        .collect();
    let conflicts = (!status.conflicts.is_empty()).then(|| {
        notice(
            t.red,
            t.danger_surface,
            t.danger_edge,
            &count(status.conflicts.len(), "conflict"),
            "edit the markers out of these files",
            status
                .conflicts
                .iter()
                .map(|path| path_row(path, "!", t.red, None, t))
                .collect(),
            t,
        )
    });
    let too_large = (!status.too_large.is_empty()).then(|| {
        notice(
            t.accent,
            t.accent.opacity(0.06),
            t.accent_border,
            "Left out of @",
            "new files over 1 MiB are not snapshotted",
            status
                .too_large
                .iter()
                .map(|file| {
                    let size =
                        format!("{:.1} MiB", file.size as f64 / 1_048_576.);
                    path_row(&file.path, "?", t.accent, Some(size), t)
                })
                .collect(),
            t,
        )
    });
    let empty = status.files.is_empty().then(|| {
        let tip = status
            .parents
            .first()
            .filter(|parent| !parent.info.bookmarks.is_empty())
            .map(|parent| {
                format!(", the tip of {}", parent.info.bookmarks.join(", "))
            })
            .unwrap_or_default();
        div()
            .flex()
            .items_center()
            .gap(sp(2.))
            .px(sp(3.))
            .pt(sp(3.))
            .pb(sp(1.5))
            .typeset(Type::SMALL)
            .text_color(t.dim)
            .child(icon(Icon::Check, IconSize::COMPACT, t.green))
            .child(format!(
                "@ is empty: edits made now become a new change on {}{tip}.",
                status
                    .parents
                    .first()
                    .map_or("its parent", |parent| parent.short_id())
            ))
    });
    div()
        .flex()
        .flex_col()
        .pb(sp(1.5))
        .child(working_copy_row(working_copy, t))
        .children(parents)
        .children(conflicts)
        .children(too_large)
        .children(empty)
        .when(!status.files.is_empty(), |body| {
            body.child(
                div()
                    .flex()
                    .items_center()
                    .h(rems(1.875))
                    .px(sp(3.))
                    .mt(sp(0.5))
                    .border_t_1()
                    .border_color(t.raised)
                    .child(heading(&format!("In @ · {}", status.summary()), t)),
            )
            .child(diff_card::file_list(
                card,
                &status.files,
                status.truncated,
                &|path| status.is_conflicted(path),
                t,
                compact,
            ))
        })
}

fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

fn kind_letter(kind: ChangeKind, t: &Theme) -> (&'static str, Hsla) {
    match kind {
        ChangeKind::Added => ("A", t.green),
        ChangeKind::Modified => ("M", t.accent),
        ChangeKind::Removed => ("D", t.red),
    }
}

/// `@`, pinned on top: its type and subject once described, else what
/// it is in words.
fn working_copy_row(change: &Change, t: &Theme) -> Div {
    let described = !change.subject.is_empty();
    let mut words = vec!["Working copy"];
    words.extend(
        change
            .state()
            .into_iter()
            .filter(|word| !["working copy", "mutable"].contains(word)),
    );
    if !described {
        words.push("no description yet");
    }
    div()
        .flex()
        .items_center()
        .gap(sp(2.5))
        .h(rems(2.125))
        .px(sp(3.))
        .bg(t.accent.opacity(0.07))
        .border_b_1()
        .border_color(t.border)
        .child(
            mono("@", Type::CAPTION, t.accent)
                .flex_shrink_0()
                .px(sp(1.5))
                .rounded(radius::SMALL)
                .bg(t.accent_soft)
                .font_weight(weight::EMPHASIS),
        )
        .when(described, |row| {
            row.children(change.kind.as_deref().map(|kind| kind_badge(kind, t)))
                .child(
                    div()
                        .flex_1()
                        .min_w(rems(0.))
                        .truncate()
                        .typeset(Type::SMALL)
                        .text_color(t.text_soft)
                        .child(change.subject.clone()),
                )
        })
        .when(!described, |row| {
            row.child(
                div()
                    .flex_1()
                    .min_w(rems(0.))
                    .truncate()
                    .typeset(Type::SMALL)
                    .text_color(t.dim)
                    .italic()
                    .child(words.join(" · ")),
            )
        })
        .children(log_card::bookmarks(change, t))
        .child(log_card::short_id(change, t))
}

fn kind_badge(kind: &str, t: &Theme) -> Div {
    let (color, bg) = log_card::kind_colors(Some(kind), t);
    mono(kind.to_owned(), Type::MICRO, color)
        .w(rems(2.5))
        .flex_shrink_0()
        .flex()
        .justify_center()
        .rounded(radius::SMALL)
        .bg(bg)
}

/// A parent, drawn as the log draws a change, a little quieter.
fn parent_row(parent: &Change, t: &Theme, compact: bool) -> Div {
    let color = parent.scope.as_deref().map_or(t.dim, |scope| t.mark(scope));
    div()
        .flex()
        .items_center()
        .gap(sp(2.5))
        .h(rems(1.875))
        .pl(sp(3.75))
        .pr(sp(3.))
        .opacity(0.8)
        .child(
            div()
                .w(rems(0.125))
                .h_full()
                .mr(sp(0.5))
                .flex_shrink_0()
                .bg(color.opacity(0.35)),
        )
        .child(mono("@-", Type::MICRO, t.dim).w(rems(1.5)).flex_shrink_0())
        .when(!compact, |row| {
            row.children(parent.kind.as_deref().map(|kind| kind_badge(kind, t)))
        })
        .child(
            div()
                .flex_1()
                .min_w(rems(0.))
                .truncate()
                .typeset(Type::SMALL)
                .text_color(t.muted)
                .child(if parent.subject.is_empty() {
                    "(no description set)".to_owned()
                } else {
                    parent.subject.clone()
                }),
        )
        .when(parent.info.immutable, |row| {
            row.child(icon(Icon::Lock, IconSize::TINY, t.dim))
        })
        .children(log_card::bookmarks(parent, t))
        .child(log_card::short_id(parent, t))
}

/// A box for what needs the model's attention: a title, a line on
/// what to do, then the paths.
#[allow(clippy::too_many_arguments)]
fn notice(
    color: Hsla,
    ground: Hsla,
    edge: Hsla,
    title: &str,
    hint: &str,
    rows: Vec<Div>,
    t: &Theme,
) -> Div {
    div()
        .flex()
        .flex_col()
        .mx(sp(3.))
        .mt(sp(2.))
        .pb(sp(1.))
        .rounded(radius::BOX)
        .border_1()
        .border_color(edge)
        .bg(ground)
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(2.))
                .px(sp(3.))
                .pt(sp(2.))
                .pb(sp(1.))
                .typeset(Type::CAPTION)
                .child(icon(Icon::Warning, IconSize::SMALL, color))
                .child(
                    div()
                        .text_color(color)
                        .font_weight(weight::EMPHASIS)
                        .child(title.to_owned()),
                )
                .child(div().text_color(t.muted).child(format!("· {hint}"))),
        )
        .children(rows)
}

/// A path in a notice: a badge, the folder dimmed, the name, and what
/// the notice says of it.
fn path_row(
    path: &str,
    letter: &'static str,
    color: Hsla,
    note: Option<String>,
    t: &Theme,
) -> Div {
    let (dir, name) = path
        .rsplit_once('/')
        .map_or(("", path), |(dir, name)| (dir, name));
    div()
        .flex()
        .items_center()
        .gap(sp(2.5))
        .h(rems(1.75))
        .px(sp(3.))
        .child(
            mono(letter, Type::MICRO, color)
                .size(rems(1.125))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(radius::SMALL)
                .bg(color.opacity(0.14))
                .font_weight(weight::EMPHASIS),
        )
        .child(
            div()
                .flex_1()
                .min_w(rems(0.))
                .flex()
                .typeset(Type::CAPTION.mono())
                .when(!dir.is_empty(), |row| {
                    row.child(
                        div()
                            .min_w(rems(0.))
                            .truncate()
                            .text_color(t.dim)
                            .child(format!("{dir}/")),
                    )
                })
                .child(
                    div()
                        .flex_shrink_0()
                        .text_color(t.text_soft)
                        .child(name.to_owned()),
                ),
        )
        .children(
            note.map(|note| mono(note, Type::MICRO, color).flex_shrink_0()),
        )
}
