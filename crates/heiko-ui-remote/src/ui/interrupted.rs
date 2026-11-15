//! What heiko closing left: a run it cut off, which waits to be resumed,
//! and a landing it finished at its next start.

use gpui::{Context, Div, div, prelude::*, rems};

use crate::{
    assets::State,
    theme::{Design as _, Theme, Type, sp},
    ui,
    view::{Origin, RunStatus, RunView},
    workspace::Workspace,
};

/// The note at the end of a run heiko closing cut off: when, at which
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
        "Its call ended with heiko, so its changes were dropped at the next \
         start."
    };
    Some(
        div()
            .flex()
            .flex_col()
            .gap(sp(2.))
            .pt(sp(2.5))
            .border_t_2()
            .border_color(t.slate)
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(sp(3.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(rems(0.))
                            .truncate()
                            .typeset(Type::SECTION.sized(17.))
                            .text_color(t.bright)
                            .child("Interrupted when heiko closed"),
                    )
                    .child(ui::mono(
                        format!("turn {}", run.turn),
                        Type::CAPTION,
                        t.dim,
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(5.))
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
                                .child(ui::primary_block(
                                    "Resume",
                                    ui::BlockSize::Small,
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

/// The note a landing heiko finished at start leaves in the chat it
/// landed on: `title` was cut off, and is done now.
pub fn finished_landing(title: &str, t: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .gap(sp(2.5))
        .pt(sp(2.5))
        .border_t_1()
        .border_color(t.hairline)
        .child(ui::mark(State::Met, t))
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
                .child("was cut off; heiko finished it at start."),
        )
}
