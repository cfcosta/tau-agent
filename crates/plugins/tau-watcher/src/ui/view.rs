//! The note as the transcript draws it, and as the line above the
//! composer.

use gpui::{AnyElement, SharedString, div, prelude::*, rems};
use tau_ui_kit::{
    components::{self as ui, ButtonKind},
    theme::{Design as _, Tone, Type, radius, sp},
};
use tau_ui_plugin::{
    Handle,
    RunInfo,
    ViewCx,
    points::{AtAnchor, AtRun},
};

use super::WatcherUi;
use crate::{
    cadence::chat_text,
    record::{Answer, Record},
    state::{Note, Status},
};

/// The note under the step that asked for it: its line, what the person
/// can do about it, and the explanation once they asked for it.
pub fn annotation(
    at: &AtAnchor,
    view: &mut ViewCx<'_, WatcherUi>,
) -> Option<AnyElement> {
    let note = view
        .state?
        .notes
        .iter()
        .find(|note| note.key == at.key)?
        .clone();
    let t = view.theme().clone();
    let amber = t.tone(Tone::Warn);
    let muted = matches!(note.status, Status::Knew | Status::Dismissed);
    let explain = (note.status == Status::Learned)
        .then(|| note.explain.clone())
        .flatten();
    let actions = actions(&note, &at.run, &at.run.id.0, view);
    Some(
        div()
            .id(SharedString::from(format!(
                "watcher-{}-{}",
                at.run.id.0, at.key
            )))
            .flex()
            .flex_col()
            .gap(sp(2.))
            .pl(sp(3.5))
            .pr(sp(3.))
            .py(sp(2.5))
            .rounded_r(radius::LARGE)
            .border_l_2()
            .border_color(if muted { t.border_strong } else { amber })
            .bg(amber.opacity(if muted { 0.03 } else { 0.07 }))
            .typeset(Type::CAPTION)
            .child(
                div()
                    .text_color(if muted { t.muted } else { t.text_soft })
                    .child(format!("{} · {}", note.tag.label(), note.line)),
            )
            .when_some(explain, |card, explain| {
                card.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(sp(1.5))
                        .child(div().text_color(t.text).child(explain.title))
                        .children(explain.bullets.into_iter().map(|bullet| {
                            div()
                                .flex()
                                .gap(sp(2.))
                                .text_color(t.text_soft)
                                .child(div().text_color(t.dim).child("•"))
                                .child(div().min_w(rems(0.)).child(bullet))
                        })),
                )
            })
            .when(note.status == Status::Knew, |card| {
                card.child(ui::text("You knew this.", Type::MICRO, t.dim))
            })
            .when(!actions.is_empty(), |card| {
                card.child(
                    div().flex().flex_wrap().gap(sp(2.)).children(actions),
                )
            })
            .into_any_element(),
    )
}

/// The newest note as one line above the composer, while it is new.
pub fn band(
    at: &AtRun,
    view: &mut ViewCx<'_, WatcherUi>,
) -> Option<AnyElement> {
    let note = view.state?.band()?.clone();
    let t = view.theme().clone();
    let amber = t.tone(Tone::Warn);
    let actions = actions(&note, &at.run, "band", view);
    Some(
        div()
            .id(SharedString::from(format!("watcher-band-{}", at.run.id.0)))
            .flex()
            .items_center()
            .gap(sp(3.))
            .pl(sp(3.5))
            .pr(sp(2.5))
            .py(sp(1.5))
            .rounded_r(radius::LARGE)
            .border_l_2()
            .border_color(amber)
            .bg(amber.opacity(0.07))
            .typeset(Type::CAPTION)
            .child(
                div()
                    .flex_1()
                    .min_w(rems(0.))
                    .truncate()
                    .text_color(t.text_soft)
                    .child(format!("{} · {}", note.tag.label(), note.line)),
            )
            .child(div().flex().flex_shrink_0().gap(sp(2.)).children(actions))
            .into_any_element(),
    )
}

/// The buttons a note gets: all of them while it is new, and Knew and
/// Chat once its explanation is open.
fn actions(
    note: &Note,
    run: &RunInfo,
    place: &str,
    view: &ViewCx<'_, WatcherUi>,
) -> Vec<AnyElement> {
    let t = view.theme().clone();
    let mut wanted = Vec::new();
    if note.status == Status::New && note.explain.is_some() {
        wanted.push(("Learn more", Answer::Learned));
    }
    if matches!(note.status, Status::New | Status::Learned) {
        wanted.push(("Knew this already", Answer::Knew));
        wanted.push(("Chat about it", Answer::Chatted));
    }
    if note.status == Status::New {
        wanted.push(("Dismiss", Answer::Dismissed));
    }
    wanted
        .into_iter()
        .map(|(label, answer)| {
            let (handle, run, key) =
                (view.handle.clone(), run.clone(), note.key.clone());
            let line = note.line.clone();
            div()
                .id(SharedString::from(format!(
                    "watcher-{place}-{}-{answer:?}",
                    note.key
                )))
                .child(ui::button(label, ButtonKind::Secondary, &t))
                .on_click(move |_, _, cx| {
                    act(&handle, &run, &key, answer, &line, cx)
                })
                .into_any_element()
        })
        .collect()
}

/// Stores the person's answer, which folds into the state as any record
/// does, and for a chat puts the note in the composer.
fn act(
    handle: &Handle,
    run: &RunInfo,
    key: &str,
    answer: Answer,
    line: &str,
    cx: &mut gpui::App,
) {
    if answer == Answer::Chatted {
        handle.composer(chat_text(line), cx);
    }
    handle.record(
        &run.id,
        Record::Answered {
            key: key.to_owned(),
            answer,
        },
        cx,
    );
}
