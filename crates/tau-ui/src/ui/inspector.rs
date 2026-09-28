//! The run's side panel: its plan and limits, its context window, and
//! what each plugin is doing. The phone layout shows the same tabs in a
//! bottom sheet.

use gpui::{
    Context,
    Div,
    IntoElement,
    SharedString,
    div,
    prelude::*,
    px,
    relative,
};

use super::{bar, heading, icon, key_values, link, mono, stop_look};
use crate::{
    assets::Icon,
    route::Route,
    theme::{Design as _, IconSize, Theme, Type, radius, sp},
    view::{ChildKind, Item, Pruned, RunStatus, RunView, tokens, usd},
    workspace::Workspace,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Run,
    Context,
    Plugins,
    Events,
}

impl Tab {
    pub const ALL: [Self; 4] =
        [Self::Run, Self::Context, Self::Plugins, Self::Events];

    pub fn label(self) -> &'static str {
        match self {
            Self::Run => "Run",
            Self::Context => "Context",
            Self::Plugins => "Plugins",
            Self::Events => "Events",
        }
    }
}

pub fn content(
    ws: &Workspace,
    run: &RunView,
    tab: Tab,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let body = div().flex().flex_col().gap(sp(5.));
    match tab {
        Tab::Run => {
            let body = run_tab(run, body, t, cx);
            if run.plugins.is_empty() {
                body
            } else {
                body.child(heading("Plugins", t))
                    .child(plugin_states(ws, run, t, cx))
            }
        }
        Tab::Context => context_tab(run, body, t, cx),
        Tab::Plugins => plugins_tab(ws, run, body, t, cx),
        Tab::Events => events_tab(run, body, t),
    }
}

fn run_tab(
    run: &RunView,
    body: Div,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let plan_route = Route::Plan(run.id.clone());
    let body = match &run.status {
        RunStatus::Finished(stop) => {
            let (color, label) = stop_look(stop, t);
            body.child(heading("Outcome", t)).child(key_values(
                [
                    ("stop".into(), mono(label, Type::CAPTION, color)),
                    (
                        "turns".into(),
                        mono(run.turn.to_string(), Type::CAPTION, t.text),
                    ),
                    (
                        "tokens".into(),
                        mono(tokens(run.usage.tokens), Type::CAPTION, t.text),
                    ),
                    (
                        "cost".into(),
                        mono(usd(run.usage.cost), Type::CAPTION, t.text),
                    ),
                    (
                        "plugins".into(),
                        mono(usd(run.usage.plugin_cost), Type::CAPTION, t.text),
                    ),
                ],
                t,
            ))
        }
        _ => body,
    };
    let plan = run.plan.iter().map(|field| {
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
    let meters = run.meters();
    body.when(!run.plan.is_empty(), |body| {
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
    .when(!meters.is_empty(), |body| {
        body.child(heading("Limits", t))
            .children(meters.into_iter().map(|meter| {
                div()
                    .flex()
                    .flex_col()
                    .gap(sp(1.25))
                    .child(
                        div()
                            .flex()
                            .typeset(Type::CAPTION)
                            .child(
                                div()
                                    .flex_1()
                                    .text_color(t.text_soft)
                                    .child(meter.label),
                            )
                            .child(mono(meter.value, Type::CAPTION, t.muted)),
                    )
                    .child(bar(meter.share, 4., t.text_soft, t.border))
            }))
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

fn context_tab(
    run: &RunView,
    body: Div,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let ledger_route = Route::Ledger(run.id.clone());
    let has_rewrite = run.last_rewrite().is_some();
    let context = &run.context;
    let Some(window) = context.window else {
        return body.child(
            div()
                .text_color(t.muted)
                .child(format!("{} tokens in context", tokens(context.used))),
        );
    };
    let share = |used: u64| used as f32 / window as f32;
    let row = |label: &'static str, used: u64, fill| {
        div()
            .flex()
            .items_center()
            .gap(sp(2.5))
            .child(
                div()
                    .w(px(48.))
                    .typeset(Type::CAPTION)
                    .text_color(t.muted)
                    .child(label),
            )
            .child(
                div()
                    .flex_1()
                    .relative()
                    .child(bar(share(used), 10., fill, t.raised))
                    .when_some(context.trigger, |track, trigger| {
                        track.child(
                            div()
                                .absolute()
                                .top(px(-3.))
                                .left(relative(trigger))
                                .w(px(2.))
                                .h(px(16.))
                                .bg(t.accent),
                        )
                    }),
            )
            .child(
                mono(tokens(used), Type::CAPTION, t.text)
                    .w(px(40.))
                    .flex()
                    .justify_end(),
            )
    };
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
    body.child(
        div()
            .flex()
            .items_center()
            .child(heading("Context window", t).flex_1())
            .when(has_rewrite, |row| {
                row.child(
                    div()
                        .id("open-ledger-panel")
                        .child(link("Ledger", t))
                        .on_click(cx.listener(move |ws, _, _, cx| {
                            ws.navigate(ledger_route.clone(), cx)
                        })),
                )
            }),
    )
    .when_some(context.before, |body, before| {
        body.child(row("Before", before, t.slate))
    })
    .child(row(
        if context.before.is_some() {
            "After"
        } else {
            "Now"
        },
        context.used,
        t.blue,
    ))
    .child(
        div()
            .typeset(Type::CAPTION)
            .text_color(t.dim)
            .line_height(relative(1.5))
            .child(format!(
                "{} window.{}",
                tokens(window),
                context.trigger.map_or(String::new(), |trigger| format!(
                    " Pruning starts at {:.0}%.",
                    trigger * 100.
                ))
            )),
    )
    .when(kept + results + calls > 0, |body| {
        body.child(heading("Ledger", t)).child(key_values(
            [
                ("kept".into(), mono(kept.to_string(), Type::CAPTION, t.text)),
                (
                    "result dropped".into(),
                    mono(results.to_string(), Type::CAPTION, t.text),
                ),
                (
                    "call dropped".into(),
                    mono(calls.to_string(), Type::CAPTION, t.text),
                ),
            ],
            t,
        ))
    })
}

fn plugins_tab(
    ws: &Workspace,
    run: &RunView,
    body: Div,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    body.child(heading("Plugins this run", t))
        .child(plugin_states(ws, run, t, cx))
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
}

/// Each plugin's state in the run; a row opens the plugin's screen.
fn plugin_states(
    ws: &Workspace,
    run: &RunView,
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
        .children(run.plugins.iter().map(|plugin| {
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

/// The tab strip, for the desktop panel.
pub fn tab_label(tab: Tab, active: bool, t: &Theme) -> impl IntoElement {
    div()
        .h(px(48.))
        .flex()
        .items_center()
        .px(sp(0.5))
        .typeset(Type::SMALL)
        .cursor_pointer()
        .text_color(if active { t.text } else { t.muted })
        .when(active, |tab| tab.border_b_2().border_color(t.accent))
        .child(tab.label())
}

fn events_tab(run: &RunView, body: Div, t: &Theme) -> Div {
    let color = |kind: &str| match kind {
        "ToolStart" | "Retry" | "Continued" => t.accent,
        "ToolEnd" | "RunStart" => t.green,
        "Usage" | "Rewrite" => t.blue,
        "PluginError" => t.red,
        _ => t.text_soft,
    };
    body.child(heading("Live events", t)).child(
        div()
            .flex()
            .flex_col()
            .gap(sp(0.5))
            .p(sp(3.))
            .rounded(radius::CONTROL)
            .border_1()
            .border_color(t.border)
            .bg(t.bg)
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
                        mono(line.kind, Type::MICRO, color(line.kind))
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
