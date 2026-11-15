//! Every stored run, in every repository, filterable, with a query box
//! for the store.

use gpui::{AnyElement, Context, SharedString, div, prelude::*, rems};
use heiko_agent::event::StopReason;

use crate::{
    attention::{day, time, when},
    route::Route,
    theme::{Design as _, Theme, Type, sp, weight},
    ui::{
        self,
        SquareStyle,
        State,
        components::ButtonKind,
        mark,
        mono,
        status_look,
    },
    view::{Origin, RunStatus, RunView},
    workspace::Workspace,
};

/// The mark a run's status is listed with: at work, stopped at a limit,
/// stopped short, or waiting on the person.
fn state(status: &RunStatus) -> State {
    match status {
        RunStatus::Planning | RunStatus::Running => State::Working,
        RunStatus::Finished(StopReason::Limit(_)) => State::Limit,
        RunStatus::Finished(StopReason::Cancelled | StopReason::Error(_)) => {
            State::Failing
        }
        RunStatus::Finished(StopReason::Stop) | RunStatus::Interrupted => {
            State::Idle
        }
    }
}

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
        "{} runs · {forks} forks · {sub_agents} sub-agent runs",
        ws.runs.len(),
    );

    // What is typed filters as it is typed: a line to write on, with no
    // box.
    let search = div()
        .flex_shrink_0()
        .pb(sp(0.75))
        .when(compact, |search| search.w_full())
        .when(!compact, |search| search.w(rems(20.)))
        .border_b_1()
        .border_color(t.slate)
        .typeset(Type::LEAD)
        .child(ws.history_filter.clone());

    let body = if compact {
        div()
            .flex()
            .flex_col()
            .child(search.mb(sp(2.)))
            .children(rows.iter().map(|(run, nested)| {
                let (color, label) = status_look(&run.status, t);
                let route = Route::Run(run.id.clone());
                div()
                    .id(SharedString::from(format!("history-{}", run.id)))
                    .flex()
                    .items_center()
                    .gap(sp(3.))
                    .min_h(rems(3.25))
                    .pl(sp(if *nested { 5. } else { 0. }))
                    .border_b_1()
                    .border_color(t.hairline)
                    .cursor_pointer()
                    .child(mark(state(&run.status), t))
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
                                    .font_weight(weight::STRONG)
                                    .text_color(t.bright)
                                    .child(run.title.clone()),
                            )
                            .child(
                                mono(
                                    format!(
                                        "{} · {label} · {} turns",
                                        ws.repo_of(run),
                                        run.turn
                                    ),
                                    Type::CAPTION,
                                    color,
                                )
                                .truncate(),
                            ),
                    )
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.navigate(route.clone(), cx)
                    }))
            }))
            .when(rows.is_empty(), |list| {
                list.child(ui::empty("No runs match.", t))
            })
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
            .chain(ws.catalog.listed().map(|repo| Some(repo.name.clone())))
            .collect();
        let mut filters = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(sp(5.))
            .gap_y(sp(2.))
            .py(sp(3.))
            .border_b_1()
            .border_color(t.slate);
        for name in names {
            filters = filters.child(repo_word(ws, name, t, cx));
        }
        let mut list = div()
            .flex()
            .flex_col()
            .child(filters.child(div().flex_1()).child(search));
        for (label, runs) in days {
            let last = runs.len().saturating_sub(1);
            for (n, (run, nested)) in runs.into_iter().enumerate() {
                // The day is said once, on its first run; a hairline
                // closes it under its last.
                let said = if n == 0 { label.as_str() } else { "" };
                list = list.child(history_row(
                    ws,
                    run,
                    nested,
                    said,
                    n == last,
                    t,
                    cx,
                ));
            }
        }
        list.when(rows.is_empty(), |list| {
            list.child(ui::empty("No runs match.", t))
        })
        .child(
            div()
                .mt(sp(11.))
                .flex()
                .flex_col()
                .gap(sp(3.))
                .child(ui::section_header("Ask the store", None, t))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(4.))
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
                                    "run query",
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

    // The screen stands in from the sidebar as a chat's header does.
    div()
        .id("history")
        .flex_1()
        .min_h(rems(0.))
        .overflow_y_scroll()
        .child(
            div()
                .flex()
                .flex_col()
                .when(compact, |page| page.px(sp(4.5)).py(sp(4.)))
                .when(!compact, |page| {
                    page.pl(sp(13.)).pr(sp(11.)).pt(sp(11.)).pb(sp(11.))
                })
                .when(!compact, |page| {
                    page.child(ui::section_header(
                        "History",
                        Some(summary.into()),
                        t,
                    ))
                })
                .child(body),
        )
        .into_any_element()
}

/// A word that shows one repository's runs, or all of them: underlined
/// as a thing to choose, in the repository's color, with how many it
/// has. The one chosen is bright over a blue line.
fn repo_word(
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
        .items_baseline()
        .gap(sp(1.5))
        .flex_shrink_0()
        .border_b_1()
        .border_color(if on { t.blue } else { t.slate })
        .typeset(Type::LEAD)
        .cursor_pointer()
        .child(
            div()
                .text_color(if on { t.bright } else { ink })
                .child(label),
        )
        .child(mono(count.to_string(), Type::CAPTION, t.dim))
        .on_click(cx.listener(move |ws, _, _, cx| {
            ws.history_repo = repo.clone();
            cx.notify();
        }))
}

/// A run in the history: the day it started, when that is new; its
/// repository's letter in a square of its color; whether it is a fork
/// or a sub-agent; its title and task; its state; and the time it
/// started. The `last` of a day closes it with a hairline.
fn history_row(
    ws: &Workspace,
    run: &RunView,
    nested: bool,
    day: &str,
    last: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let (color, label) = status_look(&run.status, t);
    let route = Route::Run(run.id.clone());
    let repo = ws.repo_of(run).to_owned();
    let kind = match (&run.origin, nested) {
        (Origin::Fork { .. }, true) => "fork",
        (Origin::SubAgent { .. }, true) => "sub",
        _ => "",
    };
    div()
        .id(SharedString::from(format!("history-{}", run.id)))
        .flex()
        .items_center()
        .gap(sp(3.))
        .h(rems(2.375))
        .border_b_1()
        .border_color(if last {
            t.hairline
        } else {
            gpui::transparent_black()
        })
        .cursor_pointer()
        .hover(|row| row.text_color(t.bright))
        .child(
            div()
                .w(rems(4.75))
                .flex_shrink_0()
                .typeset(Type::SECTION.sized(13.5))
                .text_color(t.bright)
                .child(day.to_owned()),
        )
        .child(ui::square(
            ws.repo_named(&repo).letter(),
            SquareStyle::Outlined(t.mark(&repo)),
            t,
        ))
        .child(
            mono(kind, Type::MICRO, t.dim)
                .w(rems(2.))
                .flex_shrink_0()
                .whitespace_nowrap(),
        )
        .child(
            div()
                .w(rems(8.25))
                .flex_shrink_0()
                .truncate()
                .font_weight(weight::STRONG)
                .text_color(t.bright)
                .child(run.title.clone()),
        )
        .child(
            div()
                .flex_1()
                .min_w(rems(0.))
                .truncate()
                .text_color(t.text_soft)
                .child(super::repo::task(run)),
        )
        .child(
            div()
                .w(rems(6.5))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(sp(1.75))
                .typeset(Type::SMALL)
                .text_color(color)
                .child(mark(state(&run.status), t))
                .child(div().truncate().child(label)),
        )
        .child(
            mono(time(&run.started), Type::CAPTION, t.dim)
                .w(rems(2.75))
                .flex_shrink_0()
                .text_right(),
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
        .py(sp(2.))
        .border_b_1()
        .border_color(t.hairline)
}

/// What the query returned: a table, or SQLite's reason.
fn query_result(
    result: &Result<heiko_store::Table, String>,
    t: &Theme,
) -> gpui::Div {
    let table = match result {
        Err(error) => {
            return div().py(sp(3.)).child(ui::notice(
                crate::assets::Icon::Warning,
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
        .child(line(&table.columns, t.dim).border_color(t.slate))
        .children(table.rows.iter().map(|cells| line(cells, t.text)))
        .child(
            div()
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
