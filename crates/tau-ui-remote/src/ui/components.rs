//! The components that know tau's own types. The rest are
//! `tau-ui-kit`'s, re-exported here.

use gpui::{Context, Div, SharedString, div, prelude::*, rems};
pub use tau_ui_kit::components::*;

use crate::{
    assets::Icon,
    catalog::Repo,
    theme::{Design as _, Theme, Type, sp, weight},
    workspace::Workspace,
};

/// A repository's mark: its letter on its color, 20 px in the sidebar
/// and 26 px on a phone.
pub fn repo_mark(repo: &Repo, size: f32, t: &Theme) -> Div {
    tau_ui_kit::components::repo_mark(&repo.name, size, t)
}

/// The phone's header on a task screen: back, and the title.
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
        .bg(t.panel)
        .border_b_1()
        .border_color(t.border)
        .when(ws.can_go_back(), |bar| {
            bar.child(
                icon_button("task-back", Icon::Back, 44., t)
                    .on_click(cx.listener(|ws, _, _, cx| ws.back(cx))),
            )
        })
        .when(!ws.can_go_back(), |bar| bar.pl(sp(4.)))
        .child(
            div()
                .typeset(Type::SUBTITLE)
                .font_weight(weight::STRONG)
                .child(title.into()),
        )
}
