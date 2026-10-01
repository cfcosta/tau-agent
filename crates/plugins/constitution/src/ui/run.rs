//! What the checks draw on a run: the plugin's line in its list, its
//! notes, what it decided on each card, and its inspector section.

use gpui::{AnyElement, SharedString, div, prelude::*, px, relative};
use tau_ui_kit::{
    assets::Icon,
    components::{
        self as ui,
        ButtonKind,
        NoteHead,
        bar,
        heading,
        key_values,
        link,
        mono,
    },
    format::usd,
    theme::{Design as _, Type, sp},
};
use tau_ui_plugin::{
    Link,
    PluginStatus,
    ViewCx,
    points::{AtAnchor, AtCard, AtRun},
};

use super::ConstitutionUi;
use crate::{NAME, VerdictKind};

/// The rule `rule` on its repository's page.
fn rule_link(repo: &str, rule: &str) -> Link {
    Link::page("rules")
        .param("repo", repo.to_owned())
        .param("rule", rule.to_owned())
}

pub fn status(
    _: &AtRun,
    view: &mut ViewCx<'_, ConstitutionUi>,
) -> Option<PluginStatus> {
    let (state, tone) = view.state?.status()?;
    Some(PluginStatus {
        name: NAME.into(),
        state,
        tone,
    })
}

/// One of its notes: a check Jev could not answer, an answer held or
/// flagged.
pub fn note(
    at: &AtAnchor,
    view: &mut ViewCx<'_, ConstitutionUi>,
) -> Option<AnyElement> {
    let note = view.state?.notes.get(&at.key)?.clone();
    let t = view.theme().clone();
    Some(
        ui::note(
            SharedString::from(format!("rules-{}-{}", at.run.id.0, at.key)),
            NoteHead {
                plugin: NAME.into(),
                icon: Icon::Blocked,
                tone: note.tone,
                text: note.text,
                detail: Some(note.detail),
            },
            None,
            None,
            None,
            None,
            view.compact,
            &t,
        )
        .into_any_element(),
    )
}

/// Beside a checked call's title: the rule that flagged it and its
/// score, with a way to review it; or, quietly, the scores that passed.
pub fn badge(
    at: &AtCard,
    view: &mut ViewCx<'_, ConstitutionUi>,
) -> Option<AnyElement> {
    let call = view.state?.calls.get(at.keys.first()?)?.clone();
    let t = view.theme().clone();
    match &call.verdict {
        Some(verdict) if verdict.kind == VerdictKind::Flagged => {
            let to = rule_link(&at.run.repo, &verdict.rule);
            let handle = view.handle.clone();
            Some(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(2.))
                    .flex_shrink_0()
                    .child(mono(
                        format!("{} {:.2}", verdict.rule, verdict.score),
                        Type::MICRO,
                        t.dim,
                    ))
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "review-{}",
                                at.call_id
                            )))
                            .child(link("Review", &t))
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                handle.navigate(to.clone(), cx)
                            }),
                    )
                    .into_any_element(),
            )
        }
        Some(_) => None,
        None if !view.compact && !call.scores.is_empty() => {
            let scores: Vec<String> = call
                .scores
                .iter()
                .map(|(rule, score)| format!("{rule} {score:.2}"))
                .collect();
            Some(
                mono(scores.join(" · "), Type::MICRO, t.dim)
                    .flex_shrink_0()
                    .into_any_element(),
            )
        }
        None => None,
    }
}

/// Under a blocked call: how sure Jev was against the rule's marks, and
/// the rule.
pub fn blocked(
    at: &AtCard,
    view: &mut ViewCx<'_, ConstitutionUi>,
) -> Option<AnyElement> {
    let call = view.state?.calls.get(at.keys.first()?)?.clone();
    let verdict = call
        .verdict
        .filter(|verdict| verdict.kind == VerdictKind::Blocked)?;
    let t = view.theme().clone();
    let compact = view.compact;
    let thresholds = view
        .repo(&at.run.repo)
        .and_then(|rules| rules.rule(&verdict.rule))
        .map(|rule| (rule.review as f32, rule.block as f32));
    let p = verdict.score as f32;
    let tick = |at: f32| {
        div()
            .absolute()
            .top(px(-3.))
            .left(relative(at))
            .w(px(2.))
            .h(px(14.))
            .bg(t.text)
    };
    let meter = div()
        .w(px(if compact { 150. } else { 220. }))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .gap(sp(1.5))
        .child(
            div()
                .flex()
                .typeset(Type::CAPTION)
                .child(div().flex_1().text_color(t.muted).child("p(violation)"))
                .child(mono(format!("{p:.2}"), Type::CAPTION, t.text)),
        )
        .child(
            div()
                .relative()
                .child(bar(p, 8., t.red, t.border))
                .when_some(thresholds, |track, (review, block)| {
                    track.child(tick(review)).child(tick(block))
                }),
        )
        .when_some(thresholds, |meter, (review, block)| {
            meter.child(
                div()
                    .relative()
                    .h(px(14.))
                    .child(
                        div()
                            .absolute()
                            .left(relative(review))
                            .ml(sp(-5.))
                            .child(mono("review", Type::MICRO, t.dim)),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(relative(block))
                            .ml(sp(-4.))
                            .child(mono("block", Type::MICRO, t.dim)),
                    ),
            )
        });
    let to = rule_link(&at.run.repo, &verdict.rule);
    let handle = view.handle.clone();
    Some(
        div()
            .flex()
            .when(compact, |row| row.flex_col())
            .items_start()
            .gap(sp(4.))
            .px(sp(3.))
            .py(sp(3.))
            .bg(t.danger_surface)
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap(sp(2.))
                    .child(mono(NAME, Type::CAPTION, t.red))
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "rule-{}",
                                at.call_id
                            )))
                            .child(link(format!("Rule {}", verdict.rule), &t))
                            .on_click(move |_, _, cx| {
                                handle.navigate(to.clone(), cx)
                            }),
                    ),
            )
            .child(meter)
            .into_any_element(),
    )
}

/// The inspector's section: what the checks did, and a flagged call to
/// review once the run is done.
pub fn inspector(
    at: &AtRun,
    view: &mut ViewCx<'_, ConstitutionUi>,
) -> Option<AnyElement> {
    let stats = view.state?.stats.clone();
    if stats.is_empty() {
        return None;
    }
    let t = view.theme().clone();
    let with_rules = |list: &[String]| {
        if list.is_empty() {
            "0".to_owned()
        } else {
            format!("{} ({})", list.len(), list.join(", "))
        }
    };
    let value = |text: String, color| mono(text, Type::CAPTION, color);
    let flagged = stats
        .flagged
        .first()
        .filter(|_| !at.run.live)
        .map(|rule| rule_link(&at.run.repo, rule));
    let handle = view.handle.clone();
    Some(
        div()
            .flex()
            .flex_col()
            .gap(sp(3.))
            .child(heading("Constitution checks", &t))
            .child(key_values(
                [
                    (
                        "calls checked".into(),
                        value(stats.calls.to_string(), t.text),
                    ),
                    (
                        "answers checked".into(),
                        value(stats.answers.to_string(), t.text),
                    ),
                    (
                        "questions asked".into(),
                        value(stats.questions.to_string(), t.text),
                    ),
                    (
                        "blocked".into(),
                        value(
                            with_rules(&stats.blocked),
                            if stats.blocked.is_empty() {
                                t.text
                            } else {
                                t.red
                            },
                        ),
                    ),
                    (
                        "flagged".into(),
                        value(
                            with_rules(&stats.flagged),
                            if stats.flagged.is_empty() {
                                t.text
                            } else {
                                t.accent
                            },
                        ),
                    ),
                    (
                        "held".into(),
                        value(
                            match (stats.max_holds, stats.held.is_empty()) {
                                (Some(max), false) => format!(
                                    "{} of {max} ({})",
                                    stats.held.len(),
                                    stats.held.join(", ")
                                ),
                                _ => with_rules(&stats.held),
                            },
                            if stats.held.is_empty() {
                                t.text
                            } else {
                                t.accent
                            },
                        ),
                    ),
                    ("Jev cost".into(), value(usd(stats.cost), t.text)),
                ],
                &t,
            ))
            .child(
                div()
                    .typeset(Type::CAPTION)
                    .text_color(t.dim)
                    .line_height(relative(1.5))
                    .child(
                        "Only the fields the model wrote are checked, such as \
                         edit.newText or bash.command. Tool output never goes \
                         to Jev.",
                    ),
            )
            .when_some(flagged, |section, to| {
                section.child(
                    div()
                        .id("review-flagged")
                        .child(ui::button(
                            "Review the flagged call",
                            ButtonKind::Secondary,
                            &t,
                        ))
                        .on_click(move |_, _, cx| {
                            handle.navigate(to.clone(), cx)
                        }),
                )
            })
            .into_any_element(),
    )
}
