//! Everything around the screens: the sidebar's rail and tree, the
//! status bar, and their phone versions.
//!
//! The sidebar is a rail of square tiles (Home, one per repository, one
//! per agent) beside the tree of the one chosen: a repository's chats
//! and what plugins keep for it, an agent's conversation, or the list of
//! agents. A row says where it stands with marks alone, packed at its
//! right edge in one order ([`Marks::shown`]).

use gpui::{
    Anchor,
    AnyElement,
    Context,
    Div,
    Hsla,
    IntoElement,
    SharedString,
    Stateful,
    anchored,
    deferred,
    div,
    point,
    prelude::*,
    px,
    rems,
};
use heiko_agent::event::StopReason;
use heiko_ui_plugin::{
    KindPlace,
    Link,
    NavEntry,
    RepoKind,
    points::{self, AtApp, AtRepo},
};

use super::{
    Material,
    State,
    counted_mark,
    hot_action,
    icon,
    icon_button,
    link_action,
    logo,
    mark,
    mono,
    status_look,
    text_link,
};
use crate::{
    assets::Icon,
    attention::{Attention, Marks, count},
    catalog::{ProjectStatus, Repo},
    repos::{RepoRows, TreeOf, TreeRow},
    route::{Route, Tab},
    theme::{
        Design as _,
        IconSize,
        MONO,
        Theme,
        Tone,
        Type,
        radius,
        sp,
        weight,
    },
    view::{ChildRun, Origin, RunStatus, RunView},
    workspace::Workspace,
};

/// How wide the sidebar's rail is, in rems: 60 px.
const RAIL: f32 = 3.75;

/// The desktop sidebar: the rail of tiles, and beside it the tree of
/// the one chosen, over what is the same everywhere.
pub fn sidebar(ws: &Workspace, t: &Theme, cx: &mut Context<Workspace>) -> Div {
    let tree = match ws.tree_of() {
        Some(TreeOf::Repo(name)) => repo_tree(ws, &name, t, cx),
        Some(TreeOf::Agent(name)) => agent_tree(ws, &name, t, cx),
        Some(TreeOf::Kind(plugin, kind)) => kind_tree(ws, plugin, &kind, t, cx),
        None => vec![
            div()
                .px(sp(2.))
                .py(sp(2.))
                .typeset(Type::CAPTION)
                .text_color(t.muted)
                .child("No repositories yet.")
                .into_any_element(),
        ],
    };
    div()
        .flex()
        .bg(t.panel)
        .border_r_1()
        .border_color(t.hairline)
        .child(rail(ws, t, cx))
        .child(
            div()
                .id("sidebar")
                .flex_1()
                .min_w(rems(0.))
                .h_full()
                .flex()
                .flex_col()
                .pt(sp(3.5))
                .pb(sp(3.5))
                .pl(sp(1.5))
                .pr(sp(1.))
                .overflow_y_scroll()
                .children(tree)
                .child(div().flex_1().min_h(sp(4.5)))
                .children(foot(ws, t, cx)),
        )
}

/// A row's marks, packed at its right edge in their one order.
pub fn marks(marks: Marks, t: &Theme) -> Option<Div> {
    let shown = marks.shown();
    (!shown.is_empty()).then(|| {
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(sp(1.5))
            .children(
                shown
                    .into_iter()
                    .map(|(count, state)| counted_mark(count, state, t)),
            )
    })
}

/// A row of the tree: its words, then whatever the caller packs at its
/// right edge. The one the screen is on is on a flat band.
fn row(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    ink: Hsla,
    on: bool,
    t: &Theme,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(sp(2.))
        .h(rems(1.75))
        .px(sp(2.))
        .cursor_pointer()
        .text_color(if on { t.bright } else { ink })
        .when(on, |row| row.pressed(t))
        .when(!on, |row| row.hover(|style| style.text_color(t.bright)))
        .child(
            div()
                .flex_1()
                .min_w(rems(0.))
                .truncate()
                .child(label.into()),
        )
}

/// Small print at a row's right edge: a key, a count.
fn meta(text: impl Into<SharedString>, t: &Theme) -> Div {
    mono(text, Type::CAPTION, t.dim).flex_shrink_0()
}

/// The room between two groups of the tree's rows.
fn gap() -> Div {
    div().h(sp(4.5)).flex_shrink_0()
}

/// A tree's head: what it is of, in bold, then the caller's small print
/// or marks.
fn head(id: impl Into<SharedString>, name: &str, t: &Theme) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(sp(2.))
        .h(rems(2.125))
        .px(sp(2.))
        .child(
            div()
                .flex_1()
                .min_w(rems(0.))
                .truncate()
                .typeset(Type::SUBTITLE)
                .font_weight(weight::STRONG)
                .text_color(t.bright)
                .child(name.to_owned()),
        )
}

/// A row `depth` levels under another, on the thin lines that tie it to
/// it: one down the parent's left edge, as far as the row's middle when
/// it is the `last` under it, and a tick across to the row.
fn nested(
    depth: usize,
    last: bool,
    t: &Theme,
    row: impl IntoElement,
) -> AnyElement {
    if depth == 0 {
        return row.into_any_element();
    }
    let depth = depth.min(4) as f32;
    let at = sp(3.25 + 5.25 * (depth - 1.));
    div()
        .relative()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .pl(sp(5.25 * depth))
        .child(
            div()
                .absolute()
                .left(at)
                .top_0()
                .w(rems(0.0625))
                .bg(t.border_strong)
                .map(|line| {
                    if last {
                        line.h(rems(0.875))
                    } else {
                        line.bottom_0()
                    }
                }),
        )
        .child(
            div()
                .absolute()
                .left(at)
                .top(rems(0.875))
                .w(rems(0.625))
                .h(rems(0.0625))
                .bg(t.border_strong),
        )
        .child(row)
        .into_any_element()
}

/// The rail: Home, with how many things need the person on its corner;
/// a tile per repository, and one to add another; a tile per agent. Its
/// foot is kept for the plan's usage, which nothing reports yet.
fn rail(ws: &Workspace, t: &Theme, cx: &mut Context<Workspace>) -> Div {
    let of = ws.tree_of();
    let needs = ws.need_you_everywhere();
    let home = div()
        .id("home")
        .relative()
        .w(rems(2.5))
        .h(rems(2.25))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .child(logo(t, 26.))
        .when(needs > 0, |home| {
            home.child(
                mono(
                    needs.to_string(),
                    Type::MICRO.sized(10.5),
                    t.roles.on_primary,
                )
                .absolute()
                .top(rems(-0.3125))
                .right(rems(-0.3125))
                .min_w(rems(1.0625))
                .h(rems(1.0625))
                .px(sp(0.75))
                .flex()
                .items_center()
                .justify_center()
                .border_2()
                .border_color(t.panel)
                .bg(t.roles.live)
                .font_weight(weight::STRONG),
            )
        })
        .on_click(cx.listener(|ws, _, _, cx| ws.navigate(Route::Home, cx)));
    let line = || {
        div()
            .w(rems(1.625))
            .h(rems(0.0625))
            .flex_shrink_0()
            .bg(t.border)
    };

    let repos: Vec<AnyElement> = ws
        .catalog
        .listed()
        .map(|repo| {
            let name = repo.name.clone();
            let current = of == Some(TreeOf::Repo(name.clone()));
            let menu = ws.repo_menu.as_deref() == Some(name.as_str());
            let (choose, more) = (name.clone(), name.clone());
            let square = tile(
                format!("tile-{name}"),
                repo.letter(),
                t.mark(&name),
                current,
                ws.need_you(&name) > 0,
                t,
            )
            .on_click(
                cx.listener(move |ws, _, _, cx| ws.choose_repo(&choose, cx)),
            )
            // The other button opens its menu.
            .on_mouse_down(
                gpui::MouseButton::Right,
                cx.listener(move |ws, _, _, cx| ws.toggle_repo_menu(&more, cx)),
            );
            div()
                .flex()
                .flex_shrink_0()
                .child(square)
                .when(menu, |tile| tile.child(repo_menu(ws, &name, t, cx)))
                .into_any_element()
        })
        .collect();

    let add = div()
        .id("add-repo")
        .size(rems(2.25))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius::SELECTED)
        .border_1()
        .border_dashed()
        .border_color(t.border_strong)
        .typeset(Type::LEAD.mono())
        .text_color(t.dim)
        .cursor_pointer()
        .hover(|style| style.text_color(t.text))
        .child("+")
        .on_click(cx.listener(|ws, _, _, cx| ws.pick_repos(cx)));

    // The repositories of the kinds plugins make: the agents.
    let kinds: Vec<(&'static str, RepoKind)> = ws
        .repo_kinds()
        .into_iter()
        .filter(|(_, kind)| kind.place == KindPlace::Section)
        .collect();
    let mut agents: Vec<AnyElement> = Vec::new();
    for (plugin, kind) in &kinds {
        for repo in ws.catalog.of_kind(&kind.kind) {
            let name = repo.name.clone();
            let current = of == Some(TreeOf::Agent(name.clone()));
            let pip = ws.agent_waits(&name) > 0 || ws.need_you(&name) > 0;
            let open = name.clone();
            agents.push(
                tile(
                    format!("tile-{name}"),
                    repo.letter(),
                    t.roles.agent,
                    current,
                    pip,
                    t,
                )
                .on_click(
                    cx.listener(move |ws, _, _, cx| ws.open_agent(&open, cx)),
                )
                .into_any_element(),
            );
        }
        // With none yet, the way to the kind's own page.
        if ws.catalog.of_kind(&kind.kind).next().is_none()
            && let Some(page) = ws.kind_route(plugin, &kind.page)
        {
            agents.push(
                div()
                    .id(SharedString::from(format!("kind-{}", kind.kind)))
                    .size(rems(2.25))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(radius::SELECTED)
                    .border_1()
                    .border_dashed()
                    .border_color(t.border_strong)
                    .typeset(Type::LEAD.mono())
                    .text_color(t.roles.agent)
                    .cursor_pointer()
                    .child("+")
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.navigate(page.clone(), cx)
                    }))
                    .into_any_element(),
            );
        }
    }

    div()
        .w(rems(RAIL))
        .h_full()
        .flex_shrink_0()
        .flex()
        .flex_col()
        .items_center()
        .gap(sp(2.5))
        .pt(sp(4.5))
        .pb(sp(3.5))
        .border_r_1()
        .border_color(t.hairline)
        .child(home)
        .child(line())
        .children(repos)
        .child(add)
        .when(!agents.is_empty(), |rail| {
            rail.child(line()).children(agents)
        })
        .child(div().flex_1())
}

/// A square tile of the rail: a letter on a flat block of `color`. The
/// one whose tree shows is printed over the offset, and a pip on its
/// corner says something inside needs the person.
fn tile(
    id: String,
    letter: String,
    color: Hsla,
    current: bool,
    pip: bool,
    t: &Theme,
) -> Stateful<Div> {
    div()
        .id(SharedString::from(id))
        .relative()
        .size(rems(2.25))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius::SELECTED)
        .cursor_pointer()
        .map(|tile| if current { tile.printed(3., t) } else { tile })
        .bg(color)
        .text_color(t.roles.on_primary)
        .typeset(Type::LEAD.mono())
        .font_weight(weight::STRONG)
        .child(letter)
        .when(pip, |tile| {
            tile.child(
                div()
                    .absolute()
                    .top(rems(-0.25))
                    .right(rems(-0.25))
                    .size(rems(0.6875))
                    .border_2()
                    .border_color(t.panel)
                    .bg(t.roles.live),
            )
        })
}

/// What is the same under every tree: plugins, history, what plugins
/// list everywhere, and models and accounts.
fn foot(
    ws: &Workspace,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Vec<AnyElement> {
    fn to(
        row: Stateful<Div>,
        route: Route,
        cx: &mut Context<Workspace>,
    ) -> AnyElement {
        row.on_click(
            cx.listener(move |ws, _, _, cx| ws.navigate(route.clone(), cx)),
        )
        .into_any_element()
    }
    let waiting = ws.catalog.yours_need_you();
    let yours = ws.yours_route(None);
    let on_plugins =
        ws.route == Route::Plugins || yours.as_ref() == Some(&ws.route);
    let mut rows = vec![
        to(
            row("plugins", "plugins", t.text_soft, on_plugins, t)
                .when(waiting > 0, |row| {
                    row.child(counted_mark(waiting, State::Asks, t))
                }),
            Route::Plugins,
            cx,
        ),
        to(
            row(
                "history",
                "history",
                t.text_soft,
                ws.route == Route::History,
                t,
            ),
            Route::History,
            cx,
        ),
    ];
    // What plugins list there.
    for (n, entry) in ws
        .contributions(points::EVERYWHERE, &AtApp)
        .into_iter()
        .enumerate()
    {
        let Some(route) = ws.link_route(&entry.to, None) else {
            continue;
        };
        rows.push(to(
            nav_row(format!("everywhere-{n}"), &entry, ws.route == route, t),
            route,
            cx,
        ));
    }
    // Sign-ins, keys and the models runs use.
    rows.push(to(
        row(
            "models",
            "models and accounts",
            t.text_soft,
            ws.route == Route::Models,
            t,
        ),
        Route::Models,
        cx,
    ));
    rows
}

/// The row of something a plugin lists: its name, then its count as a
/// mark when it wants attention, else its small print.
fn nav_row(id: String, entry: &NavEntry, on: bool, t: &Theme) -> Stateful<Div> {
    let row = row(id, entry.label.to_lowercase(), t.text_soft, on, t);
    match (&entry.badge, &entry.detail) {
        (Some((badge, tone)), _) => row.child(counted_mark(
            badge.trim().parse().unwrap_or(1),
            if *tone == Tone::Danger {
                State::Failing
            } else {
                State::Asks
            },
            t,
        )),
        (None, Some(detail)) => row.child(meta(detail.clone(), t)),
        (None, None) => row,
    }
}

/// The route of a link a plugin lists under `repo`: its page, about
/// that repository.
fn repo_link(ws: &Workspace, link: &Link, repo: &str) -> Option<Route> {
    let mut link = link.clone();
    if let Some(value) = link.params.get_mut("repo")
        && value.is_empty()
    {
        *value = repo.to_owned();
    }
    ws.link_route(&link, None)
}

/// The rows of what plugins keep for `repo`: memory, constitution, MCP
/// servers, each with its count.
fn repo_pages(
    ws: &Workspace,
    repo: &str,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Vec<AnyElement> {
    let at = AtRepo {
        repo: repo.to_owned(),
    };
    ws.contributions(points::SIDEBAR_REPO, &at)
        .into_iter()
        .enumerate()
        .filter_map(|(n, entry)| {
            let route = repo_link(ws, &entry.to, repo)?;
            Some(
                nav_row(format!("nav-{n}"), &entry, ws.route == route, t)
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.navigate(route.clone(), cx)
                    }))
                    .into_any_element(),
            )
        })
        .collect()
}

/// A repository's tree: its name with its branch and how far ahead of
/// the forge it is, a new run and search, its chats with what is under
/// each, then what plugins keep for it.
fn repo_tree(
    ws: &Workspace,
    name: &str,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Vec<AnyElement> {
    let repo = ws.repo_named(name);
    let branch = repo.trunk.clone().unwrap_or_else(|| "main".to_owned());
    let ahead = ws.unpushed(name).map(|(ahead, _)| ahead);
    let hovered = ws.hovered_repo.as_deref() == Some(name)
        || ws.repo_menu.as_deref() == Some(name);
    let listed = ws.catalog.is_listed(name);
    let (page, hover, more) =
        (name.to_owned(), name.to_owned(), name.to_owned());
    let mut rows = vec![
        head(format!("repo-{name}"), name, t)
            .cursor_pointer()
            .child(mono(branch, Type::CAPTION, t.roles.branch))
            .when_some(ahead, |head, ahead| {
                head.child(mono(format!("↑{ahead}"), Type::CAPTION, t.blue))
            })
            // Its menu, while the pointer is on it.
            .when(hovered && listed, |head| {
                head.child(
                    div()
                        .id("repo-more")
                        .flex_shrink_0()
                        .text_color(t.dim)
                        .hover(|style| style.text_color(t.bright))
                        .child("···")
                        .on_click(cx.listener(move |ws, _, _, cx| {
                            cx.stop_propagation();
                            ws.toggle_repo_menu(&more, cx)
                        })),
                )
            })
            .on_hover(cx.listener(move |ws, hovered: &bool, _, cx| {
                ws.hover_repo(&hover, *hovered, cx)
            }))
            .on_click(
                cx.listener(move |ws, _, _, cx| ws.open_repo_page(&page, cx)),
            )
            .into_any_element(),
    ];
    if listed {
        rows.push(
            row("new-run", "new run", t.blue, false, t)
                .child(meta("ctrl n", t))
                .on_click(
                    cx.listener(|ws, _, window, cx| {
                        ws.start_new_run(window, cx)
                    }),
                )
                .into_any_element(),
        );
    }
    rows.push(
        row("search-row", "search", t.dim, false, t)
            .child(meta("ctrl k", t))
            .on_click(
                cx.listener(|ws, _, window, cx| ws.open_search(window, cx)),
            )
            .into_any_element(),
    );
    rows.push(gap().into_any_element());
    if let Some(chats) = ws.rows_named(name) {
        rows.extend(chat_rows(ws, &chats, 0, None, t, cx));
        if chats.older > 0 {
            let name = name.to_owned();
            rows.push(
                row(
                    format!("older-{name}"),
                    count(chats.older, "older run"),
                    t.dim,
                    false,
                    t,
                )
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.show_older_runs(&name, cx)
                }))
                .into_any_element(),
            );
        }
    }
    let pages = repo_pages(ws, name, t, cx);
    if !pages.is_empty() {
        rows.push(gap().into_any_element());
        rows.extend(pages);
    }
    rows
}

/// The rows of a repository's chats, each under the lines of what it
/// came from. `shift` levels are taken off every row, and `skip` is
/// left out: an agent's tree leaves out its conversation, whose row
/// stands above them.
fn chat_rows(
    ws: &Workspace,
    chats: &RepoRows<'_>,
    shift: usize,
    skip: Option<&heiko_agent::tool::RunId>,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Vec<AnyElement> {
    let tree: Vec<TreeRow<'_>> = ws
        .repo_tree(chats)
        .into_iter()
        .filter(|row| {
            !matches!(row, TreeRow::Run { run, .. } if Some(&run.id) == skip)
        })
        .collect();
    let depth = |row: &TreeRow<'_>| match row {
        TreeRow::Run { depth, .. } | TreeRow::Child { depth, .. } => *depth,
    };
    let current = ws.shown_run().map(|run| ws.listed_as(run).clone());
    let on_run = matches!(ws.route, Route::Run(_));
    tree.iter()
        .enumerate()
        .map(|(n, row)| {
            let level = depth(row);
            // The last under its parent: what follows is no deeper
            // than the parent.
            let last = tree[n + 1..]
                .iter()
                .map(depth)
                .find(|next| *next <= level)
                .is_none_or(|next| next < level);
            let shown = level.saturating_sub(shift);
            match row {
                TreeRow::Run { run, folded, .. } => {
                    let active = on_run && current.as_ref() == Some(&run.id);
                    nested(
                        shown,
                        last,
                        t,
                        run_row(ws, run, active, shown, *folded, t, cx),
                    )
                }
                TreeRow::Child { child, .. } => {
                    nested(shown, last, t, child_row(ws, row, child, t, cx))
                }
            }
        })
        .collect()
}

/// A conversation in the tree: its title, and its marks. One with
/// replies not read yet is in bold. Hovering it offers to close it,
/// unless it is its repository's main chat, a sub-agent, or ended for
/// good. Choosing the one on screen folds what is under it away, or
/// shows it again.
fn run_row(
    ws: &Workspace,
    run: &RunView,
    active: bool,
    depth: usize,
    folded: Option<bool>,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Stateful<Div> {
    let route = Route::Run(run.id.clone());
    let hovered = ws.hovered_run.as_ref() == Some(&run.id);
    let unread = ws.unread(run);
    let ended = ws.has_ended(run);
    // A sub-agent is not closed: it leaves the list as it ends, and
    // the crew's Stop stops it.
    let sub_agent = matches!(run.origin, Origin::SubAgent { .. });
    let closes = hovered && !ended && !ws.is_main(&run.id) && !sub_agent;
    let ink = if ended {
        t.dim
    } else if depth == 0 || unread > 0 {
        t.text
    } else {
        t.text_soft
    };
    let (hover_id, close_id, fold_id) =
        (run.id.clone(), run.id.clone(), run.id.clone());
    row(format!("run-{}", run.id), run.title.clone(), ink, active, t)
        .when(unread > 0 && !active, |row| {
            row.font_weight(weight::EMPHASIS)
        })
        .map(|row| {
            if closes {
                row.child(
                    div()
                        .id("close-run")
                        .size(rems(1.125))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(Icon::Close, IconSize::TINY, t.text_soft))
                        .on_click(cx.listener(move |ws, _, _, cx| {
                            cx.stop_propagation();
                            ws.close_run(&close_id, cx)
                        })),
                )
            } else {
                row.children(marks(ws.run_marks(run, folded == Some(true)), t))
            }
        })
        .on_hover(cx.listener(move |ws, hovered: &bool, _, cx| {
            ws.hover_run(&hover_id, *hovered, cx)
        }))
        .on_click(cx.listener(move |ws, _, _, cx| {
            if active && folded.is_some() {
                ws.toggle_fold(&fold_id, cx)
            } else {
                ws.navigate(route.clone(), cx)
            }
        }))
}

/// A child the workspace has no conversation for: a fork opens the
/// comparison with its run.
fn child_row(
    ws: &Workspace,
    row_of: &TreeRow<'_>,
    child: &ChildRun,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Stateful<Div> {
    let route = row_of.route();
    let active = route.as_ref() == Some(&ws.route);
    row(
        format!("child-{}", child.id),
        child.title.clone(),
        t.text_soft,
        active,
        t,
    )
    .children(status_state(&child.status).map(|state| mark(state, t)))
    .when_some(route, |row, route| {
        row.on_click(
            cx.listener(move |ws, _, _, cx| ws.navigate(route.clone(), cx)),
        )
    })
}

/// The mark a status alone shows: working, failing or at a limit. One
/// that waits on the person's next message has none.
fn status_state(status: &RunStatus) -> Option<State> {
    match status {
        RunStatus::Planning | RunStatus::Running => Some(State::Working),
        RunStatus::Finished(StopReason::Error(_)) => Some(State::Failing),
        RunStatus::Finished(StopReason::Limit(_)) => Some(State::Limit),
        RunStatus::Finished(_) | RunStatus::Interrupted => None,
    }
}

/// An agent's marks: its conversation's, its sub-agents at work with
/// it, and what it left for the person as questions.
fn agent_marks(ws: &Workspace, repo: &Repo) -> Marks {
    let own = repo
        .main
        .as_ref()
        .and_then(|id| ws.run(id))
        .map(|main| ws.run_marks(main, true))
        .unwrap_or_default();
    own.and(Marks {
        asks: ws.agent_waits(&repo.name),
        ..Marks::default()
    })
}

/// An agent's tree (ADR 0037): its name and marks, the way to all
/// agents, its conversation with its sub-agents at work under it, what
/// plugins keep for it, and its repository.
fn agent_tree(
    ws: &Workspace,
    name: &str,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Vec<AnyElement> {
    let repo = ws.repo_named(name);
    let mut rows = vec![
        head(format!("agent-{name}"), name, t)
            .children(marks(agent_marks(ws, repo), t))
            .into_any_element(),
    ];
    // Its kind's page lists them all.
    let kind = ws.repo_kinds().into_iter().find_map(|(plugin, kind)| {
        (repo.kind.as_deref() == Some(kind.kind.as_str()))
            .then(|| Some((ws.kind_route(plugin, &kind.page)?, kind)))
            .flatten()
    });
    if let Some((page, kind)) = kind {
        rows.push(
            row(
                format!("kind-{}", kind.kind),
                kind.title.to_lowercase(),
                t.blue,
                false,
                t,
            )
            .on_click(
                cx.listener(move |ws, _, _, cx| ws.navigate(page.clone(), cx)),
            )
            .into_any_element(),
        );
    }
    rows.push(gap().into_any_element());
    let main = repo.main.as_ref().and_then(|id| ws.run(id));
    let current = ws.shown_run().map(|run| ws.listed_as(run).clone());
    let active = matches!(ws.route, Route::Run(_))
        && main.is_some_and(|main| current.as_ref() == Some(&main.id));
    let open = name.to_owned();
    rows.push(
        row("conversation", "conversation", t.text, active, t)
            .children(main.and_then(|main| marks(ws.run_marks(main, false), t)))
            .on_click(cx.listener(move |ws, _, _, cx| ws.open_agent(&open, cx)))
            .into_any_element(),
    );
    // Its row stands for its main chat: what is under it is its crew.
    if let Some(chats) = ws.rows_named(name) {
        // With no conversation yet, nothing is under it.
        let shift = usize::from(main.is_none());
        rows.extend(chat_rows(
            ws,
            &chats,
            shift,
            main.map(|main| &main.id),
            t,
            cx,
        ));
    }
    let pages = repo_pages(ws, name, t, cx);
    rows.push(gap().into_any_element());
    rows.extend(pages);
    let page = Route::Repo(name.to_owned());
    rows.push(
        row(
            "agent-repo",
            "its repository",
            t.text_soft,
            ws.route == page,
            t,
        )
        .on_click(
            cx.listener(move |ws, _, _, cx| ws.navigate(page.clone(), cx)),
        )
        .into_any_element(),
    );
    rows
}

/// The tree of a kind's own pages: its title, the way to make one, and
/// each of its repositories with its marks.
fn kind_tree(
    ws: &Workspace,
    plugin: &str,
    kind: &RepoKind,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Vec<AnyElement> {
    let mut rows = vec![
        head(format!("kind-{}", kind.kind), &kind.title, t).into_any_element(),
    ];
    let new = kind.new.as_ref().and_then(|new| ws.kind_route(plugin, new));
    if let Some(new) = new {
        rows.push(
            row(
                format!("new-{}", kind.kind),
                format!("new {}", kind.kind),
                t.blue,
                ws.route == new,
                t,
            )
            .on_click(
                cx.listener(move |ws, _, _, cx| ws.navigate(new.clone(), cx)),
            )
            .into_any_element(),
        );
    }
    rows.push(gap().into_any_element());
    let mut none = true;
    for repo in ws.catalog.of_kind(&kind.kind) {
        none = false;
        let name = repo.name.clone();
        rows.push(
            row(format!("agent-{name}"), name.clone(), t.text, false, t)
                .children(marks(agent_marks(ws, repo), t))
                .on_click(
                    cx.listener(move |ws, _, _, cx| ws.open_agent(&name, cx)),
                )
                .into_any_element(),
        );
    }
    if none {
        rows.push(
            div()
                .px(sp(2.))
                .py(sp(1.5))
                .typeset(Type::CAPTION)
                .text_color(t.muted)
                .child(kind.empty.clone())
                .into_any_element(),
        );
    }
    rows
}

/// A repository's menu, beside its tile.
fn repo_menu(
    ws: &Workspace,
    name: &str,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    // What plugins keep per repository: their entries, after heiko's.
    let plugins = ws.drawn(
        points::REPO_MENU,
        &AtRepo {
            repo: name.to_owned(),
        },
        cx,
    );
    let entry = |id: &'static str, label: String, color: Hsla| {
        div()
            .id(id)
            .debug_selector(|| id.to_owned())
            .flex()
            .items_center()
            .h(rems(2.))
            .px(sp(2.5))
            .cursor_pointer()
            .text_color(color)
            .hover(|style| style.bg(t.selected))
            .child(label)
    };
    let owned = |name: &str| name.to_owned();
    let (new, update, files, remove) =
        (owned(name), owned(name), owned(name), owned(name));
    let menu = div()
        .id("repo-menu")
        .min_w(rems(14.5))
        .flex()
        .flex_col()
        .p(sp(1.5))
        .raised(t)
        .border_color(t.border_strong)
        .lifted(t)
        .child(
            entry("menu-new", format!("New run in {name}"), t.text).on_click(
                cx.listener(move |ws, _, window, cx| {
                    ws.new_run_in(&new, window, cx)
                }),
            ),
        )
        .child(
            entry("menu-update", "Fetch new commits".into(), t.text).on_click(
                cx.listener(move |ws, _, _, cx| ws.update_repo(&update, cx)),
            ),
        )
        .child(
            entry("menu-files", "Show in files".into(), t.text).on_click(
                cx.listener(move |ws, _, _, cx| ws.show_in_files(&files, cx)),
            ),
        )
        .when(!plugins.is_empty(), |menu| {
            menu.child(super::rule(t).my(sp(1.))).children(plugins)
        })
        .child(super::rule(t).my(sp(1.)))
        .child(
            entry("menu-remove", "Remove from heiko".into(), t.red).on_click(
                cx.listener(move |ws, _, _, cx| ws.remove_repo(&remove, cx)),
            ),
        )
        .occlude()
        .on_mouse_down_out(cx.listener(|ws, _, _, cx| ws.close_repo_menu(cx)));
    deferred(
        anchored()
            .anchor(Anchor::TopLeft)
            .offset(point(px(10.), px(0.)))
            .snap_to_window_with_margin(px(8.))
            .child(menu),
    )
    .with_priority(1)
}

/// How many jobs the status bar lists one by one; more collapse to a
/// count, the first of them shown.
pub const JOBS_SHOWN: usize = 2;

/// The bar under the desktop, when there is news: heiko's own first (a
/// repository being imported or failing to, or an update), then the jobs
/// plugins run, then what plugins add (`points::ACTIVITY`). Without any,
/// there is no bar.
pub fn status_bar(
    ws: &Workspace,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Option<Div> {
    let busy = !matches!(ws.catalog.project, ProjectStatus::Unknown);
    let activity = ws.drawn(points::ACTIVITY, &AtApp, cx);
    let jobs = &ws.catalog.jobs;
    if !busy
        && ws.catalog.update.is_none()
        && jobs.is_empty()
        && activity.is_empty()
    {
        return None;
    }
    let shown = if jobs.len() > JOBS_SHOWN {
        1
    } else {
        jobs.len()
    };
    let more = (jobs.len() > JOBS_SHOWN).then(|| {
        div()
            .flex_shrink_0()
            .child(format!("{} jobs ·", jobs.len()))
    });
    Some(
        div()
            .h(rems(1.75))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(sp(4.5))
            .px(sp(5.))
            .bg(t.panel)
            .border_t_1()
            .border_color(t.hairline)
            .font_family(MONO)
            .typeset(Type::MICRO)
            .text_color(t.dim)
            .overflow_hidden()
            .children(match &ws.catalog.project {
                ProjectStatus::Unknown => None,
                ProjectStatus::Importing(name) => Some(
                    div()
                        .text_color(t.blue)
                        .child(format!("importing {name}…")),
                ),
                ProjectStatus::Updating(name) => Some(
                    div().text_color(t.blue).child(format!("updating {name}…")),
                ),
                ProjectStatus::Failed(name) => Some(
                    div()
                        .text_color(t.red)
                        .child(format!("{name} could not be imported")),
                ),
            })
            .children(ws.catalog.update.clone().map(|text| div().child(text)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(2.))
                    .min_w(rems(0.))
                    .children(more)
                    .children(
                        jobs.iter()
                            .take(shown)
                            .map(|job| job_item(ws, job, t, cx)),
                    ),
            )
            .children(activity),
    )
}

/// A job going on, in the status bar: its plugin, what it does, how far
/// it is as ticks, and what it does now. Clicking it opens the page it
/// names, or its plugin's.
fn job_item(
    ws: &Workspace,
    job: &heiko_ui_plugin::JobInfo,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let id = format!("status-job-{}-{}", job.plugin, job.name);
    let stopping = format!("{id}-stopping");
    let route = ws.job_route(job);
    let item = div()
        .id(SharedString::from(id.clone()))
        .debug_selector(move || id.clone())
        .flex()
        .items_center()
        .gap(sp(2.))
        .min_w(rems(0.))
        .child(div().flex_shrink_0().child(job.plugin.clone()))
        .child(
            div()
                .flex_shrink_0()
                .text_color(t.text_soft)
                .child(job.label.clone()),
        )
        .child(div().w(rems(3.5)).flex_shrink_0().child(super::bar(
            job.fraction,
            6.,
            t.blue,
            t.hairline,
        )))
        .when(job.stopping, |item| {
            item.child(
                div()
                    .debug_selector(move || stopping.clone())
                    .child("stopping…"),
            )
        })
        .when(!job.stopping && !job.line.is_empty(), |item| {
            item.child(div().min_w(rems(0.)).truncate().child(job.line.clone()))
        });
    match route {
        Some(route) => item
            .cursor_pointer()
            .hover(|style| style.text_color(t.text))
            .on_click(
                cx.listener(move |ws, _, _, cx| ws.navigate(route.clone(), cx)),
            )
            .into_any_element(),
        None => item.into_any_element(),
    }
}

/// How far a phone's screens stand in from its edges, in spacing
/// steps: 18 px.
const PHONE_MARGIN: f32 = 4.5;

/// The words a run's state reads as on a phone, in its color: at work
/// with its turn, else its status.
fn state_words(run: &RunView, t: &Theme) -> (Hsla, SharedString) {
    if run.ending.is_none() && run.status.is_live() {
        (t.green, format!("working, turn {}", run.turn).into())
    } else {
        super::run_look(run, t)
    }
}

/// The phone's header above a run: back and its title, with Push for a
/// main chat ahead of the forge; then its state in words, and the ways
/// to its pane, its details and closing it; over a heavy rule.
pub fn phone_run_bar(
    ws: &Workspace,
    run: &RunView,
    main: bool,
    pane: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let (color, label) = state_words(run, t);
    let id = run.id.clone();
    // A word to tap, as tall as a finger needs.
    let word = |id: &'static str, label: &'static str| {
        div()
            .id(id)
            .min_h(rems(2.75))
            .flex()
            .items_center()
            .child(text_link(label, Type::SMALL, t))
    };
    div()
        .flex_shrink_0()
        .flex()
        .flex_col()
        .mx(sp(PHONE_MARGIN))
        .pt(sp(2.))
        .pb(sp(2.5))
        .border_b_2()
        .border_color(t.rule)
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(1.))
                .min_h(rems(2.75))
                .child(
                    icon_button("phone-back", Icon::Back, 44., t)
                        .ml(sp(-3.))
                        .on_click(cx.listener(|ws, _, _, cx| ws.back(cx))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(rems(0.))
                        .truncate()
                        .typeset(Type::PAGE_TITLE.sized(19.))
                        .text_color(t.bright)
                        .child(run.title.clone()),
                )
                .children(super::push::header(ws, run, t, cx)),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(3.5))
                .typeset(Type::SMALL)
                .child(
                    div()
                        .flex_1()
                        .min_w(rems(0.))
                        .truncate()
                        .text_color(color)
                        .child(label),
                )
                // What a plugin shows in the run's pane.
                .when(pane, |bar| {
                    bar.child(word("phone-pane", "pane").on_click(
                        cx.listener(|ws, _, _, cx| ws.toggle_pane(cx)),
                    ))
                })
                .child(
                    word("phone-details", "details").on_click(
                        cx.listener(|ws, _, _, cx| ws.toggle_sheet(cx)),
                    ),
                )
                // A repository's main chat stays open.
                .when(!main, |bar| {
                    bar.child(word("phone-close", "close").on_click(
                        cx.listener(move |ws, _, _, cx| {
                            ws.close_run_to_list(&id, cx)
                        }),
                    ))
                }),
        )
}

/// The phone's header on every other screen: its title over a heavy
/// rule, with back on a screen under another, and adding a repository
/// on the list of runs.
pub fn phone_header(
    ws: &Workspace,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let title = match &ws.route {
        Route::Home => "Runs".to_owned(),
        _ => ws.route_title(),
    };
    div()
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(sp(2.))
        .min_h(rems(3.5))
        .mx(sp(PHONE_MARGIN))
        .pt(sp(2.))
        .border_b_2()
        .border_color(t.rule)
        .when(!ws.route.is_top_level(), |bar| {
            bar.child(
                icon_button("phone-back", Icon::Back, 44., t)
                    .ml(sp(-3.))
                    .on_click(cx.listener(|ws, _, _, cx| ws.back(cx))),
            )
        })
        .child(
            div()
                .flex_1()
                .min_w(rems(0.))
                .truncate()
                .typeset(Type::PAGE_TITLE.sized(19.))
                .text_color(t.bright)
                .child(title),
        )
        // What the person did here that the computer has not taken:
        // it goes once the phone reaches it.
        .when(ws.pairing.unsent > 0, |bar| {
            let unsent = ws.pairing.unsent;
            bar.child(
                div()
                    .flex_shrink_0()
                    .typeset(Type::CAPTION)
                    .text_color(t.dim)
                    .child(format!("{unsent} waiting to send")),
            )
        })
        .when(matches!(ws.route, Route::Home), |bar| {
            bar.child(
                div()
                    .id("phone-add-repo")
                    .min_h(rems(2.75))
                    .flex()
                    .items_center()
                    .child(text_link("add repository", Type::SMALL, t))
                    .on_click(cx.listener(|ws, _, _, cx| ws.pick_repos(cx))),
            )
        })
}

/// The phone's home: runs grouped by repository, each group with what
/// plugins list under it, then the agents.
pub fn phone_run_list(
    ws: &Workspace,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let groups: Vec<AnyElement> = ws
        .repo_rows("")
        .into_iter()
        .map(|rows| phone_group(ws, rows, t, cx).into_any_element())
        .collect();
    div()
        .id("phone-runs")
        .flex_1()
        .min_h(rems(0.))
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .px(sp(PHONE_MARGIN))
        .pb(sp(4.))
        .children(groups)
        .when(ws.catalog.listed().next().is_none(), |list| {
            list.child(super::empty(
                "No repositories yet. Add one to start runs in it.",
                t,
            ))
        })
        .children(phone_kinds(ws, t, cx))
}

/// The phone's repositories of each kind plugins make (`REPO_KIND`),
/// under the others: an agent's (ADR 0037).
fn phone_kinds(
    ws: &Workspace,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Vec<Div> {
    ws.repo_kinds()
        .into_iter()
        .filter(|(_, kind)| kind.place == KindPlace::Section)
        .filter_map(|(_, kind)| phone_kind(ws, &kind, t, cx))
        .collect()
}

/// A title in a phone's list, in the display face: a repository's name,
/// a kind's.
fn phone_title(text: impl Into<SharedString>, t: &Theme) -> Div {
    div()
        .min_w(rems(0.))
        .truncate()
        .typeset(Type::SECTION.sized(17.))
        .text_color(t.bright)
        .child(text.into())
}

/// A row of a phone's list: what it is drawn with, its name over one
/// line in small mono print, and what the caller packs at its end.
fn phone_row(
    id: String,
    lead: AnyElement,
    name: impl Into<SharedString>,
    line: Option<AnyElement>,
    t: &Theme,
) -> Stateful<Div> {
    div()
        .id(SharedString::from(id))
        .flex()
        .items_center()
        .gap(sp(2.5))
        .min_h(rems(3.))
        .py(sp(1.5))
        .border_b_1()
        .border_color(t.hairline)
        .child(
            div()
                .w(rems(0.875))
                .flex_shrink_0()
                .flex()
                .justify_center()
                .child(lead),
        )
        .child(
            div()
                .flex_1()
                .min_w(rems(0.))
                .flex()
                .flex_col()
                .child(
                    div()
                        .truncate()
                        .font_weight(weight::STRONG)
                        .text_color(t.bright)
                        .child(name.into()),
                )
                .children(line),
        )
}

/// A kind's repositories on the phone: one row each, opening its
/// conversation, with how many of its things need the person.
fn phone_kind(
    ws: &Workspace,
    kind: &RepoKind,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Option<Div> {
    let agents: Vec<&Repo> = ws.catalog.of_kind(&kind.kind).collect();
    if agents.is_empty() {
        return None;
    }
    let heading = div()
        .flex()
        .pt(sp(5.))
        .pb(sp(2.))
        .border_b_2()
        .border_color(t.rule)
        .child(phone_title(kind.title.clone(), t));
    let rows = agents.into_iter().map(|repo| {
        let name = repo.name.clone();
        let at = AtRepo { repo: name.clone() };
        let line = ws
            .drawn(points::KIND_ROW_LINE, &at, cx)
            .into_iter()
            .next()
            .map(|line| {
                div()
                    .truncate()
                    .font_family(MONO)
                    .typeset(Type::CAPTION)
                    .text_color(t.dim)
                    .child(line)
                    .into_any_element()
            });
        let waits = ws.agent_waits(&name) + ws.need_you(&name);
        let working = repo
            .main
            .as_ref()
            .and_then(|id| ws.run(id))
            .is_some_and(|main| main.status.is_live());
        let lead = if working {
            mark(State::Working, t).into_any_element()
        } else {
            super::dot(t.roles.agent, 6.).into_any_element()
        };
        phone_row(format!("phone-agent-{name}"), lead, name.clone(), line, t)
            .cursor_pointer()
            .when(waits > 0, |row| {
                row.child(
                    hot_action(format!("{waits} need you"), t)
                        .typeset(Type::CAPTION)
                        .font_weight(weight::STRONG),
                )
            })
            .on_click(cx.listener(move |ws, _, _, cx| ws.open_agent(&name, cx)))
    });
    Some(div().flex().flex_col().child(heading).children(rows))
}

fn phone_group(
    ws: &Workspace,
    rows: RepoRows<'_>,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let name = rows.repo.name.clone();
    let working = Marks {
        working: rows.live,
        ..Marks::default()
    };
    let asks = Marks {
        asks: ws.need_you(&name),
        ..Marks::default()
    };
    let head = div()
        .id(SharedString::from(format!("phone-repo-{name}")))
        .flex()
        .items_center()
        .gap(sp(2.5))
        .min_h(rems(2.75))
        .pt(sp(1.))
        .cursor_pointer()
        // Open, it heads its runs; closed, it is one row among others.
        .map(|head| {
            if rows.open {
                head.border_b_2().border_color(t.rule)
            } else {
                head.border_b_1().border_color(t.hairline)
            }
        })
        .child(phone_title(name.clone(), t))
        .children(marks(working, t))
        // Closed, what waits inside shows on it.
        .when(!rows.open, |head| head.children(marks(asks, t)))
        .child(div().flex_1())
        .child(
            div()
                .id("phone-repo-new")
                .min_h(rems(2.75))
                .flex()
                .items_center()
                .child(text_link("new run", Type::SMALL, t))
                .on_click({
                    let name = name.clone();
                    cx.listener(move |ws, _, window, cx| {
                        cx.stop_propagation();
                        ws.new_run_in(&name, window, cx)
                    })
                }),
        )
        .on_click({
            let name = name.clone();
            cx.listener(move |ws, _, _, cx| ws.toggle_repo_open(&name, cx))
        });
    let group = div().flex().flex_col().child(head);
    if !rows.open {
        return group;
    }
    // What plugins list under the repository.
    let entries =
        ws.contributions(points::SIDEBAR_REPO, &AtRepo { repo: name.clone() });
    let mut pages = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_x(sp(4.))
        .min_h(rems(2.75))
        .border_b_1()
        .border_color(t.hairline)
        .typeset(Type::SMALL);
    let mut listed = false;
    for (n, entry) in entries.into_iter().enumerate() {
        let Some(route) = repo_link(ws, &entry.to, &name) else {
            continue;
        };
        listed = true;
        pages = pages.child(
            div()
                .id(SharedString::from(format!("phone-nav-{n}")))
                .flex()
                .items_center()
                .gap(sp(2.))
                .min_h(rems(2.75))
                .cursor_pointer()
                .child(link_action(entry.label.clone(), t))
                .children(
                    entry
                        .detail
                        .clone()
                        .map(|detail| mono(detail, Type::CAPTION, t.dim)),
                )
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.navigate(route.clone(), cx)
                })),
        );
    }
    let mut group = group.when(listed, |group| group.child(pages));
    for row in ws.repo_tree(&rows) {
        group = match row {
            TreeRow::Run { run, .. } => {
                group.child(phone_run_row(ws, run, t, cx))
            }
            TreeRow::Child { child, .. } => {
                group.child(phone_child_row(row.route(), child, t, cx))
            }
        };
    }
    if rows.older > 0 {
        group = group.child(
            div()
                .id(SharedString::from(format!("phone-older-{name}")))
                .min_h(rems(2.75))
                .flex()
                .items_center()
                .child(text_link(
                    format!("Show {} older runs", rows.older),
                    Type::SMALL,
                    t,
                ))
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.show_older_runs(&name, cx)
                })),
        );
    }
    group
}

/// The mark a phone's row leads with: what the run is doing, or how it
/// stopped; a hollow ring when it waits on the person's next message.
fn lead_state(marks: &Marks) -> State {
    if marks.working > 0 {
        State::Working
    } else if marks.failing > 0 {
        State::Failing
    } else if marks.limit > 0 {
        State::Limit
    } else if marks.met > 0 {
        State::Met
    } else {
        State::Idle
    }
}

fn phone_run_row(
    ws: &Workspace,
    run: &RunView,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let (color, label) = status_look(&run.status, t);
    let route = Route::Run(run.id.clone());
    let attention = ws.attention(run);
    let run_marks = ws.run_marks(run, false);
    let meta = if run.status.is_live() {
        format!("{label} · turn {}", run.turn)
    } else {
        label.to_string()
    };
    // What plugins add to the row: where a goal stands, under the
    // title, and its count at the end.
    let note = ws.run_rows(run).into_iter().next();
    let (meta, color) = match note
        .as_ref()
        .and_then(|note| Some((note.phone_line.clone()?, note.tone)))
    {
        Some((line, tone)) => (line, t.tone(tone)),
        None => (meta, color),
    };
    // What the run needs of the person says it first; a working run
    // keeps its plugin's line.
    let (meta, color) = match (&attention, attention.line()) {
        (Attention::Landed, _) => ("landed".to_owned(), t.dim),
        (Attention::Dropped, _) => ("dropped".to_owned(), t.dim),
        (Attention::Working { .. }, _) | (_, None) => (meta, color),
        (attention, Some(line)) => (line, super::attention_color(attention, t)),
    };
    let badge = note
        .as_ref()
        .and_then(|note| Some((note.count.clone()?, t.tone(note.tone))));
    let asks = Marks {
        asks: run_marks.asks,
        ..Marks::default()
    };
    phone_row(
        format!("phone-run-{}", run.id),
        mark(lead_state(&run_marks), t).into_any_element(),
        run.title.clone(),
        Some(
            mono(meta, Type::CAPTION, color)
                .truncate()
                .into_any_element(),
        ),
        t,
    )
    .cursor_pointer()
    .children(marks(asks, t))
    .when_some(badge, |row, (badge, tone)| {
        row.child(mono(badge, Type::CAPTION, tone).flex_shrink_0())
    })
    .on_click(cx.listener(move |ws, _, _, cx| ws.navigate(route.clone(), cx)))
}

fn phone_child_row(
    route: Option<Route>,
    child: &ChildRun,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let (color, label) = status_look(&child.status, t);
    phone_row(
        format!("phone-child-{}", child.id),
        mark(status_state(&child.status).unwrap_or(State::Idle), t)
            .into_any_element(),
        child.title.clone(),
        Some(mono(label, Type::CAPTION, color).into_any_element()),
        t,
    )
    .pl(sp(6.))
    .when_some(route, |row, route| {
        row.cursor_pointer().on_click(
            cx.listener(move |ws, _, _, cx| ws.navigate(route.clone(), cx)),
        )
    })
}

/// The phone's tabs: plain words over a heavy rule, the one on screen
/// in bold.
pub fn phone_tab_bar(
    ws: &Workspace,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let current = ws.route.tab();
    div().flex_shrink_0().px(sp(PHONE_MARGIN)).pb(sp(3.)).child(
        div()
            .flex()
            .justify_between()
            .border_t_2()
            .border_color(t.rule)
            .children(Tab::ALL.into_iter().map(|tab| {
                let active = tab == current;
                div()
                    .id(tab.label())
                    .min_h(rems(2.75))
                    .flex()
                    .items_center()
                    .typeset(Type::BODY)
                    .text_color(if active { t.bright } else { t.dim })
                    .when(active, |tab| tab.font_weight(weight::STRONG))
                    .cursor_pointer()
                    .child(tab.label())
                    .on_click(
                        cx.listener(move |ws, _, _, cx| ws.switch_tab(tab, cx)),
                    )
            })),
    )
}
