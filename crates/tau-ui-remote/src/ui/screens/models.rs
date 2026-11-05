//! Model settings: each agent's default, which models the picker shows,
//! and plugins' sections: the provider's accounts, the forge's sign-in.

use gpui::{AnyElement, Context, SharedString, div, prelude::*, rems};

use crate::{
    theme::{Design as _, Theme, Type, sp},
    ui::{self, heading, mono},
    workspace::{PickerTarget, Workspace},
};

pub fn render(
    ws: &Workspace,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let models = &ws.catalog.models;
    let defaults = models
        .agents
        .iter()
        .map(|(agent, what)| {
            div()
                .flex()
                .items_center()
                .gap(sp(4.))
                .px(sp(4.))
                .py(sp(3.5))
                .border_b_1()
                .border_color(t.border)
                .child(
                    div()
                        .flex_1()
                        .min_w(rems(0.))
                        .flex()
                        .flex_col()
                        .gap(sp(0.75))
                        .child(agent.clone())
                        .child(ui::text(what.clone(), Type::CAPTION, t.muted)),
                )
                .child(ws.model_chip(
                    PickerTarget::Default(agent.clone()),
                    t,
                    cx,
                ))
        })
        .collect::<Vec<_>>();

    let columns = |row: gpui::Div| {
        row.grid()
            .grid_cols(if compact { 2 } else { 3 })
            .items_center()
            .px(sp(4.))
    };
    let header = columns(div())
        .h(rems(2.))
        .bg(t.card)
        .border_b_1()
        .border_color(t.border)
        .typeset(Type::MICRO)
        .text_color(t.dim)
        .child("MODEL")
        .when(!compact, |row| row.child("CONTEXT"))
        .child(div().flex().justify_end().child("SHOW"));
    let rows = models
        .options
        .iter()
        .map(|option| {
            let hidden = models.settings.is_hidden(&option.id);
            let id = option.id.clone();
            columns(div())
                .min_h(rems(2.5))
                .border_b_1()
                .border_color(t.border)
                .child(mono(option.label().to_owned(), Type::CAPTION, t.text))
                .when(!compact, |row| {
                    row.child(mono(
                        option.context_label(),
                        Type::CAPTION,
                        t.muted,
                    ))
                })
                .child(
                    div().flex().justify_end().child(
                        div()
                            .id(SharedString::from(format!("show-{id}")))
                            .child(ui::switch(!hidden, t))
                            .on_click(cx.listener(move |ws, _, _, cx| {
                                ws.toggle_model_hidden(&id, cx)
                            })),
                    ),
                )
        })
        .collect::<Vec<_>>();

    let main = div()
        .flex_1()
        .min_w(rems(0.))
        .flex()
        .flex_col()
        .gap(sp(4.5))
        .child(ui::screen_title(
            "Models",
            "What each agent runs on unless a run picks another, and which \
             models the picker offers.",
            t,
        ))
        .child(heading("Defaults", t))
        .child(ui::card(t).children(defaults))
        // What plugins add to the screen: the provider's accounts, the
        // forge's sign-in, their settings.
        .children(ws.drawn(
            tau_ui_plugin::points::MODELS,
            &tau_ui_plugin::points::AtApp,
            cx,
        ))
        .child(heading("In the picker", t))
        .child(ui::card(t).child(header).children(rows));
    ui::screen("models", compact, div().flex().child(main)).into_any_element()
}
