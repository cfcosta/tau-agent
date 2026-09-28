//! Everything around the transcript: title bar, run list, status bar,
//! and their phone versions.

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
    theme::{MONO, Theme},
    view::{ChildKind, RunView, tokens, usd},
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

pub fn title_bar(
    workspace_name: &str,
    run: Option<&RunView>,
    total_cost: f64,
    t: &Theme,
) -> Div {
    let reasoning = run
        .and_then(|run| run.plan.iter().find(|f| f.name == "reasoning"))
        .map_or("reasoning auto".to_owned(), |field| {
            format!("reasoning {}", field.value)
        });
    let model = run.map_or("gpt-5.5".to_owned(), |run| run.model.clone());
    div()
        .h(px(44.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(12.))
        .pl(px(16.))
        .pr(px(12.))
        .bg(t.panel)
        .border_b_1()
        .border_color(t.border)
        .child(logo(t, 26.))
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .child(workspace_name.to_owned()),
        )
        .child(div().flex_1())
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .w(px(380.))
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
            mono(format!("today {}", usd(total_cost)), 12., t.accent)
                .px(px(10.))
                .py(px(5.))
                .rounded(px(6.))
                .bg(t.accent_soft),
        )
}

/// The run list on the left of the desktop layout.
pub fn sidebar(
    runs: &[RunView],
    selected: usize,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let mut list = div().flex().flex_col().gap(px(2.)).child(
        div()
            .px(px(10.))
            .pb(px(6.))
            .child(super::heading("Runs", t)),
    );
    for (index, run) in runs.iter().enumerate() {
        let active = index == selected;
        list = list.child(
            div()
                .id(SharedString::from(format!("run-{index}")))
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
                .on_click(
                    cx.listener(move |ws, _, _, cx| ws.select_index(index, cx)),
                ),
        );
        list = list.children(run.children.iter().map(|child| {
            let (color, label) = status_look(&child.status, t);
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .pl(px(28.))
                .pr(px(10.))
                .py(px(6.))
                .text_color(t.text_soft)
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
        }));
    }

    let mut agents: Vec<&str> =
        runs.iter().map(|run| run.agent.as_str()).collect();
    agents.sort_unstable();
    agents.dedup();

    div()
        .flex()
        .flex_col()
        .gap(px(18.))
        .px(px(8.))
        .py(px(12.))
        .bg(t.panel)
        .border_r_1()
        .border_color(t.border)
        .overflow_hidden()
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
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(10.))
                        .py(px(6.))
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
                        .child(agent.to_owned())
                })),
        )
}

pub fn status_bar(run: Option<&RunView>, runs: usize, t: &Theme) -> Div {
    let context = run.map(|run| {
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
        .child(format!("{runs} runs"))
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
                .on_click(cx.listener(|ws, _, _, cx| ws.show_run_list(cx))),
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

/// The phone's home: live runs as cards, then the rest.
pub fn phone_run_list(
    workspace_name: &str,
    runs: &[RunView],
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let live: Vec<AnyElement> = runs
        .iter()
        .enumerate()
        .filter(|(_, run)| run.status.is_live())
        .map(|(index, run)| live_card(index, run, t, cx).into_any_element())
        .collect();
    let earlier = runs
        .iter()
        .enumerate()
        .filter(|(_, run)| !run.status.is_live())
        .map(|(index, run)| {
            let (color, label) = status_look(&run.status, t);
            div()
                .id(SharedString::from(format!("phone-run-{index}")))
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
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .child(run.title.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(color)
                                .child(format!("{label} · {} turns", run.turn)),
                        ),
                )
                .child(mono(usd(run.usage.cost), 12., t.muted))
                .on_click(
                    cx.listener(move |ws, _, _, cx| ws.select_index(index, cx)),
                )
        });

    div()
        .size_full()
        .flex()
        .flex_col()
        .child(
            div()
                .h(px(56.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(10.))
                .pl(px(16.))
                .pr(px(8.))
                .child(logo(t, 28.))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(17.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(workspace_name.to_owned()),
                )
                .child(icon_button("phone-new", Icon::Plus, 44., t).on_click(
                    cx.listener(|ws, _, window, cx| {
                        ws.start_new_run(window, cx)
                    }),
                )),
        )
        .child(
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
                        .px(px(16.))
                        .pb(px(16.))
                        .children(live),
                )
                .child(
                    div()
                        .px(px(16.))
                        .pb(px(6.))
                        .child(super::heading("Earlier", t)),
                )
                .children(earlier),
        )
        .child(phone_tab_bar(t))
}

fn live_card(
    index: usize,
    run: &RunView,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let meter = run.meters().into_iter().find(|meter| meter.label == "Cost");
    let blocked = run
        .plugins
        .iter()
        .find(|plugin| plugin.state.contains("blocked"));
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
                .id(SharedString::from(format!("open-run-{index}")))
                .child(
                    super::primary_button("Open", t)
                        .h(px(44.))
                        .rounded(px(10.)),
                )
                .on_click(
                    cx.listener(move |ws, _, _, cx| ws.select_index(index, cx)),
                ),
        )
}

fn phone_tab_bar(t: &Theme) -> Div {
    div()
        .h(px(64.))
        .flex_shrink_0()
        .grid()
        .grid_cols(3)
        .bg(t.panel)
        .border_t_1()
        .border_color(t.border)
        .children(
            [
                (Icon::Runs, "Runs", true),
                (Icon::Memory, "Memory", false),
                (Icon::History, "History", false),
            ]
            .into_iter()
            .map(|(glyph, label, active)| {
                let color = if active { t.text } else { t.muted };
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(4.))
                    .text_size(px(11.))
                    .text_color(color)
                    .child(icon(glyph, 20., color))
                    .child(label)
            }),
        )
}
