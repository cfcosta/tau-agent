//! The components that know heiko's own types. The rest are
//! `heiko-ui-kit`'s, re-exported here.

use gpui::{Context, Div, SharedString, div, prelude::*, rems};
pub use heiko_ui_kit::components::*;

use crate::{
    assets::Icon,
    theme::{Design as _, Theme, Type, sp},
    workspace::Workspace,
};

/// The phone's header on a task screen: back, and the title in the
/// display face, over a hairline.
pub fn phone_bar(
    ws: &Workspace,
    title: impl Into<SharedString>,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Div {
    div()
        .h(rems(3.5))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(sp(2.5))
        .px(sp(2.))
        .border_b_1()
        .border_color(t.hairline)
        .when(ws.can_go_back(), |bar| {
            bar.child(
                icon_button("task-back", Icon::Back, 44., t)
                    .on_click(cx.listener(|ws, _, _, cx| ws.back(cx))),
            )
        })
        .when(!ws.can_go_back(), |bar| bar.pl(sp(4.)))
        .child(
            div()
                .typeset(Type::SECTION.sized(17.))
                .text_color(t.bright)
                .child(title.into()),
        )
}
