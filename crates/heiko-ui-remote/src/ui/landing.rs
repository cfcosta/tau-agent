//! Landing a finished run (ADR 0009, 0014): a chat lands on the main
//! chat it forked. The controls the compare screen, the fork's card in
//! its parent's chat and the landing note at the end of the run's own
//! chat share.

use gpui::{Context, Div, Hsla, SharedString, div, prelude::*, relative, rems};

use crate::{
    assets::Icon,
    route::Route,
    theme::{Design as _, IconSize, Theme, Type, sp},
    ui::{self, BlockSize},
    view::{Origin, RunView},
    workspace::{LandingState, Workspace},
};

/// How wide a graph's line is, in rems: 3 px.
pub(super) const LINE: f32 = 0.1875;
/// A change's square on a graph's line, in rems: 12 px.
pub(super) const SQUARE: f32 = 0.75;

/// A note at the end of a chat: under a rule in `color`, its title in
/// the display face, and `meta` at the title's right. Callers add what
/// it holds.
pub(super) fn note(
    title: impl Into<SharedString>,
    meta: Option<Div>,
    color: Hsla,
    t: &Theme,
) -> Div {
    div()
        .flex()
        .flex_col()
        .pt(sp(2.5))
        .border_t_2()
        .border_color(color)
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(sp(3.))
                .child(
                    div()
                        .flex_1()
                        .min_w(rems(0.))
                        .typeset(Type::SECTION.sized(17.))
                        .text_color(t.bright)
                        .child(title.into()),
                )
                .children(meta.map(|meta| meta.flex_shrink_0())),
        )
}

/// Where `run` lands, as its buttons name it: its parent's title for a
/// fork. None for a sub-agent, which lands as it returns, and for a
/// main chat, which commits on trunk.
pub fn target(ws: &Workspace, run: &RunView) -> Option<String> {
    match &run.origin {
        Origin::Fork { from, .. } => Some(ws.run(from).map_or_else(
            || "its parent".to_owned(),
            |view| view.title.clone(),
        )),
        Origin::Root | Origin::SubAgent { .. } => None,
    }
}

/// The landing note at the end of `run`'s own chat: what would land,
/// and Land or Cancel.
pub fn card(
    ws: &Workspace,
    run: &RunView,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Option<Div> {
    let target = target(ws, run)?;
    let body = controls(ws, run, t, cx)?;
    Some(
        note(format!("Land {} on {target}", run.title), None, t.blue, t)
            .child(body.mt(sp(2.5))),
    )
}

/// Where the fork's line meets its parent's, as a share of the
/// diagram's width.
const MEET: f32 = 0.62;
/// How far down the diagram each line is, in rems.
const PARENT_Y: f32 = 1.25;
const FORK_Y: f32 = 2.875;
/// The most squares the fork's line has room for.
const SQUARES: usize = 8;

/// The parent's line, and the fork's line joining it with its changes
/// as squares: at a square when the landing is clean, at a cross when
/// it conflicts in `conflicts`.
fn diagram(
    parent: &str,
    fork: &str,
    changes: usize,
    conflicts: &[String],
    t: &Theme,
) -> Div {
    let (parent_color, fork_color) = (t.blue, t.offset);
    let label = |text: String, top: f32, color: Hsla| {
        ui::mono(text, Type::MICRO, color)
            .absolute()
            .left_0()
            .top(rems(top))
            .whitespace_nowrap()
    };
    let shown = changes.min(SQUARES);
    let squares = (1..=shown).map(move |n| {
        div()
            .absolute()
            .left(relative(MEET * n as f32 / (shown + 1) as f32))
            .top(rems(FORK_Y - (SQUARE - LINE) / 2.))
            .size(rems(SQUARE))
            .bg(fork_color)
    });
    // Centered on where the riser meets the parent's line.
    let meet = div()
        .absolute()
        .left(relative(MEET))
        .top(rems(PARENT_Y + LINE / 2. - 0.5))
        .ml(rems(LINE / 2. - 0.5))
        .size(rems(1.))
        .flex()
        .items_center()
        .justify_center();
    let meet = if conflicts.is_empty() {
        meet.child(div().size(rems(SQUARE)).bg(parent_color))
    } else {
        meet.child(ui::icon(Icon::Close, IconSize::HUGE, t.bright))
    };
    div()
        .relative()
        .w_full()
        .h(rems(4.5))
        .overflow_hidden()
        .child(label(parent.to_owned(), 0., parent_color))
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top(rems(PARENT_Y))
                .h(rems(LINE))
                .bg(parent_color),
        )
        .child(
            div()
                .absolute()
                .left_0()
                .w(relative(MEET))
                .top(rems(FORK_Y))
                .h(rems(LINE))
                .bg(fork_color),
        )
        .child(
            div()
                .absolute()
                .left(relative(MEET))
                .top(rems(PARENT_Y + LINE))
                .w(rems(LINE))
                .h(rems(FORK_Y - PARENT_Y))
                .bg(fork_color),
        )
        .children(squares)
        .child(meet)
        .child(label(
            match changes {
                1 => format!("{fork} · 1 change"),
                n => format!("{fork} · {n} changes"),
            },
            FORK_Y + 0.5,
            fork_color,
        ))
        .when(!conflicts.is_empty(), |diagram| {
            diagram.child(
                ui::mono(
                    format!("conflict in {}", super::queue::short(conflicts)),
                    Type::MICRO,
                    t.dim,
                )
                .absolute()
                .left(relative(MEET))
                .top(rems(PARENT_Y + 0.75))
                .ml(sp(4.))
                .whitespace_nowrap(),
            )
        })
}

/// Landing a finished run: Land and Drop, then the preview (what would
/// land, and what would conflict) with Land or Cancel.
pub fn controls(
    ws: &Workspace,
    run: &RunView,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Option<Div> {
    let parent = target(ws, run)?;
    if run.status.is_live() {
        return None;
    }
    if let Some((at, waiting)) = ws.queued(&run.id) {
        return Some(super::queue::queued(run, at, waiting, &parent, t, cx));
    }
    // A main chat that is working, or that others wait on, takes this
    // landing after them: Land queues it (ADR 0024).
    let waits = run
        .origin
        .parent()
        .and_then(|parent| ws.run(parent))
        .is_some_and(|parent| {
            parent.status.is_live()
                || !parent.landing_queue.is_empty()
                || parent.main_conflicts.is_some()
        });
    let id = run.id.clone();
    let keyed =
        |key: &str| div().id(SharedString::from(format!("{key}-{}", run.id)));
    let link =
        |label: String, key: &str| keyed(key).child(ui::link_action(label, t));
    let danger = |label: String, key: &str| {
        keyed(key).child(ui::link_action(label, t).border_color(t.red))
    };
    let primary = |label: String, key: &str| {
        keyed(key).child(ui::primary_block(label, BlockSize::Small, t))
    };
    let actions = || {
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(sp(3.5))
            .typeset(Type::SMALL)
    };
    let line = |text: String, color| ui::text(text, Type::SMALL, color);
    let body = match ws.landing(&run.id) {
        None => {
            let drop_id = id.clone();
            actions()
                .child(link(format!("land on {parent}"), "land").on_click(
                    cx.listener(move |ws, _, _, cx| {
                        ws.preview_landing(&id, cx)
                    }),
                ))
                .child(danger("drop this fork".into(), "drop").on_click(
                    cx.listener(move |ws, _, _, cx| ws.ask_drop(&drop_id, cx)),
                ))
        }
        Some(LandingState::ConfirmDrop) => {
            let (drop_id, cancel_id) = (id.clone(), id.clone());
            div()
                .flex()
                .flex_col()
                .gap(sp(2.5))
                .child(line(
                    format!(
                        "Drop {}? Its changes are abandoned and it closes. \
                         The operation log still has them.",
                        run.title
                    ),
                    t.text_soft,
                ))
                .child(
                    actions()
                        .child(danger("drop".into(), "confirm-drop").on_click(
                            cx.listener(move |ws, _, _, cx| {
                                ws.drop_child(&drop_id, cx)
                            }),
                        ))
                        .child(link("cancel".into(), "cancel-drop").on_click(
                            cx.listener(move |ws, _, _, cx| {
                                ws.cancel_landing(&cancel_id, cx)
                            }),
                        )),
                )
        }
        Some(LandingState::Dropping) => line("Dropping…".into(), t.dim),
        Some(LandingState::Previewing) => {
            line("Checking what would land…".into(), t.dim)
        }
        Some(LandingState::Landing) => {
            line(format!("Landing on {parent}…"), t.dim)
        }
        Some(LandingState::Preview(Err(error))) => div()
            .flex()
            .flex_col()
            .gap(sp(2.5))
            .child(line(error.clone(), t.red))
            .child(actions().child(
                link("close".into(), "cancel-land").on_click(
                    cx.listener(move |ws, _, _, cx| ws.cancel_landing(&id, cx)),
                ),
            )),
        Some(LandingState::Preview(Ok(preview))) => {
            let changes = &preview.changes;
            let conflicts = &preview.conflicts;
            let summary = match (changes.len(), waits) {
                (0, _) => format!("Nothing to land: {parent} has all of it."),
                (_, true) => {
                    format!("{parent} is busy; this lands after its turn.")
                }
                (1, false) => {
                    format!("1 change lands on top of {parent}'s latest.")
                }
                (n, false) => {
                    format!("{n} changes land on top of {parent}'s latest.")
                }
            };
            let rows = changes.iter().map(|change| {
                let id = &change.change_id[..change.change_id.len().min(8)];
                let title = change.description.lines().next().unwrap_or("");
                div()
                    .flex()
                    .items_baseline()
                    .gap(sp(3.5))
                    .py(sp(1.5))
                    .border_b_1()
                    .border_color(t.hairline)
                    .child(
                        div()
                            .flex_1()
                            .min_w(rems(0.))
                            .truncate()
                            .child(title.to_owned()),
                    )
                    .when(change.conflict, |row| {
                        row.child(
                            ui::text("conflict", Type::SMALL, t.red)
                                .flex_shrink_0(),
                        )
                    })
                    .child(
                        ui::mono(id.to_owned(), Type::SMALL, t.change)
                            .flex_shrink_0(),
                    )
            });
            // The compare screen is where its link would go.
            let compare = run
                .origin
                .parent()
                .filter(|_| !matches!(ws.route, Route::Compare { .. }))
                .map(|main| Route::Compare {
                    main: main.clone(),
                    fork: run.id.clone(),
                });
            let (land_id, cancel_id) = (id.clone(), id.clone());
            div()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .when(!changes.is_empty(), |column| {
                    column
                        .child(diagram(
                            &parent,
                            &run.title,
                            changes.len(),
                            conflicts,
                            t,
                        ))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .border_t_1()
                                .border_color(t.hairline)
                                .children(rows),
                        )
                })
                .child(line(summary, t.text_soft))
                .when(!conflicts.is_empty(), |column| {
                    column.child(line(
                        format!(
                            "Landing starts {parent}'s next turn to resolve \
                             {}.",
                            conflicts.join(", ")
                        ),
                        t.red,
                    ))
                })
                .child(
                    actions()
                        .pt(sp(1.))
                        .when(!changes.is_empty(), |row| {
                            row.child(
                                primary(
                                    match (waits, conflicts.is_empty()) {
                                        (true, _) => "queue to land".into(),
                                        (false, true) => "land".into(),
                                        (false, false) => {
                                            "land and resolve".into()
                                        }
                                    },
                                    "confirm-land",
                                )
                                .on_click(
                                    cx.listener(move |ws, _, _, cx| {
                                        ws.land(&land_id, cx)
                                    }),
                                ),
                            )
                        })
                        .child(link("cancel".into(), "cancel-land").on_click(
                            cx.listener(move |ws, _, _, cx| {
                                ws.cancel_landing(&cancel_id, cx)
                            }),
                        ))
                        .when_some(compare, |row, route| {
                            row.child(
                                link("compare".into(), "compare-land")
                                    .on_click(cx.listener(
                                        move |ws, _, _, cx| {
                                            ws.navigate(route.clone(), cx)
                                        },
                                    )),
                            )
                        }),
                )
        }
    };
    Some(body)
}
