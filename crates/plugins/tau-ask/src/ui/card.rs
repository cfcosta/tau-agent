//! The card of an `ask` call: what it asked, then each answer with its
//! note, read from the call's own arguments and result so a stored run
//! shows it as the live one did.

use gpui::{Div, div, prelude::*, rems};
use tau_ui_kit::{
    assets::Icon,
    components::{self as ui, mono},
    theme::{IconSize, Theme, Tone, Type, sp, weight},
};
use tau_ui_plugin::{
    CallData,
    ViewCx,
    points::{AtCard, CardView},
};

use super::AskUi;
use crate::{Ask, Reply, TOOL};

/// What the card shows: the questions, and the reply once there is one.
struct Shown {
    ask: Option<Ask>,
    reply: Option<Reply>,
    /// The call failed: cancelled, or refused questions.
    failed: Option<String>,
}

fn shown(data: &CallData) -> Shown {
    let result = data.result.as_ref();
    Shown {
        ask: serde_json::from_value(data.args.clone()).ok(),
        reply: result
            .filter(|result| !result.error)
            .and_then(|result| result.details.clone())
            .and_then(|details| serde_json::from_value(details).ok()),
        failed: result.filter(|result| result.error).map(|result| {
            result.text.lines().next().unwrap_or_default().to_owned()
        }),
    }
}

/// The card of a call to `ask`.
pub fn card(at: &AtCard, view: &mut ViewCx<'_, AskUi>) -> Option<CardView> {
    if at.tool != TOOL {
        return None;
    }
    let t = view.theme().clone();
    let shown = shown(&at.data);
    let n = shown.ask.as_ref().map_or(0, |ask| ask.questions.len());
    let label = match (&shown.reply, &shown.failed) {
        (Some(Reply::Answered { .. }), _) => format!("{n} answered"),
        (Some(Reply::Declined), _) => "declined".to_owned(),
        (None, Some(_)) => "not answered".to_owned(),
        (None, None) if at.run.live => "waiting for you".to_owned(),
        (None, None) => "not answered".to_owned(),
    };
    let head = shown.ask.as_ref().map(|ask| {
        let headers: Vec<&str> =
            ask.questions.iter().map(|q| q.header.as_str()).collect();
        mono(
            format!("ask · {}", headers.join(" · ")),
            Type::CAPTION,
            t.text_soft,
        )
        .min_w(rems(0.))
        .truncate()
        .into_any_element()
    });
    let waiting =
        shown.reply.is_none() && shown.failed.is_none() && at.run.live;
    Some(CardView {
        head,
        label: Some(label),
        failed: shown.failed.clone(),
        edge: if waiting {
            Some(Tone::Warn)
        } else {
            shown.failed.as_ref().map(|_| Tone::Danger)
        },
        shape: None,
        body: shown.ask.as_ref().map(|ask| {
            body(ask, shown.reply.as_ref(), waiting, &t).into_any_element()
        }),
        folds: false,
        inset: false,
    })
}

/// Each question's header and its answer, with the note under it.
fn body(ask: &Ask, reply: Option<&Reply>, waiting: bool, t: &Theme) -> Div {
    let answers = match reply {
        Some(Reply::Answered { answers }) => Some(answers),
        _ => None,
    };
    let rows = ask.questions.iter().enumerate().map(|(i, question)| {
        let answer = answers.and_then(|answers| answers.get(i));
        div()
            .flex()
            .items_start()
            .gap(sp(3.))
            .child(
                ui::text(question.header.clone(), Type::CAPTION, t.dim)
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
                    .child(match answer {
                        Some(answer) => {
                            ui::text(answer.text(), Type::SMALL, t.text)
                                .font_weight(weight::EMPHASIS)
                        }
                        None => ui::text(
                            question.question.clone(),
                            Type::SMALL,
                            t.muted,
                        ),
                    })
                    .children(answer.and_then(|a| a.note.clone()).map(
                        |note| {
                            div()
                                .flex()
                                .items_start()
                                .gap(sp(1.5))
                                .child(
                                    ui::icon(
                                        Icon::Pencil,
                                        IconSize::TINY,
                                        t.accent,
                                    )
                                    .mt(sp(0.5)),
                                )
                                .child(ui::text(note, Type::CAPTION, t.muted))
                        },
                    )),
            )
    });
    div()
        .flex()
        .flex_col()
        .gap(sp(2.))
        .px(sp(3.))
        .py(sp(2.5))
        .children(rows)
        .when(waiting, |body| {
            body.child(ui::text(
                "Answer in the panel below.",
                Type::CAPTION,
                t.accent,
            ))
        })
        .when(matches!(reply, Some(Reply::Declined)), |body| {
            body.child(ui::text(
                "You declined to answer.",
                Type::CAPTION,
                t.muted,
            ))
        })
}
