//! The `vcs_log` card: closed, a bar for each change; open, the working
//! copy and the stack over trunk, each in runs of one commit scope. A
//! change picked in it shows its whole description and ids.

use gpui::{Div, Hsla, SharedString, Stateful, div, prelude::*, rems};
use tau_ui_kit::{
    assets::Icon,
    components::{dot, heading, icon, mono},
    theme::{Design as _, IconSize, Theme, Type, radius, sp, weight},
};

use super::{
    Card,
    change_log::{Change, ChangeLog, ScopeRun},
};

/// A closed log's shape: a bar for each change on the stack, in its
/// scope's color with `@` taller and amber, then a short one for each
/// change on trunk.
pub fn bars(log: &ChangeLog, t: &Theme) -> Div {
    let bar = |height: f32, color| {
        div()
            .w(rems(0.3125))
            .h(rems((height) / 16.))
            .rounded(radius::HAIRLINE)
            .bg(color)
    };
    let stack = log.stack.iter().flat_map(|run| {
        let color = scope_color(run, t);
        run.changes.iter().map(move |_| bar(10., color))
    });
    div()
        .flex()
        .flex_shrink_0()
        .items_end()
        .gap(sp(0.5))
        .h(rems(0.875))
        .children(log.working_copy.as_ref().map(|_| bar(14., t.accent)))
        .children(stack)
        .when(log.trunk_len() > 0, |bars| {
            bars.child(
                div()
                    .w(rems(0.0625))
                    .h_full()
                    .mx(sp(0.75))
                    .bg(t.border_strong),
            )
            .children((0..log.trunk_len()).map(|_| bar(6., t.bar_idle)))
        })
}

/// An open log: the working copy, the stack, then trunk, and the
/// detail of the change picked.
pub fn body(card: &Card<'_>, log: &ChangeLog, t: &Theme, compact: bool) -> Div {
    let picked = card
        .ui
        .picked(&card.run, &card.call_id)
        .and_then(|id| log.change(id));
    let is_picked =
        |change: &Change| picked.is_some_and(|p| p.info == change.info);
    let working_copy = log.working_copy.as_ref().map(|change| {
        let chosen = is_picked(change);
        working_copy_row(row(card, change, chosen, t), change, t)
            .when(!chosen, |row| row.bg(t.accent.opacity(0.07)))
    });
    let stack = scope_runs(&log.stack, card, &is_picked, t, compact);
    let trunk = scope_runs(&log.trunk, card, &is_picked, t, compact);
    let shown = log.changes().count();
    div()
        .flex()
        .flex_col()
        .pb(sp(1.))
        .child(section("Stack · mutable", t))
        .children(working_copy)
        .children(stack)
        .when(!trunk.is_empty(), |body| {
            body.child(
                section("Trunk · immutable", t)
                    .mt(sp(0.5))
                    .border_t_1()
                    .border_color(t.border)
                    .child(icon(Icon::Lock, IconSize::TINY, t.dim)),
            )
            .children(trunk.into_iter().map(|run| run.opacity(0.62)))
        })
        .when(log.more, |body| {
            body.child(
                div()
                    .px(sp(3.))
                    .pt(sp(1.5))
                    .typeset(Type::CAPTION)
                    .text_color(t.dim)
                    .child(format!(
                        "The newest {shown} changes. Older ones are past the \
                         call's limit."
                    )),
            )
        })
        .children(picked.map(|change| detail(change, t)))
}

fn section(label: &str, t: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .gap(sp(2.))
        .h(rems(1.75))
        .px(sp(3.))
        .child(heading(label, t))
}

fn scope_color(run: &ScopeRun, t: &Theme) -> Hsla {
    run.scope.as_deref().map_or(t.dim, |scope| t.mark(scope))
}

/// A row that picks `change` when clicked, or puts it back.
fn row(
    card: &Card<'_>,
    change: &Change,
    chosen: bool,
    t: &Theme,
) -> Stateful<Div> {
    let run_id = card.run.clone();
    let call_id = card.call_id.clone();
    let change_id = change.info.change_id.clone();
    div()
        .id(SharedString::from(format!(
            "log-{}-{}",
            card.call_id, change.info.change_id
        )))
        .flex()
        .items_center()
        .gap(sp(2.5))
        .cursor_pointer()
        .when(chosen, |row| row.bg(t.selected))
        .when(!chosen, |row| row.hover(|row| row.bg(t.raised)))
        .on_click(card.on_ui(move |ui| ui.pick(&run_id, &call_id, &change_id)))
}

/// `@`, pinned above the stack: what it holds, in words.
fn working_copy_row(
    row: Stateful<Div>,
    change: &Change,
    t: &Theme,
) -> Stateful<Div> {
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
    row.h(rems(2.125))
        .px(sp(3.))
        .border_t_1()
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
        .child(
            div()
                .flex_1()
                .min_w(rems(0.))
                .truncate()
                .typeset(Type::SMALL)
                .text_color(if described { t.text_soft } else { t.dim })
                .when(!described, |text| text.italic())
                .child(words.join(" · ")),
        )
        .children(bookmarks(change, t))
        .child(short_id(change, t))
}

/// Each run's scope, then its changes.
fn scope_runs(
    runs: &[ScopeRun],
    card: &Card<'_>,
    is_picked: &dyn Fn(&Change) -> bool,
    t: &Theme,
    compact: bool,
) -> Vec<Div> {
    runs.iter()
        .map(|scope_run| {
            let color = scope_color(scope_run, t);
            let count = scope_run.changes.len();
            let header = div()
                .flex()
                .items_center()
                .gap(sp(2.))
                .h(rems(1.625))
                .px(sp(3.))
                .typeset(Type::CAPTION)
                .child(dot(color, 8.))
                .child(
                    div()
                        .text_color(t.text)
                        .font_weight(weight::EMPHASIS)
                        .child(
                            scope_run
                                .scope
                                .clone()
                                .unwrap_or_else(|| "no scope".into()),
                        ),
                )
                .child(div().text_color(t.dim).child(if count == 1 {
                    "1 change".to_owned()
                } else {
                    format!("{count} changes")
                }));
            let rows: Vec<_> = scope_run
                .changes
                .iter()
                .map(|change| {
                    let row = row(card, change, is_picked(change), t);
                    change_row(row, change, color, t, compact)
                })
                .collect();
            div()
                .flex()
                .flex_col()
                .pt(sp(1.))
                .pb(sp(1.5))
                .child(header)
                .children(rows)
        })
        .collect()
}

/// One change of a scope's run: its type, subject and short id.
fn change_row(
    row: Stateful<Div>,
    change: &Change,
    color: Hsla,
    t: &Theme,
    compact: bool,
) -> Stateful<Div> {
    let (kind_color, kind_bg) = kind_colors(change.kind.as_deref(), t);
    row.h(rems(1.625))
        .pl(sp(3.75))
        .pr(sp(3.))
        .child(
            div()
                .w(rems(0.125))
                .h_full()
                .mr(sp(2.75))
                .flex_shrink_0()
                .bg(color.opacity(0.35)),
        )
        .when(!compact, |row| {
            row.child(
                mono(
                    change.kind.clone().unwrap_or_else(|| "—".into()),
                    Type::MICRO,
                    kind_color,
                )
                .w(rems(2.5))
                .flex_shrink_0()
                .flex()
                .justify_center()
                .rounded(radius::SMALL)
                .bg(kind_bg),
            )
        })
        .child(
            div()
                .flex_1()
                .min_w(rems(0.))
                .truncate()
                .typeset(Type::SMALL)
                .text_color(t.text_soft)
                .child(if change.subject.is_empty() {
                    "(no description set)".to_owned()
                } else {
                    change.subject.clone()
                }),
        )
        .when(change.info.conflict, |row| {
            row.child(mono("conflict", Type::MICRO, t.red).flex_shrink_0())
        })
        .when(change.info.divergent, |row| {
            row.child(mono("divergent", Type::MICRO, t.accent).flex_shrink_0())
        })
        .when(change.info.empty, |row| {
            row.child(mono("empty", Type::MICRO, t.dim).flex_shrink_0())
        })
        .children(bookmarks(change, t))
        .child(short_id(change, t))
}

/// A chip for each of the change's bookmarks, such as `main`.
pub fn bookmarks(change: &Change, t: &Theme) -> impl Iterator<Item = Div> {
    change.info.bookmarks.iter().map(|name| {
        mono(name.clone(), Type::MICRO, t.blue)
            .flex_shrink_0()
            .px(sp(1.5))
            .rounded(radius::SMALL)
            .bg(t.blue_soft)
            .border_1()
            .border_color(t.blue_border)
    })
}

/// A conventional commit type's badge colors: text, then ground.
pub fn kind_colors(kind: Option<&str>, t: &Theme) -> (Hsla, Hsla) {
    match kind {
        Some("feat") => (t.blue, t.blue.opacity(0.1)),
        Some("fix") => (t.red, t.red.opacity(0.1)),
        _ => (t.muted, t.raised),
    }
}

/// The change's short id, in the change color.
pub fn short_id(change: &Change, t: &Theme) -> Div {
    mono(change.short_id().to_owned(), Type::CAPTION, t.change).flex_shrink_0()
}

/// The picked change: its whole description, ids and state.
fn detail(change: &Change, t: &Theme) -> Div {
    let info = &change.info;
    let state = change.state();
    let field = |name: &'static str, value: String, color| {
        div()
            .flex()
            .gap(sp(2.))
            .child(
                mono(name, Type::CAPTION, t.dim)
                    .w(rems(3.25))
                    .flex_shrink_0(),
            )
            .child(mono(value, Type::CAPTION, color))
    };
    let description = info.description.trim();
    div()
        .flex()
        .flex_col()
        .gap(sp(2.))
        .mx(sp(3.))
        .mt(sp(2.5))
        .mb(sp(2.))
        .px(sp(3.5))
        .py(sp(3.))
        .rounded(radius::BOX)
        .bg(t.info_panel)
        .border_1()
        .border_color(t.blue_border)
        .child(
            div()
                .typeset(Type::SMALL)
                .text_color(if description.is_empty() {
                    t.dim
                } else {
                    t.text
                })
                .child(if description.is_empty() {
                    "(no description set)".to_owned()
                } else {
                    description.to_owned()
                }),
        )
        .child(field("change", info.change_id.clone(), t.change))
        .child(field("commit", info.commit_id.clone(), t.blue))
        .child(field("state", state.join(" · "), t.text_soft))
        .when(!info.bookmarks.is_empty(), |panel| {
            panel.child(field("bookmarks", info.bookmarks.join(", "), t.blue))
        })
}

/// Changes as they sit on a stack, newest first, without picking: a
/// landing's preview and its card in the parent's chat.
pub fn stack_rows(changes: &[Change], t: &Theme, compact: bool) -> Vec<Div> {
    changes
        .iter()
        .map(|change| {
            let color =
                change.scope.as_deref().map_or(t.dim, |scope| t.mark(scope));
            let (kind_color, kind_bg) = kind_colors(change.kind.as_deref(), t);
            div()
                .flex()
                .items_center()
                .gap(sp(2.5))
                .h(rems(1.625))
                .pl(sp(3.75))
                .pr(sp(3.))
                .child(
                    div()
                        .w(rems(0.125))
                        .h_full()
                        .mr(sp(2.75))
                        .flex_shrink_0()
                        .bg(color.opacity(0.35)),
                )
                .when(!compact, |row| {
                    row.child(
                        mono(
                            change.kind.clone().unwrap_or_else(|| "—".into()),
                            Type::MICRO,
                            kind_color,
                        )
                        .w(rems(2.5))
                        .flex_shrink_0()
                        .flex()
                        .justify_center()
                        .rounded(radius::SMALL)
                        .bg(kind_bg),
                    )
                })
                .child(
                    div()
                        .flex_1()
                        .min_w(rems(0.))
                        .truncate()
                        .typeset(Type::SMALL)
                        .text_color(t.text_soft)
                        .child(if change.subject.is_empty() {
                            "(no description set)".to_owned()
                        } else {
                            change.subject.clone()
                        }),
                )
                .when(change.info.conflict, |row| {
                    row.child(
                        mono("conflict", Type::MICRO, t.red).flex_shrink_0(),
                    )
                })
                .child(short_id(change, t))
        })
        .collect()
}
