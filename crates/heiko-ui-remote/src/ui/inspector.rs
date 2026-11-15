//! The run's side panel: one column of sections, each a title over a
//! heavy rule and ruled rows. From the top: the context window as ticks
//! and what fills it, what plugins add, the run's own limits and
//! outcome, its plan, its sub-agents, and its plugins. The event log
//! opens from a row pinned under it. The phone layout shows the same
//! column in a bottom sheet.

use gpui::{Context, Div, Hsla, SharedString, div, prelude::*, rems};
use heiko_agent::event::LimitKind;

use super::{
    bar,
    link_action,
    mark,
    marked_ticks,
    mono,
    section_header,
    status_look,
    stop_look,
};
use crate::{
    assets::State,
    route::Route,
    theme::{Design as _, Theme, Type, sp},
    view::{ChildKind, Dropped, Item, RunStatus, RunView, tokens},
    workspace::Workspace,
};

/// How many ticks the context window is drawn with.
const CONTEXT_TICKS: usize = 24;

/// The run's state in a line: its status and turn, and the model with
/// its reasoning effort. Heads the panel.
pub fn header(ws: &Workspace, run: &RunView, t: &Theme) -> Div {
    let (color, label) = status_look(&run.status, t);
    let effort = ws
        .run_plan(run)
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
        .child(mark(
            if run.status.is_live() {
                State::Working
            } else {
                State::Idle
            },
            t,
        ))
        .child(div().typeset(Type::SMALL).text_color(color).child(label))
        .child(mono(format!("turn {}", run.turn), Type::CAPTION, t.dim))
        .child(div().flex_1())
        .child(mono(model, Type::CAPTION, t.dim).truncate())
}

/// Every section, top to bottom.
pub fn content(
    ws: &Workspace,
    run: &RunView,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let body = div().flex().flex_col().gap(sp(7.));
    let body = body.child(context(ws, run, t, cx));
    // What plugins add to the inspector.
    let body = body.children(ws.drawn(
        heiko_ui_plugin::points::INSPECTOR,
        &heiko_ui_plugin::points::AtRun { run: run.info() },
        cx,
    ));
    let body = run_section(ws, run, body, t, cx);
    plugins_section(ws, run, body, t, cx)
}

/// A section: its title over the heavy rule, `meta` at the title's
/// right, then its rows.
fn section(title: &'static str, meta: Option<String>, t: &Theme) -> Div {
    div().flex().flex_col().child(section_header(
        title,
        meta.map(SharedString::from),
        t,
    ))
}

/// A ruled row: a name, and its value at the right.
fn row(name: impl Into<SharedString>, value: Div, t: &Theme) -> Div {
    div()
        .flex()
        .items_baseline()
        .gap(sp(3.))
        .py(sp(2.25))
        .border_b_1()
        .border_color(t.hairline)
        .typeset(Type::SMALL)
        .child(
            div()
                .flex_1()
                .min_w(rems(0.))
                .text_color(t.text)
                .child(name.into()),
        )
        .child(value.flex_shrink_0())
}

/// A row's value: a count or a short word in mono.
fn value(text: impl Into<SharedString>, color: Hsla) -> Div {
    mono(text, Type::CAPTION, color)
}

/// The context window: how full it is, as ticks with the mark where a
/// plugin steps in; what fills it; what rewrites dropped; then what
/// plugins add.
fn context(
    ws: &Workspace,
    run: &RunView,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let trigger = ws.context_trigger(run);
    let context = &run.context;
    let extras = ws.drawn(
        heiko_ui_plugin::points::CONTEXT,
        &heiko_ui_plugin::points::AtRun { run: run.info() },
        cx,
    );
    let Some(window) = context.window else {
        return section("Context", None, t)
            .child(row(
                "Tokens in context",
                value(tokens(context.used), t.text_soft),
                t,
            ))
            .children(extras);
    };
    let share = context.used as f32 / window as f32;
    let over_trigger = trigger.is_some_and(|trigger| share >= trigger);
    let parts = run.context_parts().unwrap_or_default();
    let segments = [
        ("Instructions, tools, other", parts.fixed),
        ("Conversation", parts.conversation),
        ("Tool results", parts.results),
    ];
    let at = |share: f32| {
        (share.clamp(0., 1.) * CONTEXT_TICKS as f32).round() as usize
    };
    // Any use at all lights a tick.
    let lit = if context.used > 0 {
        at(share).max(1)
    } else {
        0
    };
    let marked: Vec<usize> = trigger
        .map(|trigger| at(trigger).min(CONTEXT_TICKS - 1))
        .into_iter()
        .collect();
    let (mut results, mut calls) = (0, 0);
    for item in &run.items {
        if let Item::Tool(card) = item {
            match card.dropped {
                Some(Dropped::Result) => results += 1,
                Some(Dropped::Call) => calls += 1,
                None => {}
            }
        }
    }
    section("Context", Some(format!("{:.0}%", share * 100.)), t)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .py(sp(3.))
                .border_b_1()
                .border_color(t.hairline)
                .child(mono(
                    format!("{} of {}", tokens(context.used), tokens(window)),
                    Type::SMALL,
                    if over_trigger { t.roles.live } else { t.text },
                ))
                .child(marked_ticks(
                    lit,
                    CONTEXT_TICKS,
                    if over_trigger {
                        t.roles.live
                    } else {
                        t.roles.meter
                    },
                    &marked,
                    t,
                )),
        )
        .children(
            segments.into_iter().map(|(name, used)| {
                row(name, value(tokens(used), t.text_soft), t)
            }),
        )
        .when_some(trigger, |section, trigger| {
            section.child(row(
                if over_trigger {
                    "Rewritten after this turn"
                } else {
                    "Rewriting starts at"
                },
                value(
                    format!("{:.0}%", trigger * 100.),
                    if over_trigger {
                        t.roles.live
                    } else {
                        t.text_soft
                    },
                ),
                t,
            ))
        })
        .when_some(context.before, |section, before| {
            section.child(row(
                "Last rewritten",
                value(
                    format!("{} to {}", tokens(before), tokens(context.used)),
                    t.text_soft,
                ),
                t,
            ))
        })
        .when(results + calls > 0, |section| {
            section.child(row(
                "Dropped",
                value(
                    format!("{results} results · {calls} calls"),
                    t.text_soft,
                ),
                t,
            ))
        })
        .children(extras)
}

/// The run's own limits, each a row over its ticks, and its outcome,
/// plan and children.
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
    let limits = meters.into_iter().map(|meter| {
        // Money is not shown: a cost limit reads as the share used.
        let said = match meter.kind {
            LimitKind::Usd => format!("{:.0}%", meter.share * 100.),
            _ => meter.value,
        };
        div()
            .flex()
            .flex_col()
            .gap(sp(2.))
            .py(sp(2.5))
            .border_b_1()
            .border_color(t.hairline)
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .typeset(Type::SMALL)
                    .child(div().flex_1().child(meter.label))
                    .child(value(said, t.text_soft)),
            )
            .child(bar(meter.share, 6., t.text_soft, t.hairline))
    });
    let body = body.child(
        section(
            "This run",
            Some(format!("turn {} · {}", run.turn, run.model)),
            t,
        )
        .children(limits),
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
    let body = match &run.status {
        RunStatus::Finished(stop) => {
            let (color, label) = stop_look(stop, t);
            body.child(
                section("Outcome", None, t)
                    .child(row("Stop", value(label, color), t))
                    .child(row(
                        "Turns",
                        value(run.turn.to_string(), t.text_soft),
                        t,
                    ))
                    .child(row(
                        "Tokens",
                        value(tokens(run.usage.tokens), t.text_soft),
                        t,
                    )),
            )
        }
        _ => body,
    };
    let fields = ws.run_plan(run);
    let shown = !fields.is_empty();
    let plan = fields.into_iter().map(|field| {
        let set = div()
            .flex()
            .flex_col()
            .items_end()
            .gap(sp(0.5))
            .child(value(
                field.value.clone(),
                if field.set_by.is_some() {
                    t.blue
                } else {
                    t.text_soft
                },
            ))
            .when_some(field.set_by.clone(), |set, plugin| {
                set.child(mono(format!("set by {plugin}"), Type::MICRO, t.dim))
            });
        row(field.name.clone(), set, t)
    });
    body.when(shown, |body| {
        body.child(
            section("Plan", None, t).children(plan).child(
                div().flex().pt(sp(2.5)).typeset(Type::SMALL).child(
                    div()
                        .id("open-plan")
                        .child(link_action("how it was set", t))
                        .on_click(cx.listener(move |ws, _, _, cx| {
                            ws.navigate(plan_route.clone(), cx)
                        })),
                ),
            ),
        )
    })
    .when(!run.children.is_empty(), |body| {
        body.child(
            section(
                "Sub-agents and forks",
                Some(run.children.len().to_string()),
                t,
            )
            .children(run.children.iter().map(|child| {
                let (color, label) = super::status_look(&child.status, t);
                let route = match child.kind {
                    ChildKind::Fork => Route::Compare {
                        main: run.id.clone(),
                        fork: child.id.clone(),
                    },
                    ChildKind::SubAgent => Route::Run(child.id.clone()),
                };
                let kind = match child.kind {
                    ChildKind::SubAgent => "sub-agent",
                    ChildKind::Fork => "fork",
                };
                row(
                    child.title.clone(),
                    div()
                        .flex()
                        .items_baseline()
                        .gap(sp(2.5))
                        .child(value(kind, t.dim))
                        .child(value(label, color)),
                    t,
                )
                .id(SharedString::from(format!("child-{}", child.id)))
                .cursor_pointer()
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.navigate(route.clone(), cx)
                }))
            })),
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
    let statuses = ws.run_statuses(run);
    if statuses.is_empty() {
        return body;
    }
    body.child(
        section("Plugins", Some(statuses.len().to_string()), t)
            .child(plugin_states(ws, run, &statuses, t, cx)),
    )
}

/// Each plugin's state in the run, a ruled row each. A row opens the
/// plugin's settings in place, in the run's repository or everywhere
/// (ADR 0029); a plugin with nothing to set opens its screen, and so
/// does the one that keeps the person's own plugins, each set on its
/// page (ADR 0036).
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
        .children(statuses.iter().map(|plugin| {
            let settings = ws.catalog.plugins.iter().any(|info| {
                info.name == plugin.name
                    && info.settings
                    && info.group != heiko_ui_plugin::Group::Yours
            });
            let open = settings
                && ws.inspector_plugin.as_deref() == Some(plugin.name.as_str());
            let route = ws.plugin_route_named(&plugin.name, &run.id);
            let name = plugin.name.clone();
            let row = div()
                .id(SharedString::from(format!("status-{}", plugin.name)))
                .when(settings || route.is_some(), |row| row.cursor_pointer())
                .when(settings, |row| {
                    row.on_click(cx.listener(move |ws, _, _, cx| {
                        ws.toggle_inspector_plugin(&name, cx)
                    }))
                })
                .when_some(route.filter(|_| !settings), |row, route| {
                    row.on_click(cx.listener(move |ws, _, _, cx| {
                        ws.navigate(route.clone(), cx)
                    }))
                })
                .flex()
                .flex_wrap()
                .items_baseline()
                .gap(sp(2.5))
                .py(sp(2.25))
                .border_b_1()
                .border_color(t.hairline)
                .child(
                    mono(
                        plugin.name.clone(),
                        Type::SMALL,
                        if open { t.bright } else { t.text },
                    )
                    .flex_1(),
                )
                // A long state wraps under the name, and is cut at the
                // panel's edge.
                .child(
                    mono(
                        plugin.state.clone(),
                        Type::CAPTION,
                        t.tone(plugin.tone),
                    )
                    .max_w_full()
                    .truncate(),
                );
            div().flex().flex_col().child(row).when(open, |column| {
                column.child(plugin_settings(ws, run, &plugin.name, t, cx))
            })
        }))
}

/// A plugin's settings, opened from its row: in the run's repository,
/// or everywhere, with the way to all of the plugin.
fn plugin_settings(
    ws: &Workspace,
    run: &RunView,
    plugin: &str,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let repo = (!run.repo.is_empty() && ws.catalog.repo(&run.repo).is_some())
        .then(|| run.repo.clone());
    let scope = repo.clone().filter(|_| !ws.inspector_everywhere);
    let options = repo
        .iter()
        .map(|repo| (Some(repo.clone()), repo.clone()))
        .chain(std::iter::once((None, "Everywhere".to_owned())));
    // The scope, as words: the one in force over a heavy line.
    let switch =
        div()
            .flex()
            .gap(sp(3.5))
            .children(options.map(|(option, label)| {
                let on = option == scope;
                let everywhere = option.is_none();
                div()
                    .id(SharedString::from(format!("inspector-scope-{label}")))
                    .cursor_pointer()
                    .typeset(Type::SMALL)
                    .border_b_2()
                    .border_color(if on { t.rule } else { t.bg })
                    .text_color(if on { t.bright } else { t.dim })
                    .child(label)
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.inspector_everywhere = everywhere;
                        cx.notify();
                    }))
            }));
    let all = {
        let plugin = plugin.to_owned();
        div()
            .id("inspector-all-settings")
            .typeset(Type::SMALL)
            .child(link_action("all of it", t))
            .on_click(cx.listener(move |ws, _, _, cx| {
                ws.pick_plugin(&plugin, None, cx);
                ws.navigate(Route::Plugins, cx);
            }))
    };
    div()
        .flex()
        .flex_col()
        .gap(sp(3.))
        .py(sp(3.))
        .border_b_1()
        .border_color(t.hairline)
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(sp(2.))
                .child(switch)
                .child(div().flex_1())
                .child(all),
        )
        .children(ws.plugin_settings_pane(plugin, scope.as_deref(), None, cx))
        .child(
            div()
                .typeset(Type::CAPTION)
                .text_color(t.dim)
                .child("Applies from your next message."),
        )
}

/// The row pinned under the panel that opens the event log, and the
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
        .border_color(t.hairline)
        .when(open, |panel| {
            panel.child(
                div()
                    .id("event-log")
                    .max_h(rems(17.5))
                    .overflow_y_scroll()
                    .px(sp(4.))
                    .py(sp(3.))
                    .child(event_log(run, t)),
            )
        })
        .child(
            div()
                .id("toggle-events")
                .flex()
                .items_center()
                .gap(sp(3.))
                .h(rems(2.75))
                .px(sp(4.))
                .cursor_pointer()
                .typeset(Type::SMALL)
                .on_click(cx.listener(|ws, _, _, cx| ws.toggle_events(cx)))
                .child(div().flex_1().text_color(t.text).child("Events"))
                .child(value(run.log.len().to_string(), t.dim))
                .child(link_action(if open { "hide" } else { "open" }, t)),
        )
}

fn event_log(run: &RunView, t: &Theme) -> Div {
    let color = |kind: &str| match kind {
        "ToolStart" | "Retry" | "Continued" => t.accent,
        "ToolEnd" | "RunStart" => t.green,
        "NestedStart" | "NestedEnd" => t.muted,
        "Usage" | "Rewrite" => t.blue,
        "PluginError" => t.red,
        _ => t.text_soft,
    };
    div()
        .flex()
        .flex_col()
        .gap(sp(0.5))
        .when(run.log.is_empty(), |log| {
            log.child(mono("No events yet.", Type::MICRO, t.dim))
        })
        .children(run.log.iter().rev().take(60).map(|line| {
            div()
                .flex()
                .gap(sp(2.))
                .child(
                    mono(format!("t{}", line.turn), Type::MICRO, t.dim)
                        .w(rems(1.5)),
                )
                .child(
                    mono(line.kind.clone(), Type::MICRO, color(&line.kind))
                        .w(rems(5.)),
                )
                .child(
                    mono(line.text.clone(), Type::MICRO, t.muted)
                        .flex_1()
                        .min_w(rems(0.))
                        .truncate(),
                )
        }))
}
