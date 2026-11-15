//! Model settings: each agent's default, which models the picker shows,
//! and plugins' sections: the provider's accounts, the forge's sign-in.

use gpui::{AnyElement, Context, SharedString, div, prelude::*, rems};

use crate::{
    theme::{Design as _, Theme, Type, sp, weight},
    ui::{self, mono},
    workspace::{PickerTarget, Workspace},
};

pub fn render(
    ws: &Workspace,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let models = &ws.catalog.models;
    // Each agent, what it is for, and the words that change its model.
    let defaults = models
        .agents
        .iter()
        .map(|(agent, what)| {
            div()
                .flex()
                .items_center()
                .gap(sp(4.))
                .py(sp(2.75))
                .border_b_1()
                .border_color(t.hairline)
                .child(
                    div()
                        .w(rems(if compact { 6. } else { 9. }))
                        .flex_shrink_0()
                        .truncate()
                        .font_weight(weight::STRONG)
                        .text_color(t.bright)
                        .child(agent.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(rems(0.))
                        .text_color(t.text_soft)
                        .child(what.clone()),
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
    };
    let header = columns(div())
        .pb(sp(2.))
        .border_b_1()
        .border_color(t.slate)
        .typeset(Type::CAPTION)
        .text_color(t.dim)
        .child("model")
        .when(!compact, |row| row.child("context"))
        .child(div().flex().justify_end().child("in the picker"));
    let rows = models
        .options
        .iter()
        .map(|option| {
            let hidden = models.settings.is_hidden(&option.id);
            let id = option.id.clone();
            columns(div())
                .min_h(rems(2.5))
                .border_b_1()
                .border_color(t.hairline)
                .child(mono(option.label().to_owned(), Type::SMALL, t.text))
                .when(!compact, |row| {
                    row.child(mono(
                        option.context_label(),
                        Type::CAPTION,
                        t.dim,
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

    let section = |title: &'static str| {
        div().pt(sp(8.)).child(ui::section_header(title, None, t))
    };
    let page = div()
        .flex()
        .flex_col()
        .max_w(rems(60.))
        .when(compact, |page| page.px(sp(4.5)).py(sp(4.)))
        .when(!compact, |page| {
            page.pl(sp(13.)).pr(sp(11.)).pt(sp(11.)).pb(sp(11.))
        })
        .typeset(Type::BODY)
        .child(ui::section_header("Defaults", None, t))
        .children(defaults)
        // What plugins add to the screen: the provider's accounts, the
        // forge's sign-in, their settings.
        .children(
            ws.drawn(
                heiko_ui_plugin::points::MODELS,
                &heiko_ui_plugin::points::AtApp,
                cx,
            )
            .into_iter()
            .map(|drawn| div().pt(sp(8.)).child(drawn)),
        )
        .child(section("Models"))
        .child(header.pt(sp(3.)))
        .children(rows);
    div()
        .id("models")
        .flex_1()
        .min_h(rems(0.))
        .overflow_y_scroll()
        .child(page)
        .into_any_element()
}
