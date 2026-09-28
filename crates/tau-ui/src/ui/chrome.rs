//! Everything around the screens: title bar, sidebar, status bar, and
//! their phone versions.

use gpui::{
    AnyElement,
    Context,
    Div,
    IntoElement,
    SharedString,
    div,
    prelude::*,
    px,
};

use super::{dot, icon, icon_button, logo, mono, status_icon, status_look};
use crate::{
    assets::Icon,
    catalog::ProjectStatus,
    route::{Route, Tab},
    theme::{Design as _, IconSize, MONO, Theme, Type, radius, sp, weight},
    ui::components::ButtonKind,
    view::{ChildKind, Origin, RunView, tokens, usd},
    workspace::Workspace,
};

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
        .gap(sp(2.5))
        .pl(sp(3.))
        .pr(sp(3.))
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
                .when(n == last, |crumb| crumb.font_weight(weight::EMPHASIS))
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
                .gap(sp(2.))
                .w(px(340.))
                .px(sp(2.5))
                .py(sp(1.25))
                .border_1()
                .border_color(t.border)
                .rounded(radius::CONTROL)
                .text_color(t.dim)
                .child(icon(Icon::Search, IconSize::BASE, t.dim))
                .child(div().flex_1().child("Search runs, agents, commands"))
                .child(mono("Ctrl K", Type::MICRO, t.dim)),
        )
        .child(div().flex_1())
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(1.5))
                .px(sp(2.5))
                .py(sp(1.25))
                .bg(t.raised)
                .rounded(radius::CONTROL)
                .child(mono(model, Type::CAPTION, t.text))
                .child(mono("·", Type::CAPTION, t.dim))
                .child(mono(reasoning, Type::CAPTION, t.blue)),
        )
        .child(
            mono(format!("today {}", usd(total)), Type::CAPTION, t.accent)
                .px(sp(2.5))
                .py(sp(1.25))
                .rounded(radius::CONTROL)
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
            .gap(sp(0.5))
            .children(nav.into_iter().map(|(route, glyph, label, keys)| {
                let active = active_section(&route);
                let badge =
                    (label == "Constitution" && reviews > 0).then(|| {
                        mono(reviews.to_string(), Type::MICRO, t.bg)
                            .px(sp(1.5))
                            .rounded(radius::BOX)
                            .bg(t.accent)
                    });
                div()
                    .id(label)
                    .flex()
                    .items_center()
                    .gap(sp(2.5))
                    .px(sp(2.5))
                    .py(sp(1.75))
                    .rounded(radius::CONTROL)
                    .cursor_pointer()
                    .when(active, |row| row.bg(t.selected))
                    .when(!active, |row| {
                        row.hover(|style| style.bg(gpui::white().opacity(0.03)))
                    })
                    .child(icon(
                        glyph,
                        IconSize::BASE,
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
                        row.child(mono(keys, Type::MICRO, t.dim))
                    })
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.navigate(route.clone(), cx)
                    }))
            }));

    let current = ws.current().map(|run| run.id.clone());
    let mut list = div().flex().flex_col().gap(sp(0.5)).child(
        div()
            .px(sp(2.5))
            .pb(sp(1.5))
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
                .gap(sp(2.))
                .px(sp(2.5))
                .py(sp(1.75))
                .rounded(radius::CONTROL)
                .cursor_pointer()
                .when(active, |row| row.bg(t.selected))
                .when(!active, |row| {
                    row.hover(|style| style.bg(gpui::white().opacity(0.03)))
                })
                .child(status_icon(run, t, IconSize::SMALL))
                .child(
                    div()
                        .flex_1()
                        .truncate()
                        .text_color(if active { t.text } else { t.text_soft })
                        .when(active, |title| {
                            title.font_weight(weight::EMPHASIS)
                        })
                        .child(run.title.clone()),
                )
                .child(mono(usd(run.usage.cost), Type::MICRO, t.muted))
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
                .gap(sp(2.))
                .pl(sp(7.))
                .pr(sp(2.5))
                .py(sp(1.5))
                .rounded(radius::CONTROL)
                .text_color(t.text_soft)
                .when(active, |row| row.bg(t.selected))
                .child(icon(
                    match child.kind {
                        ChildKind::SubAgent => Icon::SubAgent,
                        ChildKind::Fork => Icon::Fork,
                    },
                    IconSize::SMALL,
                    t.blue,
                ))
                .child(div().flex_1().truncate().child(child.title.clone()))
                .child(
                    div().typeset(Type::MICRO).text_color(color).child(label),
                )
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
        .gap(sp(4.5))
        .px(sp(2.))
        .py(sp(3.))
        .bg(t.panel)
        .border_r_1()
        .border_color(t.border)
        .overflow_y_scroll()
        .child(
            div()
                .id("new-run")
                .flex()
                .items_center()
                .gap(sp(2.))
                .h(px(36.))
                .px(sp(2.5))
                .border_1()
                .border_color(t.border)
                .bg(t.raised)
                .rounded(radius::CONTROL)
                .cursor_pointer()
                .child(icon(Icon::Plus, IconSize::BASE, t.text))
                .child(div().flex_1().child("New run"))
                .child(mono("Ctrl N", Type::MICRO, t.dim))
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
                .gap(sp(0.5))
                .child(
                    div()
                        .px(sp(2.5))
                        .pb(sp(1.5))
                        .child(super::heading("Agents", t)),
                )
                .children(agents.into_iter().map(|agent| {
                    div()
                        .id(SharedString::from(format!("agent-{agent}")))
                        .flex()
                        .items_center()
                        .gap(sp(2.))
                        .px(sp(2.5))
                        .py(sp(1.5))
                        .rounded(radius::CONTROL)
                        .cursor_pointer()
                        .hover(|style| style.bg(gpui::white().opacity(0.03)))
                        .child(
                            mono(
                                agent.chars().next().unwrap_or('?').to_string(),
                                Type::MICRO,
                                t.blue,
                            )
                            .size(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(radius::SMALL)
                            .bg(t.blue_border),
                        )
                        .child(div().flex_1().child(agent.to_owned()))
                        .child(mono("plugins", Type::MICRO, t.dim))
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
        .gap(sp(4.5))
        .px(sp(3.5))
        .bg(t.panel)
        .border_t_1()
        .border_color(t.border)
        .font_family(MONO)
        .typeset(Type::MICRO)
        .text_color(t.muted)
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(1.5))
                .child(dot(t.green, 6.))
                .child("tau-ui"),
        )
        .children(context)
        .children(match &ws.catalog.project {
            ProjectStatus::Unknown => None,
            ProjectStatus::Importing(name) => Some(
                div()
                    .text_color(t.accent)
                    .child(format!("importing {name}…")),
            ),
            ProjectStatus::Ready(name) => {
                Some(div().child(format!("{name} · a workspace per run")))
            }
            ProjectStatus::Checkout(_) => {
                Some(div().text_color(t.dim).child("runs in the checkout"))
            }
        })
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
        .gap(sp(1.))
        .px(sp(1.5))
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
                .gap(sp(0.5))
                .child(
                    div()
                        .typeset(Type::LEAD)
                        .font_weight(weight::STRONG)
                        .truncate()
                        .child(run.title.clone()),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(1.5))
                        .typeset(Type::CAPTION)
                        .text_color(t.muted)
                        .child(dot(color, 6.))
                        .child(label)
                        .child(mono(
                            format!(
                                "· turn {} · {}",
                                run.turn,
                                usd(run.usage.cost)
                            ),
                            Type::CAPTION,
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
        .gap(sp(1.))
        .px(sp(1.5))
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
            bar.pl(sp(4.)).child(logo(t, 28.)).child(div().w(px(6.)))
        })
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .typeset(Type::TITLE)
                .font_weight(weight::STRONG)
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
                    .gap(sp(3.))
                    .min_h(px(56.))
                    .px(sp(4.))
                    .border_b_1()
                    .border_color(t.border)
                    .cursor_pointer()
                    .child(status_icon(run, t, IconSize::BASE))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .gap(sp(0.5))
                            .child(
                                div()
                                    .truncate()
                                    .font_weight(weight::EMPHASIS)
                                    .child(run.title.clone()),
                            )
                            .child(
                                div()
                                    .typeset(Type::CAPTION)
                                    .text_color(color)
                                    .child(format!(
                                        "{label} · {} turns",
                                        run.turn
                                    )),
                            ),
                    )
                    .child(mono(usd(run.usage.cost), Type::CAPTION, t.muted))
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
        .child(div().flex().flex_col().gap(sp(3.)).p(sp(4.)).children(live))
        .child(
            div()
                .px(sp(4.))
                .pb(sp(1.5))
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
        .gap(sp(3.))
        .p(sp(3.5))
        .border_1()
        .border_color(t.accent_border)
        .rounded(radius::TILE)
        .bg(t.panel)
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(2.))
                .child(dot(t.accent, 8.))
                .child(
                    div()
                        .flex_1()
                        .font_weight(weight::STRONG)
                        .child(run.title.clone()),
                )
                .child(mono(
                    format!("turn {} · {}", run.turn, usd(run.usage.cost)),
                    Type::CAPTION,
                    t.muted,
                )),
        )
        .when_some(blocked, |card, plugin| {
            card.child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(2.))
                    .px(sp(2.5))
                    .py(sp(2.))
                    .rounded(radius::BOX)
                    .bg(t.red_soft)
                    .child(icon(Icon::Blocked, IconSize::MEDIUM, t.red))
                    .child(mono(plugin.name.clone(), Type::CAPTION, t.red))
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
                    .gap(sp(1.25))
                    .child(
                        div()
                            .flex()
                            .typeset(Type::CAPTION)
                            .child(div().flex_1().child("Cost"))
                            .child(mono(meter.value, Type::CAPTION, t.muted)),
                    )
                    .child(super::bar(meter.share, 4., t.text_soft, t.border)),
            )
        })
        .child(
            div()
                .id(SharedString::from(format!("open-{}", run.id)))
                .child(
                    super::button("Open", ButtonKind::Primary, t)
                        .h(px(44.))
                        .rounded(radius::LARGE),
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
                .gap(sp(1.))
                .typeset(Type::MICRO)
                .text_color(color)
                .cursor_pointer()
                .child(icon(tab_icon(tab), IconSize::HUGE, color))
                .child(tab.label())
                .on_click(
                    cx.listener(move |ws, _, _, cx| ws.switch_tab(tab, cx)),
                )
        }))
}
