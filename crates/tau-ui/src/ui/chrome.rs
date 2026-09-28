//! Everything around the screens: title bar, sidebar, status bar, and
//! their phone versions.

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
};

use super::{dot, icon, mono, status_icon, status_look};
use crate::{
    assets::Icon,
    route::{Route, Tab},
    theme::{MONO, Theme},
    view::{ChildKind, Origin, RunView, tokens, usd},
    workspace::Workspace,
};

pub fn logo(t: &Theme, size: f32) -> Div {
    div()
        .size(px(size))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(size / 4.))
        .bg(t.accent)
        .text_color(t.bg)
        .font_family(MONO)
        .text_size(px(size * 0.6))
        .font_weight(FontWeight::MEDIUM)
        .child("τ")
}

pub fn icon_button(
    id: &'static str,
    glyph: Icon,
    size: f32,
    t: &Theme,
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .size(px(size))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(8.))
        .cursor_pointer()
        .hover(|style| style.bg(gpui::white().opacity(0.05)))
        .child(icon(glyph, 18., t.text_soft))
}

fn tab_icon(tab: Tab) -> Icon {
    match tab {
        Tab::Runs => Icon::Runs,
        Tab::Memory => Icon::Memory,
        Tab::History => Icon::History,
        Tab::Plugins => Icon::Plug,
    }
}

/// The desktop title bar: where you are, and the way back.
pub fn title_bar(
    ws: &Workspace,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let run = ws.current();
    let reasoning = run
        .and_then(|run| run.plan.iter().find(|f| f.name == "reasoning"))
        .map_or("reasoning auto".to_owned(), |field| {
            format!("reasoning {}", field.value)
        });
    let model = run.map_or("gpt-5.5".to_owned(), |run| run.model.clone());
    let total: f64 = ws.runs.iter().map(|run| run.usage.cost).sum();
    let mut crumbs = vec![ws.name.clone()];
    match &ws.route {
        Route::Home | Route::Run(_) => {
            crumbs.extend(run.map(|run| run.title.clone()));
        }
        route => {
            crumbs.extend(
                route
                    .run()
                    .and_then(|id| ws.run(id))
                    .map(|run| run.title.clone()),
            );
            crumbs.push(route.title().to_owned());
        }
    }
    let last = crumbs.len() - 1;

    div()
        .h(px(44.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(10.))
        .pl(px(12.))
        .pr(px(12.))
        .bg(t.panel)
        .border_b_1()
        .border_color(t.border)
        .child(logo(t, 26.))
        .when(ws.can_go_back(), |bar| {
            bar.child(
                icon_button("back", Icon::Back, 30., t)
                    .on_click(cx.listener(|ws, _, _, cx| ws.back(cx))),
            )
        })
        .children(crumbs.into_iter().enumerate().flat_map(|(n, crumb)| {
            let text = div()
                .when(n == last, |crumb| crumb.font_weight(FontWeight::MEDIUM))
                .text_color(if n == last { t.text } else { t.muted })
                .child(crumb)
                .into_any_element();
            let slash = (n > 0)
                .then(|| div().text_color(t.dim).child("/").into_any_element());
            slash.into_iter().chain([text])
        }))
        .child(div().flex_1())
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .w(px(340.))
                .px(px(10.))
                .py(px(5.))
                .border_1()
                .border_color(t.border)
                .rounded(px(6.))
                .text_color(t.dim)
                .child(icon(Icon::Search, 14., t.dim))
                .child(div().flex_1().child("Search runs, agents, commands"))
                .child(mono("Ctrl K", 11., t.dim)),
        )
        .child(div().flex_1())
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(10.))
                .py(px(5.))
                .bg(t.raised)
                .rounded(px(6.))
                .child(mono(model, 12., t.text))
                .child(mono("·", 12., t.dim))
                .child(mono(reasoning, 12., t.blue)),
        )
        .child(
            mono(format!("today {}", usd(total)), 12., t.accent)
                .px(px(10.))
                .py(px(5.))
                .rounded(px(6.))
                .bg(t.accent_soft),
        )
}

/// The desktop sidebar: sections, then runs with their forks and
/// sub-agents, then agents.
pub fn sidebar(
    ws: &Workspace,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> gpui::Stateful<Div> {
    let nav = [
        (Route::History, Icon::History, "History", "Ctrl 3"),
        (
            Route::Memory { note: None },
            Icon::Memory,
            "Memory",
            "Ctrl 2",
        ),
        (Route::Plugins, Icon::Plug, "Plugins", "Ctrl 4"),
        (
            Route::Constitution { rule: None },
            Icon::Blocked,
            "Constitution",
            "",
        ),
    ];
    let active_section = |route: &Route| match (route, &ws.route) {
        (Route::Memory { .. }, Route::Memory { .. }) => true,
        (Route::Constitution { .. }, Route::Constitution { .. }) => true,
        (a, b) => a == b,
    };
    let reviews: usize = ws
        .runs
        .iter()
        .flat_map(|run| run.reviews().map(move |card| (run, card)))
        .filter(|(run, card)| {
            matches!(card.state, crate::view::ToolState::Flagged { .. })
                && !ws
                    .dismissed
                    .contains(&(run.id.clone(), card.call_id.clone()))
        })
        .count();

    let nav =
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .children(nav.into_iter().map(|(route, glyph, label, keys)| {
                let active = active_section(&route);
                let badge =
                    (label == "Constitution" && reviews > 0).then(|| {
                        mono(reviews.to_string(), 11., t.bg)
                            .px(px(6.))
                            .rounded(px(8.))
                            .bg(t.accent)
                    });
                div()
                    .id(label)
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(10.))
                    .py(px(7.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(active, |row| row.bg(t.selected))
                    .when(!active, |row| {
                        row.hover(|style| style.bg(gpui::white().opacity(0.03)))
                    })
                    .child(icon(
                        glyph,
                        14.,
                        if active { t.text } else { t.muted },
                    ))
                    .child(
                        div()
                            .flex_1()
                            .text_color(if active {
                                t.text
                            } else {
                                t.text_soft
                            })
                            .child(label),
                    )
                    .children(badge)
                    .when(!keys.is_empty(), |row| {
                        row.child(mono(keys, 11., t.dim))
                    })
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.navigate(route.clone(), cx)
                    }))
            }));

    let current = ws.current().map(|run| run.id.clone());
    let mut list = div().flex().flex_col().gap(px(2.)).child(
        div()
            .px(px(10.))
            .pb(px(6.))
            .child(super::heading("Runs", t)),
    );
    for run in ws.runs.iter().filter(|run| run.origin == Origin::Root) {
        let active = current.as_ref() == Some(&run.id)
            && matches!(ws.route, Route::Run(_) | Route::Home);
        let route = Route::Run(run.id.clone());
        list = list.child(
            div()
                .id(SharedString::from(format!("run-{}", run.id)))
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(10.))
                .py(px(7.))
                .rounded(px(6.))
                .cursor_pointer()
                .when(active, |row| row.bg(t.selected))
                .when(!active, |row| {
                    row.hover(|style| style.bg(gpui::white().opacity(0.03)))
                })
                .child(status_icon(run, t, 12.))
                .child(
                    div()
                        .flex_1()
                        .truncate()
                        .text_color(if active { t.text } else { t.text_soft })
                        .when(active, |title| {
                            title.font_weight(FontWeight::MEDIUM)
                        })
                        .child(run.title.clone()),
                )
                .child(mono(usd(run.usage.cost), 11., t.muted))
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.navigate(route.clone(), cx)
                })),
        );
        list = list.children(run.children.iter().map(|child| {
            let (color, label) = status_look(&child.status, t);
            let route = match child.kind {
                ChildKind::Fork => Some(Route::Compare {
                    main: run.id.clone(),
                    fork: child.id.clone(),
                }),
                ChildKind::SubAgent => {
                    ws.run(&child.id).map(|_| Route::Run(child.id.clone()))
                }
            };
            let active = route.as_ref() == Some(&ws.route);
            div()
                .id(SharedString::from(format!("child-{}", child.id)))
                .flex()
                .items_center()
                .gap(px(8.))
                .pl(px(28.))
                .pr(px(10.))
                .py(px(6.))
                .rounded(px(6.))
                .text_color(t.text_soft)
                .when(active, |row| row.bg(t.selected))
                .child(icon(
                    match child.kind {
                        ChildKind::SubAgent => Icon::SubAgent,
                        ChildKind::Fork => Icon::Fork,
                    },
                    12.,
                    t.blue,
                ))
                .child(div().flex_1().truncate().child(child.title.clone()))
                .child(div().text_size(px(11.)).text_color(color).child(label))
                .when_some(route, |row, route| {
                    row.cursor_pointer()
                        .hover(|style| style.bg(gpui::white().opacity(0.03)))
                        .on_click(cx.listener(move |ws, _, _, cx| {
                            ws.navigate(route.clone(), cx)
                        }))
                })
        }));
    }

    let mut agents: Vec<&str> =
        ws.runs.iter().map(|run| run.agent.as_str()).collect();
    agents.sort_unstable();
    agents.dedup();

    div()
        .id("sidebar")
        .flex()
        .flex_col()
        .gap(px(18.))
        .px(px(8.))
        .py(px(12.))
        .bg(t.panel)
        .border_r_1()
        .border_color(t.border)
        .overflow_y_scroll()
        .child(
            div()
                .id("new-run")
                .flex()
                .items_center()
                .gap(px(8.))
                .h(px(36.))
                .px(px(10.))
                .border_1()
                .border_color(t.border)
                .bg(t.raised)
                .rounded(px(6.))
                .cursor_pointer()
                .child(icon(Icon::Plus, 14., t.text))
                .child(div().flex_1().child("New run"))
                .child(mono("Ctrl N", 11., t.dim))
                .on_click(cx.listener(|ws, _, window, cx| {
                    ws.start_new_run(window, cx)
                })),
        )
        .child(nav)
        .child(list)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .px(px(10.))
                        .pb(px(6.))
                        .child(super::heading("Agents", t)),
                )
                .children(agents.into_iter().map(|agent| {
                    div()
                        .id(SharedString::from(format!("agent-{agent}")))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(10.))
                        .py(px(6.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(|style| style.bg(gpui::white().opacity(0.03)))
                        .child(
                            mono(
                                agent.chars().next().unwrap_or('?').to_string(),
                                11.,
                                t.blue,
                            )
                            .size(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.))
                            .bg(gpui::rgb(0x2c3a52)),
                        )
                        .child(div().flex_1().child(agent.to_owned()))
                        .child(mono("plugins", 11., t.dim))
                        .on_click(cx.listener(|ws, _, _, cx| {
                            ws.navigate(Route::Plugins, cx)
                        }))
                })),
        )
}

pub fn status_bar(ws: &Workspace, t: &Theme) -> Div {
    let context = ws.current().map(|run| {
        let window = run
            .context
            .window
            .map_or(String::new(), |window| format!(" / {}", tokens(window)));
        format!("context {}{window}", tokens(run.context.used))
    });
    div()
        .h(px(26.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(18.))
        .px(px(14.))
        .bg(t.panel)
        .border_t_1()
        .border_color(t.border)
        .font_family(MONO)
        .text_size(px(11.))
        .text_color(t.muted)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(dot(t.green, 6.))
                .child("tau-ui"),
        )
        .children(context)
        .child(div().flex_1())
        .child("Esc back")
        .child(format!(
            "{} · {} runs",
            ws.catalog.store.path,
            ws.runs.len()
        ))
}

/// The phone's header above a run: back, title and status, details.
pub fn phone_run_bar(
    run: &RunView,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let (color, label) = status_look(&run.status, t);
    div()
        .h(px(56.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(4.))
        .px(px(6.))
        .bg(t.panel)
        .border_b_1()
        .border_color(t.border)
        .child(
            icon_button("phone-back", Icon::Back, 44., t)
                .on_click(cx.listener(|ws, _, _, cx| ws.back(cx))),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(px(15.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .truncate()
                        .child(run.title.clone()),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(12.))
                        .text_color(t.muted)
                        .child(dot(color, 6.))
                        .child(label)
                        .child(mono(
                            format!(
                                "· turn {} · {}",
                                run.turn,
                                usd(run.usage.cost)
                            ),
                            12.,
                            t.muted,
                        )),
                ),
        )
        .child(
            icon_button("phone-details", Icon::Panel, 44., t)
                .on_click(cx.listener(|ws, _, _, cx| ws.toggle_sheet(cx))),
        )
}

/// The phone's header on every other screen.
pub fn phone_header(
    ws: &Workspace,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let title = match &ws.route {
        Route::Memory { note: Some(id) } => ws
            .catalog
            .memory
            .note(id)
            .map_or("Memory".to_owned(), |note| note.title.clone()),
        Route::Home => ws.name.clone(),
        route => route.title().to_owned(),
    };
    div()
        .h(px(56.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(4.))
        .px(px(6.))
        .bg(t.panel)
        .border_b_1()
        .border_color(t.border)
        .when(!ws.route.is_top_level(), |bar| {
            bar.child(
                icon_button("phone-back", Icon::Back, 44., t)
                    .on_click(cx.listener(|ws, _, _, cx| ws.back(cx))),
            )
        })
        .when(ws.route.is_top_level(), |bar| {
            bar.pl(px(16.)).child(logo(t, 28.)).child(div().w(px(6.)))
        })
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .text_size(px(17.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .when(matches!(ws.route, Route::Home), |bar| {
            bar.child(icon_button("phone-new", Icon::Plus, 44., t).on_click(
                cx.listener(|ws, _, window, cx| ws.start_new_run(window, cx)),
            ))
        })
}

/// The phone's home: live runs as cards, then the rest.
pub fn phone_run_list(
    ws: &Workspace,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let live: Vec<AnyElement> = ws
        .runs
        .iter()
        .filter(|run| run.status.is_live())
        .map(|run| live_card(run, t, cx).into_any_element())
        .collect();
    let earlier =
        ws.runs
            .iter()
            .filter(|run| !run.status.is_live())
            .map(|run| {
                let (color, label) = status_look(&run.status, t);
                let route = match &run.origin {
                    Origin::Fork { from, .. } => Route::Compare {
                        main: from.clone(),
                        fork: run.id.clone(),
                    },
                    _ => Route::Run(run.id.clone()),
                };
                div()
                    .id(SharedString::from(format!("phone-run-{}", run.id)))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .min_h(px(56.))
                    .px(px(16.))
                    .border_b_1()
                    .border_color(t.border)
                    .cursor_pointer()
                    .child(status_icon(run, t, 14.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .truncate()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(run.title.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(color)
                                    .child(format!(
                                        "{label} · {} turns",
                                        run.turn
                                    )),
                            ),
                    )
                    .child(mono(usd(run.usage.cost), 12., t.muted))
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.navigate(route.clone(), cx)
                    }))
            });

    div()
        .id("phone-runs")
        .flex_1()
        .min_h(px(0.))
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(12.))
                .p(px(16.))
                .children(live),
        )
        .child(
            div()
                .px(px(16.))
                .pb(px(6.))
                .child(super::heading("Earlier", t)),
        )
        .children(earlier)
}

fn live_card(
    run: &RunView,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let meter = run.meters().into_iter().find(|meter| meter.label == "Cost");
    let blocked = run
        .plugins
        .iter()
        .find(|plugin| plugin.state.contains("blocked"));
    let route = Route::Run(run.id.clone());
    div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .p(px(14.))
        .border_1()
        .border_color(t.accent_border)
        .rounded(px(14.))
        .bg(t.panel)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(dot(t.accent, 8.))
                .child(
                    div()
                        .flex_1()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(run.title.clone()),
                )
                .child(mono(
                    format!("turn {} · {}", run.turn, usd(run.usage.cost)),
                    12.,
                    t.muted,
                )),
        )
        .when_some(blocked, |card, plugin| {
            card.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(px(8.))
                    .bg(t.red_soft)
                    .child(icon(Icon::Blocked, 15., t.red))
                    .child(mono(plugin.name.clone(), 12., t.red))
                    .child(
                        div()
                            .text_color(t.text_soft)
                            .child(plugin.state.clone()),
                    ),
            )
        })
        .when_some(meter, |card, meter| {
            card.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(5.))
                    .child(
                        div()
                            .flex()
                            .text_size(px(12.))
                            .child(div().flex_1().child("Cost"))
                            .child(mono(meter.value, 12., t.muted)),
                    )
                    .child(super::bar(meter.share, 4., t.text_soft, t.border)),
            )
        })
        .child(
            div()
                .id(SharedString::from(format!("open-{}", run.id)))
                .child(
                    super::primary_button("Open", t)
                        .h(px(44.))
                        .rounded(px(10.)),
                )
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.navigate(route.clone(), cx)
                })),
        )
}

pub fn phone_tab_bar(
    ws: &Workspace,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let current = ws.route.tab();
    div()
        .h(px(64.))
        .flex_shrink_0()
        .grid()
        .grid_cols(4)
        .bg(t.panel)
        .border_t_1()
        .border_color(t.border)
        .children(Tab::ALL.into_iter().map(|tab| {
            let active = tab == current;
            let color = if active { t.text } else { t.muted };
            div()
                .id(tab.label())
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(4.))
                .text_size(px(11.))
                .text_color(color)
                .cursor_pointer()
                .child(icon(tab_icon(tab), 20., color))
                .child(tab.label())
                .on_click(
                    cx.listener(move |ws, _, _, cx| ws.switch_tab(tab, cx)),
                )
        }))
}
