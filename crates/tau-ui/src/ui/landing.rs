//! Landing a finished run (ADR 0009, 0014): a chat lands on the main
//! chat it forked. The controls the compare screen, the fork's card in
//! its parent's chat and the landing card at the end of the run's own
//! chat share.

use gpui::{Context, SharedString, div, prelude::*, px};
use tau_vcs::ui::{change_log::Change, log_card};

use crate::{
    assets::Icon,
    theme::{Design as _, IconSize, Theme, Type, radius, sp, weight},
    ui::{self, Material as _, components::ButtonKind},
    view::{Origin, RunView},
    workspace::{LandingState, Workspace},
};

/// Where `run` lands, as its buttons name it: its parent's title for a
/// fork. None for a sub-agent, which lands as it returns, and for a
/// main chat, which commits on trunk.
pub fn target(ws: &Workspace, run: &RunView) -> Option<String> {
    match &run.origin {
        Origin::Fork { from, .. } => Some(ws.run(from).map_or_else(
            || "its parent".to_owned(),
            |view| view.title.clone(),
        )),
        Origin::Root | Origin::SubAgent { .. } => None,
    }
}

/// The landing card at the end of `run`'s own chat, while a landing is
/// open: what would land, as `vcs_log` draws it, and Land or Cancel.
pub fn card(
    ws: &Workspace,
    run: &RunView,
    t: &Theme,
    compact: bool,
    cx: &mut Context<Workspace>,
) -> Option<gpui::Div> {
    let target = target(ws, run)?;
    let body = controls(ws, run, t, compact, cx)?;
    Some(
        div()
            .flex()
            .flex_col()
            .border_1()
            .border_color(t.accent_border)
            .rounded(radius::BOX)
            .raised(t)
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(2.))
                    .min_h(px(40.))
                    .px(sp(3.))
                    .child(ui::icon(Icon::Land, IconSize::COMPACT, t.accent))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .typeset(Type::SMALL)
                            .font_weight(weight::STRONG)
                            .child(format!("Land {} on {target}", run.title)),
                    ),
            )
            .child(
                body.px(sp(3.))
                    .py(sp(2.5))
                    .border_t_1()
                    .border_color(t.border),
            ),
    )
}

/// Landing a finished run: a button, then the preview (what would land,
/// and what would conflict), then Land or Cancel. A fork can be dropped
/// instead.
pub fn controls(
    ws: &Workspace,
    run: &RunView,
    t: &Theme,
    compact: bool,
    cx: &mut Context<Workspace>,
) -> Option<gpui::Div> {
    let parent = target(ws, run)?;
    if run.status.is_live() {
        return None;
    }
    let id = run.id.clone();
    let button = |label: String, kind: ButtonKind, key: &str| {
        div()
            .id(SharedString::from(format!("{key}-{}", run.id)))
            .child(ui::button(label, kind, t))
    };
    let caption = |text: String, color| {
        div().typeset(Type::CAPTION).text_color(color).child(text)
    };
    let body = match ws.landing(&run.id) {
        None => {
            let drop_id = id.clone();
            div()
                .flex()
                .gap(sp(2.))
                .child(
                    button(
                        format!("Land on {parent}"),
                        ButtonKind::Primary,
                        "land",
                    )
                    .on_click(cx.listener(
                        move |ws, _, _, cx| ws.preview_landing(&id, cx),
                    )),
                )
                .child(
                    button(
                        "Drop this fork".into(),
                        ButtonKind::Secondary,
                        "drop",
                    )
                    .on_click(cx.listener(
                        move |ws, _, _, cx| ws.ask_drop(&drop_id, cx),
                    )),
                )
        }
        Some(LandingState::ConfirmDrop) => {
            let (drop_id, cancel_id) = (id.clone(), id.clone());
            div()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .child(caption(
                    format!(
                        "Drop {}? Its own changes are abandoned and it \
                         closes. The operation log still has them.",
                        run.title
                    ),
                    t.text_soft,
                ))
                .child(
                    div()
                        .flex()
                        .gap(sp(2.))
                        .child(
                            button(
                                "Drop".into(),
                                ButtonKind::Primary,
                                "confirm-drop",
                            )
                            .on_click(cx.listener(
                                move |ws, _, _, cx| ws.drop_child(&drop_id, cx),
                            )),
                        )
                        .child(
                            button(
                                "Cancel".into(),
                                ButtonKind::Secondary,
                                "cancel-drop",
                            )
                            .on_click(cx.listener(
                                move |ws, _, _, cx| {
                                    ws.cancel_landing(&cancel_id, cx)
                                },
                            )),
                        ),
                )
        }
        Some(LandingState::Dropping) => caption("Dropping…".into(), t.dim),
        Some(LandingState::Previewing) => {
            caption("Checking what would land…".into(), t.dim)
        }
        Some(LandingState::Landing) => {
            caption(format!("Landing on {parent}…"), t.dim)
        }
        Some(LandingState::Preview(Err(error))) => div()
            .flex()
            .flex_col()
            .gap(sp(2.))
            .child(caption(error.clone(), t.red))
            .child(
                button("Close".into(), ButtonKind::Secondary, "cancel-land")
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.cancel_landing(&id, cx)
                    })),
            ),
        Some(LandingState::Preview(Ok(preview))) => {
            let changes: Vec<Change> =
                preview.changes.iter().cloned().map(Change::new).collect();
            let summary = match changes.len() {
                0 => format!("Nothing to land: {parent} has all of it."),
                1 => format!(
                    "1 change lands on {parent}, on top of its latest change."
                ),
                n => format!(
                    "{n} changes land on {parent}, on top of its latest change."
                ),
            };
            let (land_id, cancel_id) = (id.clone(), id.clone());
            div()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .child(caption(summary, t.text_soft))
                .child(
                    ui::card(t)
                        .py(sp(1.5))
                        .children(log_card::stack_rows(&changes, t, compact)),
                )
                .when(!preview.conflicts.is_empty(), |column| {
                    column.child(caption(
                        format!(
                            "Conflicts in {}. Landing starts {parent}'s next \
                             turn to resolve them and commit the result.",
                            preview.conflicts.join(", ")
                        ),
                        t.red,
                    ))
                })
                .child(
                    div()
                        .flex()
                        .gap(sp(2.))
                        .when(!changes.is_empty(), |row| {
                            row.child(
                                button(
                                    if preview.conflicts.is_empty() {
                                        "Land".into()
                                    } else {
                                        "Land and resolve".into()
                                    },
                                    ButtonKind::Primary,
                                    "confirm-land",
                                )
                                .on_click(
                                    cx.listener(move |ws, _, _, cx| {
                                        ws.land(&land_id, cx)
                                    }),
                                ),
                            )
                        })
                        .child(
                            button(
                                "Cancel".into(),
                                ButtonKind::Secondary,
                                "cancel-land",
                            )
                            .on_click(cx.listener(
                                move |ws, _, _, cx| {
                                    ws.cancel_landing(&cancel_id, cx)
                                },
                            )),
                        ),
                )
        }
    };
    Some(body)
}
