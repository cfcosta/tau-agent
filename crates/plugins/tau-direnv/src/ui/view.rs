//! What tau-direnv draws: the question that takes the composer's place,
//! the cards above a run's transcript while its environment loads or
//! after it failed, and the repository menu's toggle.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::{AnyElement, Div, SharedString, div, prelude::*, rems};
use tau_ui_kit::{
    assets::Icon,
    components::{self as ui, ButtonKind, mono},
    format::clock,
    theme::{IconSize, Theme, Type, radius, sp, weight},
};
use tau_ui_plugin::{
    Handle,
    ViewCx,
    points::{AtRepo, AtRun},
};

use super::DirenvUi;
use crate::{Act, Record};

/// The question, in the composer's place, while a live run waits for
/// the person to say whether the repository's `.envrc` loads.
pub fn question(
    at: &AtRun,
    view: &mut ViewCx<'_, DirenvUi>,
) -> Option<AnyElement> {
    if !at.run.live {
        return None;
    }
    let Some(Record::Asked { repo, envrc }) = view.state?.now.clone() else {
        return None;
    };
    let t = view.theme().clone();
    let compact = view.compact;
    let decide = |id: &'static str, label: &'static str, kind, load: bool| {
        let (handle, repo) = (view.handle.clone(), repo.clone());
        div()
            .id(id)
            .child(ui::button(label, kind, &t))
            .on_click(move |_, _, cx| decide(&handle, &repo, load, cx))
    };
    let load = decide("envrc-load", "Load it", ButtonKind::Primary, true);
    let skip =
        decide("envrc-skip", "Run without it", ButtonKind::Secondary, false);
    Some(
        div()
            .id("envrc-question")
            .flex_shrink_0()
            .mx(sp(if compact { 2. } else { 6. }))
            .mb(sp(if compact { 2. } else { 5. }))
            .flex()
            .flex_col()
            .rounded(radius::CARD)
            .border_1()
            .border_color(t.blue_border)
            .bg(t.info_panel)
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(2.))
                    .min_h(rems(2.625))
                    .px(sp(3.5))
                    .child(ui::icon(Icon::Terminal, IconSize::COMPACT, t.blue))
                    .child(
                        div()
                            .flex_1()
                            .min_w(rems(0.))
                            .font_weight(weight::STRONG)
                            .text_color(t.text)
                            .child(format!("Load {repo}'s .envrc for agent commands?")),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(sp(2.5))
                    .px(sp(3.5))
                    .py(sp(3.))
                    .border_t_1()
                    .border_color(t.border)
                    .child(
                        ui::text(
                            "The repository sets up its tools with direnv. With it, \
                             commands run in that environment in main and in every \
                             chat, as they would in your shell. tau asks once; you \
                             can change it on the repository's menu.",
                            Type::BODY,
                            t.text_soft,
                        )
                        .line_height(gpui::relative(1.55)),
                    )
                    .child(
                        div()
                            .px(sp(2.5))
                            .py(sp(2.))
                            .rounded(radius::CONTROL)
                            .bg(t.depth.well)
                            .border_1()
                            .border_color(t.border)
                            .children(
                                envrc
                                    .trim_end()
                                    .lines()
                                    .map(|line| mono(line.to_owned(), Type::SMALL, t.text_soft))
                                    .collect::<Vec<_>>(),
                            ),
                    )
                    .child(div().flex().gap(sp(2.)).child(load).child(skip)),
            )
            .into_any_element(),
    )
}

fn decide(handle: &Handle, repo: &str, load: bool, cx: &mut gpui::App) {
    handle.act(
        Act::Decide {
            repo: repo.to_owned(),
            load,
        },
        cx,
    );
}

/// Above a run's transcript: its environment loading, while it runs, or
/// why it did not load.
pub fn card(at: &AtRun, view: &mut ViewCx<'_, DirenvUi>) -> Option<AnyElement> {
    let now = view.state?.now.clone()?;
    let t = view.theme().clone();
    let side = sp(if view.compact { 4. } else { 6. });
    match now {
        Record::Loading { since } if at.run.live => {
            tick(view);
            Some(loading(since, &t).mx(side).mt(sp(3.)).into_any_element())
        }
        Record::Failed { status, output } => {
            let (handle, run) = (view.handle.clone(), at.run.id.0.to_string());
            let again = div()
                .id("envrc-again")
                .child(ui::button("Try again", ButtonKind::Secondary, &t))
                .on_click(move |_, _, cx| {
                    handle.act(Act::Reload { run: run.clone() }, cx)
                });
            Some(
                failed(&status, &output, again, &t)
                    .mx(side)
                    .mt(sp(3.))
                    .into_any_element(),
            )
        }
        _ => None,
    }
}

/// How long ago `since` (Unix milliseconds) was.
fn elapsed(since: u64) -> Duration {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |now| now.as_millis() as u64);
    Duration::from_millis(now.saturating_sub(since))
}

fn loading(since: u64, t: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .gap(sp(2.5))
        .px(sp(3.))
        .py(sp(2.5))
        .rounded(radius::BOX)
        .border_1()
        .border_color(t.border)
        .bg(t.card)
        .child(ui::dot(t.accent, 8.))
        .child(
            div()
                .flex_1()
                .min_w(rems(0.))
                .flex()
                .gap(sp(1.))
                .text_color(t.text)
                .child("Loading the repository's environment")
                .child(
                    div()
                        .text_color(t.muted)
                        .child(format!("· .envrc · {}", clock(elapsed(since)))),
                ),
        )
        .child(ui::text("commands wait for it", Type::CAPTION, t.muted))
}

fn failed(
    status: &str,
    output: &str,
    again: impl IntoElement,
    t: &Theme,
) -> Div {
    div()
        .flex()
        .flex_col()
        .rounded(radius::BOX)
        .border_1()
        .border_color(t.red_border)
        .bg(t.danger_surface)
        .overflow_hidden()
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(2.))
                .min_h(rems(2.5))
                .px(sp(3.))
                .child(ui::icon(Icon::Warning, IconSize::COMPACT, t.red))
                .child(
                    div()
                        .flex_1()
                        .min_w(rems(0.))
                        .font_weight(weight::STRONG)
                        .child("The repository's environment did not load"),
                )
                .child(mono(status.to_owned(), Type::MICRO, t.muted)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(2.5))
                .px(sp(3.))
                .py(sp(2.5))
                .border_t_1()
                .border_color(t.danger_edge)
                .children(
                    output
                        .lines()
                        .map(|line| {
                            mono(line.to_owned(), Type::SMALL, t.removed_text)
                        })
                        .collect::<Vec<_>>(),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(3.))
                        .child(
                            div().flex_1().text_color(t.text_soft).child(
                                "Commands run without it until it loads.",
                            ),
                        )
                        .child(again),
                ),
        )
}

/// Keeps the loading card's clock going: the window draws again each
/// second while a loading card showed in the last two.
fn tick(view: &mut ViewCx<'_, DirenvUi>) {
    let handle = view.handle.clone();
    view.ui.update(view.cx, |ui, cx| {
        ui.loading_shown = Some(Instant::now());
        if ui.ticking {
            return;
        }
        ui.ticking = true;
        cx.spawn(async move |ui, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let going = ui.update(cx, |ui, cx| {
                    let going = ui.loading_shown.is_some_and(|shown| {
                        shown.elapsed() < Duration::from_secs(2)
                    });
                    ui.ticking = going;
                    if going {
                        handle.refresh(cx);
                    }
                    going
                });
                if !matches!(going, Ok(true)) {
                    return;
                }
            }
        })
        .detach();
    });
}

/// The repository menu's entry, for a repository with an `.envrc`: the
/// toggle, or why there is none.
pub fn menu_entry(
    at: &AtRepo,
    view: &mut ViewCx<'_, DirenvUi>,
) -> Option<AnyElement> {
    let data = view.repo(&at.repo)?.clone();
    if !data.envrc {
        return None;
    }
    let t = view.theme().clone();
    let on = view.settings.repos.get(&at.repo) == Some(&true);
    let row = div()
        .id(SharedString::from(format!("envrc-menu-{}", at.repo)))
        .flex()
        .items_center()
        .gap(sp(2.5))
        .min_h(rems(2.125))
        .px(sp(2.5))
        .rounded(radius::CONTROL);
    if !data.direnv {
        return Some(
            row.py(sp(1.5))
                .child(ui::icon(Icon::Terminal, IconSize::BASE, t.dim))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_color(t.muted)
                                .child("Load .envrc for agent commands"),
                        )
                        .child(ui::text(
                            "direnv is not installed",
                            Type::CAPTION,
                            t.dim,
                        )),
                )
                .into_any_element(),
        );
    }
    let (handle, repo) = (view.handle.clone(), at.repo.clone());
    let hover = t.selected;
    Some(
        row.cursor_pointer()
            .text_color(t.text)
            .hover(move |style| style.bg(hover))
            .child(ui::icon(Icon::Terminal, IconSize::BASE, t.muted))
            .child(
                div()
                    .flex_1()
                    .whitespace_nowrap()
                    .child("Load .envrc for agent commands"),
            )
            .child(ui::switch(on, &t))
            .on_click(move |_, _, cx| decide(&handle, &repo, !on, cx))
            .into_any_element(),
    )
}

/// Its settings pane (ADR 0029): whether each repository's `.envrc`
/// loads for agent commands. The choice is a repository's own, so the
/// pane shows the repository in scope, or every one with an `.envrc`.
pub fn settings_pane(view: &mut ViewCx<'_, DirenvUi>) -> AnyElement {
    let t = view.theme().clone();
    let scope = view.scope().map(str::to_owned);
    let repos: Vec<(String, crate::RepoData)> = view
        .repos()
        .filter(|(name, _)| scope.as_deref().is_none_or(|scope| scope == *name))
        .map(|(name, data)| (name.to_owned(), data.clone()))
        .collect();
    let rows: Vec<AnyElement> = repos
        .iter()
        .filter(|(_, data)| data.envrc || scope.is_some())
        .map(|(repo, data)| {
            let on = view.settings.repos.get(repo) == Some(&true);
            let why = if !data.envrc {
                Some("No .envrc in this repository.")
            } else if !data.direnv {
                Some("direnv is not installed.")
            } else {
                None
            };
            let (handle, name) = (view.handle.clone(), repo.clone());
            div()
                .id(SharedString::from(format!("envrc-setting-{repo}")))
                .flex()
                .items_center()
                .gap(sp(4.))
                .px(sp(4.))
                .py(sp(3.))
                .border_b_1()
                .border_color(t.border)
                .child(
                    div()
                        .flex_1()
                        .min_w(rems(0.))
                        .flex()
                        .flex_col()
                        .gap(sp(0.75))
                        .child(mono(repo.clone(), Type::SMALL, t.text))
                        .child(ui::text(
                            why.unwrap_or("Agent commands run in its .envrc's environment."),
                            Type::CAPTION,
                            t.muted,
                        )),
                )
                .when(why.is_none(), |row| {
                    row.child(ui::switch(on, &t)).cursor_pointer().on_click(
                        move |_, _, cx| {
                            handle.act(
                                Act::Decide {
                                    repo: name.clone(),
                                    load: !on,
                                },
                                cx,
                            )
                        },
                    )
                })
                .into_any_element()
        })
        .collect();
    if rows.is_empty() {
        return ui::empty("No repository has an .envrc.", &t)
            .into_any_element();
    }
    ui::card(&t).children(rows).into_any_element()
}
