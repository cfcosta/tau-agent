//! The chat: messages, tool cards, and what plugins say in between.

use std::collections::HashSet;

use gpui::{
    AnyElement,
    Context,
    Div,
    IntoElement,
    Pixels,
    SharedString,
    Window,
    div,
    prelude::*,
    px,
    relative,
};

use super::{
    Material as _,
    bar,
    button,
    diff_card,
    dot,
    icon,
    link,
    listing_card,
    log_card,
    mono,
    rich,
    status_card,
    stop_look,
    term_card,
};
use crate::{
    assets::Icon,
    route::Route,
    theme::{Design as _, IconSize, Theme, Type, radius, sp, weight},
    ui::components::ButtonKind,
    view::{
        DiffKind,
        DiffLine,
        Item,
        NoteBody,
        PluginNote,
        Pruned,
        RunView,
        ToolBody,
        ToolCard,
        ToolState,
        proposed_text,
        tokens,
        usd,
    },
    workspace::Workspace,
};

/// The run's item at `index`, padded as the transcript lays it out: the
/// transcript is a list that builds only the items in view. `compact`
/// is the phone layout.
pub fn item(
    ws: &Workspace,
    run: &RunView,
    index: usize,
    t: &Theme,
    compact: bool,
    window: &Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let edge = sp(if compact { 4. } else { 5. });
    let side = sp(if compact { 4. } else { 6. });
    // Past the items, the landing card, while a landing is open.
    if index == run.items.len() {
        return div()
            .px(side)
            .pt(sp(3.))
            .pb(edge)
            .children(super::landing::card(ws, run, t, compact, cx))
            .into_any_element();
    }
    let Some(item) = run.items.get(index) else {
        return div().into_any_element();
    };
    let empty = HashSet::new();
    let kept = ws.kept.get(&run.id).unwrap_or(&empty);
    // What an item can take: the transcript less its sides.
    let room = ws.transcript_width().map(|width| width - side * 2.);
    div()
        .px(side)
        .pt(if index == 0 { edge } else { sp(3.) })
        .when(index + 1 == run.items.len(), |item| item.pb(edge))
        .child(item_view(
            ws, run, kept, index, item, t, compact, room, window, cx,
        ))
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn item_view(
    ws: &Workspace,
    run: &RunView,
    kept: &HashSet<String>,
    index: usize,
    item: &Item,
    t: &Theme,
    compact: bool,
    room: Option<Pixels>,
    window: &Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    match item {
        Item::User(text) => {
            user(text, t, compact, room, window).into_any_element()
        }
        Item::Goal(condition) => {
            goal_set(run, condition, t, compact).into_any_element()
        }
        Item::Text(text) => div()
            .w_full()
            .text_color(t.text_soft)
            .line_height(relative(1.6))
            .child(super::markdown(
                &format!("{}-{index}", run.id.0),
                text,
                t.text_soft,
                t,
            ))
            .into_any_element(),
        Item::Thinking(text) => thinking(text, t).into_any_element(),
        Item::TurnEnd { turn } => {
            turn_end(ws, run, *turn, t, compact, cx).into_any_element()
        }
        Item::Tool(card) => {
            tool(ws, run, card, t, compact, cx).into_any_element()
        }
        Item::Plugin(note) => {
            plugin_note(ws, run, kept, index, note, t, compact, cx)
                .into_any_element()
        }
        Item::Anchor { plugin, key } => {
            let at = tau_ui_plugin::points::AtAnchor {
                run: run.info(),
                key: key.clone(),
                index,
            };
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .children(ws.contributions_of(
                    plugin,
                    tau_ui_plugin::points::TRANSCRIPT,
                    &at,
                    cx,
                ))
                .into_any_element()
        }
        Item::Landed(card) => {
            log_card::landed(card, t, compact, cx).into_any_element()
        }
        Item::ForkReady { fork } => {
            fork_ready(ws, run, fork, t, compact, cx).into_any_element()
        }
        Item::Tau(text) => tau_message(text, t).into_any_element(),
        Item::Rewrite {
            plugin,
            tokens_before,
            tokens_after,
            detail,
        } => rewrite(
            run,
            plugin,
            *tokens_before,
            *tokens_after,
            detail.as_deref(),
            t,
            compact,
            cx,
        )
        .into_any_element(),
        Item::Retry {
            attempt,
            delay,
            error,
        } => div()
            .flex()
            .items_center()
            .gap(sp(2.))
            .typeset(Type::CAPTION)
            .text_color(t.dim)
            .child(icon(Icon::Warning, IconSize::SMALL, t.accent))
            .child(format!(
                "Retry {attempt} in {:.1}s: {error}",
                delay.as_secs_f32()
            ))
            .into_any_element(),
        // A conversation that stopped by itself waits for the next
        // message: nothing to say.
        Item::Stop {
            stop: tau_agent::event::StopReason::Stop,
            ..
        } => div().into_any_element(),
        Item::Stop {
            stop,
            turns,
            tokens: used,
            cost,
            plugin_cost,
        } => {
            let (color, label) = stop_look(stop, t);
            let glyph = match stop {
                tau_agent::event::StopReason::Stop => Icon::Check,
                tau_agent::event::StopReason::Cancelled => Icon::Stop,
                _ => Icon::Warning,
            };
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(sp(2.5))
                .py(sp(2.))
                .border_t_1()
                .border_b_1()
                .border_color(t.border)
                .child(icon(glyph, IconSize::COMPACT, color))
                .child(div().text_color(color).child(label))
                .child(mono(
                    format!(
                        "{turns} turns · {} tokens · {}",
                        tokens(*used),
                        usd(*cost)
                    ),
                    Type::CAPTION,
                    t.muted,
                ))
                .when(*plugin_cost > 0.0, |row| {
                    row.child(mono(
                        format!("plugins {}", usd(*plugin_cost)),
                        Type::CAPTION,
                        t.dim,
                    ))
                })
                .into_any_element()
        }
    }
}

fn user(
    text: &str,
    t: &Theme,
    compact: bool,
    room: Option<Pixels>,
    window: &Window,
) -> Div {
    // Attached files show as their names, not their content.
    let (said, files) = split_attachments(text);
    let widest = px(if compact { 300. } else { 620. });
    let widest = room.map_or(widest, |room| widest.min(room));
    // The bubble's padding and border, on both sides.
    let frame = sp(3.5) * 2. + px(2.);
    // A set width, as wide as the text or the widest the bubble gets, so
    // the text is measured at the width it is drawn at.
    let width = super::rich_width(said, t, window).min(widest - frame);
    div().flex().justify_end().child(
        super::bubble(t)
            .max_w(widest)
            .flex()
            .flex_col()
            .gap(sp(2.))
            .child(div().w(width).child(rich(said, t.text, t)))
            .when(!files.is_empty(), |bubble| {
                bubble.child(div().flex().flex_wrap().gap(sp(1.5)).children(
                    files.into_iter().map(|name| {
                        div()
                            .flex()
                            .items_center()
                            .gap(sp(1.))
                            .typeset(Type::CAPTION)
                            .text_color(t.text_soft)
                            .child(icon(
                                Icon::Paperclip,
                                IconSize::SMALL,
                                t.muted,
                            ))
                            .child(name.to_owned())
                    }),
                ))
            }),
    )
}

/// The person setting a goal: the condition, and the goal's limits
/// while it is the conversation's goal.
fn goal_set(run: &RunView, condition: &str, t: &Theme, compact: bool) -> Div {
    let limits = run
        .goal
        .as_ref()
        .filter(|goal| goal.condition == condition)
        .map(|goal| {
            [
                format!("up to {} continuations", goal.max_continuations),
                format!("{} budget", usd(goal.budget)),
                "checked by Jev".to_owned(),
            ]
        });
    div().flex().justify_end().child(
        super::bubble(t)
            .border_color(t.accent_border)
            .max_w(px(if compact { 300. } else { 620. }))
            .flex()
            .flex_col()
            .gap(sp(2.))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(sp(2.))
                    .child(icon(Icon::Target, IconSize::BASE, t.accent))
                    .child(mono("/goal", Type::CAPTION, t.accent))
                    .child(div().flex_1().child(rich(condition, t.text, t))),
            )
            .when_some(limits, |bubble, limits| {
                bubble.child(div().flex().flex_wrap().gap(sp(1.5)).children(
                    limits.into_iter().map(|limit| {
                        mono(limit, Type::MICRO, t.muted)
                            .px(sp(2.))
                            .py(sp(0.5))
                            .border_1()
                            .border_color(t.border)
                            .rounded(radius::LARGE)
                    }),
                ))
            }),
    )
}

/// A message's own words, and the names of the files attached after
/// them.
fn split_attachments(text: &str) -> (&str, Vec<&str>) {
    const MARK: &str = "\n\n<attached file=\"";
    let Some(start) = text.find(MARK) else {
        return (text, Vec::new());
    };
    let names = text[start..]
        .match_indices("<attached file=\"")
        .filter_map(|(at, open)| {
            let rest = &text[start + at + open.len()..];
            rest.find('"').map(|end| &rest[..end])
        })
        .collect();
    (&text[..start], names)
}

/// Where one turn ends. Nothing shows there; on a desktop, hovering the
/// gap offers a fork from that turn. A phone forks from the run's sheet.
fn turn_end(
    ws: &Workspace,
    run: &RunView,
    turn: u32,
    t: &Theme,
    compact: bool,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let group = SharedString::from(format!("turn-{turn}"));
    let forkable = !compact && ws.can_fork_at(run, turn);
    let id = run.id.clone();
    div().relative().when(forkable, |gap| {
        gap.child(
            div()
                .group(group.clone())
                .absolute()
                .right_0()
                .top(px(-10.))
                .h(px(20.))
                .w(px(240.))
                .flex()
                .items_center()
                .justify_end()
                .child(
                    div()
                        .id(SharedString::from(format!("fork-at-{turn}")))
                        .opacity(0.)
                        .group_hover(group, |style| style.opacity(1.))
                        .child(link(format!("Fork from turn {turn}"), t))
                        .on_click(cx.listener(move |ws, _, window, cx| {
                            ws.fork_from(&id, turn, window, cx)
                        })),
                ),
        )
    })
}

fn thinking(text: &str, t: &Theme) -> Div {
    let words = text.split_whitespace().count();
    div()
        .flex()
        .items_center()
        .gap(sp(2.))
        .typeset(Type::CAPTION)
        .text_color(t.dim)
        .child(icon(Icon::Chevron, IconSize::SMALL, t.dim))
        .child(format!("Reasoned · {words} words"))
}

fn tool(
    ws: &Workspace,
    run: &RunView,
    card: &ToolCard,
    t: &Theme,
    compact: bool,
    cx: &mut Context<Workspace>,
) -> Div {
    let (status, border) = match &card.state {
        ToolState::Running => (
            icon(Icon::Spinner, IconSize::COMPACT, t.accent),
            t.accent_border,
        ),
        ToolState::Done { .. } => {
            (icon(Icon::Check, IconSize::COMPACT, t.green), t.border)
        }
        ToolState::Failed(_) => {
            (icon(Icon::Blocked, IconSize::BASE, t.red), t.red_border)
        }
        ToolState::Blocked { .. } => {
            (icon(Icon::Blocked, IconSize::BASE, t.red), t.red_border)
        }
        ToolState::Flagged { .. } => (
            icon(Icon::Warning, IconSize::BASE, t.accent),
            t.accent_border,
        ),
    };
    // A status with conflicts or left-out files says so on its edge.
    let border = match &card.body {
        ToolBody::Status(status) => {
            status_card::border(status, t).unwrap_or(border)
        }
        _ => border,
    };
    let dropped = matches!(
        card.pruned,
        Some(Pruned::ResultDropped | Pruned::CallDropped)
    );
    // A pruned call shows its size from the ledger: what pruning weighed.
    let ledger_size = card.pruned.and_then(|_| {
        run.ledger
            .iter()
            .find(|entry| entry.call_id == card.call_id)
            .map(|entry| tokens(entry.tokens))
    });
    let summary = match &card.body {
        ToolBody::Files(diff) if !dropped => diff_card::files_summary(diff, t),
        ToolBody::Commit(diff) if !dropped => {
            diff_card::commit_summary(diff, t)
        }
        ToolBody::Status(status) if !dropped => {
            status_card::summary(status, t, compact)
        }
        ToolBody::Listing(listing) if !dropped => {
            listing_card::summary(listing, &card.summary, t, compact)
        }
        _ => mono(card.summary.clone(), Type::CAPTION, t.text_soft)
            .flex_1()
            .min_w(px(0.))
            .truncate()
            .when(card.pruned == Some(Pruned::CallDropped), |s| {
                s.line_through().text_color(t.muted)
            }),
    };

    let review = matches!(card.state, ToolState::Flagged { .. }).then(|| {
        let run_id = run.id.clone();
        let call_id = card.call_id.clone();
        div()
            .id(SharedString::from(format!("review-{}", card.call_id)))
            .child(link("Review", t))
            .on_click(cx.listener(move |ws, _, _, cx| {
                ws.review_call(&run_id, &call_id, cx)
            }))
    });

    // A log, diff or show starts closed; its header opens it.
    let folds = !dropped
        && matches!(
            card.body,
            ToolBody::Log(_)
                | ToolBody::Files(_)
                | ToolBody::Commit(_)
                | ToolBody::Status(_)
                | ToolBody::Listing(_)
        );
    let open = !folds || ws.card_open(&run.id, &card.call_id);
    // What plugins draw on the card, each given its own anchors on it.
    let at_card = |plugin: &str| tau_ui_plugin::points::AtCard {
        run: run.info(),
        call_id: card.call_id.clone(),
        tool: card.tool.clone(),
        keys: card
            .anchors
            .iter()
            .filter(|(by, _)| by == plugin)
            .map(|(_, key)| key.clone())
            .collect(),
    };
    let names: Vec<String> = crate::plugins::registry()
        .plugins()
        .map(|plugin| plugin.name().to_owned())
        .collect();
    let contexts: Vec<(String, tau_ui_plugin::points::AtCard)> = names
        .into_iter()
        .map(|name| {
            let at = at_card(&name);
            (name, at)
        })
        .collect();
    let context_of = |plugin: &str| {
        contexts
            .iter()
            .find(|(name, _)| name == plugin)
            .map(|(_, at)| at)
    };
    let badges = ws.contributions_with(
        tau_ui_plugin::points::CARD_BADGE,
        context_of,
        cx,
    );
    let extras =
        ws.contributions_with(tau_ui_plugin::points::CARD_BODY, context_of, cx);
    let header = div()
        .id(SharedString::from(format!("tool-{}", card.call_id)))
        .flex()
        .items_center()
        .gap(sp(2.))
        .min_h(px(if compact {
            44.
        } else if dropped {
            32.
        } else {
            36.
        }))
        .px(sp(3.))
        .child(status)
        .child(mono(card.tool.clone(), Type::CAPTION, t.blue).flex_shrink_0())
        .when_some(
            card.from_plugin.clone().filter(|_| !compact),
            |row, plugin| {
                row.child(
                    div()
                        .flex_shrink_0()
                        .px(sp(1.5))
                        .py(sp(0.25))
                        .border_1()
                        .border_color(t.border)
                        .rounded(radius::SMALL)
                        .typeset(Type::MICRO)
                        .text_color(t.dim)
                        .child(plugin),
                )
            },
        )
        .child(summary)
        .when(!compact && !card.checks.is_empty(), |row| {
            row.child(mono(card.checks.join(" · "), Type::MICRO, t.dim))
        })
        .child(match &ledger_size {
            Some(size) => {
                mono(size.clone(), Type::MICRO, t.dim).into_any_element()
            }
            None => state_label(card, t).into_any_element(),
        })
        .when_some(card.pruned, |row, pruned| {
            row.child(
                div()
                    .flex_shrink_0()
                    .px(sp(1.5))
                    .py(sp(0.25))
                    .border_1()
                    .border_color(t.border_strong)
                    .rounded(radius::SMALL)
                    .typeset(Type::MICRO)
                    .text_color(if pruned == Pruned::Kept {
                        t.muted
                    } else {
                        t.dim
                    })
                    .child(pruned.label()),
            )
        })
        .children(review)
        .children(badges)
        .when(folds, |row| {
            let run_id = run.id.clone();
            let call_id = card.call_id.clone();
            let shape = match &card.body {
                ToolBody::Log(log) => Some(log_card::bars(log, t)),
                ToolBody::Files(diff) | ToolBody::Commit(diff) => {
                    Some(diff_card::blocks(diff, t))
                }
                _ => None,
            };
            row.cursor_pointer()
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.toggle_card(&run_id, &call_id, cx)
                }))
                .when(!compact, |row| row.children(shape))
                .child(icon(
                    if open { Icon::Down } else { Icon::Chevron },
                    IconSize::SMALL,
                    t.dim,
                ))
        });

    let body: Option<AnyElement> = match (&card.state, &card.body) {
        (
            ToolState::Blocked {
                plugin,
                rule,
                reason,
                score,
            },
            _,
        ) => Some(
            blocked_body(ws, card, plugin, rule, reason, score, t, compact, cx)
                .into_any_element(),
        ),
        (ToolState::Flagged { .. }, _) => None,
        _ if dropped => None,
        (_, ToolBody::Log(log)) => open.then(|| {
            log_card::body(ws, run, card, log, t, compact, cx)
                .into_any_element()
        }),
        (_, ToolBody::Files(diff)) => open.then(|| {
            diff_card::files_body(ws, run, card, diff, t, compact, cx)
                .into_any_element()
        }),
        (_, ToolBody::Commit(diff)) => open.then(|| {
            diff_card::commit_body(ws, run, card, diff, t, compact, cx)
                .into_any_element()
        }),
        (_, ToolBody::Delegated(landed)) => {
            let child = landed.from.clone();
            Some(
                div()
                    .flex()
                    .flex_col()
                    .child(log_card::landed_body(landed, t, compact))
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "open-child-{}",
                                landed.from.0
                            )))
                            .px(sp(3.))
                            .py(sp(2.))
                            .border_t_1()
                            .border_color(t.border)
                            .child(link("Open the sub-agent's chat", t))
                            .on_click(cx.listener(move |ws, _, _, cx| {
                                ws.navigate(Route::Run(child.clone()), cx)
                            })),
                    )
                    .into_any_element(),
            )
        }
        (_, ToolBody::Listing(listing)) => open.then(|| {
            listing_card::body(listing, t, compact).into_any_element()
        }),
        (_, ToolBody::Status(status)) => open.then(|| {
            status_card::body(ws, run, card, status, t, compact, cx)
                .into_any_element()
        }),
        (_, ToolBody::Diff(lines)) => Some(diff(lines, t).into_any_element()),
        (_, ToolBody::Terminal(term)) => Some(
            term_card::body(ws, &run.id, card, term, t, compact, cx)
                .into_any_element(),
        ),
        (_, ToolBody::Output(lines)) if !lines.is_empty() => {
            Some(output(lines, t).into_any_element())
        }
        _ => None,
    };

    // What output pruning cut from the result, and the file that holds
    // the whole output, which opens with the system's application. A
    // terminal's strip says it itself.
    let terminal = matches!(card.body, ToolBody::Terminal(_));
    let cut = card
        .cut
        .as_ref()
        .filter(|_| !dropped && !terminal)
        .map(|cut| {
            let path = std::path::PathBuf::from(&cut.archive);
            div()
                .flex()
                .items_center()
                .gap(sp(2.))
                .min_w(px(0.))
                .px(sp(3.))
                .py(sp(1.5))
                .border_t_1()
                .border_color(t.border)
                .child(
                    mono(
                        format!("{} · full output", cut.label()),
                        Type::MICRO,
                        t.dim,
                    )
                    .flex_shrink_0(),
                )
                .when(!compact, |row| {
                    row.child(
                        div()
                            .id(SharedString::from(format!(
                                "archive-{}",
                                card.call_id
                            )))
                            .flex_1()
                            .min_w(px(0.))
                            .cursor_pointer()
                            .hover(|style| style.underline())
                            .child(
                                mono(cut.archive.clone(), Type::MICRO, t.blue)
                                    .truncate(),
                            )
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.open_with_system(&path)
                            })),
                    )
                })
        });

    div()
        .flex()
        .flex_col()
        .border_1()
        .border_color(border)
        .rounded(radius::BOX)
        .raised(t)
        .overflow_hidden()
        .when(dropped, |card| card.opacity(0.7))
        .child(header.when(body.is_some() && !terminal, |h| {
            h.border_b_1().border_color(t.border)
        }))
        .children(body)
        .children(cut)
        .children(extras)
}

/// A finished fork, waiting in its parent's chat: what it is, and Land
/// or Drop. A closed fork (landed or dropped) shows nothing.
/// A message tau sent to start a turn itself (ADR 0014): resolving what
/// a landing or a merge left in conflict.
fn tau_message(text: &str, t: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(sp(1.5))
        .pl(sp(3.))
        .border_l_2()
        .border_color(t.accent_border)
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(1.5))
                .child(icon(Icon::Land, IconSize::SMALL, t.accent))
                .child(mono("started by tau", Type::CAPTION, t.accent)),
        )
        .child(
            div()
                .typeset(Type::SMALL)
                .child(crate::ui::prose(text, t.muted, t)),
        )
}

fn fork_ready(
    ws: &Workspace,
    run: &RunView,
    fork: &tau_agent::tool::RunId,
    t: &Theme,
    compact: bool,
    cx: &mut Context<Workspace>,
) -> Div {
    let Some(view) = ws.run(fork).filter(|_| !ws.is_closed(fork)) else {
        return div();
    };
    let (open, compare) = (fork.clone(), fork.clone());
    let main = run.id.clone();
    div()
        .flex()
        .flex_col()
        .border_1()
        .border_color(t.border)
        .rounded(radius::BOX)
        .raised(t)
        .overflow_hidden()
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(2.))
                .min_h(px(36.))
                .px(sp(3.))
                .child(icon(Icon::Fork, IconSize::COMPACT, t.change))
                .child(mono("fork", Type::CAPTION, t.blue).flex_shrink_0())
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .typeset(Type::SMALL)
                        .text_color(t.text_soft)
                        .child(view.title.clone()),
                )
                .child(
                    mono(
                        format!("finished · turn {}", view.turn),
                        Type::CAPTION,
                        t.dim,
                    )
                    .flex_shrink_0(),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .px(sp(3.))
                .py(sp(2.5))
                .border_t_1()
                .border_color(t.border)
                .children(super::landing::controls(ws, view, t, compact, cx))
                .child(
                    div()
                        .flex()
                        .gap(sp(4.))
                        .child(
                            div()
                                .id(SharedString::from(format!(
                                    "fork-chat-{fork}"
                                )))
                                .child(link("Open its chat", t))
                                .on_click(cx.listener(move |ws, _, _, cx| {
                                    ws.navigate(Route::Run(open.clone()), cx)
                                })),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!(
                                    "fork-compare-{fork}"
                                )))
                                .child(link("Compare", t))
                                .on_click(cx.listener(move |ws, _, _, cx| {
                                    ws.navigate(
                                        Route::Compare {
                                            main: main.clone(),
                                            fork: compare.clone(),
                                        },
                                        cx,
                                    )
                                })),
                        ),
                ),
        )
}

fn state_label(card: &ToolCard, t: &Theme) -> Div {
    let (text, color): (String, _) = match &card.state {
        ToolState::Running => ("running".into(), t.accent),
        ToolState::Done { summary } => {
            let text = summary.clone().unwrap_or_default();
            let color = if text.starts_with('+') {
                t.green
            } else {
                t.dim
            };
            (text, color)
        }
        ToolState::Failed(error) => (error.clone(), t.red),
        ToolState::Blocked { rule, .. } => {
            (format!("Blocked by {rule}"), t.red)
        }
        ToolState::Flagged { rule, score, .. } => {
            return div()
                .flex()
                .items_center()
                .gap(sp(2.))
                .flex_shrink_0()
                .child(
                    div()
                        .typeset(Type::CAPTION)
                        .text_color(t.accent)
                        .child("Ran · flagged for review"),
                )
                .child(mono(format!("{rule} {score}"), Type::MICRO, t.dim));
        }
    };
    div()
        .flex_shrink_0()
        .max_w(px(260.))
        .truncate()
        .typeset(Type::CAPTION)
        .text_color(color)
        .child(text)
}

/// The first number in a score such as `p 0.95 ≥ 0.80`.
fn probability(score: &str) -> Option<f32> {
    score
        .split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .find(|part| part.contains('.'))
        .and_then(|part| part.parse().ok())
}

#[allow(clippy::too_many_arguments)]
fn blocked_body(
    ws: &Workspace,
    card: &ToolCard,
    plugin: &str,
    rule: &str,
    reason: &str,
    score: &str,
    t: &Theme,
    compact: bool,
    cx: &mut Context<Workspace>,
) -> Div {
    let repo = ws.current().map_or("", |run| ws.repo_of(run));
    let route = Route::Constitution {
        repo: repo.to_owned(),
        rule: Some(rule.to_owned()),
    };
    let proposed: Vec<DiffLine> = proposed_text(&card.args)
        .into_iter()
        .map(|text| DiffLine {
            kind: DiffKind::Added,
            text,
        })
        .collect();
    let thresholds = ws
        .repo_named(repo)
        .constitution
        .rules
        .iter()
        .find(|known| known.id == rule)
        .map(|known| (known.review, known.block));
    let meter = probability(score).map(|p| {
        let tick = |at: f32| {
            div()
                .absolute()
                .top(px(-3.))
                .left(relative(at))
                .w(px(2.))
                .h(px(14.))
                .bg(t.text)
        };
        div()
            .w(px(if compact { 150. } else { 220. }))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(sp(1.5))
            .child(
                div()
                    .flex()
                    .typeset(Type::CAPTION)
                    .child(
                        div()
                            .flex_1()
                            .text_color(t.muted)
                            .child("p(violation)"),
                    )
                    .child(mono(format!("{p:.2}"), Type::CAPTION, t.text)),
            )
            .child(
                div()
                    .relative()
                    .child(bar(p, 8., t.red, t.border))
                    .when_some(thresholds, |track, (review, block)| {
                        track.child(tick(review)).child(tick(block))
                    }),
            )
            .when_some(thresholds, |meter, (review, block)| {
                meter.child(
                    div()
                        .relative()
                        .h(px(14.))
                        .child(
                            div()
                                .absolute()
                                .left(relative(review))
                                .ml(sp(-5.))
                                .child(mono("review", Type::MICRO, t.dim)),
                        )
                        .child(
                            div()
                                .absolute()
                                .left(relative(block))
                                .ml(sp(-4.))
                                .child(mono("block", Type::MICRO, t.dim)),
                        ),
                )
            })
    });
    div()
        .flex()
        .flex_col()
        .when(!proposed.is_empty(), |body| body.child(diff(&proposed, t)))
        .child(
            div()
                .flex()
                .when(compact, |row| row.flex_col())
                .items_start()
                .gap(sp(4.))
                .px(sp(3.))
                .py(sp(3.))
                .bg(t.danger_surface)
                .border_t_1()
                .border_color(t.danger_edge)
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .gap(sp(1.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(sp(2.))
                                .child(mono(
                                    plugin.to_owned(),
                                    Type::CAPTION,
                                    t.red,
                                ))
                                .child(
                                    div()
                                        .typeset(Type::CAPTION)
                                        .text_color(t.muted)
                                        .child("Reason sent to the model"),
                                )
                                .child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "rule-{}",
                                            card.call_id
                                        )))
                                        .child(link(format!("Rule {rule}"), t))
                                        .on_click(cx.listener(
                                            move |ws, _, _, cx| {
                                                ws.navigate(route.clone(), cx)
                                            },
                                        )),
                                ),
                        )
                        .child(
                            div()
                                .text_color(t.text_soft)
                                .line_height(relative(1.5))
                                .child(rich(reason, t.text_soft, t)),
                        ),
                )
                .children(meter)
                .when(probability(score).is_none(), |row| {
                    row.child(mono(score.to_owned(), Type::CAPTION, t.red))
                }),
        )
}

pub fn diff(lines: &[DiffLine], t: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .py(sp(1.5))
        .font_family(crate::theme::MONO)
        .typeset(Type::CAPTION)
        .line_height(px(20.))
        .children(lines.iter().map(|line| {
            let (sign, color, bg) = match line.kind {
                DiffKind::Added => {
                    ("+ ", t.added_text, Some(t.green.opacity(0.12)))
                }
                DiffKind::Removed => {
                    ("- ", t.removed_text, Some(t.red.opacity(0.12)))
                }
                DiffKind::Context => ("  ", t.dim, None),
            };
            div()
                .px(sp(3.))
                .text_color(color)
                .whitespace_nowrap()
                .overflow_hidden()
                .when_some(bg, |row, bg| row.bg(bg))
                .child(format!("{sign}{}", line.text))
        }))
}

pub fn output(lines: &[String], t: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .px(sp(3.))
        .py(sp(2.))
        .font_family(crate::theme::MONO)
        .typeset(Type::CAPTION)
        .line_height(px(20.))
        .text_color(t.muted)
        .children(lines.iter().map(|line| {
            let pass = line.trim_start().starts_with("PASS");
            div()
                .whitespace_nowrap()
                .overflow_hidden()
                .when(pass, |row| row.text_color(t.green))
                .child(line.clone())
        }))
}

#[allow(clippy::too_many_arguments)]
fn plugin_note(
    ws: &Workspace,
    run: &RunView,
    kept: &HashSet<String>,
    index: usize,
    note: &PluginNote,
    t: &Theme,
    compact: bool,
    cx: &mut Context<Workspace>,
) -> Div {
    // A chart's note starts closed; its header opens it. The phone
    // links to the chart instead.
    let folds = false;
    let open = !folds || ws.note_open(&run.id, index);
    let header = div()
        .id(SharedString::from(format!("note-{index}")))
        .flex()
        .items_center()
        .gap(sp(2.))
        .when(folds, |row| {
            let run_id = run.id.clone();
            row.cursor_pointer()
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.toggle_note(&run_id, index, cx)
                }))
                .child(icon(
                    if open { Icon::Down } else { Icon::Chevron },
                    IconSize::SMALL,
                    t.dim,
                ))
        })
        .child(
            div()
                .size(px(20.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(radius::TAG)
                .bg(t.info_surface)
                .child(if note.plugin == tau_goal::NAME {
                    icon(Icon::Target, IconSize::COMPACT, t.tone(note.tone))
                } else {
                    icon(Icon::Plug, IconSize::COMPACT, t.blue)
                }),
        )
        .child(
            mono(note.plugin.clone(), Type::CAPTION, t.tone(note.tone))
                .flex_shrink_0(),
        )
        .when(!compact, |row| {
            row.child(div().child(rich(&note.text, t.text_soft, t)))
        })
        .child(div().flex_1())
        .when_some(note.detail.clone().filter(|_| !compact), |row, detail| {
            row.child(mono(detail, Type::MICRO, t.dim).flex_shrink_0())
        })
        .when_some(
            ws.plugin_route_named(&note.plugin, &run.id),
            |row, route| {
                row.child(
                    div()
                        .id(SharedString::from(format!("details-{index}")))
                        .child(link("Details", t))
                        .on_click(cx.listener(move |ws, _, _, cx| {
                            cx.stop_propagation();
                            ws.navigate(route.clone(), cx)
                        })),
                )
            },
        );

    let body: Option<AnyElement> = match &note.body {
        NoteBody::None => None,
        NoteBody::Chips(chips) => {
            Some(chips_view(ws, chips, index, t, cx).into_any_element())
        }
        NoteBody::Proposals(proposals) => Some(
            div()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .children(proposals.iter().enumerate().map(|(n, proposal)| {
                    let is_kept = kept.contains(&proposal.title);
                    let run_id = run.id.clone();
                    let title = proposal.title.clone();
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(sp(2.5))
                        .px(sp(3.))
                        .py(sp(2.5))
                        .border_1()
                        .border_color(t.border)
                        .rounded(radius::BOX)
                        .raised(t)
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(sp(0.75))
                                .flex_1()
                                .min_w(px(180.))
                                .child(
                                    div()
                                        .font_weight(weight::EMPHASIS)
                                        .child(proposal.title.clone()),
                                )
                                .child(
                                    div()
                                        .typeset(Type::CAPTION)
                                        .text_color(t.muted)
                                        .child(proposal.detail.clone()),
                                ),
                        )
                        .child(if is_kept {
                            div()
                                .flex()
                                .items_center()
                                .gap(sp(1.5))
                                .typeset(Type::CAPTION)
                                .text_color(t.green)
                                .child(icon(
                                    Icon::Check,
                                    IconSize::COMPACT,
                                    t.green,
                                ))
                                .child("Kept")
                                .into_any_element()
                        } else {
                            div()
                                .id(SharedString::from(format!(
                                    "keep-{index}-{n}"
                                )))
                                .child(button("Keep", ButtonKind::Primary, t))
                                .on_click(cx.listener(move |ws, _, _, cx| {
                                    ws.keep_note(&run_id, &title, cx)
                                }))
                                .into_any_element()
                        })
                }))
                .into_any_element(),
        ),
    };

    div()
        .flex()
        .flex_col()
        .gap(sp(2.))
        .px(sp(3.))
        .py(sp(2.))
        .rounded(radius::BOX)
        .bg(t.blue_soft)
        .border_1()
        .border_dashed()
        .border_color(t.blue_border)
        .child(header)
        .when(compact, |card| {
            card.child(
                div()
                    .text_color(t.text_soft)
                    .line_height(relative(1.45))
                    .child(rich(&note.text, t.text_soft, t)),
            )
        })
        .when_some(body.filter(|_| open), |card, body| {
            let indent = match (compact, folds) {
                (true, _) => 0.,
                (false, true) => 12.,
                (false, false) => 7.,
            };
            card.child(div().pl(sp(indent)).child(body))
        })
}

/// Memory note titles; the ones memory holds open the note.
pub fn chips_view(
    ws: &Workspace,
    chips: &[String],
    index: usize,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    // Notes come from the memory of the open run's repository.
    let repo = ws.current().map_or("", |run| ws.repo_of(run));
    div().flex().flex_wrap().gap(sp(1.5)).children(
        chips.iter().enumerate().map(|(n, chip)| {
            let route = ws.repo_named(repo).memory.by_title(chip).map(|note| {
                Route::Memory {
                    repo: repo.to_owned(),
                    note: Some(note.id.clone()),
                }
            });
            div()
                .id(SharedString::from(format!("chip-{index}-{n}")))
                .px(sp(2.25))
                .py(sp(0.75))
                .rounded(radius::CARD)
                .border_1()
                .border_color(t.border)
                .typeset(Type::CAPTION)
                .text_color(t.text_soft)
                .child(chip.clone())
                .when_some(route, |chip, route| {
                    chip.cursor_pointer()
                        .hover(|style| style.border_color(t.blue))
                        .on_click(cx.listener(move |ws, _, _, cx| {
                            ws.navigate(route.clone(), cx)
                        }))
                })
        }),
    )
}

#[allow(clippy::too_many_arguments)]
fn rewrite(
    run: &RunView,
    plugin: &str,
    before: u64,
    after: u64,
    detail: Option<&str>,
    t: &Theme,
    compact: bool,
    cx: &mut Context<Workspace>,
) -> Div {
    let route = Route::Ledger(run.id.clone());
    let saved = before.saturating_sub(after);
    let line = || div().flex_1().h(px(1.)).bg(t.blue_border);
    div()
        .flex()
        .flex_col()
        .gap(sp(1.5))
        .py(sp(1.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(3.))
                .child(line())
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(sp(2.))
                        .px(sp(3.))
                        .py(sp(1.5))
                        .rounded(radius::BUBBLE)
                        .border_1()
                        .border_dashed()
                        .border_color(t.blue_border)
                        .bg(t.blue_soft)
                        .child(icon(Icon::Plug, IconSize::COMPACT, t.blue))
                        .when(!compact, |pill| {
                            pill.child(mono(
                                plugin.to_owned(),
                                Type::CAPTION,
                                t.blue,
                            ))
                        })
                        .child(div().text_color(t.text_soft).child(format!(
                            "pruned {} tokens: {} to {}",
                            tokens(saved),
                            tokens(before),
                            tokens(after)
                        )))
                        .when_some(
                            detail.filter(|_| !compact),
                            |pill, detail| {
                                pill.child(mono(
                                    detail.to_owned(),
                                    Type::MICRO,
                                    t.dim,
                                ))
                            },
                        )
                        .child(
                            div()
                                .id("open-ledger")
                                .child(link("Ledger", t))
                                .on_click(cx.listener(move |ws, _, _, cx| {
                                    ws.navigate(route.clone(), cx)
                                })),
                        ),
                )
                .child(line()),
        )
        .child(
            div()
                .flex()
                .justify_center()
                .gap(sp(1.5))
                .typeset(Type::CAPTION)
                .text_color(t.dim)
                .child(dot(t.blue_border, 4.))
                .child(
                    "The next request resends the pruned transcript once, \
                     then turns are deltas again.",
                ),
        )
}

#[cfg(test)]
mod attachment_tests {
    use super::split_attachments;

    #[test]
    fn attached_files_are_named_not_shown() {
        let text = "use them\n\n<attached file=\"a.md\">\nA\n</attached>\n\n<attached file=\"b.rs\">\nB\n</attached>";
        assert_eq!(split_attachments(text), ("use them", vec!["a.md", "b.rs"]));
        assert_eq!(split_attachments("plain"), ("plain", vec![]));
    }
}
