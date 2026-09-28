//! The chat: messages, tool cards, and what plugins say in between.

use std::collections::HashSet;

use gpui::{
    AnyElement,
    Context,
    Div,
    IntoElement,
    SharedString,
    div,
    prelude::*,
    px,
    relative,
};

use super::{bar, button, dot, icon, link, mono, rich, stop_look};
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

/// Every item of the run, top to bottom. `compact` is the phone layout.
pub fn items(
    ws: &Workspace,
    run: &RunView,
    t: &Theme,
    compact: bool,
    cx: &mut Context<Workspace>,
) -> Vec<AnyElement> {
    let empty = HashSet::new();
    let kept = ws.kept.get(&run.id).unwrap_or(&empty);
    run.items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            item_view(ws, run, kept, index, item, t, compact, cx)
        })
        .collect()
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
    cx: &mut Context<Workspace>,
) -> AnyElement {
    match item {
        Item::User(text) => user(text, t, compact).into_any_element(),
        Item::Text(text) => div()
            .max_w(px(760.))
            .text_color(t.text_soft)
            .line_height(relative(1.6))
            .child(rich(text, t.text_soft, t))
            .into_any_element(),
        Item::Thinking(text) => thinking(text, t).into_any_element(),
        Item::TurnEnd { turn } => {
            turn_end(run, *turn, t, compact, cx).into_any_element()
        }
        Item::Tool(card) => {
            tool(ws, run, card, t, compact, cx).into_any_element()
        }
        Item::Plugin(note) => {
            plugin_note(ws, run, kept, index, note, t, compact, cx)
                .into_any_element()
        }
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

fn user(text: &str, t: &Theme, compact: bool) -> Div {
    div().flex().justify_end().child(
        super::bubble(t)
            .max_w(px(if compact { 300. } else { 620. }))
            .child(rich(text, t.text, t)),
    )
}

/// Where a turn ended: a quiet rule with the turn's number, and a way to
/// fork the run from here, shown on hover (always on a phone, which has
/// no hover).
/// Where one turn ends. Nothing shows there; on a desktop, hovering the
/// gap offers a fork from that turn. A phone forks from the run's sheet.
fn turn_end(
    run: &RunView,
    turn: u32,
    t: &Theme,
    compact: bool,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let group = SharedString::from(format!("turn-{turn}"));
    let forkable = !compact && Workspace::can_fork_at(run, turn);
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
    let summary = mono(card.summary.clone(), Type::CAPTION, t.text_soft)
        .flex_1()
        .min_w(px(0.))
        .truncate()
        .when(card.pruned == Some(Pruned::CallDropped), |s| {
            s.line_through().text_color(t.muted)
        });

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

    let header = div()
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
        .children(review);

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
        (_, ToolBody::Diff(lines)) => Some(diff(lines, t).into_any_element()),
        (_, ToolBody::Output(lines)) if !lines.is_empty() => {
            Some(output(lines, t).into_any_element())
        }
        _ => None,
    };

    div()
        .flex()
        .flex_col()
        .border_1()
        .border_color(border)
        .rounded(radius::BOX)
        .bg(t.card)
        .overflow_hidden()
        .when(dropped, |card| card.opacity(0.7))
        .child(
            header.when(body.is_some(), |h| {
                h.border_b_1().border_color(t.border)
            }),
        )
        .children(body)
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
    let header = div()
        .flex()
        .items_center()
        .gap(sp(2.))
        .child(
            div()
                .size(px(20.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(radius::TAG)
                .bg(t.info_surface)
                .child(icon(Icon::Plug, IconSize::COMPACT, t.blue)),
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
                            ws.navigate(route.clone(), cx)
                        })),
                )
            },
        );

    let note_plugin = note.plugin.clone();
    let body: Option<AnyElement> = match &note.body {
        NoteBody::None => None,
        NoteBody::Chips(chips) => {
            Some(chips_view(ws, chips, index, t, cx).into_any_element())
        }
        NoteBody::Distribution {
            levels,
            chosen,
            note,
            ..
        } if compact => {
            // The phone shows the answer and links to the chart.
            let level = levels
                .get(*chosen)
                .map_or(String::new(), |(name, _)| name.clone());
            ws.plugin_route_named(&note_plugin, &run.id).map(|route| {
                div()
                    .id(SharedString::from(format!("why-{index}")))
                    .child(link(format!("Why {level}"), t).typeset(Type::SMALL))
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.navigate(route.clone(), cx)
                    }))
                    .into_any_element()
            })
        }
        NoteBody::Distribution {
            levels,
            chosen,
            note,
            ..
        } => Some(
            distribution(levels, *chosen, note, t, compact, 44.)
                .into_any_element(),
        ),
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
                        .bg(t.card)
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
        .when_some(body, |card, body| {
            card.child(div().pl(sp(if compact { 0. } else { 7. })).child(body))
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

/// A probability per level, the chosen one highlighted. `height` is
/// the tallest bar.
pub fn distribution(
    levels: &[(String, f32)],
    chosen: usize,
    note: &str,
    t: &Theme,
    compact: bool,
    height: f32,
) -> Div {
    let columns = levels.iter().enumerate().map(|(index, (name, p))| {
        let pick = index == chosen;
        let ink = if pick { t.text } else { t.muted };
        div()
            .flex()
            .flex_col()
            .items_center()
            .justify_end()
            .gap(sp(0.75))
            .w(px(if compact { 52. } else { 58. }))
            .child(mono(format!("{p:.2}"), Type::MICRO, ink))
            .child(
                div()
                    .w(px(22.))
                    .h(px((p * height).max(3.)))
                    .rounded_t(radius::BAR)
                    .bg(if pick { t.blue } else { t.bar_idle }),
            )
            .child(
                mono(name.clone(), Type::MICRO, ink)
                    .w_full()
                    .pt(sp(0.75))
                    .border_t_1()
                    .border_color(t.border_strong)
                    .flex()
                    .justify_center(),
            )
    });
    div()
        .flex()
        .flex_wrap()
        .items_end()
        .gap(sp(5.))
        .child(
            div()
                .flex()
                .items_end()
                .gap(sp(1.))
                .h(px(height + 36.))
                .children(columns),
        )
        .child(
            div()
                .max_w(px(280.))
                .typeset(Type::CAPTION)
                .text_color(t.muted)
                .line_height(relative(1.5))
                .child(note.to_owned()),
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
