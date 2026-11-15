//! What each plugin's `start` decided before a run's session opened.

use gpui::{AnyElement, Context, Div, div, prelude::*, rems};
use heiko_agent::tool::RunId;

use crate::{
    theme::{Design as _, Theme, Type, sp},
    ui::{self, SquareStyle, mono, rich},
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
    let plugin_steps = ws.drawn(
        heiko_ui_plugin::points::PLAN_STEPS,
        &heiko_ui_plugin::points::AtRun { run: run.info() },
        cx,
    );
    let no_plugin_steps = plugin_steps.is_empty();
    let effort = ws
        .run_plan(run)
        .into_iter()
        .find(|field| field.name == "reasoning")
        .map_or("its own effort".to_owned(), |field| field.value.clone());
    let prompt = run.items.iter().find_map(|item| match item {
        Item::User(text) => Some(text.clone()),
        _ => None,
    });

    // Each plugin's start is a numbered step; opening the session is
    // the last.
    let noted = notes.iter().map(|note| {
        div()
            .flex()
            .flex_wrap()
            .items_baseline()
            .gap(sp(2.5))
            .child(mono(note.plugin.clone(), Type::SMALL, t.text))
            .child(div().text_color(t.muted).child(note.text.clone()))
            .child(div().flex_1())
            .children(
                note.detail
                    .clone()
                    .map(|detail| mono(detail, Type::CAPTION, t.dim)),
            )
    });
    let done: Vec<Div> = noted
        .chain(plugin_steps.into_iter().map(|body| div().child(body)))
        .enumerate()
        .map(|(n, content)| {
            step(
                ui::square(
                    (n + 1).to_string(),
                    SquareStyle::Outlined(t.green),
                    t,
                ),
                content,
                t,
            )
        })
        .collect();
    let opening = step(
        ui::square(
            (done.len() + 1).to_string(),
            SquareStyle::Filled(t.blue),
            t,
        ),
        div()
            .flex()
            .flex_wrap()
            .items_baseline()
            .gap(sp(2.5))
            .child("Opening session")
            .child(mono(
                format!("{} · {effort}", run.model),
                Type::CAPTION,
                t.dim,
            )),
        t,
    );
    let timeline = div()
        .flex()
        .flex_col()
        .child(ui::section_header(
            "Preparing the run",
            Some("each plugin's start, in order".into()),
            t,
        ))
        .when(notes.is_empty() && no_plugin_steps, |timeline| {
            timeline.child(
                div()
                    .py(sp(3.))
                    .border_b_1()
                    .border_color(t.hairline)
                    .text_color(t.muted)
                    .child("No plugin changed this run's plan."),
            )
        })
        .children(done)
        .child(opening)
        .child(
            div()
                .pt(sp(3.))
                .typeset(Type::SMALL.leading(1.5))
                .text_color(t.dim)
                .child(
                    "The effort holds until the run stops. Your next \
                     message is scored again, on whatever model it goes to.",
                ),
        );

    let side = div()
        .flex()
        .flex_col()
        .when(!compact, |side| side.w(rems(21.625)).flex_shrink_0())
        .child(ui::section_header("Plan after start", None, t))
        .child(plan_table(&ws.run_plan(run), t));

    let main = div()
        .flex_1()
        .min_w(rems(0.))
        .flex()
        .flex_col()
        .gap(sp(6.))
        .children(prompt.map(|prompt| {
            ui::bubble(t)
                .flex()
                .flex_col()
                .gap(sp(1.5))
                .child(mono("you", Type::CAPTION, t.dim))
                .child(
                    div()
                        .typeset(if compact {
                            Type::TITLE.leading(1.35)
                        } else {
                            Type::HEADING.leading(1.35)
                        })
                        .text_color(t.bright)
                        .child(rich(&prompt, t.bright, t)),
                )
        }))
        .child(timeline);

    ui::screen(
        "plan",
        compact,
        div()
            .flex()
            .when(compact, |layout| layout.flex_col())
            .gap(sp(if compact { 6. } else { 10. }))
            .child(main)
            .child(side),
    )
    .into_any_element()
}

/// The plan as ruled rows: name, value, and who set it.
fn plan_table(plan: &[crate::view::PlanField], t: &Theme) -> Div {
    div().flex().flex_col().children(plan.iter().map(|field| {
        div()
            .flex()
            .gap(sp(3.))
            .py(sp(2.25))
            .border_b_1()
            .border_color(t.hairline)
            .child(
                mono(field.name.clone(), Type::CAPTION, t.muted)
                    .w(rems(6.))
                    .flex_shrink_0(),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(rems(0.))
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

/// One step of the run's start: its numbered square, then what it
/// decided, over a hairline.
fn step(number: Div, content: Div, t: &Theme) -> Div {
    div()
        .flex()
        .items_start()
        .gap(sp(3.5))
        .py(sp(3.))
        .border_b_1()
        .border_color(t.hairline)
        .child(number)
        .child(div().flex_1().min_w(rems(0.)).pt(sp(0.5)).child(content))
}
