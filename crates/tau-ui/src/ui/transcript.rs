//! The chat: messages, tool cards, and what plugins say in between.

use std::collections::HashSet;

use gpui::{
    AnyElement,
    Context,
    Div,
    FontWeight,
    IntoElement,
    SharedString,
    div,
    prelude::*,
    px,
    relative,
    rgb,
};

use super::{dot, icon, mono, primary_button, rich, stop_look};
use crate::{
    assets::Icon,
    theme::Theme,
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
/// `kept` holds the titles of notes the user already kept.
pub fn items(
    run: &RunView,
    kept: &HashSet<String>,
    t: &Theme,
    compact: bool,
    cx: &mut Context<Workspace>,
) -> Vec<AnyElement> {
    run.items
        .iter()
        .enumerate()
        .map(|(index, item)| item_view(run, kept, index, item, t, compact, cx))
        .collect()
}

fn item_view(
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
            .child(rich(text, t))
            .into_any_element(),
        Item::Thinking(text) => thinking(text, t).into_any_element(),
        Item::Tool(card) => tool(run, card, t, compact, cx).into_any_element(),
        Item::Plugin(note) => {
            plugin_note(run, kept, index, note, t, compact, cx)
                .into_any_element()
        }
        Item::Rewrite {
            plugin,
            tokens_before,
            tokens_after,
            detail,
        } => rewrite(
            plugin,
            *tokens_before,
            *tokens_after,
            detail.as_deref(),
            t,
            compact,
        )
        .into_any_element(),
        Item::Retry {
            attempt,
            delay,
            error,
        } => div()
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(12.))
            .text_color(t.dim)
            .child(icon(Icon::Warning, 12., t.accent))
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
                .gap(px(10.))
                .py(px(8.))
                .border_t_1()
                .border_b_1()
                .border_color(t.border)
                .child(icon(glyph, 13., color))
                .child(div().text_color(color).child(label))
                .child(mono(
                    format!(
                        "{turns} turns · {} tokens · {}",
                        tokens(*used),
                        usd(*cost)
                    ),
                    12.,
                    t.muted,
                ))
                .when(*plugin_cost > 0.0, |row| {
                    row.child(mono(
                        format!("plugins {}", usd(*plugin_cost)),
                        12.,
                        t.dim,
                    ))
                })
                .into_any_element()
        }
    }
}

fn user(text: &str, t: &Theme, compact: bool) -> Div {
    div().flex().justify_end().child(
        div()
            .max_w(px(if compact { 300. } else { 620. }))
            .px(px(14.))
            .py(px(12.))
            .bg(t.raised)
            .border_1()
            .border_color(rgb(0x30323a))
            .rounded(px(10.))
            .line_height(relative(1.55))
            .child(rich(text, t)),
    )
}

fn thinking(text: &str, t: &Theme) -> Div {
    let words = text.split_whitespace().count();
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .text_size(px(12.))
        .text_color(t.dim)
        .child(icon(Icon::Chevron, 12., t.dim))
        .child(format!("Reasoned · {words} words"))
}

fn tool(
    run: &RunView,
    card: &ToolCard,
    t: &Theme,
    compact: bool,
    cx: &mut Context<Workspace>,
) -> Div {
    let (status, border) = match &card.state {
        ToolState::Running => {
            (icon(Icon::Spinner, 13., t.accent), t.accent_border)
        }
        ToolState::Done { .. } => (icon(Icon::Check, 13., t.green), t.border),
        ToolState::Failed(_) => (icon(Icon::Blocked, 14., t.red), t.red_border),
        ToolState::Blocked { .. } => {
            (icon(Icon::Blocked, 14., t.red), t.red_border)
        }
        ToolState::Flagged { .. } => {
            (icon(Icon::Warning, 14., t.accent), t.accent_border)
        }
    };
    let dropped = matches!(
        card.pruned,
        Some(Pruned::ResultDropped | Pruned::CallDropped)
    );
    let summary = mono(card.summary.clone(), 12., t.text_soft)
        .flex_1()
        .min_w(px(0.))
        .truncate()
        .when(card.pruned == Some(Pruned::CallDropped), |s| {
            s.line_through()
        });

    let header = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .min_h(px(if compact { 44. } else { 34. }))
        .px(px(12.))
        .child(status)
        .child(mono(card.tool.clone(), 12., t.blue).flex_shrink_0())
        .when_some(card.from_plugin.clone(), |row, plugin| {
            row.child(
                div()
                    .flex_shrink_0()
                    .px(px(6.))
                    .py(px(1.))
                    .border_1()
                    .border_color(t.border)
                    .rounded(px(4.))
                    .text_size(px(11.))
                    .text_color(t.dim)
                    .child(plugin),
            )
        })
        .child(summary)
        .when(!compact, |row| {
            row.children(
                card.checks
                    .iter()
                    .map(|check| mono(check.clone(), 11., t.dim)),
            )
        })
        .child(state_label(card, t))
        .when_some(card.pruned, |row, pruned| {
            row.child(
                div()
                    .flex_shrink_0()
                    .px(px(6.))
                    .py(px(1.))
                    .border_1()
                    .border_color(t.border_strong)
                    .rounded(px(4.))
                    .text_size(px(11.))
                    .text_color(if pruned == Pruned::Kept {
                        t.muted
                    } else {
                        t.dim
                    })
                    .child(pruned.label()),
            )
        });

    let body: Option<AnyElement> = match (&card.state, &card.body) {
        (
            ToolState::Blocked {
                plugin,
                reason,
                score,
                ..
            },
            _,
        ) => Some(
            blocked_body(card, plugin, reason, score, t).into_any_element(),
        ),
        (ToolState::Flagged { .. }, _) => None,
        _ if dropped => None,
        (_, ToolBody::Diff(lines)) => Some(diff(lines, t).into_any_element()),
        (_, ToolBody::Output(lines)) if !lines.is_empty() => {
            Some(output(lines, t).into_any_element())
        }
        _ => None,
    };

    let review = matches!(card.state, ToolState::Flagged { .. }).then(|| {
        let run_id = run.id.clone();
        let call_id = card.call_id.clone();
        div()
            .id(SharedString::from(format!("review-{}", card.call_id)))
            .child(super::button("Review", t))
            .on_click(cx.listener(move |ws, _, _, cx| {
                ws.review_call(&run_id, &call_id, cx)
            }))
    });

    div()
        .flex()
        .flex_col()
        .border_1()
        .border_color(border)
        .rounded(px(8.))
        .bg(t.card)
        .overflow_hidden()
        .when(dropped, |card| card.opacity(0.6))
        .child(
            header.when(body.is_some(), |h| {
                h.border_b_1().border_color(t.border)
            }),
        )
        .children(body)
        .when_some(review, |card, review| {
            card.child(
                div()
                    .flex()
                    .justify_end()
                    .px(px(12.))
                    .pb(px(8.))
                    .child(review),
            )
        })
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
        ToolState::Blocked { .. } => ("Blocked".into(), t.red),
        ToolState::Flagged { rule, score, .. } => {
            (format!("Ran · flagged by {rule} {score}"), t.accent)
        }
    };
    div()
        .flex_shrink_0()
        .max_w(px(260.))
        .truncate()
        .text_size(px(12.))
        .text_color(color)
        .child(text)
}

fn blocked_body(
    card: &ToolCard,
    plugin: &str,
    reason: &str,
    score: &str,
    t: &Theme,
) -> Div {
    let proposed: Vec<DiffLine> = proposed_text(&card.args)
        .into_iter()
        .map(|text| DiffLine {
            kind: DiffKind::Added,
            text,
        })
        .collect();
    div()
        .flex()
        .flex_col()
        .when(!proposed.is_empty(), |body| body.child(diff(&proposed, t)))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(12.))
                .px(px(12.))
                .py(px(10.))
                .bg(rgb(0x1d1716))
                .border_t_1()
                .border_color(rgb(0x3a2724))
                .child(mono(plugin.to_owned(), 12., t.red))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(200.))
                        .text_color(t.text_soft)
                        .line_height(relative(1.5))
                        .child(format!(
                            "{reason} The model got this reason back."
                        )),
                )
                .child(mono(score.to_owned(), 12., t.red)),
        )
}

fn diff(lines: &[DiffLine], t: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .py(px(6.))
        .font_family(crate::theme::MONO)
        .text_size(px(12.))
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
                .px(px(12.))
                .text_color(color)
                .whitespace_nowrap()
                .overflow_hidden()
                .when_some(bg, |row, bg| row.bg(bg))
                .child(format!("{sign}{}", line.text))
        }))
}

fn output(lines: &[String], t: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .px(px(12.))
        .py(px(8.))
        .font_family(crate::theme::MONO)
        .text_size(px(12.))
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

fn plugin_note(
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
        .gap(px(8.))
        .child(
            div()
                .size(px(20.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .bg(rgb(0x1f2633))
                .child(icon(Icon::Plug, 13., t.blue)),
        )
        .child(
            mono(note.plugin.clone(), 12., t.tone(note.tone)).flex_shrink_0(),
        )
        .when(!compact, |row| {
            row.child(div().text_color(t.text_soft).child(note.text.clone()))
        })
        .child(div().flex_1())
        .when_some(note.detail.clone(), |row, detail| {
            row.child(mono(detail, 11., t.dim).flex_shrink_0())
        });

    let body: Option<AnyElement> = match &note.body {
        NoteBody::None => None,
        NoteBody::Chips(chips) => Some(
            div()
                .flex()
                .flex_wrap()
                .gap(px(6.))
                .children(chips.iter().map(|chip| {
                    div()
                        .px(px(9.))
                        .py(px(3.))
                        .rounded(px(12.))
                        .border_1()
                        .border_color(t.border)
                        .text_size(px(12.))
                        .text_color(t.text_soft)
                        .child(chip.clone())
                }))
                .into_any_element(),
        ),
        NoteBody::Distribution {
            levels,
            chosen,
            note,
        } => Some(
            distribution(levels, *chosen, note, t, compact).into_any_element(),
        ),
        NoteBody::Proposals(proposals) => Some(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .children(proposals.iter().enumerate().map(|(n, proposal)| {
                    let is_kept = kept.contains(&proposal.title);
                    let run_id = run.id.clone();
                    let title = proposal.title.clone();
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(10.))
                        .px(px(12.))
                        .py(px(10.))
                        .border_1()
                        .border_color(t.border)
                        .rounded(px(8.))
                        .bg(t.card)
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(3.))
                                .flex_1()
                                .min_w(px(180.))
                                .child(
                                    div()
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(proposal.title.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(t.muted)
                                        .child(proposal.detail.clone()),
                                ),
                        )
                        .child(if is_kept {
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .text_size(px(12.))
                                .text_color(t.green)
                                .child(icon(Icon::Check, 13., t.green))
                                .child("Kept")
                                .into_any_element()
                        } else {
                            div()
                                .id(SharedString::from(format!(
                                    "keep-{index}-{n}"
                                )))
                                .child(primary_button("Keep", t))
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
        .gap(px(8.))
        .px(px(12.))
        .py(px(8.))
        .rounded(px(8.))
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
                    .child(note.text.clone()),
            )
        })
        .when_some(body, |card, body| {
            card.child(div().pl(px(if compact { 0. } else { 28. })).child(body))
        })
}

fn distribution(
    levels: &[(String, f32)],
    chosen: usize,
    note: &str,
    t: &Theme,
    compact: bool,
) -> Div {
    let columns = levels.iter().enumerate().map(|(index, (name, p))| {
        let pick = index == chosen;
        let ink = if pick { t.text } else { t.muted };
        div()
            .flex()
            .flex_col()
            .items_center()
            .justify_end()
            .gap(px(3.))
            .w(px(if compact { 52. } else { 58. }))
            .child(mono(format!("{p:.2}"), 11., ink))
            .child(
                div()
                    .w(px(22.))
                    .h(px((p * 44.).max(3.)))
                    .rounded_t(px(3.))
                    .bg(if pick { t.blue } else { rgb(0x3d4452).into() }),
            )
            .child(
                mono(name.clone(), 11., ink)
                    .w_full()
                    .pt(px(3.))
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
        .gap(px(20.))
        .child(
            div()
                .flex()
                .items_end()
                .gap(px(4.))
                .h(px(80.))
                .children(columns),
        )
        .child(
            div()
                .max_w(px(280.))
                .text_size(px(12.))
                .text_color(t.muted)
                .line_height(relative(1.5))
                .child(note.to_owned()),
        )
}

fn rewrite(
    plugin: &str,
    before: u64,
    after: u64,
    detail: Option<&str>,
    t: &Theme,
    compact: bool,
) -> Div {
    let saved = before.saturating_sub(after);
    let line = || div().flex_1().h(px(1.)).bg(t.blue_border);
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .py(px(4.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(line())
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(8.))
                        .px(px(12.))
                        .py(px(6.))
                        .rounded(px(16.))
                        .border_1()
                        .border_dashed()
                        .border_color(t.blue_border)
                        .bg(t.blue_soft)
                        .child(icon(Icon::Plug, 13., t.blue))
                        .when(!compact, |pill| {
                            pill.child(mono(plugin.to_owned(), 12., t.blue))
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
                                pill.child(mono(detail.to_owned(), 11., t.dim))
                            },
                        ),
                )
                .child(line()),
        )
        .child(
            div()
                .flex()
                .justify_center()
                .gap(px(6.))
                .text_size(px(12.))
                .text_color(t.dim)
                .child(dot(t.blue_border, 4.))
                .child(
                    "The next request resends the pruned transcript once, \
                     then turns are deltas again.",
                ),
        )
}
