//! The agent's plugins, in the order the loop asks them: what each one
//! does, and what it is doing in the open run or what it cost.

use gpui::{AnyElement, Context, SharedString, div, prelude::*, px};

use crate::{
    theme::{Design as _, Theme, Type, radius, sp},
    ui::{self, mono},
    view::{tokens, usd},
    workspace::Workspace,
};

pub fn render(
    ws: &Workspace,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let catalog = &ws.catalog;
    let run = ws.current();
    // What plugins charged, by the catalog's count of each.
    let plugin_cost: f64 =
        catalog.plugins.iter().map(|plugin| plugin.spend).sum();
    let rows: Vec<AnyElement> = catalog
        .plugins
        .iter()
        .map(|plugin| {
            let route = ws.plugin_route(plugin);
            let ink =
                t.roles.plugin(&plugin.name).unwrap_or(t.mark(&plugin.name));
            // What it does in the open run, else what it cost.
            let (state, state_ink) = run
                .and_then(|run| {
                    run.plugins.iter().find(|status| status.name == plugin.name)
                })
                .map(|status| (status.state.clone(), t.tone(status.tone)))
                .unwrap_or_else(|| (usd(plugin.spend), t.dim));
            div()
                .id(SharedString::from(format!("plugin-{}", plugin.name)))
                .flex()
                .items_center()
                .gap(sp(3.))
                .px(sp(1.))
                .py(sp(3.))
                .border_b_1()
                .border_color(t.border)
                .when(route.is_some(), |row| {
                    row.cursor_pointer().hover(|style| style.bg(t.card))
                })
                .child(
                    div()
                        .size(px(7.))
                        .flex_shrink_0()
                        .rounded(radius::FULL)
                        .bg(ink),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .child(mono(plugin.name.clone(), Type::SMALL, ink))
                        .child(
                            div()
                                .truncate()
                                .typeset(Type::CAPTION)
                                .text_color(t.muted)
                                .child(plugin.description.clone()),
                        ),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .typeset(Type::CAPTION)
                        .text_color(state_ink)
                        .child(state),
                )
                .when_some(route, |row, route| {
                    row.on_click(cx.listener(move |ws, _, _, cx| {
                        ws.navigate(route.clone(), cx)
                    }))
                })
                .into_any_element()
        })
        .collect();
    let jev = catalog.jev.as_ref().map(|jev| {
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(sp(2.))
            .typeset(Type::CAPTION)
            .text_color(t.muted)
            .child(div().size(px(7.)).rounded(radius::FULL).bg(t.green))
            .child(div().text_color(t.text).child("tau-jev"))
            .child(format!("connected to {}", jev.model))
            .child("·")
            .child(format!(
                "{} requests, {} tokens in",
                jev.requests,
                tokens(jev.input_tokens)
            ))
            .child("·")
            .child(div().text_color(t.roles.cost).child(usd(jev.spent)))
            .child("·")
            .child(format!("p50 {} ms", jev.latency_p50_ms))
            .when(jev.failed > 0, |line| {
                line.child("·").child(
                    div()
                        .text_color(t.red)
                        .child(format!("{} failed", jev.failed)),
                )
            })
    });
    ui::screen(
        "plugins",
        compact,
        div()
            .w_full()
            .max_w(px(1040.))
            .flex()
            .flex_col()
            .gap(sp(5.))
            .child(ui::screen_title(
                "Plugins",
                format!(
                    "What runs alongside every {} run, in the order the loop asks them. {} charged by plugins.",
                    catalog.agent,
                    usd(plugin_cost)
                ),
                t,
            ))
            .child(
                div()
                    .grid()
                    .grid_cols(if compact { 1 } else { 2 })
                    .gap_x(sp(8.))
                    .children(rows),
            )
            .children(jev),
    )
    .into_any_element()
}
