//! Pushing a repository's main chat to the forge (ADR 0023): the
//! header's Push block with its ahead count, and the notes at the end
//! of the main chat for a push that went and one that the forge's moved
//! branch refused. Labels name the forge as the catalog does.

use gpui::{AnyElement, Context, Hsla, div, prelude::*, rems};

use crate::{
    assets::Icon,
    push::{PushState, Pushed},
    theme::{IconSize, Theme, Type, sp},
    ui::{
        self,
        BlockSize,
        State,
        landing::{LINE, SQUARE, note},
    },
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

/// The main chat's header: Push, with how far ahead of the forge trunk
/// is. Nothing when trunk has nothing to push, and for any other chat.
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
    // How far ahead trunk is goes on the block itself.
    let label = if pushing {
        "pushing…".to_owned()
    } else {
        format!("push ↑{ahead}")
    };
    Some(
        div()
            .id("push")
            .child(ui::primary_block(label, BlockSize::Small, t))
            .when(pushing, |button| button.opacity(0.6))
            .when(!pushing, |button| {
                button.on_click(
                    cx.listener(move |ws, _, _, cx| ws.push(&repo, false, cx)),
                )
            }),
    )
}

/// The note at the end of the main chat for its repository's last push:
/// the changes it took, or why the forge refused it, with Fetch and push.
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
            let forge = ws.catalog.forge_name().to_owned();
            Some(moved_card(&repo, &forge, branch, *ahead, t, cx))
        }
    }
}

/// How wide the graph's gutter is, in rems: its line runs down the
/// middle, and its squares sit on the line.
const GUTTER: f32 = 1.;

/// A row of the graph: what sits on the line, then what it is.
fn graph_row(on_line: AnyElement, what: gpui::Div) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(sp(2.5))
        .py(sp(1.))
        .child(
            div()
                .w(rems(GUTTER))
                .flex_shrink_0()
                .flex()
                .justify_center()
                .child(on_line),
        )
        .child(what.flex_1().min_w(rems(0.)))
}

/// Changes as squares on a vertical line in `color` that ends at
/// `branch` on the forge, under `head`: an arrow when they went there, a
/// cross when they did not.
fn graph(
    head: AnyElement,
    branch: &str,
    color: Hsla,
    rows: impl IntoIterator<Item = gpui::Div>,
    t: &Theme,
) -> gpui::Div {
    let line = div()
        .absolute()
        .top(rems(0.75))
        .bottom_0()
        .left(rems((GUTTER - LINE) / 2.))
        .w(rems(LINE))
        .bg(color);
    div()
        .relative()
        .flex()
        .flex_col()
        .pb(sp(1.5))
        .child(line)
        .child(graph_row(
            // On the ground's color, so the line ends at the mark.
            div().bg(t.bg).child(head).into_any_element(),
            ui::mono(format!("origin/{branch}"), Type::SMALL, color),
        ))
        .children(rows)
}

fn square(color: Hsla) -> AnyElement {
    div().size(rems(SQUARE)).bg(color).into_any_element()
}

fn pushed_card(pushed: &Pushed, t: &Theme) -> gpui::Div {
    let span = format!(
        "{} → {}",
        pushed.from.as_deref().map_or("·", Pushed::short),
        Pushed::short(&pushed.to)
    );
    let rows = pushed.changes.iter().map(|change| {
        let id = &change.change_id[..change.change_id.len().min(8)];
        graph_row(
            square(t.change),
            div()
                .flex()
                .gap(sp(3.))
                .child(
                    ui::mono(id.to_owned(), Type::SMALL, t.change)
                        .flex_shrink_0(),
                )
                .child(
                    ui::mono(change.title.clone(), Type::SMALL, t.text_soft)
                        .min_w(rems(0.))
                        .truncate(),
                ),
        )
    });
    note(
        format!(
            "Pushed {} to origin/{}",
            changes(pushed.changes.len()),
            pushed.branch
        ),
        Some(ui::mono(span, Type::CAPTION, t.dim)),
        t.blue,
        t,
    )
    .child(
        graph(
            ui::icon(Icon::Push, IconSize::LARGE, t.blue).into_any_element(),
            &pushed.branch,
            t.blue,
            rows,
            t,
        )
        .mt(sp(3.)),
    )
}

fn moved_card(
    repo: &str,
    forge: &str,
    branch: &str,
    ahead: u32,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> gpui::Div {
    let repo = repo.to_owned();
    let waiting = graph_row(
        square(t.red),
        ui::mono(
            format!("{} not pushed", changes(ahead as usize)),
            Type::SMALL,
            t.text_soft,
        ),
    );
    note(
        format!("{forge}'s {branch} moved since the last fetch"),
        None,
        t.red,
        t,
    )
    .child(
        graph(
            ui::mark(State::Failing, t).into_any_element(),
            branch,
            t.red,
            [waiting],
            t,
        )
        .mt(sp(3.)),
    )
    .child(ui::text(
        "Fetching puts them on top of the new commits.",
        Type::SMALL,
        t.text_soft,
    ))
    .child(
        div().flex().mt(sp(3.)).child(
            div()
                .id("fetch-and-push")
                .child(ui::hot_action("fetch and push", t))
                .on_click(
                    cx.listener(move |ws, _, _, cx| ws.push(&repo, true, cx)),
                ),
        ),
    )
}
