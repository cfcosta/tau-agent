//! What waits on a main chat (ADR 0024): the chats queued to land on it
//! once its turn ends, and the conflicts a turn left on its stack. Both
//! sit at the end of the main chat's transcript; a queued chat's own
//! landing note says where it waits.

use gpui::{Context, Div, SharedString, div, prelude::*, rems};

use crate::{
    queue::Waiting,
    theme::{Design as _, Theme, Type, sp},
    ui::{self, BlockSize, SquareStyle, State},
    view::RunView,
    workspace::Workspace,
};

/// The blocks at the end of `run`'s transcript when it is a main chat
/// with chats waiting, or with conflicts left on it.
pub fn cards(
    ws: &Workspace,
    run: &RunView,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Vec<Div> {
    let mut cards = Vec::new();
    if !run.landing_queue.is_empty() {
        cards.push(queue_card(ws, run, t, cx));
    }
    if let Some(card) = conflicts_card(ws, run, t, cx) {
        cards.push(card);
    }
    cards
}

/// Files by their names, as a row has room for.
pub(super) fn short(files: &[String]) -> String {
    files
        .iter()
        .map(|file| file.rsplit('/').next().unwrap_or(file))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A row between hairlines: what leads it, what it is, and what sits
/// at its right.
fn row(lead: Div, t: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .gap(sp(3.5))
        .py(sp(2.25))
        .border_b_1()
        .border_color(t.hairline)
        .child(lead)
}

/// A row's action: an underlined word in small print.
fn action(
    id: SharedString,
    label: &'static str,
    t: &Theme,
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .child(ui::link_action(label, t).typeset(Type::SMALL))
}

fn queue_card(
    ws: &Workspace,
    run: &RunView,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let queue = &run.landing_queue;
    let title = match queue.len() {
        1 => "1 chat waits to land".to_owned(),
        n => format!("{n} chats wait to land"),
    };
    let note = if run.main_conflicts.is_some() {
        "once main is clean"
    } else if run.status.is_live() {
        "after this turn, in order"
    } else if queue[0].needs_confirmation() {
        "the first waits for you"
    } else {
        "in order"
    };
    let rows = queue.iter().enumerate().map(|(at, waiting)| {
        let id = heiko_agent::tool::RunId(waiting.run.as_str().into());
        let title = ws
            .run(&id)
            .map_or_else(|| waiting.title.clone(), |view| view.title.clone());
        let changes = match waiting.changes {
            1 => "1 change".to_owned(),
            n => format!("{n} changes"),
        };
        let place = ui::square(
            (at + 1).to_string(),
            // The first is the next to land.
            if at == 0 {
                SquareStyle::Filled(t.blue)
            } else {
                SquareStyle::Quiet
            },
            t,
        );
        let state = match waiting.conflicts.as_slice() {
            [] => ui::text("clean", Type::SMALL, t.green),
            files => div()
                .flex()
                .gap(sp(1.5))
                .typeset(Type::SMALL)
                .child(
                    div()
                        .text_color(t.red)
                        .child(format!("conflicts in {}", short(files))),
                )
                .child(div().text_color(t.dim).child(
                    if waiting.needs_confirmation() {
                        "· needs confirmation"
                    } else {
                        "· you confirmed"
                    },
                )),
        };
        let open = id.clone();
        row(place, t)
            .child(
                div()
                    .id(SharedString::from(format!("queued-{}", waiting.run)))
                    .flex_1()
                    .min_w(rems(0.))
                    .flex()
                    .items_baseline()
                    .gap(sp(2.))
                    .cursor_pointer()
                    .child(div().min_w(rems(0.)).truncate().child(title))
                    .child(
                        div().flex_shrink_0().text_color(t.dim).child(changes),
                    )
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.navigate(crate::route::Route::Run(open.clone()), cx)
                    })),
            )
            .child(state.flex_shrink_0())
            .child(
                action(
                    SharedString::from(format!("unqueue-{}", waiting.run)),
                    "unqueue",
                    t,
                )
                .on_click(cx.listener(move |ws, _, _, cx| ws.unqueue(&id, cx))),
            )
    });
    div()
        .flex()
        .flex_col()
        .child(ui::section_header(title, Some(note.into()), t))
        .children(rows)
}

fn conflicts_card(
    ws: &Workspace,
    run: &RunView,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Option<Div> {
    let conflicts = run.main_conflicts.as_ref().filter(|c| !c.dismissed)?;
    let from = conflicts.from.as_ref().map(|from| {
        let id = heiko_agent::tool::RunId(from.as_str().into());
        ws.run(&id)
            .map_or_else(|| "a chat".to_owned(), |view| view.title.clone())
    });
    let (when, what) = match &from {
        Some(title) => (
            "after heiko's turn",
            format!(
                "The turn heiko started to resolve {title} left these in \
                 conflict. Nothing lands or forks until main is clean."
            ),
        ),
        None => (
            "after main's turn",
            "Main's last turn left these in conflict. Nothing lands or \
             forks until main is clean."
                .to_owned(),
        ),
    };
    let (again, dismiss) = (run.id.clone(), run.id.clone());
    let live = run.status.is_live();
    let files = conflicts.files.iter().map(|file| {
        row(div().flex_shrink_0().child(ui::mark(State::Failing, t)), t)
            .child(
                ui::mono(file.clone(), Type::SMALL, t.text)
                    .flex_1()
                    .min_w(rems(0.))
                    .truncate(),
            )
            .child(ui::text("conflict", Type::SMALL, t.red).flex_shrink_0())
    });
    Some(
        div()
            .flex()
            .flex_col()
            .child(ui::section_header(
                "Conflicts are still on main",
                Some(when.into()),
                t,
            ))
            .children(files)
            .child(
                ui::text(what, Type::SMALL, t.text_soft)
                    .leading(1.55)
                    .mt(sp(2.5)),
            )
            .child(if live {
                ui::text("heiko is resolving them…", Type::SMALL, t.dim)
                    .mt(sp(2.5))
            } else {
                div()
                    .flex()
                    .items_center()
                    .gap(sp(3.5))
                    .mt(sp(3.))
                    .child(
                        div()
                            .id("resolve-again")
                            .child(ui::primary_block(
                                "resolve again",
                                BlockSize::Small,
                                t,
                            ))
                            .on_click(cx.listener(move |ws, _, _, cx| {
                                ws.resolve_again(&again, cx)
                            })),
                    )
                    .child(
                        action("write-to-main".into(), "I'll write to main", t)
                            .on_click(cx.listener(move |ws, _, _, cx| {
                                ws.dismiss_conflicts(&dismiss, cx)
                            })),
                    )
            }),
    )
}

/// A queued chat's own landing note body: where it waits, and Unqueue.
/// One whose conflicts changed since the person confirmed says so, and
/// offers to land with them.
pub fn queued(
    run: &RunView,
    at: usize,
    waiting: &Waiting,
    parent: &str,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let (land, unqueue) = (run.id.clone(), run.id.clone());
    let unqueue_link = action(
        SharedString::from(format!("unqueue-own-{}", run.id)),
        "unqueue",
        t,
    )
    .on_click(cx.listener(move |ws, _, _, cx| ws.unqueue(&unqueue, cx)));
    let place = ui::square(at.to_string(), SquareStyle::Outlined(t.blue), t);
    if waiting.needs_confirmation() {
        return div()
            .flex()
            .flex_col()
            .gap(sp(2.5))
            .child(
                div().flex().items_start().gap(sp(3.5)).child(place).child(
                    ui::text(
                        format!(
                            "Queued, but it now conflicts in {}, which \
                                 you did not confirm. Landing starts \
                                 {parent}'s turn to resolve them.",
                            waiting.conflicts.join(", ")
                        ),
                        Type::SMALL,
                        t.red,
                    )
                    .flex_1()
                    .min_w(rems(0.))
                    .leading(1.55),
                ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(3.5))
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "reconfirm-{}",
                                run.id
                            )))
                            .child(ui::primary_block(
                                "land and resolve",
                                BlockSize::Small,
                                t,
                            ))
                            .on_click(cx.listener(move |ws, _, _, cx| {
                                ws.land(&land, cx)
                            })),
                    )
                    .child(unqueue_link),
            );
    }
    div()
        .flex()
        .items_center()
        .gap(sp(3.5))
        .child(place)
        .child(
            div()
                .flex_1()
                .min_w(rems(0.))
                .text_color(t.text_soft)
                .child(format!("Queued to land after {parent}'s turn")),
        )
        .child(unqueue_link)
}
