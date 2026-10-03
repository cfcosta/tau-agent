//! The chat: messages, tool cards, and what plugins say in between.

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

use super::{Material as _, icon, link, mono, rich, stop_look};
use crate::{
    assets::Icon,
    route::Route,
    theme::{Design as _, IconSize, Theme, Type, radius, sp},
    view::{
        DiffKind,
        DiffLine,
        Dropped,
        Item,
        LandedCard,
        PluginNote,
        RunView,
        Tone,
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
    // Past the items, the landing card, while a landing is open, or
    // where a landed chat's work went.
    if index == run.items.len() {
        let card = match run.ending {
            Some(_) => super::ending::landed_card(ws, run, t, cx),
            None => super::interrupted::card(run, t, cx)
                .or_else(|| super::landing::card(ws, run, t, compact, cx)),
        };
        // A main chat's: what waits to land on it, and conflicts left
        // on it.
        let queue = super::queue::cards(ws, run, t, cx);
        return div()
            .px(side)
            .pt(sp(3.))
            .pb(edge)
            .flex()
            .flex_col()
            .gap(sp(4.))
            .children(queue)
            .children(card)
            .into_any_element();
    }
    let Some(item) = run.items.get(index) else {
        return div().into_any_element();
    };
    // What an item can take: the transcript less its sides.
    let room = ws.transcript_width().map(|width| width - side * 2.);
    div()
        .px(side)
        .pt(if index == 0 { edge } else { sp(3.) })
        .when(index + 1 == run.items.len(), |item| item.pb(edge))
        .child(item_view(
            ws, run, index, item, t, compact, room, window, cx,
        ))
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn item_view(
    ws: &Workspace,
    run: &RunView,
    index: usize,
    item: &Item,
    t: &Theme,
    compact: bool,
    room: Option<Pixels>,
    window: &Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    match item {
        // A plugin that reads the message as its own draws it.
        Item::User(text) => ws
            .contributions(
                tau_ui_plugin::points::USER_MESSAGE,
                &tau_ui_plugin::points::AtMessage {
                    run: run.info(),
                    text: text.clone(),
                    index,
                },
                cx,
            )
            .into_iter()
            .next()
            .unwrap_or_else(|| {
                user(text, t, compact, room, window).into_any_element()
            }),
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
            plugin_note(ws, run, index, note, t, compact, cx).into_any_element()
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
        Item::Landed(card) if card.recovered => div()
            .flex()
            .flex_col()
            .gap(sp(2.))
            .child(super::interrupted::finished_landing(&card.title, t))
            .child(landed(card, t, compact, cx))
            .into_any_element(),
        Item::Landed(card) => landed(card, t, compact, cx).into_any_element(),
        Item::ForkReady { fork } => {
            fork_ready(ws, run, fork, t, compact, cx).into_any_element()
        }
        Item::Tau(text) => tau_message(text, t).into_any_element(),
        // The plugin that named its rewrite draws it.
        Item::Rewrite {
            plugin,
            tokens,
            key,
        } => key
            .as_ref()
            .and_then(|key| {
                ws.contributions_of(
                    plugin,
                    tau_ui_plugin::points::REWRITE,
                    &tau_ui_plugin::points::AtRewrite {
                        run: run.info(),
                        key: key.clone(),
                        tokens: *tokens,
                        index,
                    },
                    cx,
                )
                .into_iter()
                .next()
            })
            .unwrap_or_else(|| {
                rewrite(plugin, *tokens, t, compact).into_any_element()
            }),
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
    let dropped = card.dropped.is_some();
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
        data: card.data.clone(),
        summary: card.summary.clone(),
        cut: card.cut.as_deref().cloned(),
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
    // The tool's own plugin draws the card; a call a rewrite dropped
    // shows only its arguments.
    let view = ws
        .contributions_with(tau_ui_plugin::points::CARD, context_of, cx)
        .into_iter()
        .next()
        .filter(|_| !dropped)
        .unwrap_or_default();
    let badges = ws.contributions_with(
        tau_ui_plugin::points::CARD_BADGE,
        context_of,
        cx,
    );
    let extras =
        ws.contributions_with(tau_ui_plugin::points::CARD_BODY, context_of, cx);

    let failed =
        view.failed.is_some() || matches!(card.state, ToolState::Failed(_));
    let (status, border) = match &card.state {
        ToolState::Running => (
            icon(Icon::Spinner, IconSize::COMPACT, t.accent),
            t.accent_border,
        ),
        _ if failed => {
            (icon(Icon::Blocked, IconSize::BASE, t.red), t.red_border)
        }
        ToolState::Done { .. } | ToolState::Failed(_) => {
            (icon(Icon::Check, IconSize::COMPACT, t.green), t.border)
        }
        ToolState::Blocked { .. } => {
            (icon(Icon::Blocked, IconSize::BASE, t.red), t.red_border)
        }
        ToolState::Flagged { .. } => (
            icon(Icon::Warning, IconSize::BASE, t.accent),
            t.accent_border,
        ),
    };
    // A result that wants attention says so on the card's edge.
    let border = match view.edge {
        Some(Tone::Danger) => t.red_border,
        Some(Tone::Warn) => t.accent_border,
        _ => border,
    };
    let summary = match view.head {
        Some(head) => head,
        None => mono(card.summary.clone(), Type::CAPTION, t.text_soft)
            .flex_1()
            .min_w(px(0.))
            .truncate()
            .when(card.dropped == Some(Dropped::Call), |s| {
                s.line_through().text_color(t.muted)
            })
            .into_any_element(),
    };
    // A body that folds starts closed; its header opens it.
    let folds = view.folds && view.body.is_some();
    let open = !folds || ws.card_open(&run.id, &card.call_id);
    let label = match &view.failed {
        Some(failure) => label(failure.clone(), t.red),
        None => match (&card.state, view.label) {
            (ToolState::Done { .. }, Some(text)) => {
                let color = if text.starts_with('+') {
                    t.green
                } else {
                    t.dim
                };
                label(text, color)
            }
            _ => state_label(card, t),
        },
    };
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
        .child(label)
        .children(badges)
        .when(folds, |row| {
            let run_id = run.id.clone();
            let call_id = card.call_id.clone();
            row.cursor_pointer()
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.toggle_card(&run_id, &call_id, cx)
                }))
                .when(!compact, |row| row.children(view.shape))
                .child(icon(
                    if open { Icon::Down } else { Icon::Chevron },
                    IconSize::SMALL,
                    t.dim,
                ))
        });

    let body: Option<AnyElement> = match &card.state {
        ToolState::Blocked { plugin, reason } => Some(
            blocked_body(card, plugin, reason, t, compact).into_any_element(),
        ),
        ToolState::Flagged { .. } => None,
        _ => view.body.filter(|_| open),
    };

    // What a plugin cut from the result, and the file that holds the
    // whole output, which opens with the system's application; unless
    // the body says it itself.
    let cut =
        card.cut
            .as_ref()
            .filter(|_| !dropped && !view.inset)
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
                                    mono(
                                        cut.archive.clone(),
                                        Type::MICRO,
                                        t.blue,
                                    )
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
        .child(header.when(body.is_some() && !view.inset, |h| {
            h.border_b_1().border_color(t.border)
        }))
        .children(body)
        .children(cut)
        .children(extras)
}

/// A child run's landing, in its parent's chat: what came, and what is
/// left to resolve. The child's title opens its closed chat.
fn landed(
    card: &LandedCard,
    t: &Theme,
    compact: bool,
    cx: &mut Context<Workspace>,
) -> Div {
    let from = card.from.clone();
    let count = match card.changes.len() {
        1 => "1 change".to_owned(),
        n => format!("{n} changes"),
    };
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
                .id(SharedString::from(format!("landed-{}", card.from.0)))
                .flex()
                .items_center()
                .gap(sp(2.))
                .min_h(px(36.))
                .px(sp(3.))
                .cursor_pointer()
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.navigate(Route::Run(from.clone()), cx)
                }))
                .child(icon(Icon::Fork, IconSize::COMPACT, t.change))
                .child(mono("landed", Type::CAPTION, t.blue).flex_shrink_0())
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .typeset(Type::SMALL)
                        .text_color(t.text_soft)
                        .child(card.title.clone()),
                )
                .child(mono(count, Type::CAPTION, t.dim).flex_shrink_0()),
        )
        .child(tau_vcs::ui::landed::landed_body(card, t, compact))
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

/// A card's line at the header's end.
fn label(text: String, color: gpui::Hsla) -> Div {
    div()
        .flex_shrink_0()
        .max_w(px(260.))
        .truncate()
        .typeset(Type::CAPTION)
        .text_color(color)
        .child(text)
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
        ToolState::Blocked { plugin, .. } => {
            (format!("Blocked by {plugin}"), t.red)
        }
        ToolState::Flagged { .. } => {
            ("Ran · flagged for review".into(), t.accent)
        }
    };
    label(text, color)
}

/// A refused call: what the model proposed, and the reason it got
/// back. The plugin that refused it draws the rest under it.
fn blocked_body(
    card: &ToolCard,
    plugin: &str,
    reason: &str,
    t: &Theme,
    compact: bool,
) -> Div {
    let proposed: Vec<DiffLine> = proposed_text(card.args())
        .into_iter()
        .map(|text| DiffLine {
            kind: DiffKind::Added,
            text,
        })
        .collect();
    div()
        .flex()
        .flex_col()
        .when(!proposed.is_empty(), |body| {
            body.child(tau_ui_kit::diff::view(&proposed, t))
        })
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
                                ),
                        )
                        .child(
                            div()
                                .text_color(t.text_soft)
                                .line_height(relative(1.5))
                                .child(rich(reason, t.text_soft, t)),
                        ),
                ),
        )
}

#[allow(clippy::too_many_arguments)]
fn plugin_note(
    ws: &Workspace,
    run: &RunView,
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
}

/// A rewrite no plugin draws: who rewrote the context, and what it
/// saved when the run saw it.
fn rewrite(
    plugin: &str,
    tokens_seen: Option<(u64, u64)>,
    t: &Theme,
    compact: bool,
) -> Div {
    let line = || div().flex_1().h(px(1.)).bg(t.blue_border);
    let what = match tokens_seen {
        Some((before, after)) => format!(
            "rewrote the context: {} to {}",
            tokens(before),
            tokens(after)
        ),
        None => "rewrote the context".to_owned(),
    };
    div()
        .flex()
        .items_center()
        .gap(sp(3.))
        .py(sp(1.))
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
                    pill.child(mono(plugin.to_owned(), Type::CAPTION, t.blue))
                })
                .child(div().text_color(t.text_soft).child(what)),
        )
        .child(line())
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
