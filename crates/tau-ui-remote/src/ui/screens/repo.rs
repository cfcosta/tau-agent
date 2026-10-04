//! A repository's page: its header, and tabs for its runs and for what
//! plugins keep for it (memory, rules, MCP servers). The sidebar lists
//! only runs; the rest of a repository lives here.

use gpui::{AnyElement, Context, SharedString, div, prelude::*, px};

use crate::{
    route::Route,
    theme::{Design as _, Theme, Type, radius, sp, weight},
    ui::{self, components::ButtonKind, mono, status_look},
    view::{RunView, usd},
    workspace::Workspace,
};

/// One tab: its label, small print beside it, and where it leads.
struct RepoTab {
    label: String,
    detail: Option<String>,
    route: Route,
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
    })
    .chain(entries.into_iter().filter_map(|entry| {
        Some(RepoTab {
            route: ws.link_route(&entry.to, None)?,
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
                .min_w(px(0.))
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
                        .when(live > 0, |line| {
                            line.child("·").child(
                                div()
                                    .text_color(t.roles.live)
                                    .child(format!("{live} running")),
                            )
                        }),
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
    let tab_bar =
        div()
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
                            div().text_color(t.dim).child(detail)
                        }))
                        .on_click(cx.listener(move |ws, _, _, cx| {
                            ws.navigate(route.clone(), cx)
                        }))
                },
            ));
    div()
        .flex_1()
        .min_h(px(0.))
        .flex()
        .flex_col()
        .child(header)
        .child(tab_bar)
        .child(div().flex_1().min_h(px(0.)).flex().flex_col().child(body))
        .into_any_element()
}

/// The Runs tab: every run in the repository, newest first.
pub fn render(
    ws: &Workspace,
    repo: &str,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let mut runs: Vec<&RunView> = ws
        .runs
        .iter()
        .filter(|run| ws.repo_of(run) == repo)
        .collect();
    runs.reverse();
    let rows: Vec<AnyElement> = runs
        .iter()
        .map(|run| run_row(run, t, cx).into_any_element())
        .collect();
    let list = div()
        .w_full()
        .max_w(px(960.))
        .flex()
        .flex_col()
        .when(rows.is_empty(), |list| {
            list.child(ui::empty("No runs in this repository yet.", t))
        })
        .children(rows);
    let body = ui::screen(
        "repo-runs",
        compact,
        div().w_full().flex().justify_center().child(list),
    )
    .into_any_element();
    framed(ws, repo, body, compact, t, cx)
}

/// A run in the list: its status, title, agent, turns and cost.
fn run_row(
    run: &RunView,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let (color, label) = status_look(&run.status, t);
    let route = Route::Run(run.id.clone());
    div()
        .id(SharedString::from(format!("repo-run-{}", run.id)))
        .flex()
        .items_center()
        .gap(sp(3.5))
        .px(sp(1.))
        .py(sp(3.))
        .border_b_1()
        .border_color(t.border)
        .cursor_pointer()
        .hover(|row| row.bg(t.card))
        .child(
            div()
                .size(px(7.))
                .flex_shrink_0()
                .rounded(radius::FULL)
                .bg(color),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
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
                    div().typeset(Type::CAPTION).text_color(color).child(label),
                ),
        )
        .child(
            div()
                .w(px(90.))
                .typeset(Type::CAPTION)
                .text_color(t.roles.agent)
                .child(run.agent.clone()),
        )
        .child(
            mono(
                match run.limits.max_turns {
                    Some(max) => format!("{}/{max}", run.turn),
                    None => run.turn.to_string(),
                },
                Type::CAPTION,
                t.text_soft,
            )
            .w(px(60.))
            .text_right(),
        )
        .child(
            mono(usd(run.usage.cost), Type::CAPTION, t.roles.cost)
                .w(px(70.))
                .text_right(),
        )
        .on_click(
            cx.listener(move |ws, _, _, cx| ws.navigate(route.clone(), cx)),
        )
}
