//! Everything around the screens: title bar, sidebar, status bar, and
//! their phone versions.

use gpui::{
    Anchor,
    AnyElement,
    Context,
    Div,
    IntoElement,
    SharedString,
    anchored,
    deferred,
    div,
    point,
    prelude::*,
    px,
};
use tau_agent::tool::RunId;

use super::{
    Edge,
    Material,
    dot,
    icon,
    icon_button,
    live_dot,
    logo,
    mono,
    status_icon,
    status_look,
};
use crate::{
    assets::Icon,
    catalog::ProjectStatus,
    repos::RepoRows,
    route::{Route, Tab},
    theme::{
        Design as _,
        IconSize,
        MONO,
        Theme,
        Type,
        control,
        radius,
        sp,
        weight,
    },
    view::{ChildKind, RunView, tokens, usd},
    workspace::Workspace,
};

fn tab_icon(tab: Tab) -> Icon {
    match tab {
        Tab::Runs => Icon::Runs,
        Tab::History => Icon::History,
        Tab::Plugins => Icon::Plug,
        Tab::Models => Icon::Settings,
    }
}

/// The desktop title bar: where you are, and the way back.
pub fn title_bar(
    ws: &Workspace,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let run = ws.current();
    // What the composer's next message goes to: the open chat's model,
    // or the next run's.
    let choice = ws.composer_target().map_or_else(
        || ws.fork_model().clone(),
        |target| ws.choice_for(&target),
    );
    let reasoning = format!("reasoning {}", choice.effort.label());
    let model = choice.model.clone();
    let total: f64 = ws.runs.iter().map(|run| run.usage.cost).sum();
    let mut crumbs = vec![ws.name.clone()];
    match &ws.route {
        Route::Home | Route::Run(_) => {
            crumbs.extend(run.map(|run| run.title.clone()));
        }
        route => {
            crumbs.extend(
                route
                    .run()
                    .and_then(|id| ws.run(id))
                    .map(|run| run.title.clone()),
            );
            crumbs.push(ws.route_title(cx));
        }
    }
    let last = crumbs.len() - 1;

    div()
        .h(px(44.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(sp(2.5))
        .pl(sp(3.))
        .pr(sp(3.))
        .chrome(Edge::Top, t)
        .child(logo(t, 26.))
        .when(ws.can_go_back(), |bar| {
            bar.child(
                icon_button("back", Icon::Back, 30., t)
                    .on_click(cx.listener(|ws, _, _, cx| ws.back(cx))),
            )
        })
        .children(crumbs.into_iter().enumerate().flat_map(|(n, crumb)| {
            let text = div()
                .when(n == last, |crumb| crumb.font_weight(weight::EMPHASIS))
                .text_color(if n == last { t.text } else { t.muted })
                .child(crumb)
                .into_any_element();
            let slash = (n > 0)
                .then(|| div().text_color(t.dim).child("/").into_any_element());
            slash.into_iter().chain([text])
        }))
        .child(div().flex_1())
        .child(
            div()
                .id("open-search")
                .flex()
                .items_center()
                .gap(sp(2.))
                .w(px(340.))
                .px(sp(2.5))
                .py(sp(1.25))
                .well(t)
                .rounded(radius::CONTROL)
                .text_color(t.dim)
                .cursor_pointer()
                .child(icon(Icon::Search, IconSize::BASE, t.dim))
                .child(
                    div().flex_1().child("Search runs, repositories, actions"),
                )
                .child(mono("Ctrl K", Type::MICRO, t.dim))
                .on_click(
                    cx.listener(|ws, _, window, cx| ws.open_search(window, cx)),
                ),
        )
        .child(div().flex_1())
        .child(
            div()
                .id("title-model")
                .flex()
                .items_center()
                .gap(sp(1.5))
                .px(sp(2.5))
                .py(sp(1.25))
                .key(t)
                .rounded(radius::CONTROL)
                .cursor_pointer()
                .hover(|style| style.border_color(t.border_strong))
                .child(mono(model, Type::CAPTION, t.text))
                .child(mono("·", Type::CAPTION, t.dim))
                .child(mono(reasoning, Type::CAPTION, t.blue))
                .on_click(cx.listener(|ws, _, window, cx| {
                    ws.title_model_clicked(window, cx)
                })),
        )
        .child(
            mono(format!("today {}", usd(total)), Type::CAPTION, t.accent)
                .px(sp(2.5))
                .py(sp(1.25))
                .rounded(radius::CONTROL)
                .bg(t.accent_soft),
        )
}

/// The desktop sidebar: a new run in the selected repository, the
/// repositories as a tree (each with its memory, constitution and runs),
/// then what is the same everywhere.
pub fn sidebar(
    ws: &Workspace,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> gpui::Stateful<Div> {
    let selected = ws.selected_repo().map(str::to_owned);
    let filter = ws.sidebar_filter.read(cx).text().to_owned();
    let rows = ws.repo_rows(&filter);
    let many = ws.catalog.repos.len() > 3;

    let new_run = div()
        .id("new-run")
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(sp(2.))
        .h(control::MEDIUM)
        .px(sp(2.5))
        .key(t)
        .rounded(radius::CONTROL)
        .cursor_pointer()
        .hover(|style| style.border_color(t.border_strong))
        .child(icon(Icon::Plus, IconSize::BASE, t.text))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .gap(sp(1.))
                .child("New run")
                .when_some(selected.clone(), |label, repo| {
                    label
                        .child(div().text_color(t.muted).child("in"))
                        .child(div().truncate().child(repo))
                }),
        )
        .child(mono("Ctrl N", Type::MICRO, t.dim))
        .on_click(
            cx.listener(|ws, _, window, cx| ws.start_new_run(window, cx)),
        );

    let filter_field = div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(sp(2.))
        .h(px(34.))
        .mt(sp(2.))
        .px(sp(2.5))
        .well(t)
        .rounded(radius::CONTROL)
        .child(icon(Icon::Search, IconSize::COMPACT, t.dim))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .child(ws.sidebar_filter.clone()),
        );

    let add = div()
        .id("add-repo")
        .size(px(22.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius::TAG)
        .cursor_pointer()
        .hover(|style| style.bg(t.selected))
        .child(icon(Icon::Plus, IconSize::COMPACT, t.muted))
        .on_click(cx.listener(|ws, _, _, cx| ws.pick_github_repos(cx)));

    let everywhere = [
        (Route::History, Icon::History, "History", "all repos"),
        (Route::Plugins, Icon::Plug, "Plugins", ""),
        (Route::Models, Icon::Settings, "Models", ""),
        (Route::Phones, Icon::Phone, "Phones", ""),
    ];

    div()
        .id("sidebar")
        .flex()
        .flex_col()
        .gap(sp(0.5))
        .p(sp(2.))
        .chrome(Edge::Left, t)
        .overflow_y_scroll()
        .child(new_run)
        .when(many, |bar| bar.child(filter_field))
        .child(section("Repositories", Some(add.into_any_element()), t))
        .children({
            let groups: Vec<AnyElement> = rows
                .into_iter()
                .map(|rows| repo_group(ws, rows, t, cx).into_any_element())
                .collect();
            groups
        })
        .when(ws.catalog.repos.is_empty(), |bar| {
            bar.child(
                div()
                    .px(sp(2.5))
                    .py(sp(2.))
                    .typeset(Type::CAPTION)
                    .text_color(t.muted)
                    .child("No repositories yet. Add one to start runs in it."),
            )
        })
        .child(div().flex_1().min_h(sp(4.)))
        .child(section("Everywhere", None, t))
        .children(everywhere.into_iter().map(|(route, glyph, label, meta)| {
            let active = ws.route == route;
            nav_row(glyph, label, meta.into(), active, t)
                .id(label)
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.navigate(route.clone(), cx)
                }))
        }))
        .children({
            // What plugins list everywhere.
            let entries = ws.contributions(
                tau_ui_plugin::points::SIDEBAR,
                &tau_ui_plugin::points::AtApp,
                cx,
            );
            entries
                .into_iter()
                .enumerate()
                .map(|(n, entry)| {
                    nav_entry_row(
                        ws,
                        entry,
                        SharedString::from(format!("nav-everywhere-{n}")),
                        t,
                        cx,
                    )
                })
                .collect::<Vec<_>>()
        })
}

/// A plugin's navigation entry as a sidebar row, active while its page
/// is open.
fn nav_entry_row(
    ws: &Workspace,
    entry: tau_ui_plugin::NavEntry,
    id: SharedString,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> gpui::Stateful<Div> {
    let route = ws.link_route(&entry.to, None);
    let active = route.as_ref() == Some(&ws.route);
    nav_row(
        entry.icon,
        &entry.label,
        entry.detail.clone().unwrap_or_default().into(),
        active,
        t,
    )
    .when_some(entry.badge.clone(), |row, (count, tone)| {
        row.child(
            mono(count, Type::MICRO, t.bg)
                .px(sp(1.5))
                .rounded(radius::BOX)
                .bg(t.tone(tone)),
        )
    })
    .id(id)
    .when_some(route, |row, route| {
        row.on_click(
            cx.listener(move |ws, _, _, cx| ws.navigate(route.clone(), cx)),
        )
    })
}

/// A sidebar section's heading, with an action at its end.
fn section(title: &str, action: Option<AnyElement>, t: &Theme) -> Div {
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .pt(sp(4.))
        .pb(sp(1.5))
        .px(sp(2.5))
        .child(div().flex_1().child(super::heading(title, t)))
        .children(action)
}

/// A row that opens a screen: an icon, a label, and a quiet detail.
fn nav_row(
    glyph: Icon,
    label: &str,
    meta: SharedString,
    active: bool,
    t: &Theme,
) -> Div {
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(sp(2.5))
        .h(px(34.))
        .px(sp(2.5))
        .rounded(radius::CONTROL)
        .cursor_pointer()
        .when(active, |row| row.pressed(t))
        .when(!active, |row| {
            row.hover(|style| style.bg(gpui::white().opacity(0.03)))
        })
        .child(icon(
            glyph,
            IconSize::BASE,
            if active { t.text } else { t.muted },
        ))
        .child(
            div()
                .flex_1()
                .text_color(if active { t.text } else { t.text_soft })
                .child(label.to_owned()),
        )
        .when(!meta.is_empty(), |row| {
            row.child(mono(meta, Type::MICRO, t.dim))
        })
}

/// A repository in the sidebar's tree: its row, then, when open, its
/// memory, constitution and runs.
fn repo_group(
    ws: &Workspace,
    rows: RepoRows<'_>,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let name = rows.repo.name.clone();
    let menu = ws.repo_menu.as_deref() == Some(name.as_str());
    let hovered = menu || ws.hovered_repo.as_deref() == Some(name.as_str());
    let action = |id: &'static str, glyph: Option<Icon>, t: &Theme| {
        div()
            .id(id)
            .size(px(24.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(radius::TAG)
            .key(t)
            .text_color(t.text_soft)
            .cursor_pointer()
            .hover(|style| style.border_color(t.border_strong))
            .map(|button| match glyph {
                Some(glyph) => {
                    button.child(icon(glyph, IconSize::COMPACT, t.text_soft))
                }
                None => button.child("···"),
            })
    };
    let head = div()
        .id(SharedString::from(format!("repo-{name}")))
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(sp(2.))
        .h(control::MEDIUM)
        .px(sp(2.))
        .rounded(radius::CONTROL)
        .cursor_pointer()
        .when(hovered, |row| row.bg(t.selected))
        .when(!hovered && rows.open, |row| row.bg(t.raised))
        .child(icon(
            if rows.open { Icon::Down } else { Icon::Chevron },
            IconSize::SMALL,
            t.muted,
        ))
        .child(super::repo_mark(rows.repo, 20., t))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .when(rows.open, |label| label.font_weight(weight::STRONG))
                .child(name.clone()),
        )
        .map(|row| {
            if hovered {
                let (new, more) = (name.clone(), name.clone());
                row.child(action("repo-new", Some(Icon::Plus), t).on_click(
                    cx.listener(move |ws, _, window, cx| {
                        cx.stop_propagation();
                        ws.new_run_in(&new, window, cx)
                    }),
                ))
                .child(action("repo-more", None, t).on_click(cx.listener(
                    move |ws, _, _, cx| {
                        cx.stop_propagation();
                        ws.toggle_repo_menu(&more, cx)
                    },
                )))
            } else {
                row.when(rows.live > 0, |row| {
                    row.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(sp(1.25))
                            .typeset(Type::MICRO)
                            .text_color(t.accent)
                            .child(live_dot(t.accent, 6.))
                            .child(rows.live.to_string()),
                    )
                })
                .child(mono(
                    rows.total.to_string(),
                    Type::MICRO,
                    t.dim,
                ))
            }
        })
        .on_hover({
            let name = name.clone();
            cx.listener(move |ws, hovered: &bool, _, cx| {
                ws.hover_repo(&name, *hovered, cx)
            })
        })
        .on_click({
            let name = name.clone();
            cx.listener(move |ws, _, _, cx| ws.toggle_repo_open(&name, cx))
        });

    let group = div()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .child(head)
        .when(menu, |group| group.child(repo_menu(&name, t, cx)));
    if !rows.open {
        return group;
    }

    let repo = rows.repo;
    let memory_active =
        matches!(&ws.route, Route::Memory { repo: r, .. } if *r == name);
    let rules_active =
        matches!(&ws.route, Route::Constitution { repo: r, .. } if *r == name);
    let reviews = ws
        .runs
        .iter()
        .filter(|run| ws.repo_of(run) == name)
        .flat_map(|run| run.reviews().map(move |card| (run, card)))
        .filter(|(run, card)| {
            matches!(card.state, crate::view::ToolState::Flagged { .. })
                && !ws
                    .dismissed
                    .contains(&(run.id.clone(), card.call_id.clone()))
        })
        .count();
    let current = ws.current().map(|run| run.id.clone());
    let on_run = matches!(ws.route, Route::Run(_) | Route::Home);

    let mut body = div()
        .flex()
        .flex_col()
        .gap(sp(0.5))
        .ml(sp(4.25))
        .pl(sp(2.))
        .pt(sp(0.5))
        .pb(sp(1.5))
        .border_l_1()
        .border_color(t.border)
        .child(
            nav_row(
                Icon::Memory,
                "Memory",
                format!("{} notes", repo.memory.notes.len()).into(),
                memory_active,
                t,
            )
            .id(SharedString::from(format!("memory-{name}")))
            .on_click({
                let name = name.clone();
                cx.listener(move |ws, _, _, cx| ws.open_memory(&name, cx))
            }),
        )
        .child(
            nav_row(
                Icon::Blocked,
                "Constitution",
                format!("{} rules", repo.constitution.rules.len()).into(),
                rules_active,
                t,
            )
            .when(reviews > 0, |row| {
                row.child(
                    mono(reviews.to_string(), Type::MICRO, t.bg)
                        .px(sp(1.5))
                        .rounded(radius::BOX)
                        .bg(t.accent),
                )
            })
            .id(SharedString::from(format!("constitution-{name}")))
            .on_click({
                let name = name.clone();
                cx.listener(move |ws, _, _, cx| ws.open_constitution(&name, cx))
            }),
        );
    // What plugins list under the repository.
    let entries = ws.contributions(
        tau_ui_plugin::points::SIDEBAR_REPO,
        &tau_ui_plugin::points::AtRepo { repo: name.clone() },
        cx,
    );
    for (n, entry) in entries.into_iter().enumerate() {
        body = body.child(nav_entry_row(
            ws,
            entry,
            SharedString::from(format!("nav-{name}-{n}")),
            t,
            cx,
        ));
    }
    for (n, run) in rows.runs.iter().enumerate() {
        // The first run is the main chat, when there is one.
        let limit = if n == 0 { rows.main_children } else { None };
        let tree = Tree {
            current: current.as_ref(),
            on_run,
            flat: rows.flat,
        };
        body = tree.rows(body, ws, run, 0, limit, t, cx);
    }
    if rows.older > 0 {
        let name = name.clone();
        body = body.child(
            super::text_link(
                format!("Show {} older runs", rows.older),
                Type::CAPTION,
                t,
            )
            .id(SharedString::from(format!("older-{name}")))
            .h(px(30.))
            .flex()
            .items_center()
            .px(sp(2.5))
            .cursor_pointer()
            .on_click(
                cx.listener(move |ws, _, _, cx| ws.show_older_runs(&name, cx)),
            ),
        );
    }
    group.child(body)
}

/// How a repository's tree is drawn: which conversation is open, and
/// whether a filter's matches are listed alone.
struct Tree<'a> {
    current: Option<&'a RunId>,
    on_run: bool,
    flat: bool,
}

impl Tree<'_> {
    /// `run`'s row, `depth` levels in, then what is under it: its forks
    /// and sub-agents newest first, `limit` of them when given.
    #[allow(clippy::too_many_arguments)]
    fn rows(
        &self,
        mut body: Div,
        ws: &Workspace,
        run: &RunView,
        depth: usize,
        limit: Option<usize>,
        t: &Theme,
        cx: &mut Context<Workspace>,
    ) -> Div {
        let active = self.on_run && self.current == Some(&run.id);
        body = body.child(run_row(ws, run, active, depth, t, cx));
        if self.flat {
            return body;
        }
        // A fork is a conversation of its own: it opens like one.
        let children: Vec<&RunView> = ws.open_children(run).collect();
        let shown = limit.unwrap_or(children.len());
        for child in children.into_iter().take(shown) {
            body = self.rows(body, ws, child, depth + 1, None, t, cx);
        }
        for child in &run.children {
            if ws.is_closed(&child.id) || ws.run(&child.id).is_some() {
                continue;
            }
            body = body.child(child_row(ws, run, child, depth + 1, t, cx));
        }
        body
    }
}

/// A child the workspace has no conversation for: a fork opens the
/// comparison with its run.
fn child_row(
    ws: &Workspace,
    run: &RunView,
    child: &crate::view::ChildRun,
    depth: usize,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let route = (child.kind == ChildKind::Fork).then(|| Route::Compare {
        main: run.id.clone(),
        fork: child.id.clone(),
    });
    let active = route.as_ref() == Some(&ws.route);
    div()
        .id(SharedString::from(format!("child-{}", child.id)))
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(sp(2.))
        .h(px(34.))
        .pl(sp(indent(depth)))
        .pr(sp(2.5))
        .rounded(radius::CONTROL)
        .text_color(t.text_soft)
        .when(active, |row| row.pressed(t))
        .child(icon(
            match child.kind {
                ChildKind::SubAgent => Icon::SubAgent,
                ChildKind::Fork => Icon::Fork,
            },
            IconSize::SMALL,
            t.blue,
        ))
        .child(div().flex_1().truncate().child(child.title.clone()))
        .when_some(route, |row, route| {
            row.cursor_pointer()
                .hover(|style| style.bg(gpui::white().opacity(0.03)))
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.navigate(route.clone(), cx)
                }))
        })
}

/// A tree row's left padding, `depth` levels in.
fn indent(depth: usize) -> f32 {
    2.5 + 4.5 * depth.min(4) as f32
}

/// A conversation in the tree: whether it is working, its title, and
/// its unread replies; hovering it offers to close it, unless it is its
/// repository's main chat. A fork sits under its run, with the fork
/// mark.
fn run_row(
    ws: &Workspace,
    run: &RunView,
    active: bool,
    depth: usize,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let nested = depth > 0;
    let route = Route::Run(run.id.clone());
    let hovered = ws.hovered_run.as_ref() == Some(&run.id);
    let unread = ws.unread(run);
    let (hover_id, close_id) = (run.id.clone(), run.id.clone());
    // What plugins add to the row: a line under the title, and a count
    // at the end.
    let note = ws.run_rows(run, cx).into_iter().next();
    let title = div()
        .truncate()
        .text_color(if active { t.text } else { t.text_soft })
        .when(active || unread > 0, |title| {
            title.font_weight(weight::EMPHASIS)
        })
        .child(run.title.clone());
    div()
        .id(SharedString::from(format!("run-{}", run.id)))
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(sp(2.5))
        .min_h(px(34.))
        .py(sp(note
            .as_ref()
            .and_then(|note| note.line.as_ref())
            .map_or(0., |_| 1.5)))
        .pl(sp(indent(depth)))
        .pr(sp(2.))
        .rounded(radius::CONTROL)
        .cursor_pointer()
        .when(active, |row| row.pressed(t))
        .when(!active, |row| {
            row.hover(|style| style.bg(gpui::white().opacity(0.03)))
        })
        .map(|row| {
            if nested {
                row.child(icon(Icon::Fork, IconSize::SMALL, t.blue))
            } else {
                row.child(status_icon(run, t, IconSize::SMALL))
            }
        })
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(sp(0.25))
                .child(title)
                .when_some(
                    note.as_ref()
                        .and_then(|note| Some((note.tone, note.line.clone()?))),
                    |column, (tone, line)| {
                        column.child(
                            div()
                                .truncate()
                                .typeset(Type::MICRO)
                                .text_color(
                                    if tone == crate::view::Tone::Warn {
                                        t.dim
                                    } else {
                                        t.tone(tone)
                                    },
                                )
                                .child(line),
                        )
                    },
                ),
        )
        .map(|row| {
            if hovered && !ws.is_main(&run.id) {
                row.child(
                    div()
                        .id("close-run")
                        .size(px(20.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(radius::TAG)
                        .hover(|style| style.bg(t.border_strong))
                        .child(icon(Icon::Close, IconSize::SMALL, t.text_soft))
                        .on_click(cx.listener(move |ws, _, _, cx| {
                            cx.stop_propagation();
                            ws.close_run(&close_id, cx)
                        })),
                )
            } else if unread > 0 {
                row.child(super::count_pill(unread, t))
            } else if let Some((glyph, tone, count)) =
                note.as_ref().and_then(|note| {
                    Some((note.icon, note.tone, note.count.clone()?))
                })
            {
                row.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(1.))
                        .child(icon(glyph, IconSize::SMALL, t.tone(tone)))
                        .child(mono(count, Type::MICRO, t.tone(tone))),
                )
            } else if nested && run.status.is_live() {
                row.child(live_dot(t.accent, 6.))
            } else {
                row
            }
        })
        .on_hover(cx.listener(move |ws, hovered: &bool, _, cx| {
            ws.hover_run(&hover_id, *hovered, cx)
        }))
        .on_click(
            cx.listener(move |ws, _, _, cx| ws.navigate(route.clone(), cx)),
        )
}

/// A repository's menu, under its row.
fn repo_menu(
    name: &str,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let entry = |id: &'static str, glyph: Icon, label: String, color| {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(sp(2.5))
            .h(px(34.))
            .px(sp(2.5))
            .rounded(radius::CONTROL)
            .cursor_pointer()
            .text_color(color)
            .hover(|style| style.bg(t.selected))
            .child(icon(
                glyph,
                IconSize::BASE,
                if color == t.red { t.red } else { t.muted },
            ))
            .child(label)
    };
    let owned = |name: &str| name.to_owned();
    let (new, memory, rules, update, files, remove) = (
        owned(name),
        owned(name),
        owned(name),
        owned(name),
        owned(name),
        owned(name),
    );
    let menu = div()
        .id("repo-menu")
        .w(px(232.))
        .flex()
        .flex_col()
        .p(sp(1.5))
        .raised(t)
        .border_1()
        .border_color(t.border_strong)
        .rounded(radius::LARGE)
        .child(
            entry("menu-new", Icon::Plus, format!("New run in {name}"), t.text)
                .on_click(cx.listener(move |ws, _, window, cx| {
                    ws.new_run_in(&new, window, cx)
                })),
        )
        .child(
            entry("menu-memory", Icon::Memory, "Memory".into(), t.text)
                .on_click(
                    cx.listener(move |ws, _, _, cx| {
                        ws.open_memory(&memory, cx)
                    }),
                ),
        )
        .child(
            entry("menu-rules", Icon::Blocked, "Constitution".into(), t.text)
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.open_constitution(&rules, cx)
                })),
        )
        .child(
            entry(
                "menu-update",
                Icon::Arrow,
                "Fetch new commits".into(),
                t.text,
            )
            .on_click(
                cx.listener(move |ws, _, _, cx| ws.update_repo(&update, cx)),
            ),
        )
        .child(
            entry("menu-files", Icon::Folder, "Show in files".into(), t.text)
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.show_in_files(&files, cx)
                })),
        )
        .child(div().h(px(1.)).my(sp(1.)).bg(t.border))
        .child(
            entry(
                "menu-remove",
                Icon::Warning,
                "Remove from tau".into(),
                t.red,
            )
            .on_click(
                cx.listener(move |ws, _, _, cx| ws.remove_repo(&remove, cx)),
            ),
        )
        .on_mouse_down_out(cx.listener(|ws, _, _, cx| ws.close_repo_menu(cx)));
    deferred(
        anchored()
            .anchor(Anchor::TopLeft)
            .offset(point(px(120.), px(2.)))
            .snap_to_window_with_margin(px(8.))
            .child(menu),
    )
    .with_priority(1)
}

pub fn status_bar(ws: &Workspace, t: &Theme) -> Div {
    let context = ws.current().map(|run| {
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
        .gap(sp(4.5))
        .px(sp(3.5))
        .chrome(Edge::Bottom, t)
        .font_family(MONO)
        .typeset(Type::MICRO)
        .text_color(t.muted)
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(1.5))
                .child(live_dot(t.green, 6.))
                .child("tau-ui"),
        )
        .children(context)
        .children(match &ws.catalog.project {
            ProjectStatus::Unknown => None,
            ProjectStatus::Importing(name) => Some(
                div()
                    .text_color(t.accent)
                    .child(format!("importing {name}…")),
            ),
            ProjectStatus::Updating(name) => Some(
                div()
                    .text_color(t.accent)
                    .child(format!("updating {name}…")),
            ),
            ProjectStatus::Failed(name) => Some(
                div()
                    .text_color(t.red)
                    .child(format!("{name} could not be imported")),
            ),
        })
        .children(ws.catalog.update.clone().map(|text| div().child(text)))
        .child(div().flex_1())
        .child("Esc back")
        .child(format!(
            "{} · {} runs · {} repos",
            ws.catalog.store.path,
            ws.runs.len(),
            ws.catalog.repos.len()
        ))
}

/// The phone's header above a run: back, title and status, close,
/// details.
pub fn phone_run_bar(
    run: &RunView,
    main: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let (color, label) = status_look(&run.status, t);
    let id = run.id.clone();
    div()
        .h(px(56.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(sp(1.))
        .px(sp(1.5))
        .chrome(Edge::Top, t)
        .child(
            icon_button("phone-back", Icon::Back, 44., t)
                .on_click(cx.listener(|ws, _, _, cx| ws.back(cx))),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(sp(0.5))
                .child(
                    div()
                        .typeset(Type::LEAD)
                        .font_weight(weight::STRONG)
                        .truncate()
                        .child(run.title.clone()),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(1.5))
                        .typeset(Type::CAPTION)
                        .text_color(t.muted)
                        .child(dot(color, 6.))
                        .child(label)
                        .child(mono(
                            format!(
                                "· turn {} · {}",
                                run.turn,
                                usd(run.usage.cost)
                            ),
                            Type::CAPTION,
                            t.muted,
                        )),
                ),
        )
        // A repository's main chat stays open.
        .when(!main, |bar| {
            bar.child(icon_button("phone-close", Icon::Close, 44., t).on_click(
                cx.listener(move |ws, _, _, cx| ws.close_run_to_list(&id, cx)),
            ))
        })
        .child(
            icon_button("phone-details", Icon::Panel, 44., t)
                .on_click(cx.listener(|ws, _, _, cx| ws.toggle_sheet(cx))),
        )
}

/// The phone's header on every other screen.
pub fn phone_header(
    ws: &Workspace,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let title = match &ws.route {
        Route::Memory {
            repo,
            note: Some(id),
        } => ws
            .repo_named(repo)
            .memory
            .note(id)
            .map_or("Memory".to_owned(), |note| note.title.clone()),
        Route::Home => "Runs".to_owned(),
        _ => ws.route_title(cx),
    };
    div()
        .h(px(56.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(sp(1.))
        .px(sp(1.5))
        .chrome(Edge::Top, t)
        .when(!ws.route.is_top_level(), |bar| {
            bar.child(
                icon_button("phone-back", Icon::Back, 44., t)
                    .on_click(cx.listener(|ws, _, _, cx| ws.back(cx))),
            )
        })
        .when(ws.route.is_top_level(), |bar| {
            bar.pl(sp(4.)).child(logo(t, 28.)).child(div().w(px(6.)))
        })
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .typeset(Type::TITLE)
                .font_weight(weight::STRONG)
                .child(title),
        )
        .when(matches!(ws.route, Route::Home), |bar| {
            bar.child(
                icon_button("phone-add-repo", Icon::Folder, 44., t).on_click(
                    cx.listener(|ws, _, _, cx| ws.pick_github_repos(cx)),
                ),
            )
        })
}

/// The phone's home: runs grouped by repository, each group with its
/// memory and rules.
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
        .min_h(px(0.))
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .children(groups)
        .when(ws.catalog.repos.is_empty(), |list| {
            list.child(super::empty(
                "No repositories yet. Add one to start runs in it.",
                t,
            ))
        })
}

fn phone_group(
    ws: &Workspace,
    rows: RepoRows<'_>,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let name = rows.repo.name.clone();
    let head = div()
        .id(SharedString::from(format!("phone-repo-{name}")))
        .flex()
        .items_center()
        .gap(sp(3.))
        .min_h(px(52.))
        .px(sp(4.))
        .chrome(Edge::Top, t)
        .cursor_pointer()
        .child(icon(
            if rows.open { Icon::Down } else { Icon::Chevron },
            IconSize::BASE,
            t.muted,
        ))
        .child(super::repo_mark(rows.repo, 26., t))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .typeset(Type::SUBTITLE)
                .font_weight(weight::STRONG)
                .child(name.clone()),
        )
        .when(rows.live > 0, |row| {
            row.child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(1.25))
                    .typeset(Type::CAPTION)
                    .text_color(t.accent)
                    .child(dot(t.accent, 7.))
                    .child(rows.live.to_string()),
            )
        })
        .child(
            icon_button("phone-repo-new", Icon::Plus, 44., t)
                .mr(sp(-3.))
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
    let chip = |id: &'static str, glyph: Icon, label: &str, count: usize| {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(sp(1.5))
            .px(sp(3.))
            .py(sp(2.))
            .rounded(radius::BOX)
            .key(t)
            .typeset(Type::SMALL)
            .cursor_pointer()
            .child(icon(glyph, IconSize::BASE, t.text_soft))
            .child(label.to_owned())
            .child(mono(count.to_string(), Type::SMALL, t.dim))
    };
    let (memory, rules) = (name.clone(), name.clone());
    let chips = div()
        .flex()
        .gap(sp(2.))
        .px(sp(4.))
        .py(sp(2.5))
        .border_b_1()
        .border_color(t.border)
        .child(
            chip(
                "phone-memory",
                Icon::Memory,
                "Memory",
                rows.repo.memory.notes.len(),
            )
            .on_click(
                cx.listener(move |ws, _, _, cx| ws.open_memory(&memory, cx)),
            ),
        )
        .child(
            chip(
                "phone-rules",
                Icon::Blocked,
                "Rules",
                rows.repo.constitution.rules.len(),
            )
            .on_click(
                cx.listener(move |ws, _, _, cx| {
                    ws.open_constitution(&rules, cx)
                }),
            ),
        );
    // What plugins list under the repository.
    let entries = ws.contributions(
        tau_ui_plugin::points::SIDEBAR_REPO,
        &tau_ui_plugin::points::AtRepo { repo: name.clone() },
        cx,
    );
    let mut chips = chips;
    for (n, entry) in entries.into_iter().enumerate() {
        let Some(route) = ws.link_route(&entry.to, None) else {
            continue;
        };
        chips = chips.child(
            div()
                .id(SharedString::from(format!("phone-nav-{n}")))
                .flex()
                .items_center()
                .gap(sp(1.5))
                .px(sp(3.))
                .py(sp(2.))
                .rounded(radius::BOX)
                .key(t)
                .typeset(Type::SMALL)
                .cursor_pointer()
                .child(icon(entry.icon, IconSize::BASE, t.text_soft))
                .child(entry.label.clone())
                .children(
                    entry
                        .detail
                        .clone()
                        .map(|detail| mono(detail, Type::SMALL, t.dim)),
                )
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.navigate(route.clone(), cx)
                })),
        );
    }
    let mut group = group.child(chips);
    for (n, run) in rows.runs.iter().enumerate() {
        group = group.child(phone_run_row(ws, run, t, cx));
        if rows.flat {
            continue;
        }
        // The main chat's chats are listed like it, newest first, each
        // with what is under it.
        let chats: Vec<&RunView> = if n == 0 && ws.is_main(&run.id) {
            ws.open_children(run)
                .take(rows.main_children.unwrap_or(usize::MAX))
                .collect()
        } else {
            Vec::new()
        };
        group = phone_children(group, ws, run, &chats, t, cx);
        for chat in chats {
            group = group.child(phone_run_row(ws, chat, t, cx));
            group = phone_children(group, ws, chat, &[], t, cx);
        }
    }
    if rows.older > 0 {
        group = group.child(
            super::text_link(
                format!("Show {} older runs", rows.older),
                Type::SMALL,
                t,
            )
            .id(SharedString::from(format!("phone-older-{name}")))
            .min_h(px(44.))
            .flex()
            .items_center()
            .px(sp(4.))
            .border_b_1()
            .border_color(t.border)
            .cursor_pointer()
            .on_click(
                cx.listener(move |ws, _, _, cx| ws.show_older_runs(&name, cx)),
            ),
        );
    }
    group
}

/// `run`'s open children that open as chats of their own, but for
/// those listed as rows already (`listed`).
fn phone_children(
    mut group: Div,
    ws: &Workspace,
    run: &RunView,
    listed: &[&RunView],
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    for (route, child) in run
        .children
        .iter()
        .filter(|child| !ws.is_closed(&child.id))
        .filter(|child| !listed.iter().any(|view| view.id == child.id))
        .filter_map(|child| child_route(ws, child))
    {
        group = group.child(phone_child_row(route, child, t, cx));
    }
    group
}

/// A phone lists a run's children that open as chats of their own:
/// forks, and sub-agents with a chat in this workspace.
fn child_route<'a>(
    ws: &Workspace,
    child: &'a crate::view::ChildRun,
) -> Option<(Route, &'a crate::view::ChildRun)> {
    (child.kind == ChildKind::Fork || ws.run(&child.id).is_some())
        .then(|| (Route::Run(child.id.clone()), child))
}

fn phone_run_row(
    ws: &Workspace,
    run: &RunView,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let (color, label) = status_look(&run.status, t);
    let route = Route::Run(run.id.clone());
    let unread = ws.unread(run);
    let meta = if run.status.is_live() {
        format!("{label} · turn {}", run.turn)
    } else {
        label.to_string()
    };
    // What plugins add to the row: where a goal stands, under the
    // title, and its count at the end.
    let note = ws.run_rows(run, cx).into_iter().next();
    let (meta, color) = match note
        .as_ref()
        .and_then(|note| Some((note.phone_line.clone()?, note.tone)))
    {
        Some((line, tone)) => (line, t.tone(tone)),
        None => (meta, color),
    };
    let badge = note.as_ref().and_then(|note| {
        Some((note.count.clone()?, t.tone(note.tone), note.icon))
    });
    div()
        .id(SharedString::from(format!("phone-run-{}", run.id)))
        .flex()
        .items_center()
        .gap(sp(3.))
        .min_h(px(56.))
        .px(sp(4.))
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
                .gap(sp(0.75))
                .child(
                    div()
                        .truncate()
                        .when(unread > 0, |title| {
                            title.font_weight(weight::EMPHASIS)
                        })
                        .child(run.title.clone()),
                )
                .child(mono(meta, Type::MICRO, color).truncate()),
        )
        .when(unread > 0, |row| row.child(super::count_pill(unread, t)))
        .when_some(badge.filter(|_| unread == 0), |row, (badge, tone, _)| {
            row.child(mono(badge, Type::CAPTION, tone))
        })
        .on_click(
            cx.listener(move |ws, _, _, cx| ws.navigate(route.clone(), cx)),
        )
}

fn phone_child_row(
    route: Route,
    child: &crate::view::ChildRun,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let (color, label) = status_look(&child.status, t);
    div()
        .id(SharedString::from(format!("phone-child-{}", child.id)))
        .flex()
        .items_center()
        .gap(sp(3.))
        .min_h(px(56.))
        .pl(sp(11.))
        .pr(sp(4.))
        .border_b_1()
        .border_color(t.border)
        .cursor_pointer()
        .child(icon(Icon::Fork, IconSize::COMPACT, t.blue))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(sp(0.75))
                .child(div().truncate().child(child.title.clone()))
                .child(mono(label, Type::MICRO, color)),
        )
        .on_click(
            cx.listener(move |ws, _, _, cx| ws.navigate(route.clone(), cx)),
        )
}

pub fn phone_tab_bar(
    ws: &Workspace,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let current = ws.route.tab();
    div()
        .h(px(64.))
        .flex_shrink_0()
        .grid()
        .grid_cols(4)
        .chrome(Edge::Bottom, t)
        .children(Tab::ALL.into_iter().map(|tab| {
            let active = tab == current;
            let color = if active { t.text } else { t.muted };
            div()
                .id(tab.label())
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(sp(1.))
                .typeset(Type::MICRO)
                .text_color(color)
                .cursor_pointer()
                .child(icon(tab_icon(tab), IconSize::HUGE, color))
                .child(tab.label())
                .on_click(
                    cx.listener(move |ws, _, _, cx| ws.switch_tab(tab, cx)),
                )
        }))
}
