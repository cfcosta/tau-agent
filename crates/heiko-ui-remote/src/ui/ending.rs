//! A chat that ended for good, by landing or by being dropped: its
//! screen is read-only. The transcript ends on where its work went, and
//! a note under a heavy rule takes the composer's place, with a way to a
//! new chat.

use gpui::{Context, Div, div, prelude::*, rems};

use crate::{
    assets::State,
    route::Route,
    theme::{Design as _, Theme, Type, sp, weight},
    ui,
    view::{Ending, RunView},
    workspace::Workspace,
};

/// What the read-only tag in the header says.
pub const READ_ONLY: &str = "read-only";

/// The card at the end of a landed chat: where it landed, how many
/// changes, and a link there. `None` for a chat that did not land.
pub fn landed_card(
    ws: &Workspace,
    run: &RunView,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Option<Div> {
    let Some(Ending::Landed { on, changes }) = &run.ending else {
        return None;
    };
    let target = ws
        .run(on)
        .map_or_else(|| "main".to_owned(), |view| view.title.clone());
    let count = match changes {
        1 => "1 change".to_owned(),
        n => format!("{n} changes"),
    };
    let on = on.clone();
    Some(
        div()
            .flex()
            .items_center()
            .gap(sp(2.5))
            .pt(sp(2.5))
            .border_t_2()
            .border_color(t.green)
            .child(ui::mark(State::Met, t))
            .child(
                div()
                    .flex_1()
                    .min_w(rems(0.))
                    .flex()
                    .flex_wrap()
                    .gap(sp(1.))
                    .child("Landed on")
                    .child(
                        div()
                            .font_weight(weight::STRONG)
                            .text_color(t.bright)
                            .child(target.clone()),
                    )
                    .child(format!("· {count}")),
            )
            .child(
                div()
                    .id("open-landed-on")
                    .typeset(Type::SMALL)
                    .child(ui::link_action(format!("open {target}"), t))
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.navigate(Route::Run(on.clone()), cx)
                    })),
            ),
    )
}

/// What takes the composer's place in a chat that ended: why it takes
/// no more messages, and a new chat from main.
pub fn note(
    run: &RunView,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Option<Div> {
    let why = match run.ending.as_ref()? {
        Ending::Landed { .. } => {
            "This chat landed, so it no longer takes messages. Its work is \
             on main."
        }
        Ending::Dropped => {
            "This chat was dropped, so it no longer takes messages. Its \
             changes were abandoned."
        }
    };
    let repo = run.repo.clone();
    Some(
        div()
            .flex_shrink_0()
            .w_full()
            // On a desktop, the transcript's column, as the composer.
            .when(!compact, |note| note.max_w(rems(50.5)).mx_auto())
            .px(sp(if compact { 3. } else { 6. }))
            .pt(sp(2.))
            .pb(sp(if compact { 4.5 } else { 6. }))
            .child(
                div()
                    .flex()
                    .when(compact, |note| note.flex_col())
                    .when(!compact, |note| note.items_center())
                    .gap(sp(if compact { 3. } else { 5. }))
                    .pt(sp(3.5))
                    .border_t_2()
                    .border_color(t.rule)
                    .child(
                        div()
                            .flex_1()
                            .min_w(rems(0.))
                            .text_color(t.muted)
                            .line_height(gpui::relative(1.5))
                            .child(why),
                    )
                    .child(
                        div().flex().child(
                            div()
                                .id("new-chat-from-main")
                                .child(ui::primary_block(
                                    "New chat from main",
                                    ui::BlockSize::Medium,
                                    t,
                                ))
                                .on_click(cx.listener(
                                    move |ws, _, window, cx| {
                                        ws.new_run_in(&repo, window, cx)
                                    },
                                )),
                        ),
                    ),
            ),
    )
}
