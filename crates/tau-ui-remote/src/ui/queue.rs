//! What waits on a main chat (ADR 0021): the chats queued to land on it
//! once its turn ends, and the conflicts a turn left on its stack. Both
//! are cards at the end of the main chat's transcript; a queued chat's
//! own landing card says where it waits.

use gpui::{Context, Div, SharedString, div, prelude::*, px};

use crate::{
    assets::Icon,
    queue::Waiting,
    theme::{Design as _, IconSize, Theme, Type, radius, sp, weight},
    ui::{self, Material as _, components::ButtonKind},
    view::RunView,
    workspace::Workspace,
};

/// The cards at the end of `run`'s transcript when it is a main chat
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

/// How a queued chat's landing reads: clean, or the files it would
/// conflict in and whether the person confirmed them.
pub fn state(waiting: &Waiting) -> String {
    match waiting.conflicts.as_slice() {
        [] => "clean".into(),
        files if waiting.needs_confirmation() => {
            format!("conflicts in {} · needs confirmation", short(files))
        }
        files => format!("conflicts in {} · you confirmed", short(files)),
    }
}

/// Files by their names, as a row has room for.
fn short(files: &[String]) -> String {
    files
        .iter()
        .map(|file| file.rsplit('/').next().unwrap_or(file))
        .collect::<Vec<_>>()
        .join(", ")
}

fn header(icon: Icon, color: gpui::Hsla, title: String, note: Div) -> Div {
    div()
        .flex()
        .items_center()
        .gap(sp(2.))
        .min_h(px(40.))
        .px(sp(3.))
        .child(ui::icon(icon, IconSize::COMPACT, color))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .font_weight(weight::STRONG)
                .child(title),
        )
        .child(note)
}

/// A quiet text button, as a row's Unqueue.
fn quiet(
    id: SharedString,
    label: &'static str,
    t: &Theme,
) -> gpui::Stateful<Div> {
    let hover = t.text;
    div()
        .id(id)
        .flex_shrink_0()
        .cursor_pointer()
        .typeset(Type::SMALL)
        .text_color(t.text_soft)
        .hover(move |style| style.text_color(hover))
        .child(label)
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
        "They land once main is clean"
    } else if run.status.is_live() {
        "They land in order when main finishes this turn"
    } else if queue[0].needs_confirmation() {
        "The first waits for you to confirm its conflicts"
    } else {
        "They land in order"
    };
    let rows = queue.iter().enumerate().map(|(at, waiting)| {
        let id = tau_agent::tool::RunId(waiting.run.as_str().into());
        let title = ws
            .run(&id)
            .map_or_else(|| waiting.title.clone(), |view| view.title.clone());
        let changes = match waiting.changes {
            1 => " · 1 change".to_owned(),
            n => format!(" · {n} changes"),
        };
        let color = if waiting.conflicts.is_empty() {
            t.green
        } else {
            t.red
        };
        let open = id.clone();
        div()
            .flex()
            .items_center()
            .gap(sp(2.5))
            .px(sp(3.))
            .py(sp(2.))
            .when(at > 0, |row| row.border_t_1().border_color(t.border_soft))
            .child(div().w(px(14.)).child(ui::mono(
                format!("{}", at + 1),
                Type::MICRO,
                t.dim,
            )))
            .child(
                div()
                    .id(SharedString::from(format!("queued-{}", waiting.run)))
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .cursor_pointer()
                    .child(title)
                    .child(div().text_color(t.muted).child(changes))
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.navigate(crate::route::Route::Run(open.clone()), cx)
                    })),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .typeset(Type::CAPTION)
                    .text_color(color)
                    .child(state(waiting)),
            )
            .child(
                quiet(
                    SharedString::from(format!("unqueue-{}", waiting.run)),
                    "Unqueue",
                    t,
                )
                .on_click(cx.listener(move |ws, _, _, cx| ws.unqueue(&id, cx))),
            )
    });
    div()
        .flex()
        .flex_col()
        .border_1()
        .border_color(t.accent_border)
        .rounded(radius::BOX)
        .raised(t)
        .overflow_hidden()
        .child(header(
            Icon::History,
            t.accent,
            title,
            div().typeset(Type::CAPTION).text_color(t.muted).child(note),
        ))
        .child(
            div()
                .flex()
                .flex_col()
                .border_t_1()
                .border_color(t.border)
                .children(rows),
        )
}

fn conflicts_card(
    ws: &Workspace,
    run: &RunView,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Option<Div> {
    let conflicts = run.main_conflicts.as_ref().filter(|c| !c.dismissed)?;
    let from = conflicts.from.as_ref().map(|from| {
        let id = tau_agent::tool::RunId(from.as_str().into());
        ws.run(&id)
            .map_or_else(|| "a chat".to_owned(), |view| view.title.clone())
    });
    let (when, what) = match &from {
        Some(title) => (
            "after tau's turn",
            div()
                .flex()
                .flex_wrap()
                .gap_x(sp(1.))
                .child("The turn tau started to resolve")
                .child(div().text_color(t.text).child(title.clone()))
                .child("ended with conflicts left in these files."),
        ),
        None => (
            "after main's turn",
            div().child(
                "Main's last turn ended with conflicts left in these files.",
            ),
        ),
    };
    let (again, dismiss) = (run.id.clone(), run.id.clone());
    let live = run.status.is_live();
    Some(
        div()
            .flex()
            .flex_col()
            .border_1()
            .border_color(t.red_border)
            .rounded(radius::BOX)
            .bg(t.danger_surface)
            .overflow_hidden()
            .child(header(
                Icon::Warning,
                t.red,
                "Conflicts are still on main".into(),
                ui::mono(when, Type::MICRO, t.muted),
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(sp(2.5))
                    .px(sp(3.))
                    .py(sp(2.5))
                    .border_t_1()
                    .border_color(t.danger_edge)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .text_color(t.text_soft)
                            .line_height(gpui::relative(1.5))
                            .child(what)
                            .child(
                                "Landing waits until main is clean, and new \
                                 chats wait too, so they don't start from \
                                 conflicted code.",
                            ),
                    )
                    .child(div().flex().flex_col().children(
                        conflicts.files.iter().map(|file| {
                            ui::mono(file.clone(), Type::SMALL, t.removed_text)
                        }),
                    ))
                    .child(if live {
                        div()
                            .typeset(Type::CAPTION)
                            .text_color(t.dim)
                            .child("tau is resolving them…")
                    } else {
                        div()
                            .flex()
                            .gap(sp(2.))
                            .child(
                                div()
                                    .id("resolve-again")
                                    .child(ui::button(
                                        "Resolve again",
                                        ButtonKind::Primary,
                                        t,
                                    ))
                                    .on_click(cx.listener(
                                        move |ws, _, _, cx| {
                                            ws.resolve_again(&again, cx)
                                        },
                                    )),
                            )
                            .child(
                                div()
                                    .id("write-to-main")
                                    .child(ui::button(
                                        "I'll write to main",
                                        ButtonKind::Secondary,
                                        t,
                                    ))
                                    .on_click(cx.listener(
                                        move |ws, _, _, cx| {
                                            ws.dismiss_conflicts(&dismiss, cx)
                                        },
                                    )),
                            )
                    }),
            ),
    )
}

/// A queued chat's own landing card body: where it waits, and Unqueue.
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
    let unqueue_button = div()
        .id(SharedString::from(format!("unqueue-own-{}", run.id)))
        .child(ui::button("Unqueue", ButtonKind::Secondary, t))
        .on_click(cx.listener(move |ws, _, _, cx| ws.unqueue(&unqueue, cx)));
    if waiting.needs_confirmation() {
        return div()
            .flex()
            .flex_col()
            .gap(sp(2.))
            .child(div().typeset(Type::CAPTION).text_color(t.red).child(
                format!(
                    "Queued (position {at}), but its landing now conflicts \
                         in {}, which you did not confirm. It waits for you: \
                         landing starts {parent}'s turn to resolve them.",
                    waiting.conflicts.join(", ")
                ),
            ))
            .child(
                div()
                    .flex()
                    .gap(sp(2.))
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "reconfirm-{}",
                                run.id
                            )))
                            .child(ui::button(
                                "Land and resolve",
                                ButtonKind::Primary,
                                t,
                            ))
                            .on_click(cx.listener(move |ws, _, _, cx| {
                                ws.land(&land, cx)
                            })),
                    )
                    .child(unqueue_button),
            );
    }
    div()
        .flex()
        .items_center()
        .gap(sp(3.))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .items_center()
                .gap(sp(2.))
                .child(ui::icon(Icon::History, IconSize::COMPACT, t.accent))
                .child(div().text_color(t.text_soft).child(format!(
                    "Queued — lands after {parent}'s turn (position {at})"
                ))),
        )
        .child(unqueue_button)
}
