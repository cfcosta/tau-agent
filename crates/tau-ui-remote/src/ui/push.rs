//! Pushing a repository's main chat to GitHub (ADR 0023): the header's
//! ahead count and Push button, the sidebar row's line and button, and
//! the cards at the end of the main chat for a push that went and one
//! that GitHub's moved branch refused.

use gpui::{Context, SharedString, div, prelude::*, rems};

use crate::{
    assets::Icon,
    push::{PushState, Pushed},
    theme::{Design as _, IconSize, Theme, Type, radius, sp, weight},
    ui::{self, Material as _, components::ButtonKind},
    view::RunView,
    workspace::Workspace,
};

/// `n changes`, or `1 change`.
fn changes(n: usize) -> String {
    if n == 1 {
        "1 change".into()
    } else {
        format!("{n} changes")
    }
}

/// The main chat's header: how far ahead of GitHub trunk is, and Push
/// to GitHub. Nothing when trunk has nothing to push, and for any other
/// chat.
pub fn header(
    ws: &Workspace,
    run: &RunView,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Option<gpui::Stateful<gpui::Div>> {
    let repo = ws.main_repo(&run.id)?.to_owned();
    let (ahead, _) = ws.unpushed(&repo)?;
    let pushing =
        matches!(ws.push_state(&repo), Some(PushState::Pushing { .. }));
    // How far ahead trunk is goes on the button itself.
    let label = if pushing {
        "Pushing…".to_owned()
    } else {
        format!("Push ↑{ahead}")
    };
    Some(
        div()
            .id("push")
            .child(ui::button(label, ButtonKind::Primary, t))
            .when(pushing, |button| button.opacity(0.6))
            .when(!pushing, |button| {
                button.on_click(
                    cx.listener(move |ws, _, _, cx| ws.push(&repo, false, cx)),
                )
            }),
    )
}

/// The sidebar's main row: `n changes not on GitHub`, and a small Push
/// button, when trunk has something to push.
pub fn row(
    ws: &Workspace,
    run: &RunView,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Option<(SharedString, gpui::Stateful<gpui::Div>)> {
    let repo = ws.main_repo(&run.id)?.to_owned();
    let (ahead, _) = ws.unpushed(&repo)?;
    let pushing =
        matches!(ws.push_state(&repo), Some(PushState::Pushing { .. }));
    let line = format!("{} not on GitHub", changes(ahead as usize));
    let button = div()
        .id(SharedString::from(format!("push-{repo}")))
        .flex_shrink_0()
        .px(sp(2.))
        .py(sp(1.))
        .rounded(radius::CONTROL)
        .bg(t.accent)
        .text_color(t.bg)
        .typeset(Type::MICRO)
        .font_weight(weight::EMPHASIS)
        .child(if pushing { "Pushing…" } else { "Push" })
        .when(pushing, |button| button.opacity(0.6))
        .when(!pushing, |button| {
            button.cursor_pointer().on_click(cx.listener(
                move |ws, _, _, cx| {
                    cx.stop_propagation();
                    ws.push(&repo, false, cx)
                },
            ))
        });
    Some((line.into(), button))
}

/// The card at the end of the main chat for its repository's last push:
/// the changes it took, or why GitHub refused it, with Fetch and push.
pub fn card(
    ws: &Workspace,
    run: &RunView,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Option<gpui::Div> {
    let repo = ws.main_repo(&run.id)?.to_owned();
    match ws.push_state(&repo)? {
        PushState::Pushing { .. } => None,
        PushState::Pushed(pushed) => Some(pushed_card(pushed, t)),
        PushState::Moved { branch, ahead } => {
            Some(moved_card(&repo, branch, *ahead, t, cx))
        }
    }
}

/// A card's frame: its border, its surface, and a head row with an
/// icon, a title and what sits at its right.
fn frame(
    glyph_color: gpui::Hsla,
    title: String,
    right: Option<gpui::Div>,
) -> (gpui::Div, gpui::Div) {
    let head = div()
        .flex()
        .items_center()
        .gap(sp(2.))
        .min_h(rems(2.5))
        .px(sp(3.))
        .child(ui::icon(Icon::Push, IconSize::COMPACT, glyph_color))
        .child(
            div()
                .flex_1()
                .min_w(rems(0.))
                .truncate()
                .font_weight(weight::STRONG)
                .child(title),
        )
        .children(right);
    let card = div()
        .flex()
        .flex_col()
        .border_1()
        .rounded(radius::BOX)
        .overflow_hidden();
    (card, head)
}

fn pushed_card(pushed: &Pushed, t: &Theme) -> gpui::Div {
    let span = format!(
        "{} → {}",
        pushed.from.as_deref().map_or("·", Pushed::short),
        Pushed::short(&pushed.to)
    );
    let (card, head) = frame(
        t.green,
        format!(
            "Pushed {} to origin/{}",
            changes(pushed.changes.len()),
            pushed.branch
        ),
        Some(ui::mono(span, Type::MICRO, t.muted)),
    );
    let list = div()
        .flex()
        .flex_col()
        .gap(sp(1.5))
        .px(sp(3.))
        .py(sp(2.5))
        .border_t_1()
        .border_color(t.border)
        .children(pushed.changes.iter().map(|change| {
            let id = &change.change_id[..change.change_id.len().min(8)];
            div()
                .flex()
                .gap(sp(2.))
                .min_w(rems(0.))
                .child(ui::mono(id.to_owned(), Type::CAPTION, t.change))
                .child(
                    ui::mono(change.title.clone(), Type::CAPTION, t.text_soft)
                        .truncate(),
                )
        }));
    card.border_color(t.border)
        .raised(t)
        .child(head)
        .child(list)
}

fn moved_card(
    repo: &str,
    branch: &str,
    ahead: u32,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> gpui::Div {
    let (card, head) = frame(
        t.red,
        format!("GitHub's {branch} moved since the last fetch"),
        None,
    );
    let repo = repo.to_owned();
    let body = div()
        .flex()
        .items_center()
        .gap(sp(3.))
        .px(sp(3.))
        .py(sp(2.5))
        .border_t_1()
        .border_color(t.danger_edge)
        .child(
            div()
                .flex_1()
                .min_w(rems(0.))
                .text_color(t.text_soft)
                .line_height(gpui::relative(1.5))
                .child(format!(
                    "Nothing was pushed. Fetching puts main's {} on top of \
                     the new commits, and you can push again.",
                    changes(ahead as usize)
                )),
        )
        .child(
            div()
                .id("fetch-and-push")
                .child(ui::button("Fetch and push", ButtonKind::Secondary, t))
                .on_click(
                    cx.listener(move |ws, _, _, cx| ws.push(&repo, true, cx)),
                ),
        );
    card.border_color(t.red_border)
        .bg(t.danger_surface)
        .child(head)
        .child(body)
}
