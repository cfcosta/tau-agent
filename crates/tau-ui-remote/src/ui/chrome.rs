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
    rems,
};

use super::{
    Edge,
    Material,
    attention_color,
    attention_icon,
    attention_tint,
    dot,
    icon,
    icon_button,
    logo,
    mono,
    status_look,
};
use crate::{
    assets::Icon,
    attention::Attention,
    catalog::ProjectStatus,
    repos::{RepoRows, TreeRow},
    route::{Route, Tab},
    theme::{Design as _, IconSize, MONO, Theme, Type, radius, sp, weight},
    view::{ChildKind, Ending, Origin, RunView, usd},
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

/// The desktop sidebar: a new run in the selected repository, the
/// repositories as a tree (each with what plugins list under it, and its
/// runs),
/// then what is the same everywhere.
pub fn sidebar(
    ws: &Workspace,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> gpui::Stateful<Div> {
    let filter = ws.sidebar_filter.read(cx).text().to_owned();
    let rows = ws.repo_rows(&filter);
    let many = ws.catalog.listed().count() > 3;

    let new_run = div()
        .id("new-run")
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(sp(2.))
        .h(rems(2.125))
        .px(sp(2.5))
        .rounded(radius::CONTROL)
        .cursor_pointer()
        .text_color(t.text_soft)
        .hover(|style| style.bg(t.raised))
        .child(icon(Icon::Plus, IconSize::BASE, t.roles.branch))
        .child(div().flex_1().child("New run"))
        .child(mono("Ctrl N", Type::MICRO, t.dim))
        .on_click(
            cx.listener(|ws, _, window, cx| ws.start_new_run(window, cx)),
        );

    let filter_field = div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(sp(2.))
        .h(rems(2.125))
        .mt(sp(2.))
        .px(sp(2.5))
        .well(t)
        .rounded(radius::CONTROL)
        .child(icon(Icon::Search, IconSize::COMPACT, t.dim))
        .child(
            div()
                .flex_1()
                .min_w(rems(0.))
                .child(ws.sidebar_filter.clone()),
        );

    let add = div()
        .id("add-repo")
        .size(rems(1.375))
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius::TAG)
        .cursor_pointer()
        .hover(|style| style.bg(t.selected))
        .child(icon(Icon::Plus, IconSize::COMPACT, t.muted))
        .on_click(cx.listener(|ws, _, _, cx| ws.pick_github_repos(cx)));

    let plugins = ws.catalog.plugins.len().to_string();
    let phones = match ws.phones.paired.len() {
        0 => String::new(),
        n => format!("{n} paired"),
    };
    let everywhere = [
        (Route::History, "History", "all repos".to_owned(), t.dim),
        (Route::Plugins, "Plugins", plugins, t.dim),
        (Route::Phones, "Phones", phones, t.green),
        // Sign-ins, keys and the models runs use.
        (Route::Models, "Models and accounts", String::new(), t.dim),
    ];

    // The app's name, and what all runs cost today.
    let total: f64 = ws.runs.iter().map(|run| run.usage.cost).sum();
    let brand = div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(sp(2.5))
        .px(sp(2.))
        .pt(sp(1.))
        .pb(sp(3.))
        .child(logo(t, 22.))
        .child(div().font_weight(weight::STRONG).child("tau"))
        .child(div().flex_1())
        .child(
            div()
                .flex()
                .gap(sp(1.))
                .typeset(Type::CAPTION)
                .text_color(t.dim)
                .child("today")
                .child(div().text_color(t.roles.cost).child(usd(total))),
        );

    div()
        .id("sidebar")
        .flex()
        .flex_col()
        .gap(sp(0.5))
        .p(sp(2.5))
        .bg(t.panel)
        .border_r_1()
        .border_color(t.border)
        .overflow_y_scroll()
        .child(brand)
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
        .when(ws.catalog.listed().next().is_none(), |bar| {
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
        .children(everywhere.into_iter().map(|(route, label, meta, ink)| {
            let active = ws.route == route;
            div()
                .id(label)
                .flex()
                .flex_shrink_0()
                .items_center()
                .h(rems(1.875))
                .px(sp(2.))
                .rounded(radius::BOX)
                .cursor_pointer()
                .text_color(if active { t.text } else { t.text_soft })
                .when(active, |row| row.bg(t.raised))
                .when(!active, |row| row.hover(|style| style.bg(t.card)))
                .child(div().flex_1().child(label))
                .child(div().typeset(Type::CAPTION).text_color(ink).child(meta))
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.navigate(route.clone(), cx)
                }))
        }))
}

/// A heading in the sidebar, with an action at its end.
fn section(title: &str, action: Option<AnyElement>, t: &Theme) -> Div {
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .pt(sp(3.5))
        .pb(sp(1.5))
        .px(sp(2.))
        .typeset(Type::CAPTION)
        .text_color(t.dim)
        .child(div().flex_1().child(title.to_owned()))
        .children(action)
}

/// A repository in the sidebar's tree: its row, then, when open, what
/// plugins list under it, and its runs.
fn repo_group(
    ws: &Workspace,
    rows: RepoRows<'_>,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let name = rows.repo.name.clone();
    let menu = ws.repo_menu.as_deref() == Some(name.as_str());
    let hovered = menu || ws.hovered_repo.as_deref() == Some(name.as_str());
    let need_you = ws.need_you(&name, cx);
    let on_page = super::screens::repo::owner(ws, &ws.route, cx).as_deref()
        == Some(name.as_str());
    let action = |id: &'static str, glyph: Option<Icon>, t: &Theme| {
        div()
            .id(id)
            .size(rems(1.5))
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
        .h(rems(2.))
        .px(sp(2.))
        .rounded(radius::BOX)
        .cursor_pointer()
        .text_color(if rows.open { t.text } else { t.text_soft })
        .when(on_page, |row| row.bg(t.raised))
        .when(hovered && !on_page, |row| row.bg(t.card))
        // The chevron folds the repository; the rest of the row opens
        // its page.
        .child(
            div()
                .id(SharedString::from(format!("fold-{name}")))
                .size(rems(1.))
                .flex()
                .items_center()
                .justify_center()
                .child(icon(
                    if rows.open { Icon::Down } else { Icon::Chevron },
                    IconSize::TINY,
                    t.dim,
                ))
                .on_click({
                    let name = name.clone();
                    cx.listener(move |ws, _, _, cx| {
                        cx.stop_propagation();
                        ws.toggle_repo_open(&name, cx)
                    })
                }),
        )
        .child(super::repo_mark(rows.repo, 18., t))
        .child(
            div()
                .flex_1()
                .min_w(rems(0.))
                .truncate()
                .when(rows.open, |label| label.font_weight(weight::EMPHASIS))
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
                let branch = rows
                    .repo
                    .trunk
                    .clone()
                    .unwrap_or_else(|| "main".to_owned());
                row.when(need_you > 0, |row| {
                    row.child(need_you_pill(need_you, Type::MICRO, t))
                })
                .map(|row| {
                    if rows.open {
                        let ahead = ws.unpushed(&name).map(|(ahead, _)| ahead);
                        row.child(mono(branch, Type::MICRO, t.roles.branch))
                            .when_some(ahead, |row, ahead| {
                                row.child(mono(
                                    format!("↑{ahead}"),
                                    Type::MICRO,
                                    t.roles.waiting,
                                ))
                            })
                    } else {
                        row.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(sp(1.5))
                                .typeset(Type::CAPTION)
                                .text_color(t.dim)
                                .when(rows.live > 0, |count| {
                                    count.child(dot(t.roles.live, 6.))
                                })
                                .child(rows.total.to_string()),
                        )
                    }
                })
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
            cx.listener(move |ws, _, _, cx| ws.open_repo_page(&name, cx))
        });

    let group = div()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .child(head)
        .when(menu, |group| group.child(repo_menu(ws, &name, t, cx)));
    if !rows.open {
        return group;
    }

    // A sub-agent that ended is not listed: its main chat stands for it.
    let current = ws.current().map(|run| ws.listed_as(run).clone());
    let on_run = matches!(ws.route, Route::Run(_) | Route::Home);

    let mut body = div()
        .flex()
        .flex_col()
        .gap(sp(0.25))
        .pl(sp(6.5))
        .pt(sp(0.5))
        .pb(sp(1.5));
    // What plugins keep for the repository is on its page; the tree
    // lists its runs.
    for row in ws.repo_tree(&rows) {
        body = match row {
            TreeRow::Run { run, depth, folded } => {
                let active = on_run && current.as_ref() == Some(&run.id);
                body.child(run_row(ws, run, active, depth, folded, t, cx))
            }
            TreeRow::Child { child, depth, .. } => {
                body.child(child_row(ws, &row, child, depth, t, cx))
            }
        };
    }
    if rows.older > 0 {
        let name = name.clone();
        body = body.child(
            div()
                .typeset(Type::CAPTION)
                .text_color(t.dim)
                .hover(|style| style.text_color(t.text_soft))
                .child(format!("{} older runs", rows.older))
                .id(SharedString::from(format!("older-{name}")))
                .h(rems(1.625))
                .flex()
                .items_center()
                .pl(sp(5.5))
                .cursor_pointer()
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.show_older_runs(&name, cx)
                })),
        );
    }
    group.child(body)
}

/// How many of a repository's chats need the person: they ask, would
/// conflict, or are ready to land.
fn need_you_pill(count: usize, size: Type, t: &Theme) -> Div {
    div()
        .flex_shrink_0()
        .px(sp(1.75))
        .rounded(radius::FULL)
        .border_1()
        .border_color(t.blue_border)
        .bg(t.blue.opacity(0.1))
        .typeset(size)
        .text_color(t.blue)
        .child(format!("{count} need you"))
}

/// A child the workspace has no conversation for: a fork opens the
/// comparison with its run.
fn child_row(
    ws: &Workspace,
    row: &TreeRow<'_>,
    child: &crate::view::ChildRun,
    depth: usize,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let route = row.route();
    let active = route.as_ref() == Some(&ws.route);
    div()
        .id(SharedString::from(format!("child-{}", child.id)))
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(sp(2.))
        .h(rems(2.125))
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

/// A conversation in the tree: an icon in the color of where it stands,
/// git's way (a draft at work, a pull request to land, a merge once it
/// landed), its title, and its counts: changes to push or land,
/// conflicting files, its place in the queue. Its unread replies show
/// as a count in their place. Hovering it offers to close it, unless it
/// is its repository's main chat or it ended for good. A fork or
/// sub-agent sits under its run, which folds them away with a chevron
/// when `folded` is given.
fn run_row(
    ws: &Workspace,
    run: &RunView,
    active: bool,
    depth: usize,
    folded: Option<bool>,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let route = Route::Run(run.id.clone());
    let hovered = ws.hovered_run.as_ref() == Some(&run.id);
    let unread = ws.unread(run);
    let (hover_id, close_id, fold_id) =
        (run.id.clone(), run.id.clone(), run.id.clone());
    let attention = ws.attention(run, cx);
    let ended = matches!(attention, Attention::Landed | Attention::Dropped)
        || ws.has_ended(run);
    let note = ws.run_rows(run, cx).into_iter().next();
    let is_main = ws.is_main(&run.id);
    let unpushed = is_main
        .then(|| ws.main_repo(&run.id))
        .flatten()
        .and_then(|repo| ws.unpushed(repo))
        .map_or(0, |(ahead, _)| ahead);
    let (glyph, ink) = state_icon(&attention, run, is_main, unpushed, t);
    let counts = counts(&attention, run, note.as_ref(), unpushed, t);
    // Main's crew at work, counted while main is folded: unfolded, each
    // is a row under it.
    let crew = if is_main && folded == Some(true) {
        ws.working_crew(&run.id)
    } else {
        0
    };
    let strong = active || unread > 0 || attention.needs_you();
    div()
        .id(SharedString::from(format!("run-{}", run.id)))
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(sp(2.))
        .h(rems(1.875))
        .pl(sp(2. + 4. * depth.min(4) as f32))
        .pr(sp(2.))
        .rounded(radius::BOX)
        .cursor_pointer()
        .text_color(if ended {
            t.dim
        } else if active || strong {
            t.text
        } else {
            t.text_soft
        })
        .when(active, |row| row.bg(t.raised))
        .when(!active, |row| row.hover(|style| style.bg(t.card)))
        .child(icon(
            glyph,
            IconSize::SMALL,
            if ended { t.dim } else { ink },
        ))
        .child(
            div()
                .flex_1()
                .min_w(rems(0.))
                .truncate()
                .when(strong, |title| title.font_weight(weight::EMPHASIS))
                .child(run.title.clone()),
        )
        .when(crew > 0, |row| {
            row.child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(sp(0.75))
                    .child(icon(Icon::SubAgent, IconSize::TINY, t.roles.live))
                    .child(mono(crew.to_string(), Type::CAPTION, t.roles.live)),
            )
        })
        .when_some(folded, |row, folded| {
            row.child(
                div()
                    .id("fold-run")
                    .size(rems(1.125))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(radius::TAG)
                    .hover(|style| style.bg(t.border_strong))
                    .child(icon(
                        if folded { Icon::Chevron } else { Icon::Down },
                        IconSize::TINY,
                        t.dim,
                    ))
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        cx.stop_propagation();
                        ws.toggle_fold(&fold_id, cx)
                    })),
            )
        })
        .map(|row| {
            // A sub-agent is not closed: it leaves the list as it ends,
            // and the tray's Stop stops it.
            let sub_agent = matches!(run.origin, Origin::SubAgent { .. });
            if hovered && !ended && !ws.is_main(&run.id) && !sub_agent {
                row.child(
                    div()
                        .id("close-run")
                        .size(rems(1.125))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(radius::TAG)
                        .hover(|style| style.bg(t.border_strong))
                        .child(icon(Icon::Close, IconSize::TINY, t.text_soft))
                        .on_click(cx.listener(move |ws, _, _, cx| {
                            cx.stop_propagation();
                            ws.close_run(&close_id, cx)
                        })),
                )
            } else if unread > 0 {
                row.child(super::count_pill(unread, t))
            } else {
                row.child(
                    div().flex().gap(sp(1.5)).children(
                        counts.into_iter().map(|(count, ink)| {
                            mono(count, Type::CAPTION, ink)
                        }),
                    ),
                )
            }
        })
        .on_hover(cx.listener(move |ws, hovered: &bool, _, cx| {
            ws.hover_run(&hover_id, *hovered, cx)
        }))
        .on_click(
            cx.listener(move |ws, _, _, cx| ws.navigate(route.clone(), cx)),
        )
}

/// The icon for where a run stands, and its color: git's, for what is
/// git's (a branch for main, at work or at rest; a draft at work, a pull
/// request to land, a merge or a closed one at the end), else what it
/// needs of the person.
/// A run at rest is drawn as what it is: a fork, a sub-agent, a chat.
pub fn state_icon(
    attention: &Attention,
    run: &RunView,
    is_main: bool,
    unpushed: u32,
    t: &Theme,
) -> (Icon, gpui::Hsla) {
    match attention {
        // Main is the branch others land on, whatever it does.
        Attention::Working { .. } if is_main => (Icon::Branch, t.roles.live),
        Attention::Working { .. }
            if matches!(run.origin, Origin::SubAgent { .. }) =>
        {
            (Icon::SubAgent, t.roles.live)
        }
        Attention::Working { .. } => (Icon::Draft, t.roles.live),
        Attention::Asks { .. } => (Icon::Question, t.roles.waiting),
        Attention::ReadyToLand { .. } => (Icon::PullRequest, t.green),
        Attention::WouldConflict { .. } | Attention::ConflictsOnMain { .. } => {
            (Icon::Warning, t.red)
        }
        Attention::Queued(queued) if queued.needs_confirmation => {
            (Icon::Clock, t.roles.waiting)
        }
        Attention::Queued(_) => (Icon::Clock, t.dim),
        Attention::Interrupted => (Icon::Interrupted, t.dim),
        Attention::Failed => (Icon::Failed, t.red),
        Attention::Landed => (Icon::Merge, t.change),
        Attention::Dropped => (Icon::Closed, t.dim),
        Attention::Idle if is_main => (
            Icon::Branch,
            if unpushed > 0 { t.roles.waiting } else { t.dim },
        ),
        Attention::Idle => match run.origin {
            Origin::Fork { .. } => (Icon::Fork, t.dim),
            Origin::SubAgent { .. } => (Icon::SubAgent, t.dim),
            Origin::Root => (Icon::Chat, t.dim),
        },
    }
}

/// A run's counts, each in its color: main's changes to push, a fork's
/// changes to land or its conflicting files, its place in the landing
/// queue, the changes it landed; else what a plugin says of it (a goal
/// met).
pub fn counts(
    attention: &Attention,
    run: &RunView,
    note: Option<&tau_ui_plugin::RowNote>,
    unpushed: u32,
    t: &Theme,
) -> Vec<(String, gpui::Hsla)> {
    let files = |n: usize| {
        if n == 1 {
            "1 file".to_owned()
        } else {
            format!("{n} files")
        }
    };
    let mut out = Vec::new();
    if unpushed > 0 {
        out.push((format!("↑{unpushed}"), t.roles.waiting));
    }
    match attention {
        Attention::ReadyToLand { changes } => {
            out.push((changes.to_string(), t.green));
        }
        Attention::WouldConflict { files: names }
        | Attention::ConflictsOnMain { files: names } => {
            out.push((files(names.len()), t.red));
        }
        Attention::Queued(queued) => {
            out.push((format!("#{}", queued.position), t.dim));
        }
        Attention::Landed => {
            if let Some(Ending::Landed { changes, .. }) = run.ending
                && changes > 0
            {
                out.push((changes.to_string(), t.dim));
            }
        }
        Attention::Idle => {
            if let Some(said) = note
                .and_then(|note| Some((note.count.clone()?, t.tone(note.tone))))
            {
                out.push(said);
            }
        }
        _ => {}
    }
    out
}

/// A repository's menu, under its row.
fn repo_menu(
    ws: &Workspace,
    name: &str,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    // What plugins keep per repository: their entries, after tau's.
    let plugins = ws.contributions(
        tau_ui_plugin::points::REPO_MENU,
        &tau_ui_plugin::points::AtRepo {
            repo: name.to_owned(),
        },
        cx,
    );
    let entry = |id: &'static str, glyph: Icon, label: String, color| {
        div()
            .id(id)
            .debug_selector(|| id.to_owned())
            .flex()
            .items_center()
            .gap(sp(2.5))
            .h(rems(2.125))
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
    let (new, memory, update, files, remove) = (
        owned(name),
        owned(name),
        owned(name),
        owned(name),
        owned(name),
    );
    let menu = div()
        .id("repo-menu")
        .min_w(rems(14.5))
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
        .when(!plugins.is_empty(), |menu| {
            menu.child(div().h(rems(0.0625)).my(sp(1.)).bg(t.border))
                .children(plugins)
        })
        .child(div().h(rems(0.0625)).my(sp(1.)).bg(t.border))
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
        .occlude()
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

/// The bar under the desktop, when there is news: a repository being
/// imported or failing to, or an update. Without any, there is no bar.
pub fn status_bar(ws: &Workspace, t: &Theme) -> Option<Div> {
    let busy = !matches!(ws.catalog.project, ProjectStatus::Unknown);
    if !busy && ws.catalog.update.is_none() {
        return None;
    }
    Some(
        div()
            .h(rems(1.625))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(sp(4.5))
            .px(sp(3.5))
            .chrome(Edge::Bottom, t)
            .font_family(MONO)
            .typeset(Type::MICRO)
            .text_color(t.muted)
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
            .children(ws.catalog.update.clone().map(|text| div().child(text))),
    )
}

/// The phone's header above a run: back, title and status, close,
/// details.
pub fn phone_run_bar(
    run: &RunView,
    main: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let (color, label) = super::run_look(run, t);
    let id = run.id.clone();
    div()
        .h(rems(3.5))
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
                .min_w(rems(0.))
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
        Route::Home => "Runs".to_owned(),
        _ => ws.route_title(cx),
    };
    div()
        .h(rems(3.5))
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
            bar.pl(sp(4.))
                .child(logo(t, 28.))
                .child(div().w(rems(0.375)))
        })
        .child(
            div()
                .flex_1()
                .min_w(rems(0.))
                .truncate()
                .typeset(Type::TITLE)
                .font_weight(weight::STRONG)
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
                icon_button("phone-add-repo", Icon::Folder, 44., t).on_click(
                    cx.listener(|ws, _, _, cx| ws.pick_github_repos(cx)),
                ),
            )
        })
}

/// The phone's home: runs grouped by repository, each group with what
/// plugins list under it.
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
        .children(groups)
        .when(ws.catalog.listed().next().is_none(), |list| {
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
    let need_you = ws.need_you(&name, cx);
    let head = div()
        .id(SharedString::from(format!("phone-repo-{name}")))
        .flex()
        .items_center()
        .gap(sp(3.))
        .min_h(rems(3.25))
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
                .min_w(rems(0.))
                .truncate()
                .typeset(Type::SUBTITLE)
                .font_weight(weight::STRONG)
                .child(name.clone()),
        )
        .when(need_you > 0, |row| {
            row.child(need_you_pill(need_you, Type::CAPTION, t))
        })
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
    let chips = div()
        .flex()
        .gap(sp(2.))
        .px(sp(4.))
        .py(sp(2.5))
        .border_b_1()
        .border_color(t.border);
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
            super::text_link(
                format!("Show {} older runs", rows.older),
                Type::SMALL,
                t,
            )
            .id(SharedString::from(format!("phone-older-{name}")))
            .min_h(rems(2.75))
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

fn phone_run_row(
    ws: &Workspace,
    run: &RunView,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let (color, label) = status_look(&run.status, t);
    let route = Route::Run(run.id.clone());
    let unread = ws.unread(run);
    let attention = ws.attention(run, cx);
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
    // What the run needs of the person says it first, as the sidebar
    // does; a working run keeps its plugin's line.
    let (meta, color) = match (&attention, attention.line()) {
        (Attention::Landed, _) => ("landed".to_owned(), t.dim),
        (Attention::Dropped, _) => ("dropped".to_owned(), t.dim),
        (Attention::Working { .. }, _) | (_, None) => (meta, color),
        (attention, Some(line)) => (line, attention_color(attention, t)),
    };
    let badge = note.as_ref().and_then(|note| {
        Some((note.count.clone()?, t.tone(note.tone), note.icon))
    });
    div()
        .id(SharedString::from(format!("phone-run-{}", run.id)))
        .flex()
        .items_center()
        .gap(sp(3.))
        .min_h(rems(3.5))
        .px(sp(4.))
        .border_b_1()
        .border_color(t.border)
        .cursor_pointer()
        .when_some(attention_tint(&attention, t), |row, tint| row.bg(tint))
        .child(attention_icon(&attention, run, false, t, IconSize::BASE))
        .child(
            div()
                .flex_1()
                .min_w(rems(0.))
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
    route: Option<Route>,
    child: &crate::view::ChildRun,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let (color, label) = status_look(&child.status, t);
    let glyph = match child.kind {
        ChildKind::SubAgent => Icon::SubAgent,
        ChildKind::Fork => Icon::Fork,
    };
    div()
        .id(SharedString::from(format!("phone-child-{}", child.id)))
        .flex()
        .items_center()
        .gap(sp(3.))
        .min_h(rems(3.5))
        .pl(sp(11.))
        .pr(sp(4.))
        .border_b_1()
        .border_color(t.border)
        .child(icon(glyph, IconSize::COMPACT, t.blue))
        .child(
            div()
                .flex_1()
                .min_w(rems(0.))
                .flex()
                .flex_col()
                .gap(sp(0.75))
                .child(div().truncate().child(child.title.clone()))
                .child(mono(label, Type::MICRO, color)),
        )
        .when_some(route, |row, route| {
            row.cursor_pointer().on_click(
                cx.listener(move |ws, _, _, cx| ws.navigate(route.clone(), cx)),
            )
        })
}

pub fn phone_tab_bar(
    ws: &Workspace,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    let current = ws.route.tab();
    div()
        .h(rems(4.))
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
