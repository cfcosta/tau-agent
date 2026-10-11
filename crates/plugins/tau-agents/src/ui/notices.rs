//! What an agent left for the person (ADR 0037): each note, with a way
//! to dismiss it, and each question, with a way to answer it. Its panel
//! and the Agents page draw the same rows.

use gpui::{
    Div,
    InteractiveElement,
    ParentElement,
    SharedString,
    StatefulInteractiveElement,
    Styled,
    div,
    prelude::FluentBuilder,
};
use tau_ui_kit::{
    components::{ButtonKind, button, field},
    theme::{Design as _, Theme, Type, sp},
};
use tau_ui_plugin::ViewCx;

use super::AgentsUi;
use crate::{
    Act,
    Agent,
    Notice,
    shown::{now, relative},
};

/// A row for each of `agent`'s notices, oldest first. A question with
/// choices answers with one click; one without takes the answer field,
/// the first such question only, so the field is never drawn twice.
pub(super) fn rows(
    agent: &Agent,
    view: &mut ViewCx<'_, AgentsUi>,
    t: &Theme,
) -> Vec<Div> {
    let now = now();
    let first_open = agent
        .notices
        .iter()
        .position(|notice| notice.choices.as_ref().is_some_and(Vec::is_empty));
    agent
        .notices
        .iter()
        .enumerate()
        .map(|(at, notice)| {
            let actions =
                actions(agent, notice, first_open == Some(at), view, t);
            div()
                .flex()
                .flex_col()
                .gap(sp(1.5))
                .child(div().typeset(Type::MICRO).text_color(t.dim).child(
                    format!(
                        "{} · {}",
                        if notice.asks() { "asks" } else { "says" },
                        relative(notice.at, now)
                    ),
                ))
                .child(
                    div()
                        .typeset(Type::SMALL)
                        .text_color(t.text)
                        .child(notice.text.clone()),
                )
                .child(actions)
        })
        .collect()
}

/// What the person can do with `notice`.
fn actions(
    agent: &Agent,
    notice: &Notice,
    takes_the_field: bool,
    view: &mut ViewCx<'_, AgentsUi>,
    t: &Theme,
) -> Div {
    let id = |what: &str| {
        SharedString::from(format!(
            "notice-{}-{}-{what}",
            agent.name, notice.id
        ))
    };
    let row = div().flex().flex_wrap().items_center().gap(sp(2.));
    let (name, key) = (agent.name.clone(), notice.id.clone());
    match &notice.choices {
        None => {
            let handle = view.handle.clone();
            row.child(
                button("Dismiss", ButtonKind::Secondary, t)
                    .id(id("dismiss"))
                    .on_click(move |_, _, cx| {
                        handle.act(
                            Act::Dismiss {
                                agent: name.clone(),
                                notice: key.clone(),
                            },
                            cx,
                        )
                    }),
            )
        }
        Some(choices) if !choices.is_empty() => {
            row.children(choices.iter().enumerate().map(|(n, choice)| {
                let handle = view.handle.clone();
                let (name, key, answer) =
                    (name.clone(), key.clone(), choice.clone());
                button(
                    choice.clone(),
                    if n == 0 {
                        ButtonKind::Primary
                    } else {
                        ButtonKind::Secondary
                    },
                    t,
                )
                .id(id(&format!("choice-{n}")))
                .on_click(move |_, _, cx| {
                    handle.act(
                        Act::Answer {
                            agent: name.clone(),
                            notice: key.clone(),
                            answer: answer.clone(),
                        },
                        cx,
                    )
                })
            }))
        }
        Some(_) if takes_the_field => {
            let input = view.ui.read(view.cx).answer.clone();
            let ready = !input.read(view.cx).text().trim().is_empty();
            let ui = view.ui.clone();
            row.child(div().flex_1().child(field(&input, true, t)))
                .child(
                    button("Answer", ButtonKind::Primary, t)
                        .id(id("answer"))
                        .when(!ready, |button| button.opacity(0.5))
                        .on_click(move |_, _, cx| {
                            ui.update(cx, |ui, cx| {
                                ui.send_answer(&name, &key, cx)
                            })
                        }),
                )
        }
        Some(_) => row.child(
            div()
                .typeset(Type::CAPTION)
                .text_color(t.dim)
                .child("Answer the question before it first."),
        ),
    }
}
