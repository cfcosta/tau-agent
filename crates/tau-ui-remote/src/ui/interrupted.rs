//! What tau closing left: a run it cut off, which waits to be resumed,
//! and a landing it finished at its next start.

use gpui::{Context, Div, div, prelude::*, rems};

use crate::{
    assets::Icon,
    theme::{IconSize, Theme, Type, radius, sp, weight},
    ui::{self, Material as _, components::ButtonKind},
    view::{Origin, RunStatus, RunView},
    workspace::Workspace,
};

/// The card at the end of a run tau closing cut off: when, at which
/// turn, what its files hold, and Resume. A sub-agent's has no Resume:
/// its call is gone.
pub fn card(
    run: &RunView,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Option<Div> {
    if run.status != RunStatus::Interrupted {
        return None;
    }
    let resumes = !matches!(run.origin, Origin::SubAgent { .. });
    let id = run.id.clone();
    let what = if resumes {
        "Its files are as the last turn left them, uncommitted changes \
         included. Resuming tells the model the run was cut off and goes on."
    } else {
        "Its call ended with tau, so its changes were dropped at the next \
         start."
    };
    Some(
        div()
            .flex()
            .flex_col()
            .border_1()
            .border_color(t.border)
            .rounded(radius::BOX)
            .raised(t)
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(2.))
                    .min_h(rems(2.5))
                    .px(sp(3.))
                    .child(ui::icon(
                        Icon::Pause,
                        IconSize::COMPACT,
                        t.text_soft,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w(rems(0.))
                            .truncate()
                            .font_weight(weight::STRONG)
                            .child("Interrupted when tau closed"),
                    )
                    .child(ui::mono(
                        format!("turn {}", run.turn),
                        Type::MICRO,
                        t.muted,
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(3.))
                    .px(sp(3.))
                    .py(sp(2.5))
                    .border_t_1()
                    .border_color(t.border)
                    .child(
                        div()
                            .flex_1()
                            .min_w(rems(0.))
                            .text_color(t.text_soft)
                            .line_height(gpui::relative(1.5))
                            .child(what),
                    )
                    .when(resumes, |row| {
                        row.child(
                            div()
                                .id("resume-cut-off")
                                .child(ui::button(
                                    "Resume",
                                    ButtonKind::Primary,
                                    t,
                                ))
                                .on_click(cx.listener(move |ws, _, _, cx| {
                                    ws.resume_cut_off(&id, cx)
                                })),
                        )
                    }),
            ),
    )
}

/// The card a landing tau finished at start leaves in the chat it
/// landed on: `title` was cut off, and is done now.
pub fn finished_landing(title: &str, t: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .gap(sp(2.))
        .px(sp(3.))
        .py(sp(2.5))
        .rounded(radius::BOX)
        .border_1()
        .border_color(t.border)
        .raised(t)
        .child(ui::icon(Icon::Check, IconSize::COMPACT, t.green))
        .child(
            div()
                .flex_1()
                .min_w(rems(0.))
                .flex()
                .flex_wrap()
                .gap(sp(1.))
                .text_color(t.text_soft)
                .child("A landing of")
                .child(div().text_color(t.text).child(title.to_owned()))
                .child("was cut off; tau finished it at start."),
        )
}
