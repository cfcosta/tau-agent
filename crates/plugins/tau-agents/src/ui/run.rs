//! What an agent's conversation shows around it: a banner with where it
//! stands, a panel in the inspector, and its events as cards.

use gpui::{
    AnyElement,
    Div,
    InteractiveElement,
    IntoElement,
    ParentElement,
    SharedString,
    StatefulInteractiveElement,
    Styled,
    div,
    prelude::FluentBuilder,
    rems,
};
use tau_ui_kit::{
    assets::Icon,
    components::{ButtonKind, bar, button, field, icon, mono},
    format::usd,
    theme::{Design as _, IconSize, Theme, Tone, Type, radius, sp, weight},
};
use tau_ui_plugin::{
    ViewCx,
    points::{AtMessage, AtRun},
};

use super::{AgentsUi, mark, status_pill};
use crate::{
    Act,
    Agent,
    Status,
    event::Message,
    shown::{now, relative},
};

/// Above an agent's conversation: the agent, where it stands, its model,
/// and the way to pause it or let it go on.
pub(super) fn banner(
    at: &AtRun,
    view: &mut ViewCx<'_, AgentsUi>,
) -> Option<AnyElement> {
    let agent = view.data.agent(&at.run.repo)?.clone();
    let t = view.theme().clone();
    let paused = agent.status == Status::Paused;
    let toggle = {
        let handle = view.handle.clone();
        let name = agent.name.clone();
        button(
            if paused { "Resume" } else { "Pause" },
            ButtonKind::Secondary,
            &t,
        )
        .id("agent-pause")
        .on_click(move |_, _, cx| {
            let agent = name.clone();
            if paused {
                handle.act(Act::Resume { agent }, cx)
            } else {
                handle.act(Act::Pause { agent }, cx)
            }
        })
    };
    let queued = (agent.queued > 0).then(|| {
        div().typeset(Type::CAPTION).text_color(t.dim).child(
            match agent.queued {
                1 => "1 event waiting".to_owned(),
                n => format!("{n} events waiting"),
            },
        )
    });
    Some(
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(sp(3.))
            .px(sp(4.))
            .py(sp(2.5))
            .border_b_1()
            .border_color(t.border)
            .child(mark(&agent.name, 28., &t))
            .child(
                div()
                    .flex_1()
                    .min_w(rems(12.))
                    .flex()
                    .flex_col()
                    .gap(sp(0.5))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(sp(2.))
                            .child(mono(agent.name.clone(), Type::BODY, t.text))
                            .child(status_pill(&agent.status, &t)),
                    )
                    .child(
                        div()
                            .typeset(Type::CAPTION)
                            .text_color(t.dim)
                            .truncate()
                            .child(format!(
                                "{} · {} effort · {} of {} today",
                                agent.model,
                                agent.effort,
                                usd(agent.spend_today),
                                usd(agent.cap_today),
                            )),
                    ),
            )
            .children(queued)
            .child(toggle)
            .into_any_element(),
    )
}

/// The agent's panel in the inspector: what it spent today, what wakes
/// it, and its versions.
pub(super) fn panel(
    at: &AtRun,
    view: &mut ViewCx<'_, AgentsUi>,
) -> Option<AnyElement> {
    let agent = view.data.agent(&at.run.repo)?.clone();
    let t = view.theme().clone();
    let now = now();
    let allow = (agent.status == Status::AtCap).then(|| {
        let handle = view.handle.clone();
        let name = agent.name.clone();
        button("Allow $1 more today", ButtonKind::Primary, &t)
            .id("agent-allow")
            .on_click(move |_, _, cx| {
                handle.act(
                    Act::AllowMore {
                        agent: name.clone(),
                        amount: 1.0,
                    },
                    cx,
                )
            })
    });
    let today = section("Today", &t)
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(sp(1.))
                .typeset(Type::CAPTION)
                .text_color(t.dim)
                .child(
                    div()
                        .typeset(Type::SMALL)
                        .text_color(t.text)
                        .child(usd(agent.spend_today)),
                )
                .child(format!("of {}", usd(agent.cap_today))),
        )
        .child(bar(agent.spent_share(), 5., t.roles.cost, t.raised))
        .child(
            div()
                .typeset(Type::CAPTION)
                .text_color(t.dim)
                .child("At its cap it stops and waits. Only you can raise it."),
        )
        .children(allow);
    let wakes =
        section("What wakes it", &t).children(if agent.triggers.is_empty() {
            vec![
                div()
                    .typeset(Type::CAPTION)
                    .text_color(t.dim)
                    .child("Nothing yet: it declares this in its agent.luau."),
            ]
        } else {
            agent
                .triggers
                .iter()
                .map(|trigger| {
                    let when = trigger
                        .next
                        .map(|next| relative(next, now))
                        .or_else(|| {
                            trigger.last.map(|last| relative(last, now))
                        })
                        .unwrap_or_default();
                    let row = div()
                        .flex()
                        .items_center()
                        .gap(sp(2.))
                        .child(
                            mono(
                                trigger.kind_label(),
                                Type::MICRO,
                                t.text_soft,
                            )
                            .px(sp(1.5))
                            .py(sp(0.25))
                            .rounded(radius::SMALL)
                            .bg(t.raised),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(rems(0.))
                                .truncate()
                                .typeset(Type::CAPTION)
                                .text_color(t.text_soft)
                                .child(trigger.label()),
                        )
                        .child(
                            div()
                                .typeset(Type::MICRO)
                                .text_color(t.dim)
                                .child(when),
                        );
                    // A poll that failed says why, under it.
                    div().flex().flex_col().gap(sp(0.5)).child(row).children(
                        trigger.error.clone().map(|error| {
                            div()
                                .typeset(Type::MICRO)
                                .text_color(t.red)
                                .line_clamp(2)
                                .child(error)
                        }),
                    )
                })
                .collect()
        });
    let keys = secrets(&agent, view, &t);
    let left = super::notices::rows(&agent, view, &t);
    let versions = section("Versions", &t).children(
        agent
            .versions
            .iter()
            .map(|version| version_row(version, &t)),
    );
    Some(
        div()
            .flex()
            .flex_col()
            .when(!left.is_empty(), |panel| {
                panel.child(section("For you", &t).gap(sp(3.)).children(left))
            })
            .child(today)
            .child(wakes)
            .child(keys)
            .when(!agent.versions.is_empty(), |panel| panel.child(versions))
            .when_some(agent.sandbox.clone(), |panel, problem| {
                panel.child(
                    section("Commands", &t).child(
                        div()
                            .typeset(Type::CAPTION)
                            .text_color(t.tone(Tone::Warn))
                            .child(problem),
                    ),
                )
            })
            .when_some(broken(&agent), |panel, error| {
                panel.child(section("Broken", &t).child(
                    div().typeset(Type::CAPTION).text_color(t.red).child(error),
                ))
            })
            .into_any_element(),
    )
}

/// The secrets `agent` names, by name, and a way to save one: tau's
/// secret store holds the values, which never come back here.
fn secrets(agent: &Agent, view: &mut ViewCx<'_, AgentsUi>, t: &Theme) -> Div {
    let rows = agent.secrets.iter().map(|secret| {
        let (state, tone) = if secret.set {
            ("set", t.green)
        } else {
            ("missing", t.roles.waiting)
        };
        let remove = secret.set.then(|| {
            let handle = view.handle.clone();
            let name = secret.name.clone();
            div()
                .id(SharedString::from(format!(
                    "secret-remove-{}",
                    secret.name
                )))
                .typeset(Type::MICRO)
                .text_color(t.dim)
                .cursor_pointer()
                .child("Remove")
                .on_click(move |_, _, cx| {
                    handle.act(Act::RemoveSecret { name: name.clone() }, cx)
                })
        });
        div()
            .flex()
            .items_center()
            .gap(sp(2.))
            .child(div().flex_1().min_w(rems(0.)).truncate().child(mono(
                secret.name.clone(),
                Type::CAPTION,
                t.text_soft,
            )))
            .child(div().typeset(Type::MICRO).text_color(tone).child(state))
            .children(remove)
    });
    let ui = view.ui.read(view.cx);
    let (name, value) = (ui.secret_name.clone(), ui.secret_value.clone());
    let ready = !name.read(view.cx).text().trim().is_empty()
        && !value.read(view.cx).text().is_empty();
    let save = {
        let ui = view.ui.clone();
        button("Save", ButtonKind::Secondary, t)
            .id("secret-save")
            .when(!ready, |button| button.opacity(0.5))
            .on_click(move |_, _, cx| {
                ui.update(cx, |ui, cx| ui.save_secret(cx))
            })
    };
    section("Keys", t)
        .children(rows)
        .when(agent.secrets.is_empty(), |section| {
            section.child(
                div()
                    .typeset(Type::CAPTION)
                    .text_color(t.dim)
                    .child("Its agent.luau names none yet."),
            )
        })
        .child(
            div()
                .typeset(Type::CAPTION)
                .text_color(t.dim)
                .child("Kept in tau's secret store for every agent. Requests name them; no one reads them back."),
        )
        .child(field(&name, true, t))
        .child(field(&value, true, t))
        .child(save)
}

/// Why nothing wakes `agent`, when nothing does.
fn broken(agent: &Agent) -> Option<String> {
    match &agent.status {
        Status::Broken { error } => Some(error.clone()),
        _ => None,
    }
}

/// A section of the agent's panel, under its heading.
fn section(title: &'static str, t: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(sp(2.))
        .p(sp(3.))
        .border_b_1()
        .border_color(t.border)
        .child(
            div()
                .typeset(Type::CAPTION)
                .font_weight(weight::STRONG)
                .text_color(t.text)
                .child(title),
        )
}

/// A version of its `agent.luau`: live, failed, or one before.
fn version_row(version: &crate::Version, t: &Theme) -> Div {
    let passed = version.tests.iter().filter(|test| test.passed).count();
    let color = if version.live {
        t.green
    } else if version.error.is_some() {
        t.red
    } else {
        t.dim
    };
    let state = if version.live {
        "live".to_owned()
    } else if version.error.is_some() {
        "never live".to_owned()
    } else {
        String::new()
    };
    let short: String = version.commit.chars().take(8).collect();
    div()
        .flex()
        .flex_col()
        .gap(sp(0.5))
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(2.))
                .child(tau_ui_kit::components::dot(color, 7.))
                .child(mono(short, Type::MICRO, t.text_soft))
                .child(
                    div()
                        .flex_1()
                        .min_w(rems(0.))
                        .truncate()
                        .typeset(Type::CAPTION)
                        .text_color(t.text_soft)
                        .child(version.summary.clone()),
                )
                .child(
                    div().typeset(Type::MICRO).text_color(color).child(state),
                ),
        )
        .when(!version.tests.is_empty(), |row| {
            row.child(
                div()
                    .pl(sp(4.))
                    .typeset(Type::MICRO)
                    .text_color(t.dim)
                    .child(format!(
                        "{passed} of {} tests pass",
                        version.tests.len()
                    )),
            )
        })
}

/// An event message, drawn as the batch it is: how many came and when,
/// then a line each with its data under it.
pub(super) fn event_card(
    at: &AtMessage,
    view: &mut ViewCx<'_, AgentsUi>,
) -> Option<AnyElement> {
    let message = Message::parse(&at.text)?;
    let t = view.theme().clone();
    let count = message.events.len();
    let head = div()
        .flex()
        .items_center()
        .gap(sp(2.))
        .px(sp(3.))
        .py(sp(2.))
        .child(icon(Icon::Clock, IconSize::SMALL, t.roles.agent))
        .child(
            div()
                .flex_1()
                .typeset(Type::CAPTION)
                .text_color(t.text)
                .child(match count {
                    1 => "1 event".to_owned(),
                    n => format!("{n} events"),
                }),
        )
        .child(mono(message.span.clone(), Type::MICRO, t.dim));
    let lines = message.events.iter().enumerate().map(|(index, event)| {
        div()
            .id(SharedString::from(format!("event-{}-{index}", at.index)))
            .flex()
            .flex_col()
            .gap(sp(0.5))
            .px(sp(3.))
            .py(sp(1.75))
            .border_t_1()
            .border_color(t.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(2.))
                    .child(
                        mono(event.kind.as_str(), Type::MICRO, t.text_soft)
                            .px(sp(1.5))
                            .py(sp(0.25))
                            .rounded(radius::SMALL)
                            .bg(t.raised),
                    )
                    .child(
                        div()
                            .typeset(Type::CAPTION)
                            .text_color(t.text)
                            .child(event.trigger.clone()),
                    )
                    .child(
                        mono(event.id.clone(), Type::MICRO, t.dim)
                            .flex_1()
                            .min_w(rems(0.))
                            .truncate(),
                    )
                    .child(mono(event.time.clone(), Type::MICRO, t.dim)),
            )
            .when(!event.data.is_empty() && event.data != "null", |line| {
                line.child(
                    mono(event.data.clone(), Type::MICRO, t.muted).truncate(),
                )
            })
    });
    Some(
        div()
            .max_w(rems(40.))
            .rounded(radius::BOX)
            .border_1()
            .border_color(t.border)
            .bg(t.card)
            .overflow_hidden()
            .child(head)
            .children(lines)
            .into_any_element(),
    )
}
