//! The agent's plugins in registration order, the seams each one uses,
//! and what they cost.

use gpui::{AnyElement, Context, SharedString, div, prelude::*, px, relative};

use crate::{
    catalog::{PluginInfo, Seam},
    theme::Theme,
    ui::{self, bar, dot, heading, key_values, mono},
    view::{Item, tokens, usd},
    workspace::Workspace,
};

pub fn render(
    ws: &Workspace,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let catalog = &ws.catalog;
    let rows = catalog.plugins.iter().enumerate().map(|(index, plugin)| {
        let route = ws.plugin_route(plugin);
        let seams = Seam::ALL.iter().map(|seam| {
            let used = plugin.seams.contains(seam);
            div()
                .when(!compact, |cell| cell.flex_1().flex().justify_center())
                .child(
                    div()
                        .size(px(10.))
                        .rounded(px(5.))
                        .border_1()
                        .border_color(if used {
                            t.blue
                        } else {
                            t.border_strong
                        })
                        .when(used, |dot| dot.bg(t.blue)),
                )
        });
        let name = div()
            .flex()
            .flex_col()
            .gap(px(3.))
            .child(mono(plugin.name.clone(), 13., t.text))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(t.muted)
                    .child(plugin.description.clone()),
            );
        div()
            .id(SharedString::from(format!("plugin-{}", plugin.name)))
            .flex()
            .when(compact, |row| row.flex_col().items_start().gap(px(8.)))
            .when(!compact, |row| row.items_center().gap(px(8.)))
            .px(px(16.))
            .py(px(12.))
            .border_b_1()
            .border_color(t.border)
            .when(route.is_some(), |row| {
                row.cursor_pointer()
                    .hover(|style| style.bg(gpui::white().opacity(0.03)))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .when(!compact, |cell| cell.w(px(300.)).flex_shrink_0())
                    .child(mono((index + 1).to_string(), 12., t.dim))
                    .child(name),
            )
            .child(
                div()
                    .flex()
                    .gap(px(if compact { 10. } else { 0. }))
                    .when(!compact, |cells| cells.flex_1())
                    .children(seams),
            )
            .child(
                mono(usd(plugin.spend), 12., t.muted).when(!compact, |cell| {
                    cell.w(px(80.)).flex().justify_end()
                }),
            )
            .when_some(route, |row, route| {
                row.on_click(cx.listener(move |ws, _, _, cx| {
                    ws.navigate(route.clone(), cx)
                }))
            })
    });
    let header = div()
        .flex()
        .items_end()
        .gap(px(8.))
        .px(px(16.))
        .py(px(10.))
        .bg(t.panel)
        .border_b_1()
        .border_color(t.border)
        .child(div().w(px(300.)).child(heading("Plugin", t)))
        .child(div().flex_1().flex().children(Seam::ALL.iter().map(|seam| {
            div().flex_1().flex().justify_center().child(mono(
                seam.label(),
                11.,
                t.dim,
            ))
        })))
        .child(
            div()
                .w(px(80.))
                .flex()
                .justify_end()
                .child(heading("Spend", t)),
        );

    let rewrites = ws
        .runs
        .iter()
        .flat_map(|run| &run.items)
        .filter(|item| matches!(item, Item::Rewrite { .. }))
        .count();
    let plugin_cost: f64 =
        ws.runs.iter().map(|run| run.usage.plugin_cost).sum();
    let legend = div()
        .flex()
        .flex_wrap()
        .gap(px(16.))
        .text_size(px(12.))
        .text_color(t.muted)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(dot(t.blue, 10.))
                .child("uses the seam"),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(
                    div()
                        .size(px(10.))
                        .rounded(px(5.))
                        .border_1()
                        .border_color(t.border_strong),
                )
                .child("default no-op"),
        )
        .child("Pick a plugin to see its work.");
    let facts = div()
        .grid()
        .grid_cols(if compact { 1 } else { 3 })
        .gap(px(12.))
        .child(fact("Settings fixed at start", "Only `start` may change settings, through the `RunPlan`. After that, every turn is a delta.", t))
        .child(fact("Context rewrites", &format!("{rewrites} stored rewrites across these runs. Each one costs a single full resend."), t))
        .child(fact("Plugins pay their own way", &format!("{} charged by plugins, counted in each run's limits.", usd(plugin_cost)), t));

    let main = div()
        .flex_1()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap(px(18.))
        .child(ui::screen_title(
            format!("Plugins on {}", catalog.agent),
            format!(
                "In registration order{}. The loop asks them in this order at every seam, and the first rewrite at a turn boundary wins.",
                catalog.agent_source.as_ref().map_or(String::new(), |source| format!(", from {source}"))
            ),
            t,
        ))
        .child(ui::card(t).when(!compact, |card| card.child(header)).children(rows))
        .child(legend)
        .child(facts);

    let side = div()
        .flex()
        .flex_col()
        .gap(px(20.))
        .when(!compact, |side| side.w(px(300.)).flex_shrink_0())
        .children(catalog.jev.as_ref().map(|jev| {
            div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(heading("tau-jev", t))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(dot(t.green, 8.))
                        .child(format!("Connected to {}", jev.model)),
                )
                .child(key_values(
                    [
                        ("key".into(), mono(jev.key_env.clone(), 12., t.text)),
                        ("price".into(), mono(jev.price.clone(), 12., t.text)),
                        (
                            "requests".into(),
                            mono(jev.requests.to_string(), 12., t.text),
                        ),
                        (
                            "input".into(),
                            mono(
                                format!("{} tokens", tokens(jev.input_tokens)),
                                12.,
                                t.text,
                            ),
                        ),
                        ("spent".into(), mono(usd(jev.spent), 12., t.text)),
                        (
                            "latency p50".into(),
                            mono(
                                format!("{} ms", jev.latency_p50_ms),
                                12.,
                                t.text,
                            ),
                        ),
                        (
                            "retried".into(),
                            mono(jev.retried.to_string(), 12., t.text),
                        ),
                    ],
                    t,
                ))
        }))
        .child(spend(&catalog.plugins, t));

    ui::screen(
        "plugins",
        compact,
        div()
            .flex()
            .when(compact, |layout| layout.flex_col())
            .gap(px(28.))
            .child(main)
            .child(side),
    )
    .into_any_element()
}

fn fact(title: &str, body: &str, t: &Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .p(px(14.))
        .border_1()
        .border_color(t.border)
        .rounded(px(8.))
        .child(
            div()
                .text_size(px(12.))
                .text_color(t.muted)
                .child(title.to_owned()),
        )
        .child(div().line_height(relative(1.5)).child(ui::rich(body, t)))
}

fn spend(plugins: &[PluginInfo], t: &Theme) -> impl IntoElement {
    let mut sorted: Vec<&PluginInfo> =
        plugins.iter().filter(|p| p.spend > 0.0).collect();
    sorted.sort_by(|a, b| b.spend.total_cmp(&a.spend));
    let top = sorted.first().map_or(1.0, |p| p.spend);
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(heading("Spend by plugin", t))
        .children(sorted.into_iter().map(|plugin| {
            div()
                .flex()
                .flex_col()
                .gap(px(5.))
                .child(
                    div()
                        .flex()
                        .child(mono(plugin.name.clone(), 12., t.text).flex_1())
                        .child(mono(usd(plugin.spend), 12., t.muted)),
                )
                .child(bar((plugin.spend / top) as f32, 6., t.blue, t.raised))
        }))
}
