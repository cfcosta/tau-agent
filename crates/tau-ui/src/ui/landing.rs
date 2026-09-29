//! Landing or dropping a finished fork (ADR 0009): the controls the
//! compare screen and the fork's card in its parent's chat share.

use gpui::{Context, SharedString, div, prelude::*};

use crate::{
    change_log::Change,
    theme::{Design as _, Theme, Type, sp},
    ui::{self, components::ButtonKind, log_card},
    view::{Origin, RunView},
    workspace::{LandingState, Workspace},
};

/// Landing a finished fork on the run it forked (ADR 0009): a button,
/// then the preview (what would land, and what would conflict), then
/// Land or Cancel.
pub fn controls(
    ws: &Workspace,
    run: &RunView,
    t: &Theme,
    compact: bool,
    cx: &mut Context<Workspace>,
) -> Option<gpui::Div> {
    let Origin::Fork { from, .. } = &run.origin else {
        return None;
    };
    if run.status.is_live() {
        return None;
    }
    let parent = ws
        .run(from)
        .map_or_else(|| "its parent".to_owned(), |view| view.title.clone());
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
                    "1 change lands on {parent}, on top of its latest turn."
                ),
                n => format!(
                    "{n} changes land on {parent}, on top of its latest turn."
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
                        .bg(t.card)
                        .py(sp(1.5))
                        .children(log_card::stack_rows(&changes, t, compact)),
                )
                .when(!preview.conflicts.is_empty(), |column| {
                    column.child(caption(
                        format!(
                            "Conflicts in {}. They land as conflict markers \
                             for {parent}'s next turn to resolve.",
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
                                        "Land with conflicts".into()
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
