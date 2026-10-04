//! A repository's page: its header, and tabs for its runs and for what
//! plugins keep for it (memory, rules, MCP servers). The sidebar lists
//! only runs; the rest of a repository lives here.

use gpui::{AnyElement, Context, SharedString, div, prelude::*, rems};
use tau_agent::event::StopReason;

use crate::{
    attention::Attention,
    route::Route,
    theme::{Design as _, Theme, Type, radius, sp, weight},
    ui::{self, components::ButtonKind, mono, status_look},
    view::{Ending, Item, RunStatus, RunView, usd},
    workspace::Workspace,
};

/// One tab: its label, small print beside it, and where it leads.
struct RepoTab {
    label: String,
    detail: Option<String>,
    route: Route,
    /// The plugin whose page it is; none for Runs.
    plugin: Option<String>,
}

/// The repository's tabs: its runs, then each page a plugin lists for
/// it.
fn tabs(
    ws: &Workspace,
    repo: &str,
    cx: &mut Context<Workspace>,
) -> Vec<RepoTab> {
    let runs = ws.runs.iter().filter(|run| ws.repo_of(run) == repo).count();
    let entries = ws.contributions(
        tau_ui_plugin::points::SIDEBAR_REPO,
        &tau_ui_plugin::points::AtRepo {
            repo: repo.to_owned(),
        },
        cx,
    );
    std::iter::once(RepoTab {
        label: "Runs".into(),
        detail: Some(runs.to_string()),
        route: Route::Repo(repo.to_owned()),
        plugin: None,
    })
    .chain(entries.into_iter().filter_map(|entry| {
        Some(RepoTab {
            route: ws.link_route(&entry.to, None)?,
            plugin: entry.to.plugin.clone(),
            label: entry.label,
            detail: entry.badge.map(|(count, _)| count).or(entry.detail),
        })
    }))
    .collect()
}

/// The repository whose page `route` is a tab of, if any: its Runs
/// tab, or a page a plugin lists for it.
pub fn owner(
    ws: &Workspace,
    route: &Route,
    cx: &mut Context<Workspace>,
) -> Option<String> {
    if let Route::Repo(repo) = route {
        return Some(repo.clone());
    }
    let repo = route.repo()?.to_owned();
    tabs(ws, &repo, cx)
        .iter()
        .any(|tab| &tab.route == route)
        .then_some(repo)
}

/// `body` under the repository's header and tabs, the tab for the
/// current route selected.
pub fn framed(
    ws: &Workspace,
    repo: &str,
    body: AnyElement,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let path = ws
        .catalog
        .repos
        .iter()
        .find(|known| known.name == repo)
        .map(|known| known.path.clone());
    let live = ws
        .runs
        .iter()
        .filter(|run| ws.repo_of(run) == repo && run.status.is_live())
        .count();
    let total = ws.runs.iter().filter(|run| ws.repo_of(run) == repo).count();
    let branch = ws
        .catalog
        .repos
        .iter()
        .find(|known| known.name == repo)
        .and_then(|known| known.trunk.clone())
        .unwrap_or_else(|| "main".to_owned());
    let new_in = repo.to_owned();
    let header = div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(sp(3.5))
        .px(sp(if compact { 4. } else { 9. }))
        .pt(sp(5.5))
        .child(tau_ui_kit::components::repo_mark(repo, 34., t))
        .child(
            div()
                .flex_1()
                .min_w(rems(0.))
                .flex()
                .flex_col()
                .child(
                    div()
                        .typeset(Type::HEADING)
                        .font_weight(weight::STRONG)
                        .child(repo.to_owned()),
                )
                .child(
                    div()
                        .flex()
                        .gap(sp(1.5))
                        .typeset(Type::CAPTION)
                        .text_color(t.muted)
                        .children(path.map(|path| {
                            mono(path, Type::CAPTION, t.dim).truncate()
                        }))
                        .child("· on")
                        .child(mono(branch, Type::CAPTION, t.roles.branch))
                        .when(live > 0, |line| {
                            line.child("·").child(
                                div()
                                    .text_color(t.roles.live)
                                    .child(format!("{live} running")),
                            )
                        })
                        .child(format!("· {total} runs")),
                ),
        )
        .child(
            div()
                .id("repo-new-run")
                .child(ui::button("New run", ButtonKind::Primary, t))
                .on_click(cx.listener(move |ws, _, window, cx| {
                    ws.new_run_in(&new_in, window, cx)
                })),
        );
    let tab_bar = div()
        .flex()
        .flex_shrink_0()
        .gap(sp(6.))
        .px(sp(if compact { 4. } else { 9. }))
        .pt(sp(4.5))
        .border_b_1()
        .border_color(t.border)
        .children(tabs(ws, repo, cx).into_iter().enumerate().map(
            |(n, tab)| {
                let on = tab.route == ws.route;
                let route = tab.route.clone();
                div()
                    .id(SharedString::from(format!("repo-tab-{n}")))
                    .flex()
                    .gap(sp(1.5))
                    .pb(sp(2.5))
                    .cursor_pointer()
                    .text_color(if on { t.text } else { t.text_soft })
                    .when(on, |tab| {
                        tab.font_weight(weight::EMPHASIS)
                            .border_b_2()
                            .border_color(t.roles.primary)
                    })
                    .child(tab.label)
                    .children(tab.detail.map(|detail| {
                        let ink = tab
                            .plugin
                            .as_deref()
                            .and_then(|plugin| t.roles.plugin(plugin))
                            .unwrap_or(t.dim);
                        div().text_color(ink).child(detail)
                    }))
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.navigate(route.clone(), cx)
                    }))
            },
        ));
    div()
        .flex_1()
        .min_h(rems(0.))
        .flex()
        .flex_col()
        .child(header)
        .child(tab_bar)
        .child(div().flex_1().min_h(rems(0.)).flex().flex_col().child(body))
        .into_any_element()
}

/// Which runs the Runs tab lists.
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

/// The widths of the list's columns after the run's: agent, turns,
/// spent, changes, updated.
const COLUMNS: [(&str, f32); 5] = [
    ("Agent", 90.),
    ("Turns", 64.),
    ("Spent", 76.),
    ("Changes", 84.),
    ("Updated", 110.),
];

/// The Runs tab: the repository's runs, newest first, filtered by what
/// they need.
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
        .map(|run| (run, ws.attention(run, cx)))
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
    let chips = div().flex().flex_wrap().gap(sp(1.5)).children(
        RunsFilter::ALL.into_iter().map(|filter| {
            let on = ws.runs_filter == filter;
            div()
                .id(SharedString::from(format!("runs-filter-{filter:?}")))
                .flex()
                .gap(sp(1.5))
                .px(sp(2.75))
                .py(sp(1.5))
                .rounded(radius::BOX)
                .cursor_pointer()
                .text_color(if on { t.text } else { t.text_soft })
                .when(on, |chip| chip.bg(t.raised))
                .when(!on, |chip| chip.hover(|style| style.bg(t.card)))
                .child(filter.label())
                .child(
                    div()
                        .text_color(filter.ink(t))
                        .child(count(filter).to_string()),
                )
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.runs_filter = filter;
                    cx.notify();
                }))
        }),
    );
    let spent: f64 = runs.iter().map(|(run, _)| run.usage.cost).sum();
    let heading = div()
        .flex()
        .gap(sp(3.5))
        .px(sp(1.))
        .pb(sp(2.))
        .border_b_1()
        .border_color(t.border)
        .typeset(Type::CAPTION)
        .text_color(t.dim)
        .child(div().w(rems(0.4375)))
        .child(div().flex_1().child("Run"))
        .children(COLUMNS.iter().enumerate().map(|(n, (label, width))| {
            div()
                .w(rems((*width) / 16.))
                .when(n > 0, |cell| cell.text_right())
                .child(*label)
        }));
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
        .child(
            div()
                .flex()
                .items_center()
                .child(chips)
                .child(div().flex_1())
                .child(
                    div()
                        .flex()
                        .gap(sp(1.5))
                        .typeset(Type::CAPTION)
                        .text_color(t.muted)
                        .child("spent")
                        .child(
                            div().text_color(t.roles.cost).child(usd(spent)),
                        ),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .child(heading)
                .when(rows.is_empty(), |list| {
                    list.child(ui::empty("No runs here.", t))
                })
                .children(rows),
        );
    let body = ui::screen(
        "repo-runs",
        compact,
        div().w_full().flex().justify_center().child(list),
    )
    .into_any_element();
    framed(ws, repo, body, compact, t, cx)
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

/// A run in the list: its state, title and task, agent, turns, cost,
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
        |width: f32| div().w(rems((width) / 16.)).flex_shrink_0().text_right();
    div()
        .id(SharedString::from(format!("repo-run-{}", run.id)))
        .flex()
        .items_center()
        .gap(sp(3.5))
        .px(sp(1.))
        .py(sp(2.75))
        .border_b_1()
        .border_color(t.border)
        .cursor_pointer()
        .hover(|row| row.bg(t.card))
        .child(
            div()
                .size(rems(0.4375))
                .flex_shrink_0()
                .rounded(radius::FULL)
                .bg(color),
        )
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
                .w(rems((COLUMNS[0].1) / 16.))
                .flex_shrink_0()
                .typeset(Type::CAPTION)
                .text_color(t.roles.agent)
                .child(run.agent.clone()),
        )
        .child(cell(COLUMNS[1].1).child(mono(
            match run.limits.max_turns {
                Some(max) => format!("{}/{max}", run.turn),
                None => run.turn.to_string(),
            },
            Type::CAPTION,
            t.text_soft,
        )))
        .child(cell(COLUMNS[2].1).child(mono(
            usd(run.usage.cost),
            Type::CAPTION,
            t.roles.cost,
        )))
        .child(cell(COLUMNS[3].1).child(mono(
            changes.map_or(String::new(), |n| match n {
                1 => "1 change".into(),
                n => format!("{n} changes"),
            }),
            Type::CAPTION,
            t.green,
        )))
        .child(
            cell(COLUMNS[4].1)
                .typeset(Type::CAPTION)
                .text_color(t.dim)
                .child(run.started.clone()),
        )
        .on_click(
            cx.listener(move |ws, _, _, cx| ws.navigate(route.clone(), cx)),
        )
}
