//! The run's side panel: one column of what the run is doing and what
//! it may still spend. From the top: its goal, the context window and
//! what fills it, the run's own limits
//! and outcome, its plan, its sub-agents, and its plugins. The event log
//! opens from a bar pinned under it. The phone layout shows the same
//! column in a bottom sheet.

use gpui::{Context, Div, SharedString, div, prelude::*, px, relative};

use super::{
    Material as _,
    bar,
    heading,
    icon,
    key_values,
    link,
    mono,
    status_look,
    stop_look,
};
use crate::{
    assets::Icon,
    route::Route,
    theme::{Design as _, IconSize, Theme, Type, radius, sp},
    view::{ChildKind, Item, Pruned, RunStatus, RunView, tokens, usd},
    workspace::Workspace,
};

/// The run's state in a line: its status and turn, and the model with
/// its reasoning effort. Heads the panel.
pub fn header(
    ws: &Workspace,
    run: &RunView,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let (color, label) = status_look(&run.status, t);
    let effort = ws
        .run_plan(run, cx)
        .into_iter()
        .find(|field| field.name == "reasoning")
        .map(|field| field.value.clone());
    let model = match effort {
        Some(effort) => format!("{} · {effort}", run.model),
        None => run.model.clone(),
    };
    div()
        .flex()
        .items_center()
        .gap(sp(2.5))
        .child(if run.status.is_live() {
            super::live_dot(color, 8.)
        } else {
            super::dot(color, 8.)
        })
        .child(div().typeset(Type::SMALL).text_color(t.text).child(label))
        .child(mono(format!("turn {}", run.turn), Type::CAPTION, t.dim))
        .child(div().flex_1())
        .child(
            mono(model, Type::MICRO, t.blue)
                .px(sp(1.75))
                .py(sp(0.75))
                .rounded(radius::CONTROL)
                .border_1()
                .border_color(t.blue_border)
                .bg(t.blue_soft)
                .truncate(),
        )
}

/// Every section, top to bottom.
pub fn content(
    ws: &Workspace,
    run: &RunView,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let body = div().flex().flex_col().gap(sp(5.5));
    let body = if run.goal.is_some() {
        crate::goal::tab(run, body, t)
    } else {
        body
    };
    let body = body.child(context(ws, run, t, cx));
    // What plugins add to the inspector.
    let body = body.children(ws.contributions(
        tau_ui_plugin::points::INSPECTOR,
        &tau_ui_plugin::points::AtRun { run: run.info() },
        cx,
    ));
    let body = run_section(ws, run, body, t, cx);
    plugins_section(ws, run, body, t, cx)
}

/// The context window: how full it is, what fills it, where pruning
/// starts, and what pruning did.
fn context(
    ws: &Workspace,
    run: &RunView,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let trigger = ws.context_trigger(run, cx);
    let ledger_route = Route::Ledger(run.id.clone());
    let has_rewrite = run.last_rewrite().is_some();
    let context = &run.context;
    let heading_row = div()
        .flex()
        .items_center()
        .child(heading("Context", t).flex_1())
        .when(has_rewrite, |row| {
            row.child(
                div()
                    .id("open-ledger-panel")
                    .child(link("Ledger", t))
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.navigate(ledger_route.clone(), cx)
                    })),
            )
        });
    let section = div().flex().flex_col().gap(sp(3.)).child(heading_row);
    let Some(window) = context.window else {
        return section.child(
            div()
                .typeset(Type::SMALL)
                .text_color(t.muted)
                .child(format!("{} tokens in context", tokens(context.used))),
        );
    };
    let share = context.used as f32 / window as f32;
    let over_trigger = trigger.is_some_and(|trigger| share >= trigger);
    let parts = run.context_parts().unwrap_or_default();
    let segments = [
        ("Instructions, tools, other", parts.fixed, t.slate),
        ("Conversation", parts.conversation, t.blue),
        ("Tool results", parts.results, t.green),
    ];
    let stack = div()
        .relative()
        .h(px(12.))
        .child(
            div()
                .flex()
                .h(px(12.))
                .rounded(radius::SMALL)
                .overflow_hidden()
                .well(t)
                .children(
                    segments.iter().filter(|(_, used, _)| *used > 0).map(
                        |(_, used, color)| {
                            div()
                                .h_full()
                                .w(relative(*used as f32 / window as f32))
                                .bg(*color)
                        },
                    ),
                ),
        )
        .when_some(trigger, |track, trigger| {
            track.child(
                div()
                    .absolute()
                    .top(px(-4.))
                    .left(relative(trigger))
                    .w(px(2.))
                    .h(px(20.))
                    .bg(t.accent),
            )
        });
    let legend =
        div()
            .grid()
            .grid_cols(1)
            .gap(sp(1.5))
            .children(segments.iter().map(|(name, used, color)| {
                div()
                    .flex()
                    .items_center()
                    .gap(sp(2.))
                    .typeset(Type::CAPTION)
                    .text_color(t.muted)
                    .child(
                        div().size(px(8.)).rounded(radius::HAIRLINE).bg(*color),
                    )
                    .child(div().flex_1().child(*name))
                    .child(mono(tokens(*used), Type::CAPTION, t.text_soft))
            }));
    let (mut kept, mut results, mut calls) = (0, 0, 0);
    for item in &run.items {
        if let Item::Tool(card) = item {
            match card.pruned {
                Some(Pruned::Kept) => kept += 1,
                Some(Pruned::ResultDropped) => results += 1,
                Some(Pruned::CallDropped) => calls += 1,
                None => {}
            }
        }
    }
    let mut note = String::new();
    if let Some(trigger) = trigger {
        if over_trigger {
            note.push_str(&format!(
                "Past {:.0}%: pruning runs after this turn.",
                trigger * 100.
            ));
        } else {
            note.push_str(&format!(
                "Pruning starts at {:.0}%.",
                trigger * 100.
            ));
        }
    }
    if let Some(before) = context.before {
        note.push_str(&format!(
            " Last pruned from {} to {}.",
            tokens(before),
            tokens(context.used)
        ));
    }
    if kept + results + calls > 0 {
        note.push_str(&format!(
            " {kept} kept, {results} results and {calls} calls dropped."
        ));
    }
    let used_color = if over_trigger { t.accent } else { t.text };
    section
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(sp(1.5))
                .child(mono(tokens(context.used), Type::LEAD, used_color))
                .child(mono(
                    format!("/ {} tokens", tokens(window)),
                    Type::CAPTION,
                    t.muted,
                ))
                .child(div().flex_1())
                .child(mono(
                    format!("{:.0}%", share * 100.),
                    Type::CAPTION,
                    t.muted,
                )),
        )
        .child(stack)
        .child(legend)
        .when(!note.is_empty(), |section| {
            section.child(
                div()
                    .typeset(Type::MICRO)
                    .text_color(if over_trigger { t.accent } else { t.dim })
                    .line_height(relative(1.5))
                    .child(note.trim().to_owned()),
            )
        })
}

/// The run's own limits as tiles, and its outcome, plan and children.
fn run_section(
    ws: &Workspace,
    run: &RunView,
    body: Div,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let meters = run.meters();
    if meters.is_empty() {
        return run_details(ws, run, body, t, cx);
    }
    let columns = match meters.len() {
        4 => 2,
        n => n.clamp(1, 3),
    };
    let tiles = div()
        .grid()
        .grid_cols(columns as u16)
        .gap(sp(2.5))
        .children(meters.into_iter().map(|meter| {
            div()
                .flex()
                .flex_col()
                .gap(sp(1.5))
                .p(sp(2.5))
                .rounded(radius::BOX)
                .border_1()
                .border_color(t.border)
                .raised(t)
                .child(
                    div()
                        .typeset(Type::MICRO)
                        .text_color(t.dim)
                        .child(meter.label),
                )
                .child(mono(meter.value, Type::CAPTION, t.text).truncate())
                .child(bar(meter.share, 3., t.text_soft, t.raised))
        }));
    let body = body.child(
        div()
            .flex()
            .flex_col()
            .gap(sp(2.5))
            .child(heading("This run", t))
            .child(tiles),
    );
    run_details(ws, run, body, t, cx)
}

fn run_details(
    ws: &Workspace,
    run: &RunView,
    body: Div,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let plan_route = Route::Plan(run.id.clone());
    let rules = &run.constitution;
    let body = match &run.status {
        RunStatus::Finished(stop) => {
            let (color, label) = stop_look(stop, t);
            let continuations = (!rules.held.is_empty()).then(|| {
                (
                    SharedString::from("continuations"),
                    mono(
                        format!(
                            "{} ({})",
                            rules.held.len(),
                            rules.held.join(", ")
                        ),
                        Type::CAPTION,
                        t.accent,
                    ),
                )
            });
            let flagged = rules.flagged.first().cloned();
            let repo = ws.repo_of(run).to_owned();
            body.child(heading("Outcome", t))
                .child(key_values(
                    [
                        ("stop".into(), mono(label, Type::CAPTION, color)),
                        (
                            "turns".into(),
                            mono(run.turn.to_string(), Type::CAPTION, t.text),
                        ),
                        (
                            "tokens".into(),
                            mono(
                                tokens(run.usage.tokens),
                                Type::CAPTION,
                                t.text,
                            ),
                        ),
                        (
                            "cost".into(),
                            mono(usd(run.usage.cost), Type::CAPTION, t.text),
                        ),
                        (
                            "plugins".into(),
                            mono(
                                usd(run.usage.plugin_cost),
                                Type::CAPTION,
                                t.text,
                            ),
                        ),
                    ]
                    .into_iter()
                    .chain(continuations),
                    t,
                ))
                .when_some(flagged, |body, rule| {
                    let route = Route::Constitution {
                        repo,
                        rule: Some(rule),
                    };
                    body.child(
                        div()
                            .id("review-flagged")
                            .child(crate::ui::button(
                                "Review the flagged call",
                                crate::ui::ButtonKind::Secondary,
                                t,
                            ))
                            .on_click(cx.listener(move |ws, _, _, cx| {
                                ws.navigate(route.clone(), cx)
                            })),
                    )
                })
        }
        _ => body,
    };
    let fields = ws.run_plan(run, cx);
    let shown = !fields.is_empty();
    let plan = fields.into_iter().map(|field| {
        let value = div()
            .flex()
            .flex_col()
            .gap(sp(0.5))
            .child(mono(
                field.value.clone(),
                Type::CAPTION,
                if field.set_by.is_some() {
                    t.blue
                } else {
                    t.text
                },
            ))
            .when_some(field.set_by.clone(), |value, plugin| {
                value.child(mono(
                    format!("set by {plugin}"),
                    Type::MICRO,
                    t.dim,
                ))
            });
        (SharedString::from(field.name.clone()), value)
    });
    body.when(shown, |body| {
        body.child(
            div()
                .flex()
                .items_center()
                .child(heading("This run's plan", t).flex_1())
                .child(
                    div()
                        .id("open-plan")
                        .child(link("How it was set", t))
                        .on_click(cx.listener(move |ws, _, _, cx| {
                            ws.navigate(plan_route.clone(), cx)
                        })),
                ),
        )
        .child(key_values(plan, t))
    })
    .when(!run.children.is_empty(), |body| {
        body.child(heading("Sub-agents and forks", t)).children(
            run.children.iter().map(|child| {
                let (color, label) = super::status_look(&child.status, t);
                let route = match child.kind {
                    ChildKind::Fork => Route::Compare {
                        main: run.id.clone(),
                        fork: child.id.clone(),
                    },
                    ChildKind::SubAgent => Route::Run(child.id.clone()),
                };
                div()
                    .id(SharedString::from(format!("child-{}", child.id)))
                    .cursor_pointer()
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.navigate(route.clone(), cx)
                    }))
                    .flex()
                    .items_center()
                    .gap(sp(2.))
                    .child(icon(
                        match child.kind {
                            ChildKind::SubAgent => Icon::SubAgent,
                            ChildKind::Fork => Icon::Fork,
                        },
                        IconSize::SMALL,
                        t.blue,
                    ))
                    .child(div().flex_1().child(child.title.clone()))
                    .child(
                        div()
                            .typeset(Type::CAPTION)
                            .text_color(color)
                            .child(label),
                    )
            }),
        )
    })
}

fn plugins_section(
    ws: &Workspace,
    run: &RunView,
    body: Div,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let rules = &run.constitution;
    let with_rules = |count: usize, list: &[String]| {
        if count == 0 {
            "0".to_owned()
        } else {
            format!("{count} ({})", list.join(", "))
        }
    };
    let statuses = ws.run_statuses(run, cx);
    if statuses.is_empty() && rules.is_empty() {
        return body;
    }
    body.child(heading("Plugins", t))
        .child(plugin_states(ws, run, &statuses, t, cx))
        .child(
            div()
                .typeset(Type::CAPTION)
                .text_color(t.dim)
                .line_height(relative(1.5))
                .child(format!(
                    "Plugins charged {} to this run, counted in its limits.",
                    usd(run.usage.plugin_cost)
                )),
        )
        .when(!rules.is_empty(), |body| {
            let value = |text: String, color| mono(text, Type::CAPTION, color);
            body.child(heading("Constitution checks", t))
                .child(key_values(
                    [
                        (
                            "calls checked".into(),
                            value(rules.calls.to_string(), t.text),
                        ),
                        (
                            "answers checked".into(),
                            value(rules.answers.to_string(), t.text),
                        ),
                        (
                            "questions asked".into(),
                            value(rules.questions.to_string(), t.text),
                        ),
                        (
                            "blocked".into(),
                            value(
                                with_rules(rules.blocked.len(), &rules.blocked),
                                if rules.blocked.is_empty() { t.text } else { t.red },
                            ),
                        ),
                        (
                            "flagged".into(),
                            value(
                                with_rules(rules.flagged.len(), &rules.flagged),
                                if rules.flagged.is_empty() { t.text } else { t.accent },
                            ),
                        ),
                        (
                            "held".into(),
                            value(
                                match (rules.max_holds, rules.held.is_empty()) {
                                    (Some(max), false) => format!(
                                        "{} of {max} ({})",
                                        rules.held.len(),
                                        rules.held.join(", ")
                                    ),
                                    _ => with_rules(rules.held.len(), &rules.held),
                                },
                                t.text,
                            ),
                        ),
                        ("Jev cost".into(), value(usd(rules.cost), t.text)),
                    ],
                    t,
                ))
                .child(
                    div()
                        .typeset(Type::CAPTION)
                        .text_color(t.dim)
                        .line_height(relative(1.5))
                        .child(
                            "Only the fields the model wrote are checked, such as \
                             edit.newText or bash.command. Tool output never goes \
                             to Jev.",
                        ),
                )
        })
}

/// Each plugin's state in the run; a row opens the plugin's screen.
fn plugin_states(
    ws: &Workspace,
    run: &RunView,
    statuses: &[crate::view::PluginStatus],
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    div()
        .flex()
        .flex_col()
        .border_1()
        .border_color(t.border)
        .rounded(radius::BOX)
        .overflow_hidden()
        .children(statuses.iter().map(|plugin| {
            let route = ws.plugin_route_named(&plugin.name, &run.id);
            div()
                .id(SharedString::from(format!("status-{}", plugin.name)))
                .when_some(route, |row, route| {
                    row.cursor_pointer()
                        .hover(|style| style.bg(gpui::white().opacity(0.03)))
                        .on_click(cx.listener(move |ws, _, _, cx| {
                            ws.navigate(route.clone(), cx)
                        }))
                })
                .flex()
                .items_center()
                .gap(sp(2.5))
                .min_h(px(36.))
                .px(sp(3.))
                .border_b_1()
                .border_color(t.border)
                .child(
                    mono(plugin.name.clone(), Type::CAPTION, t.text).flex_1(),
                )
                .child(
                    div()
                        .typeset(Type::CAPTION)
                        .text_color(t.tone(plugin.tone))
                        .child(plugin.state.clone()),
                )
        }))
}

/// The bar pinned under the panel that opens the event log, and the
/// log when it is open.
pub fn events(
    ws: &Workspace,
    run: &RunView,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let open = ws.events_open();
    div()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .border_t_1()
        .border_color(t.border)
        .when(open, |panel| {
            panel.child(
                div()
                    .id("event-log")
                    .max_h(px(280.))
                    .overflow_y_scroll()
                    .px(sp(4.))
                    .pt(sp(3.))
                    .child(event_log(run, t)),
            )
        })
        .child(
            div()
                .id("toggle-events")
                .flex()
                .items_center()
                .gap(sp(2.))
                .h(px(44.))
                .px(sp(4.))
                .cursor_pointer()
                .hover(|style| style.bg(gpui::white().opacity(0.03)))
                .on_click(cx.listener(|ws, _, _, cx| ws.toggle_events(cx)))
                .child(heading("Events", t).flex_1())
                .child(mono(run.log.len().to_string(), Type::CAPTION, t.dim))
                .child(icon(
                    if open { Icon::Down } else { Icon::Chevron },
                    IconSize::SMALL,
                    t.muted,
                )),
        )
}

fn event_log(run: &RunView, t: &Theme) -> Div {
    let color = |kind: &str| match kind {
        "ToolStart" | "Retry" | "Continued" => t.accent,
        "ToolEnd" | "RunStart" => t.green,
        "Usage" | "Rewrite" => t.blue,
        "PluginError" => t.red,
        _ => t.text_soft,
    };
    div().child(
        div()
            .flex()
            .flex_col()
            .gap(sp(0.5))
            .p(sp(3.))
            .rounded(radius::CONTROL)
            .well(t)
            .when(run.log.is_empty(), |log| {
                log.child(mono("No events yet.", Type::MICRO, t.dim))
            })
            .children(run.log.iter().rev().take(60).map(|line| {
                div()
                    .flex()
                    .gap(sp(2.))
                    .child(
                        mono(format!("t{}", line.turn), Type::MICRO, t.dim)
                            .w(px(24.)),
                    )
                    .child(
                        mono(line.kind.clone(), Type::MICRO, color(&line.kind))
                            .w(px(80.)),
                    )
                    .child(
                        mono(line.text.clone(), Type::MICRO, t.muted)
                            .flex_1()
                            .min_w(px(0.))
                            .truncate(),
                    )
            })),
    )
}
