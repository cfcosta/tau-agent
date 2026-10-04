//! What each plugin's `start` decided before a run's session opened.

use gpui::{AnyElement, Context, Div, Hsla, div, prelude::*, relative, rems};
use tau_agent::tool::RunId;

use crate::{
    assets::Icon,
    theme::{Design as _, IconSize, Theme, Type, radius, sp, weight},
    ui::{self, dot, heading, icon, mono, rich},
    view::{Item, PluginNote},
    workspace::Workspace,
};

pub fn render(
    ws: &Workspace,
    run: &RunId,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let Some(run) = ws.run(run) else {
        return ui::empty("That run is not in this workspace.", t)
            .into_any_element();
    };
    let notes: Vec<&PluginNote> = run.start_notes().collect();
    // What plugins with their UI decided in `start`.
    let plugin_steps = ws.contributions(
        tau_ui_plugin::points::PLAN_STEPS,
        &tau_ui_plugin::points::AtRun { run: run.info() },
        cx,
    );
    let no_plugin_steps = plugin_steps.is_empty();
    let effort = ws
        .run_plan(run, cx)
        .into_iter()
        .find(|field| field.name == "reasoning")
        .map_or("its own effort".to_owned(), |field| field.value.clone());
    let prompt = run.items.iter().find_map(|item| match item {
        Item::User(text) => Some(text.clone()),
        _ => None,
    });

    let steps = notes.iter().map(|note| {
        step(
            t.green,
            div().flex().flex_col().gap(sp(3.)).pb(sp(4.5)).child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(sp(2.5))
                    .child(mono(note.plugin.clone(), Type::SMALL, t.text))
                    .child(div().text_color(t.muted).child(note.text.clone()))
                    .child(div().flex_1())
                    .children(
                        note.detail
                            .clone()
                            .map(|detail| mono(detail, Type::CAPTION, t.muted)),
                    ),
            ),
            true,
            t,
        )
    });

    let timeline = div()
        .flex()
        .flex_col()
        .border_1()
        .border_color(t.blue_border)
        .rounded(radius::LARGE)
        .bg(t.info_panel)
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(2.5))
                .px(sp(4.))
                .py(sp(3.))
                .border_b_1()
                .border_color(t.border)
                .child(
                    div()
                        .font_weight(weight::STRONG)
                        .child("Preparing the run"),
                )
                .child(
                    div()
                        .typeset(Type::CAPTION)
                        .text_color(t.muted)
                        .child("each plugin's start, in order"),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .p(sp(4.))
                .children(steps)
                .children(plugin_steps.into_iter().map(|body| {
                    step(t.green, div().pb(sp(4.5)).child(body), true, t)
                }))
                .when(notes.is_empty() && no_plugin_steps, |timeline| {
                    timeline.child(ui::empty(
                        "No plugin changed this run's plan.",
                        t,
                    ))
                })
                .child(step(
                    t.accent,
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(sp(2.5))
                        .child("Opening session")
                        .child(mono(
                            format!("{} · {effort}", run.model),
                            Type::CAPTION,
                            t.muted,
                        )),
                    false,
                    t,
                )),
        );

    let notice = div()
        .flex()
        .items_center()
        .gap(sp(2.5))
        .px(sp(3.5))
        .py(sp(2.5))
        .border_1()
        .border_dashed()
        .border_color(t.border_strong)
        .rounded(radius::BOX)
        .text_color(t.muted)
        .line_height(relative(1.5))
        .child(icon(Icon::Warning, IconSize::BASE, t.muted))
        .child(div().flex_1().child(
            "The effort holds until the run stops. Your next message is \
             scored again, on whatever model it goes to.",
        ));

    let side = div()
        .flex()
        .flex_col()
        .gap(sp(3.5))
        .when(!compact, |side| side.w(rems(21.625)).flex_shrink_0())
        .child(heading("RunPlan after start", t))
        .child(plan_table(&ws.run_plan(run, cx), t));

    let main = div()
        .flex_1()
        .min_w(rems(0.))
        .flex()
        .flex_col()
        .gap(sp(4.))
        .children(prompt.map(|prompt| {
            div().flex().justify_end().child(
                ui::bubble(t)
                    .max_w(rems(40.))
                    .child(rich(&prompt, t.text, t)),
            )
        }))
        .child(timeline)
        .child(notice);

    ui::screen(
        "plan",
        compact,
        div()
            .flex()
            .when(compact, |layout| layout.flex_col())
            .gap(sp(6.))
            .child(main)
            .child(side),
    )
    .into_any_element()
}

/// The plan as a bordered table: name, value, and who set it.
fn plan_table(plan: &[crate::view::PlanField], t: &Theme) -> Div {
    ui::card(t).children(plan.iter().map(|field| {
        div()
            .flex()
            .gap(sp(3.))
            .px(sp(3.))
            .py(sp(2.5))
            .border_b_1()
            .border_color(t.border)
            .child(mono(field.name.clone(), Type::CAPTION, t.muted).w(rems(6.)))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(sp(0.5))
                    .child(mono(field.value.clone(), Type::CAPTION, t.text))
                    .children(field.set_by.clone().map(|by| {
                        mono(format!("set by {by}"), Type::MICRO, t.blue)
                    })),
            )
    }))
}

/// One stop on the timeline: a dot, a rail down to the next, the content.
fn step(color: Hsla, content: Div, rail: bool, t: &Theme) -> impl IntoElement {
    div()
        .flex()
        .gap(sp(3.))
        .child(
            div()
                .w(rems(0.875))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .items_center()
                .child(div().mt(sp(1.)).child(dot(color, 10.)))
                .when(rail, |column| {
                    column.child(
                        div().flex_1().w(rems(0.0625)).my(sp(1.)).bg(t.border),
                    )
                }),
        )
        .child(div().flex_1().min_w(rems(0.)).child(content))
}
