//! Every stored run, in every repository, filterable, with a query box
//! for the store.

use gpui::{AnyElement, Context, SharedString, div, prelude::*, px};

use crate::{
    assets::Icon,
    route::Route,
    theme::{Design as _, IconSize, Theme, Type, radius, sp, weight},
    ui::{self, components::ButtonKind, icon, mono, status_icon, status_look},
    view::{Origin, RunView, tokens, usd},
    workspace::Workspace,
};

const COLUMNS: [(&str, f32, bool); 8] = [
    ("Run", 2.4, false),
    ("Repository", 1.2, false),
    ("Agent", 1.0, false),
    ("Model", 1.0, false),
    ("Started", 1.2, false),
    ("Turns", 0.6, true),
    ("Tokens", 0.8, true),
    ("Cost", 0.8, true),
];

pub fn render(
    ws: &Workspace,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let filter = ws.history_filter.read(cx).text().to_lowercase();
    let matches = |run: &RunView| {
        filter.is_empty()
            || [&run.title, &run.agent, &run.model, &run.repo]
                .iter()
                .any(|field| field.to_lowercase().contains(&filter))
            || status_look(&run.status, t)
                .1
                .to_lowercase()
                .contains(&filter)
    };
    // Roots first, each followed by its forks and sub-agents.
    let mut rows: Vec<(&RunView, bool)> = Vec::new();
    for run in ws.runs.iter().filter(|run| run.origin == Origin::Root) {
        rows.push((run, false));
        rows.extend(
            ws.runs
                .iter()
                .filter(|child| child.origin.parent() == Some(&run.id))
                .map(|child| (child, true)),
        );
    }
    let rows: Vec<_> =
        rows.into_iter().filter(|(run, _)| matches(run)).collect();

    let total: f64 = ws.runs.iter().map(|run| run.usage.cost).sum();
    let forks = ws
        .runs
        .iter()
        .filter(|run| matches!(run.origin, Origin::Fork { .. }))
        .count();
    let sub_agents: usize = ws
        .runs
        .iter()
        .flat_map(|run| &run.children)
        .filter(|child| child.kind == crate::view::ChildKind::SubAgent)
        .count();
    let summary = format!(
        "{} across {} runs, {forks} forks and {sub_agents} sub-agent runs, \
         in {} ({}).",
        usd(total),
        ws.runs.len(),
        ws.catalog.store.path,
        ws.catalog.store.size
    );

    let search = div()
        .flex()
        .items_center()
        .gap(sp(2.))
        .h(px(36.))
        .px(sp(2.5))
        .when(compact, |search| search.w_full())
        .when(!compact, |search| search.w(px(320.)))
        .border_1()
        .border_color(t.border_strong)
        .rounded(radius::CONTROL)
        .typeset(Type::CAPTION)
        .child(icon(Icon::Search, IconSize::BASE, t.dim))
        .child(ws.history_filter.clone());

    let body = if compact {
        div()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(t.border)
            .children(rows.iter().map(|(run, nested)| {
                let (color, label) = status_look(&run.status, t);
                let route = Route::Run(run.id.clone());
                div()
                    .id(SharedString::from(format!("history-{}", run.id)))
                    .flex()
                    .items_center()
                    .gap(sp(3.))
                    .min_h(px(56.))
                    .pl(sp(if *nested { 6. } else { 0. }))
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
                                        "{} · {label} · {} turns",
                                        ws.repo_of(run),
                                        run.turn
                                    )),
                            ),
                    )
                    .child(mono(usd(run.usage.cost), Type::CAPTION, t.muted))
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.navigate(route.clone(), cx)
                    }))
            }))
            .into_any_element()
    } else {
        let header = COLUMNS.iter().fold(
            row(t).bg(t.panel).child(div().w(px(20.))).text_color(t.dim),
            |header, (label, grow, right)| {
                header.child(
                    cell(*grow, *right)
                        .typeset(Type::MICRO)
                        .child(label.to_uppercase()),
                )
            },
        );
        let header =
            header.child(cell(1.1, false).typeset(Type::MICRO).child("STOP"));
        ui::card(t)
            .child(header)
            .children(rows.iter().map(|(run, nested)| {
                let (color, label) = status_look(&run.status, t);
                let route = Route::Run(run.id.clone());
                let active = ws.route.run() == Some(&run.id);
                row(t)
                    .id(SharedString::from(format!("history-{}", run.id)))
                    .cursor_pointer()
                    .when(active, |row| row.bg(t.selected))
                    .hover(|style| style.bg(gpui::white().opacity(0.03)))
                    .child(div().w(px(20.)).child(status_icon(
                        run,
                        t,
                        IconSize::SMALL,
                    )))
                    .child(
                        cell(2.4, false)
                            .pl(sp(if *nested { 4. } else { 0. }))
                            .flex()
                            .items_center()
                            .gap(sp(1.5))
                            .when(*nested, |cell| {
                                cell.child(icon(
                                    if matches!(run.origin, Origin::Fork { .. })
                                    {
                                        Icon::Fork
                                    } else {
                                        Icon::SubAgent
                                    },
                                    IconSize::SMALL,
                                    t.blue,
                                ))
                            })
                            .child(
                                div()
                                    .truncate()
                                    .font_weight(weight::EMPHASIS)
                                    .child(run.title.clone()),
                            ),
                    )
                    .child(
                        cell(1.2, false)
                            .truncate()
                            .text_color(t.text_soft)
                            .child(ws.repo_of(run).to_owned()),
                    )
                    .child(
                        cell(1.0, false)
                            .text_color(t.text_soft)
                            .child(run.agent.clone()),
                    )
                    .child(cell(1.0, false).child(mono(
                        run.model.clone(),
                        Type::CAPTION,
                        t.muted,
                    )))
                    .child(cell(1.2, false).child(mono(
                        if run.started.is_empty() {
                            "—".into()
                        } else {
                            run.started.clone()
                        },
                        Type::CAPTION,
                        t.muted,
                    )))
                    .child(cell(0.6, true).child(mono(
                        run.turn.to_string(),
                        Type::CAPTION,
                        t.text,
                    )))
                    .child(cell(0.8, true).child(mono(
                        tokens(run.usage.tokens),
                        Type::CAPTION,
                        t.text,
                    )))
                    .child(cell(0.8, true).child(mono(
                        usd(run.usage.cost),
                        Type::CAPTION,
                        t.text,
                    )))
                    .child(cell(1.1, false).text_color(color).child(label))
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.navigate(route.clone(), cx)
                    }))
            }))
            .when(rows.is_empty(), |card| {
                card.child(ui::empty("No runs match.", t))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(3.))
                    .px(sp(4.))
                    .py(sp(3.))
                    .bg(t.panel)
                    .border_t_1()
                    .border_color(t.border)
                    .child(mono("SQL", Type::MICRO, t.dim))
                    .child(
                        mono(
                            ws.catalog.store.sample_query.clone(),
                            Type::CAPTION,
                            t.text_soft,
                        )
                        .flex_1()
                        .truncate(),
                    )
                    .child(
                        div()
                            .id("run-query")
                            .child(ui::button(
                                "Run query",
                                ButtonKind::Secondary,
                                t,
                            ))
                            .on_click(
                                cx.listener(|ws, _, _, cx| ws.run_query(cx)),
                            ),
                    ),
            )
            .into_any_element()
    };

    ui::screen(
        "history",
        compact,
        div()
            .flex()
            .flex_col()
            .gap(sp(4.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_end()
                    .gap(sp(3.))
                    .child(
                        div().flex_1().min_w(px(280.)).child(ui::screen_title(
                            "Run history",
                            summary,
                            t,
                        )),
                    )
                    .child(search),
            )
            .child(body),
    )
    .into_any_element()
}

fn row(t: &Theme) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(sp(3.))
        .px(sp(4.))
        .py(sp(2.5))
        .border_b_1()
        .border_color(t.border)
}

fn cell(grow: f32, right: bool) -> gpui::Div {
    div()
        .flex_grow()
        .flex_basis(gpui::relative(grow / 8.))
        .min_w(px(0.))
        .truncate()
        .when(right, |cell| cell.flex().justify_end())
}
