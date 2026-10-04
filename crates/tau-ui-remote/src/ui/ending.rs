//! A chat that ended for good, by landing or by being dropped: its
//! screen is read-only. The transcript ends on where its work went, and
//! a note takes the composer's place, with a way to a new chat.

use gpui::{Context, Div, div, prelude::*, rems};

use crate::{
    assets::Icon,
    route::Route,
    theme::{IconSize, Theme, Type, radius, sp, weight},
    ui::{self, components::ButtonKind},
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
            .gap(sp(2.))
            .px(sp(3.))
            .py(sp(2.5))
            .rounded(radius::BOX)
            .border_1()
            .border_color(t.green.opacity(0.35))
            .bg(t.green_soft)
            .child(ui::icon(Icon::Check, IconSize::COMPACT, t.green))
            .child(
                div()
                    .flex_1()
                    .min_w(rems(0.))
                    .flex()
                    .flex_wrap()
                    .gap(sp(1.))
                    .child("Landed on")
                    .child(
                        div().font_weight(weight::STRONG).child(target.clone()),
                    )
                    .child(format!("· {count}")),
            )
            .child(
                div()
                    .id("open-landed-on")
                    .child(
                        ui::text_link(
                            format!("Open {target}"),
                            Type::CAPTION,
                            t,
                        )
                        .text_color(t.accent),
                    )
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
            .px(sp(if compact { 3. } else { 8. }))
            .pt(sp(2.))
            .pb(sp(if compact { 4.5 } else { 5. }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(3.))
                    .p(sp(3.5))
                    .rounded(radius::LARGE)
                    .border_1()
                    .border_dashed()
                    .border_color(t.border_strong)
                    .child(
                        div()
                            .flex_1()
                            .min_w(rems(0.))
                            .text_color(t.muted)
                            .line_height(gpui::relative(1.5))
                            .child(why),
                    )
                    .child(
                        div()
                            .id("new-chat-from-main")
                            .child(ui::button(
                                "New chat from main",
                                ButtonKind::Secondary,
                                t,
                            ))
                            .on_click(cx.listener(move |ws, _, window, cx| {
                                ws.new_run_in(&repo, window, cx)
                            })),
                    ),
            ),
    )
}
