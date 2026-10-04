//! Every stored run, in every repository, filterable, with a query box
//! for the store.

use gpui::{AnyElement, Context, SharedString, div, prelude::*, rems};

use crate::{
    assets::Icon,
    route::Route,
    theme::{Design as _, IconSize, Theme, Type, radius, sp, weight},
    ui::{self, components::ButtonKind, icon, mono, status_icon, status_look},
    view::{Origin, RunView, usd},
    workspace::Workspace,
};

pub fn render(
    ws: &Workspace,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let filter = ws.history_filter.read(cx).text().to_lowercase();
    let only = ws.history_repo.clone();
    let matches = |run: &RunView| {
        only.as_deref().is_none_or(|repo| ws.repo_of(run) == repo)
            && (filter.is_empty()
                || [&run.title, &run.agent, &run.model, &run.repo]
                    .iter()
                    .any(|field| field.to_lowercase().contains(&filter))
                || status_look(&run.status, t)
                    .1
                    .to_lowercase()
                    .contains(&filter))
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
        "Every run, in every repository: {} runs, {forks} forks and \
         {sub_agents} sub-agent runs, {} in all.",
        ws.runs.len(),
        usd(total),
    );

    let search = div()
        .flex()
        .items_center()
        .gap(sp(2.))
        .h(rems(2.25))
        .px(sp(2.5))
        .when(compact, |search| search.w_full())
        .when(!compact, |search| search.w(rems(20.)))
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
                    .min_h(rems(3.5))
                    .pl(sp(if *nested { 6. } else { 0. }))
                    .border_b_1()
                    .border_color(t.border)
                    .cursor_pointer()
                    .child(status_icon(run, t, IconSize::BASE))
                    .child(
                        div()
                            .flex_1()
                            .min_w(rems(0.))
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
        // The newest first, under the day they started.
        let mut newest = rows.clone();
        newest.sort_by_key(|(run, _)| std::cmp::Reverse(when(&run.started)));
        let mut days: Vec<(String, Vec<(&RunView, bool)>)> = Vec::new();
        for (run, nested) in &newest {
            let label = day(&run.started);
            match days.last_mut() {
                Some((last, runs)) if *last == label => {
                    runs.push((run, *nested))
                }
                _ => days.push((label, vec![(run, *nested)])),
            }
        }
        let names: Vec<Option<String>> = std::iter::once(None)
            .chain(ws.catalog.repos.iter().map(|repo| Some(repo.name.clone())))
            .collect();
        let mut chips = div().flex().flex_wrap().gap(sp(1.5));
        for name in names {
            chips = chips.child(repo_chip(ws, name, t, cx));
        }
        let mut list = div().flex().flex_col().child(chips.pb(sp(2.)));
        for (label, runs) in days {
            list = list.child(
                div()
                    .pt(sp(3.5))
                    .pb(sp(1.))
                    .px(sp(1.))
                    .typeset(Type::CAPTION)
                    .text_color(t.dim)
                    .child(label),
            );
            for (run, nested) in runs {
                list = list.child(history_row(ws, run, nested, t, cx));
            }
        }
        list.when(rows.is_empty(), |list| {
            list.child(ui::empty("No runs match.", t))
        })
        .child(
            div()
                .mt(sp(6.))
                .flex()
                .flex_col()
                .gap(sp(2.))
                .child(
                    div()
                        .typeset(Type::CAPTION)
                        .text_color(t.dim)
                        .child("Ask the store"),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(3.))
                        .child(
                            div()
                                .flex_1()
                                .min_w(rems(0.))
                                .child(ui::field(&ws.query, true, t)),
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
                                    cx.listener(|ws, _, _, cx| {
                                        ws.run_query(cx)
                                    }),
                                ),
                        ),
                )
                .children(
                    ws.query_result().map(|result| query_result(result, t)),
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
                        div()
                            .flex_1()
                            .min_w(rems(17.5))
                            .child(ui::screen_title("History", summary, t)),
                    )
                    .child(search),
            )
            .child(body),
    )
    .into_any_element()
}

/// The day a run started on, as a heading: Today, Yesterday, Sep 26.
fn day(started: &str) -> String {
    let mut words = started.split_whitespace();
    match words.next() {
        Some("today") => "Today".into(),
        Some("yesterday") => "Yesterday".into(),
        Some(month) => match words.next() {
            Some(date) => format!("{month} {date}"),
            None => month.to_owned(),
        },
        None => "Earlier".into(),
    }
}

/// A key that orders runs by when they started, from what the run
/// list shows: `today 14:02`, `yesterday 22:10`, `Sep 26 19:02`.
fn when(started: &str) -> (u8, u8, u8, String) {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct",
        "Nov", "Dec",
    ];
    let words: Vec<&str> = started.split_whitespace().collect();
    let clock = words.last().copied().unwrap_or_default().to_owned();
    match words.first().copied() {
        Some("today") => (2, 0, 0, clock),
        Some("yesterday") => (1, 0, 0, clock),
        Some(month) => {
            let month = MONTHS.iter().position(|m| *m == month).unwrap_or(0);
            let date = words.get(1).and_then(|d| d.parse().ok()).unwrap_or(0);
            (0, month as u8 + 1, date, clock)
        }
        None => (0, 0, 0, clock),
    }
}

/// When in its day a run started: `14:02`.
fn time(started: &str) -> String {
    started
        .split_whitespace()
        .last()
        .unwrap_or_default()
        .to_owned()
}

/// A chip that shows one repository's runs, or all of them.
fn repo_chip(
    ws: &Workspace,
    repo: Option<String>,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let on = ws.history_repo == repo;
    let count = ws
        .runs
        .iter()
        .filter(|run| {
            repo.as_deref().is_none_or(|repo| ws.repo_of(run) == repo)
        })
        .count();
    let ink = repo.as_deref().map_or(t.text, |repo| t.mark(repo));
    let label = repo.clone().unwrap_or_else(|| "All repositories".into());
    div()
        .id(SharedString::from(format!("history-repo-{label}")))
        .flex()
        .gap(sp(1.5))
        .px(sp(2.75))
        .py(sp(1.5))
        .rounded(radius::BOX)
        .cursor_pointer()
        .when(on, |chip| chip.bg(t.raised))
        .when(!on, |chip| chip.hover(|style| style.bg(t.card)))
        .child(div().text_color(if on { t.text } else { ink }).child(label))
        .child(div().text_color(t.dim).child(count.to_string()))
        .on_click(cx.listener(move |ws, _, _, cx| {
            ws.history_repo = repo.clone();
            cx.notify();
        }))
}

/// A run in the history: its repository's mark, title, task, state,
/// cost and the time it started.
fn history_row(
    ws: &Workspace,
    run: &RunView,
    nested: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let (color, label) = status_look(&run.status, t);
    let route = Route::Run(run.id.clone());
    let repo = ws.repo_of(run).to_owned();
    div()
        .id(SharedString::from(format!("history-{}", run.id)))
        .flex()
        .items_center()
        .gap(sp(3.5))
        .px(sp(1.))
        .py(sp(2.25))
        .border_b_1()
        .border_color(t.border)
        .cursor_pointer()
        .hover(|row| row.bg(t.card))
        .child(tau_ui_kit::components::repo_mark(&repo, 18., t))
        .child(
            div()
                .w(rems(11.875))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(sp(1.5))
                .when(nested, |cell| {
                    cell.pl(sp(3.)).child(icon(
                        if matches!(run.origin, Origin::Fork { .. }) {
                            Icon::Fork
                        } else {
                            Icon::SubAgent
                        },
                        IconSize::SMALL,
                        t.roles.agent,
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
            div()
                .flex_1()
                .min_w(rems(0.))
                .truncate()
                .typeset(Type::CAPTION)
                .text_color(t.muted)
                .child(super::repo::task(run)),
        )
        .child(
            div()
                .w(rems(6.875))
                .flex_shrink_0()
                .typeset(Type::CAPTION)
                .text_color(color)
                .child(label),
        )
        .child(
            mono(usd(run.usage.cost), Type::CAPTION, t.roles.cost)
                .w(rems(4.375))
                .flex_shrink_0()
                .text_right(),
        )
        .child(
            div()
                .w(rems(3.75))
                .flex_shrink_0()
                .text_right()
                .typeset(Type::CAPTION)
                .text_color(t.dim)
                .child(time(&run.started)),
        )
        .on_click(
            cx.listener(move |ws, _, _, cx| ws.navigate(route.clone(), cx)),
        )
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

/// What the query returned: a table, or SQLite's reason.
fn query_result(
    result: &Result<tau_store::Table, String>,
    t: &Theme,
) -> gpui::Div {
    let table = match result {
        Err(error) => {
            return div().px(sp(4.)).py(sp(3.)).child(ui::notice(
                Icon::Warning,
                error.clone(),
                t.red,
                Type::SMALL,
                t,
            ));
        }
        Ok(table) => table,
    };
    let columns = table.columns.len().max(1) as f32;
    let line = |cells: &[String], color| {
        row(t).children(cells.iter().map(|cell| {
            mono(cell.clone(), Type::CAPTION, color)
                .flex_basis(gpui::relative(1. / columns))
                .flex_grow(1.)
                .min_w(rems(0.))
                .truncate()
        }))
    };
    div()
        .flex()
        .flex_col()
        .child(line(&table.columns, t.dim))
        .children(table.rows.iter().map(|cells| line(cells, t.text)))
        .child(
            div()
                .px(sp(4.))
                .py(sp(2.))
                .typeset(Type::CAPTION)
                .text_color(t.dim)
                .child(match (table.rows.len(), table.truncated) {
                    (0, _) => "No rows.".to_owned(),
                    (n, true) => format!("The first {n} rows."),
                    (1, false) => "1 row.".to_owned(),
                    (n, false) => format!("{n} rows."),
                }),
        )
}
