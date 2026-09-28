//! What each plugin's `start` decided before a run's session opened.

use gpui::{AnyElement, Context, SharedString, div, prelude::*, px, relative};
use tau_agent::tool::RunId;

use crate::{
    theme::Theme,
    ui::{self, dot, heading, key_values, mono, transcript},
    view::{NoteBody, PluginNote},
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
    let effort = run
        .plan
        .iter()
        .find(|field| field.name == "reasoning")
        .map_or("its own effort".to_owned(), |field| field.value.clone());

    let steps = notes.iter().enumerate().map(|(index, note)| {
        let body = match &note.body {
            NoteBody::Distribution {
                levels,
                chosen,
                note,
            } => Some(
                div()
                    .p(px(16.))
                    .border_1()
                    .border_color(t.border)
                    .rounded(px(8.))
                    .bg(t.bg)
                    .child(transcript::distribution(
                        levels, *chosen, note, t, compact, 96.,
                    ))
                    .into_any_element(),
            ),
            NoteBody::Chips(chips) => Some(
                transcript::chips_view(ws, chips, index, t, cx)
                    .into_any_element(),
            ),
            _ => None,
        };
        step(
            t.green,
            div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .pb(px(18.))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(10.))
                        .child(mono(note.plugin.clone(), 13., t.text))
                        .child(
                            div().text_color(t.muted).child(note.text.clone()),
                        )
                        .child(div().flex_1())
                        .children(
                            note.detail
                                .clone()
                                .map(|detail| mono(detail, 12., t.dim)),
                        ),
                )
                .children(body),
            true,
            t,
        )
    });

    let timeline = div()
        .flex()
        .flex_col()
        .p(px(16.))
        .border_1()
        .border_color(t.blue_border)
        .rounded(px(10.))
        .bg(t.card)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .pb(px(14.))
                .child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child("Preparing the run"),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(t.muted)
                        .child("each plugin's start, in order"),
                ),
        )
        .children(steps)
        .when(notes.is_empty(), |timeline| {
            timeline.child(ui::empty("No plugin changed this run's plan.", t))
        })
        .child(step(
            t.accent,
            div()
                .flex()
                .flex_wrap()
                .gap(px(10.))
                .child("Opening session")
                .child(mono(format!("{} · {effort}", run.model), 12., t.muted)),
            false,
            t,
        ));

    let plan = run.plan.iter().map(|field| {
        (
            SharedString::from(field.name.clone()),
            div()
                .flex()
                .flex_col()
                .items_end()
                .gap(px(2.))
                .child(mono(
                    field.value.clone(),
                    12.,
                    if field.set_by.is_some() {
                        t.blue
                    } else {
                        t.text
                    },
                ))
                .children(
                    field
                        .set_by
                        .clone()
                        .map(|by| mono(format!("set by {by}"), 11., t.dim)),
                ),
        )
    });
    let side = div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .when(!compact, |side| side.w(px(320.)).flex_shrink_0())
        .child(heading("RunPlan after start", t))
        .child(key_values(plan, t))
        .child(
            div()
                .text_size(px(12.))
                .text_color(t.dim)
                .line_height(relative(1.5))
                .child("The plan is fixed for the whole run. A phase that needs a different effort starts a new run, and that run is planned again."),
        );

    ui::screen(
        "plan",
        compact,
        div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(ui::screen_title(
                format!("How {} was set up", run.title),
                "Plugins may change the run's settings only here, before the session opens.",
                t,
            ))
            .child(
                div()
                    .flex()
                    .when(compact, |layout| layout.flex_col())
                    .gap(px(24.))
                    .child(div().flex_1().min_w(px(0.)).child(timeline))
                    .child(side),
            ),
    )
    .into_any_element()
}

/// One stop on the timeline: a dot, a rail down to the next, the content.
fn step(
    color: gpui::Hsla,
    content: gpui::Div,
    rail: bool,
    t: &Theme,
) -> impl IntoElement {
    div()
        .flex()
        .gap(px(12.))
        .child(
            div()
                .w(px(14.))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .items_center()
                .child(div().mt(px(4.)).child(dot(color, 10.)))
                .when(rail, |column| {
                    column
                        .child(div().flex_1().w(px(1.)).my(px(4.)).bg(t.border))
                }),
        )
        .child(div().flex_1().min_w(px(0.)).child(content))
}
