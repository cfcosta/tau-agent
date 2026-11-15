//! A repository's page: its runs, as ruled rows. What plugins keep for
//! the repository (memory, rules, MCP servers) is listed under it in the
//! sidebar's tree; each of those pages is drawn here under its title.

use gpui::{AnyElement, Context, SharedString, div, prelude::*, rems};
use heiko_agent::event::StopReason;
use heiko_ui_kit::assets::State;

use crate::{
    attention::Attention,
    route::Route,
    theme::{Design as _, Theme, Type, sp, weight},
    ui::{self, components::BlockSize, mono, status_look},
    view::{Ending, Item, RunStatus, RunView},
    workspace::Workspace,
};

/// A page a plugin lists for `repo`: what the sidebar calls it, and
/// where it leads.
fn pages(ws: &Workspace, repo: &str) -> Vec<(String, Route)> {
    ws.contributions(
        heiko_ui_plugin::points::SIDEBAR_REPO,
        &heiko_ui_plugin::points::AtRepo {
            repo: repo.to_owned(),
        },
    )
    .into_iter()
    .filter_map(|entry| Some((entry.label, ws.link_route(&entry.to, None)?)))
    .collect()
}

/// The repository `route` is a page of, if any: its runs, or a page a
/// plugin lists for it.
pub fn owner(ws: &Workspace, route: &Route) -> Option<String> {
    if let Route::Repo(repo) = route {
        return Some(repo.clone());
    }
    let repo = route.repo()?.to_owned();
    pages(ws, &repo)
        .iter()
        .any(|(_, page)| page == route)
        .then_some(repo)
}

/// The room at a page's sides.
fn gutter(compact: bool) -> gpui::Rems {
    sp(if compact { 4. } else { 8. })
}

/// `body`, a page a plugin lists for `repo`, under its title: what the
/// sidebar calls the page, with the repository's name beside it.
pub fn framed(
    ws: &Workspace,
    repo: &str,
    body: AnyElement,
    compact: bool,
    t: &Theme,
    _: &mut Context<Workspace>,
) -> AnyElement {
    let title = pages(ws, repo)
        .into_iter()
        .find(|(_, page)| page == &ws.route)
        .map(|(label, _)| label);
    div()
        .flex_1()
        .min_h(rems(0.))
        .flex()
        .flex_col()
        .children(title.map(|title| {
            div().flex_shrink_0().px(gutter(compact)).pt(sp(6.)).child(
                ui::section_header(title, Some(repo.to_owned().into()), t),
            )
        }))
        .child(div().flex_1().min_h(rems(0.)).flex().flex_col().child(body))
        .into_any_element()
}

/// Which runs the repository's page lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunsFilter {
    #[default]
    All,
    Running,
    /// Waiting on the person: to land, or asking.
    ToLand,
    Failing,
    Done,
}

impl RunsFilter {
    const ALL: [Self; 5] = [
        Self::All,
        Self::Running,
        Self::ToLand,
        Self::Failing,
        Self::Done,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Running => "Running",
            Self::ToLand => "To land",
            Self::Failing => "Failing",
            Self::Done => "Done",
        }
    }

    /// The filter a run's state puts it under: what it needs, else how
    /// it stopped.
    fn of(run: &RunView, attention: &Attention) -> Self {
        match attention {
            Attention::Working { .. } => Self::Running,
            Attention::Asks { .. }
            | Attention::ReadyToLand { .. }
            | Attention::Queued(_) => Self::ToLand,
            Attention::Failed
            | Attention::WouldConflict { .. }
            | Attention::ConflictsOnMain { .. }
            | Attention::Interrupted => Self::Failing,
            Attention::Idle
                if matches!(
                    run.status,
                    RunStatus::Finished(
                        StopReason::Limit(_) | StopReason::Error(_)
                    )
                ) =>
            {
                Self::Failing
            }
            Attention::Landed | Attention::Dropped | Attention::Idle => {
                Self::Done
            }
        }
    }

    fn ink(self, t: &Theme) -> gpui::Hsla {
        match self {
            Self::All => t.dim,
            Self::Running => t.roles.live,
            Self::ToLand => t.roles.waiting,
            Self::Failing => t.red,
            Self::Done => t.green,
        }
    }
}

/// The widths of a row's columns after the run's: agent, turns,
/// changes, started.
const COLUMNS: [f32; 4] = [90., 64., 84., 110.];

/// The repository's runs, newest first, filtered by what they need.
pub fn render(
    ws: &Workspace,
    repo: &str,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let mut runs: Vec<(&RunView, Attention)> = ws
        .runs
        .iter()
        .filter(|run| ws.repo_of(run) == repo)
        .map(|run| (run, ws.attention(run)))
        .collect();
    runs.reverse();
    let count = |filter: RunsFilter| {
        runs.iter()
            .filter(|(run, attention)| {
                filter == RunsFilter::All
                    || RunsFilter::of(run, attention) == filter
            })
            .count()
    };
    let filters = div()
        .flex()
        .flex_wrap()
        .items_baseline()
        .gap(sp(5.))
        .children(RunsFilter::ALL.into_iter().map(|filter| {
            let on = ws.runs_filter == filter;
            div()
                .id(SharedString::from(format!("runs-filter-{filter:?}")))
                .flex()
                .items_baseline()
                .gap(sp(1.5))
                .pb(sp(0.5))
                .border_b_1()
                .border_color(if on { t.rule } else { t.hairline })
                .typeset(Type::SMALL)
                .cursor_pointer()
                .text_color(if on { t.bright } else { t.text_soft })
                .when(on, |word| word.font_weight(weight::EMPHASIS))
                .child(filter.label())
                .child(mono(
                    count(filter).to_string(),
                    Type::CAPTION,
                    filter.ink(t),
                ))
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.runs_filter = filter;
                    cx.notify();
                }))
        }));
    let known = ws.catalog.repos.iter().find(|known| known.name == repo);
    let branch = known
        .and_then(|known| known.trunk.clone())
        .unwrap_or_else(|| "main".to_owned());
    let place = match known {
        Some(known) => format!("{} · {branch}", known.path),
        None => branch,
    };
    let actions = ws.drawn(
        heiko_ui_plugin::points::REPO_ACTIONS,
        &heiko_ui_plugin::points::AtRepo {
            repo: repo.to_owned(),
        },
        cx,
    );
    let new_in = repo.to_owned();
    let header = ui::section_header(repo.to_owned(), None, t)
        .items_center()
        .child(mono(place, Type::CAPTION, t.dim).truncate())
        .children(actions)
        .child(
            div()
                .id("repo-new-run")
                .child(ui::primary_block("New run", BlockSize::Small, t))
                .on_click(cx.listener(move |ws, _, window, cx| {
                    ws.new_run_in(&new_in, window, cx)
                })),
        );
    let rows: Vec<AnyElement> = runs
        .iter()
        .filter(|(run, attention)| {
            ws.runs_filter == RunsFilter::All
                || RunsFilter::of(run, attention) == ws.runs_filter
        })
        .map(|(run, attention)| {
            run_row(run, attention, t, cx).into_any_element()
        })
        .collect();
    let list = div()
        .w_full()
        .max_w(rems(65.))
        .flex()
        .flex_col()
        .gap(sp(4.))
        .child(header)
        .child(filters)
        .child(
            div()
                .flex()
                .flex_col()
                .border_t_1()
                .border_color(t.hairline)
                .when(rows.is_empty(), |list| {
                    list.child(ui::empty("No runs here.", t))
                })
                .children(rows),
        );
    ui::screen(
        "repo-runs",
        compact,
        div().w_full().flex().justify_center().child(list),
    )
    .into_any_element()
}

/// What a run was asked to do: its first message, on one line.
pub(crate) fn task(run: &RunView) -> String {
    run.items
        .iter()
        .find_map(|item| match item {
            Item::User(text) => Some(
                text.split("\n\n<attached file=")
                    .next()
                    .unwrap_or(text)
                    .trim()
                    .replace('\n', " ")
                    .replace('`', ""),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

/// The mark a run's row shows: what it needs, else how it stopped.
fn state(run: &RunView, attention: &Attention) -> State {
    match attention {
        Attention::Working { .. } => State::Working,
        Attention::Asks { .. } => State::Asks,
        Attention::ReadyToLand { .. } | Attention::Landed => State::Met,
        Attention::Failed
        | Attention::WouldConflict { .. }
        | Attention::ConflictsOnMain { .. } => State::Failing,
        Attention::Queued(_) | Attention::Interrupted | Attention::Dropped => {
            State::Idle
        }
        Attention::Idle => match run.status {
            RunStatus::Finished(StopReason::Limit(_)) => State::Limit,
            RunStatus::Finished(StopReason::Error(_)) => State::Failing,
            _ => State::Idle,
        },
    }
}

/// A run in the list: its state's mark, title and task, agent, turns,
/// changes and when it started.
fn run_row(
    run: &RunView,
    attention: &Attention,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let filter = RunsFilter::of(run, attention);
    let (color, label) = match attention {
        Attention::Idle => status_look(&run.status, t),
        _ => (filter.ink(t), attention.word().into()),
    };
    // A run that stopped on a limit or an error reads as failing.
    let color = if filter == RunsFilter::Failing {
        t.red
    } else {
        color
    };
    let changes = match (&run.ending, &run.forecast) {
        (Some(Ending::Landed { changes, .. }), _) => Some(*changes),
        (_, Some(forecast)) => Some(forecast.changes),
        _ => None,
    };
    let route = Route::Run(run.id.clone());
    let cell =
        |width: f32| div().w(rems(width / 16.)).flex_shrink_0().text_right();
    div()
        .id(SharedString::from(format!("repo-run-{}", run.id)))
        .flex()
        .items_center()
        .gap(sp(3.5))
        .px(sp(1.))
        .py(sp(2.75))
        .border_b_1()
        .border_color(t.hairline)
        .cursor_pointer()
        .hover(|row| row.bg(t.card))
        .child(ui::mark(state(run, attention), t))
        .child(
            div()
                .flex_1()
                .min_w(rems(0.))
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(sp(2.5))
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
                                .child(label),
                        ),
                )
                .child(
                    div()
                        .truncate()
                        .typeset(Type::CAPTION)
                        .text_color(t.muted)
                        .child(task(run)),
                ),
        )
        .child(
            div()
                .w(rems(COLUMNS[0] / 16.))
                .flex_shrink_0()
                .typeset(Type::CAPTION)
                .text_color(t.roles.agent)
                .child(run.agent.clone()),
        )
        .child(cell(COLUMNS[1]).child(mono(
            match run.limits.max_turns {
                Some(max) => format!("{}/{max}", run.turn),
                None => run.turn.to_string(),
            },
            Type::CAPTION,
            t.text_soft,
        )))
        .child(cell(COLUMNS[2]).child(mono(
            changes.map_or(String::new(), |n| match n {
                1 => "1 change".into(),
                n => format!("{n} changes"),
            }),
            Type::CAPTION,
            t.green,
        )))
        .child(
            cell(COLUMNS[3])
                .typeset(Type::CAPTION)
                .text_color(t.dim)
                .child(run.started.clone()),
        )
        .on_click(
            cx.listener(move |ws, _, _, cx| ws.navigate(route.clone(), cx)),
        )
}
