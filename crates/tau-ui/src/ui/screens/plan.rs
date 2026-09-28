//! What each plugin's `start` decided before a run's session opened.

use gpui::{
    AnyElement,
    Context,
    Div,
    FontWeight,
    Hsla,
    div,
    prelude::*,
    px,
    relative,
    rgb,
};
use tau_agent::tool::RunId;

use crate::{
    assets::Icon,
    theme::Theme,
    ui::{self, dot, heading, icon, mono, rich, transcript},
    view::{Item, NoteBody, PluginNote, RunView},
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
    let prompt = run.items.iter().find_map(|item| match item {
        Item::User(text) => Some(text.clone()),
        _ => None,
    });

    let steps = notes.iter().enumerate().map(|(index, note)| {
        let body = match &note.body {
            NoteBody::Distribution {
                levels,
                chosen,
                confidence,
                hints,
                ..
            } => Some(
                chart(levels, *chosen, *confidence, hints, t, compact)
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
                .gap(px(12.))
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
                                .map(|detail| mono(detail, 12., t.muted)),
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
        .border_1()
        .border_color(t.blue_border)
        .rounded(px(10.))
        .bg(rgb(0x171a21))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(16.))
                .py(px(12.))
                .border_b_1()
                .border_color(t.border)
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Preparing the run"),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(t.muted)
                        .child("each plugin's start, in order"),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .p(px(16.))
                .children(steps)
                .when(notes.is_empty(), |timeline| {
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
                        .gap(px(10.))
                        .child("Opening session")
                        .child(mono(
                            format!("{} · {effort}", run.model),
                            12.,
                            t.muted,
                        )),
                    false,
                    t,
                )),
        );

    let notice = div()
        .flex()
        .items_center()
        .gap(px(10.))
        .px(px(14.))
        .py(px(10.))
        .border_1()
        .border_dashed()
        .border_color(t.border_strong)
        .rounded(px(8.))
        .text_color(t.muted)
        .line_height(relative(1.5))
        .child(icon(Icon::Warning, 14., t.muted))
        .child(div().flex_1().child(
            "The effort is fixed for the whole run. If a later phase needs \
             more or less, start a new run and it is scored again.",
        ));

    let side = div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .when(!compact, |side| side.w(px(346.)).flex_shrink_0())
        .child(heading("RunPlan after start", t))
        .child(plan_table(run, t));

    let main = div()
        .flex_1()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap(px(16.))
        .children(prompt.map(|prompt| {
            div().flex().justify_end().child(
                div()
                    .max_w(px(640.))
                    .px(px(14.))
                    .py(px(12.))
                    .bg(t.raised)
                    .border_1()
                    .border_color(rgb(0x30323a))
                    .rounded(px(10.))
                    .line_height(relative(1.55))
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
            .gap(px(24.))
            .child(main)
            .child(side),
    )
    .into_any_element()
}

/// The effort chart at full size: the confidence, then a bar per level
/// with what the level suits.
fn chart(
    levels: &[(String, f32)],
    chosen: usize,
    confidence: Option<(f32, f32)>,
    hints: &[String],
    t: &Theme,
    compact: bool,
) -> Div {
    let max_bar = 104.;
    let bars = levels.iter().enumerate().map(|(index, (_, p))| {
        let pick = index == chosen;
        let ink: Hsla = if pick { t.text } else { t.muted };
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_end()
            .gap(px(4.))
            .child(mono(format!("{p:.2}"), 11., ink))
            .child(
                div()
                    .w(px(36.))
                    .h(px((p * max_bar).max(3.)))
                    .rounded_t(px(4.))
                    .bg(if pick { t.blue } else { rgb(0x3d4452).into() }),
            )
    });
    let labels = levels.iter().enumerate().map(|(index, (name, _))| {
        let pick = index == chosen;
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(2.))
            .child(mono(name.clone(), 12., if pick { t.text } else { t.muted }))
            .children(hints.get(index).map(|hint| {
                div()
                    .text_size(px(11.))
                    .text_color(t.dim)
                    .child(hint.clone())
            }))
    });
    div()
        .flex()
        .when(compact, |chart| chart.flex_col())
        .gap(px(20.))
        .p(px(16.))
        .border_1()
        .border_color(t.border)
        .rounded(px(8.))
        .bg(t.bg)
        .children(confidence.map(|(value, threshold)| {
            let verdict = if value >= threshold {
                "is applied"
            } else {
                "is not applied"
            };
            div()
                .w(px(180.))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .justify_end()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(t.muted)
                        .child("Confidence"),
                )
                .child(mono(format!("{value:.2}"), 28., t.text))
                .child(div().text_size(px(12.)).text_color(t.muted).child(
                    format!(
                        "threshold {threshold:.2}, so the choice {verdict}"
                    ),
                ))
        }))
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(t.muted)
                        .child("Probability of each effort level"),
                )
                .child(
                    div()
                        .flex()
                        .items_end()
                        .h(px(max_bar + 24.))
                        .border_b_1()
                        .border_color(t.border_strong)
                        .children(bars),
                )
                .child(div().flex().children(labels)),
        )
}

/// The plan as a bordered table: name, value, and who set it.
fn plan_table(run: &RunView, t: &Theme) -> Div {
    ui::card(t).children(run.plan.iter().map(|field| {
        div()
            .flex()
            .gap(px(12.))
            .px(px(12.))
            .py(px(10.))
            .border_b_1()
            .border_color(t.border)
            .child(mono(field.name.clone(), 12., t.muted).w(px(96.)))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(mono(field.value.clone(), 12., t.text))
                    .children(
                        field.set_by.clone().map(|by| {
                            mono(format!("set by {by}"), 11., t.blue)
                        }),
                    ),
            )
    }))
}

/// One stop on the timeline: a dot, a rail down to the next, the content.
fn step(color: Hsla, content: Div, rail: bool, t: &Theme) -> impl IntoElement {
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
