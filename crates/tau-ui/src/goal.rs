//! A conversation's goal (`/goal`, tau-goal): the banner above the
//! transcript with what can be done about it, the phone's bar, the
//! Goal tab, and the goal's line in the sidebar.

use gpui::{AnyElement, Context, Div, Hsla, SharedString, div, prelude::*, px};
use tau_agent::tool::RunId;
use tau_goal::{Exhausted, Goal, Record, Status};

use crate::{
    assets::Icon,
    theme::{Design as _, IconSize, Theme, Type, radius, sp},
    ui::{self, ButtonKind},
    view::{RunView, usd},
    workspace::Workspace,
};

/// What a person sends to go on toward the goal.
pub const KEEP_GOING: &str = "Keep going toward the goal.";

/// The goal's color: working, met, or stopped.
pub fn tone(goal: &Goal, t: &Theme) -> Hsla {
    match goal.status {
        Status::Active | Status::Paused => t.accent,
        Status::Met => t.green,
        Status::Stopped(_) => t.red,
    }
}

/// Where the goal stands, in a line.
pub fn status_line(goal: &Goal) -> String {
    let used = format!(
        "{} of {} continuations",
        goal.continuations, goal.max_continuations
    );
    let spent = format!("{} of {}", usd(goal.spent), usd(goal.budget));
    let checks = goal.checks.len();
    let line = match goal.status {
        Status::Active if checks == 0 => {
            "set · checked when the agent stops".to_owned()
        }
        Status::Active => {
            format!("check {checks} not met · {used} · {spent}")
        }
        Status::Paused => format!("paused · {used} · {spent}"),
        Status::Met => format!(
            "met at check {checks} · {} continuations · {}",
            goal.continuations,
            usd(goal.spent)
        ),
        Status::Stopped(Exhausted::Continuations) => format!(
            "stopped: {} of {} continuations used · not met · {}",
            goal.continuations,
            goal.max_continuations,
            usd(goal.spent)
        ),
        Status::Stopped(Exhausted::Budget) => {
            format!("stopped: over its {} budget · not met", usd(goal.budget))
        }
    };
    match &goal.error {
        Some(_) => format!("{line} · the last check failed"),
        None => line,
    }
}

/// The goal as the sidebar and the runs list say it: `2/10`, `met`.
pub fn badge(goal: &Goal) -> String {
    match goal.status {
        Status::Active => {
            format!("{}/{}", goal.continuations, goal.max_continuations)
        }
        Status::Paused => "paused".into(),
        Status::Met => "met".into(),
        Status::Stopped(_) => {
            format!("{}/{}", goal.continuations, goal.max_continuations)
        }
    }
}

impl Workspace {
    pub fn pause_goal(&mut self, run: &RunId, cx: &mut Context<Self>) {
        self.goal_control(run, Record::Paused, cx);
    }

    pub fn clear_goal(&mut self, run: &RunId, cx: &mut Context<Self>) {
        self.goal_control(run, Record::Cleared, cx);
    }

    /// Resume, or Keep going: the goal is active again (a stopped one
    /// with more continuations), and a finished conversation goes on.
    pub fn keep_going(&mut self, run: &RunId, cx: &mut Context<Self>) {
        let Some(view) = self.run(run) else { return };
        let live = view.status.is_live();
        match view.goal.as_ref().map(|goal| goal.status) {
            Some(Status::Paused) => self.goal_control(run, Record::Resumed, cx),
            Some(Status::Stopped(_)) => self.goal_control(
                run,
                Record::Extended {
                    by: tau_goal::DEFAULT_CONTINUATIONS,
                },
                cx,
            ),
            _ => {}
        }
        if !live {
            self.resume_run(run, KEEP_GOING.to_owned(), cx);
        }
    }

    /// Edit, or New goal: the composer holds `/goal` and the condition.
    pub fn edit_goal(&mut self, run: &RunId, cx: &mut Context<Self>) {
        let condition = self
            .run(run)
            .and_then(|view| view.goal.as_ref())
            .filter(|goal| goal.status != Status::Met)
            .map(|goal| goal.condition.clone())
            .unwrap_or_default();
        self.composer.update(cx, |input, cx| {
            input.set_text(format!("/goal {condition}"), cx)
        });
        cx.notify();
    }

    /// Above the transcript while the conversation has a goal.
    pub(crate) fn goal_banner(
        &self,
        run: &RunView,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let goal = run.goal.as_ref()?;
        let tone = tone(goal, t);
        let (bg, border) = match goal.status {
            Status::Met => (t.green_soft, t.green.opacity(0.35)),
            Status::Stopped(_) => (t.red_soft, t.red_border),
            _ => (t.accent_soft, t.accent_border),
        };
        let id = run.id.clone();
        let action = |label: &'static str,
                      kind: ButtonKind,
                      act: fn(
            &mut Workspace,
            &RunId,
            &mut Context<Workspace>,
        )| {
            let id = id.clone();
            div()
                .id(SharedString::from(format!("goal-{label}")))
                .child(ui::button(label, kind, t))
                .on_click(cx.listener(move |ws, _, _, cx| act(ws, &id, cx)))
        };
        let actions: Vec<_> = match goal.status {
            Status::Active => vec![
                action("Pause", ButtonKind::Secondary, Self::pause_goal),
                action("Edit", ButtonKind::Secondary, Self::edit_goal),
                action("Clear", ButtonKind::Secondary, Self::clear_goal),
            ],
            Status::Paused => vec![
                action("Resume", ButtonKind::Primary, Self::keep_going),
                action("Edit", ButtonKind::Secondary, Self::edit_goal),
                action("Clear", ButtonKind::Secondary, Self::clear_goal),
            ],
            Status::Met => vec![
                action("New goal", ButtonKind::Secondary, Self::edit_goal),
                action("Clear", ButtonKind::Secondary, Self::clear_goal),
            ],
            Status::Stopped(_) => vec![
                action(
                    "Keep going · +10",
                    ButtonKind::Primary,
                    Self::keep_going,
                ),
                action("Edit", ButtonKind::Secondary, Self::edit_goal),
                action("Clear", ButtonKind::Secondary, Self::clear_goal),
            ],
        };
        Some(
            div()
                .flex_shrink_0()
                .mx(sp(6.))
                .mt(sp(3.))
                .flex()
                .items_center()
                .gap(sp(3.))
                .px(sp(3.))
                .py(sp(2.5))
                .rounded(radius::BOX)
                .bg(bg)
                .border_1()
                .border_color(border)
                .child(ui::icon(Icon::Target, IconSize::BASE, tone))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .gap(sp(0.75))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(sp(2.))
                                .child(ui::mono("goal", Type::MICRO, tone))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .truncate()
                                        .text_color(t.text)
                                        .child(goal.condition.clone()),
                                ),
                        )
                        .child(ui::mono(
                            status_line(goal),
                            Type::CAPTION,
                            t.muted,
                        )),
                )
                .children(actions)
                .into_any_element(),
        )
    }

    /// On a phone: the goal in a bar under the run's header, which opens
    /// the Goal tab.
    pub(crate) fn phone_goal_bar(
        &self,
        run: &RunView,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let goal = run.goal.as_ref()?;
        let tone = tone(goal, t);
        Some(
            div()
                .id("phone-goal")
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(sp(2.5))
                .min_h(px(48.))
                .px(sp(3.5))
                .py(sp(1.5))
                .bg(t.accent_soft)
                .border_b_1()
                .border_color(t.accent_border)
                .cursor_pointer()
                .child(ui::icon(Icon::Target, IconSize::BASE, tone))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .gap(sp(0.5))
                        .child(div().truncate().child(goal.condition.clone()))
                        .child(
                            ui::mono(status_line(goal), Type::MICRO, tone)
                                .truncate(),
                        ),
                )
                .child(ui::icon(Icon::Chevron, IconSize::BASE, t.muted))
                .on_click(cx.listener(|ws, _, _, cx| {
                    if !ws.sheet_is_open() {
                        ws.toggle_sheet(cx);
                    }
                }))
                .into_any_element(),
        )
    }
}

/// The Goal tab: the condition, where it stands, its limits, and every
/// check.
pub(crate) fn tab(run: &RunView, body: Div, t: &Theme) -> Div {
    let Some(goal) = &run.goal else {
        return body.child(ui::text(
            "No goal. Type /goal and a condition to set one.",
            Type::SMALL,
            t.muted,
        ));
    };
    let tone = tone(goal, t);
    let status = match goal.status {
        Status::Active => "working toward it",
        Status::Paused => "paused",
        Status::Met => "met",
        Status::Stopped(_) => "stopped · not met",
    };
    let share = |used: f64, of: f64| {
        if of > 0.0 {
            (used / of).clamp(0.0, 1.0) as f32
        } else {
            1.0
        }
    };
    let meter = |name: &'static str, value: String, share: f32, fill: Hsla| {
        div()
            .flex()
            .flex_col()
            .gap(sp(1.25))
            .child(
                div()
                    .flex()
                    .typeset(Type::CAPTION)
                    .child(div().flex_1().text_color(t.text_soft).child(name))
                    .child(ui::mono(value, Type::CAPTION, t.muted)),
            )
            .child(ui::bar(share, 4., fill, t.border))
    };
    let checks = goal.checks.iter().rev().map(|check| {
        let (verdict, color) = if check.met {
            ("met", t.green)
        } else {
            ("not met", t.accent)
        };
        let then = match check.continuation {
            Some(n) => format!("sent back, {n} of {}", goal.max_continuations),
            None if check.met => "the run stopped".to_owned(),
            None => "stopped here".to_owned(),
        };
        div()
            .flex()
            .gap(sp(2.5))
            .px(sp(3.))
            .py(sp(2.25))
            .border_b_1()
            .border_color(t.border)
            .child(
                ui::mono(format!("#{}", check.n), Type::MICRO, t.dim)
                    .w(px(22.)),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(sp(0.75))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(sp(2.))
                            .child(ui::mono(verdict, Type::CAPTION, color))
                            .child(div().flex_1())
                            .child(ui::mono(
                                format!(
                                    "turn {} · p {:.2}",
                                    check.turn, check.p
                                ),
                                Type::MICRO,
                                t.dim,
                            )),
                    )
                    .child(ui::text(then, Type::CAPTION, t.muted)),
            )
    });
    let working = run.status.is_live() && goal.status == Status::Active;
    let cost: f64 = goal.checks.iter().map(|check| check.cost).sum();
    body.child(
        div()
            .flex()
            .items_center()
            .child(div().flex_1().child(ui::heading("Goal", t)))
            .child(ui::pill(status, tone, t.raised)),
    )
    .child(
        div()
            .px(sp(3.))
            .py(sp(2.5))
            .rounded(radius::BOX)
            .border_1()
            .border_color(t.border)
            .bg(t.card)
            .child(ui::rich(&goal.condition, t.text, t)),
    )
    .child(ui::text(checks_note(goal), Type::CAPTION, t.dim))
    .child(ui::heading("Limits", t))
    .child(meter(
        "Continuations",
        format!("{} / {}", goal.continuations, goal.max_continuations),
        share(goal.continuations as f64, goal.max_continuations as f64),
        tone,
    ))
    .child(meter(
        "Cost",
        format!("{} / {}", usd(goal.spent), usd(goal.budget)),
        share(goal.spent, goal.budget),
        t.text_soft,
    ))
    .child(ui::heading("Checks", t))
    .child(
        div()
            .flex()
            .flex_col()
            .rounded(radius::BOX)
            .border_1()
            .border_color(t.border)
            .overflow_hidden()
            .when(working, |list| {
                list.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(2.))
                        .px(sp(3.))
                        .py(sp(2.25))
                        .border_b_1()
                        .border_color(t.border)
                        .child(ui::icon(
                            Icon::Spinner,
                            IconSize::SMALL,
                            t.accent,
                        ))
                        .child(ui::text(
                            "working · checked when it stops",
                            Type::CAPTION,
                            t.muted,
                        )),
                )
            })
            .children(checks)
            .when(goal.checks.is_empty() && !working, |list| {
                list.child(div().px(sp(3.)).py(sp(2.25)).child(ui::text(
                    "Not checked yet.",
                    Type::CAPTION,
                    t.muted,
                )))
            }),
    )
    .when_some(goal.error.clone(), |body, error| {
        body.child(ui::text(error, Type::CAPTION, t.red))
    })
    .child(ui::key_values(
        [
            (
                "checked by".into(),
                ui::mono("tau-goal · Jev", Type::CAPTION, t.text),
            ),
            (
                "Jev cost".into(),
                ui::mono(format!("${cost:.5}"), Type::CAPTION, t.text),
            ),
        ],
        t,
    ))
    .child(ui::text(
        "Jev reads the goal, the agent's last answer and its last tool \
         results. When they fall short, the agent is sent back.",
        Type::CAPTION,
        t.dim,
    ))
}

/// How the goal is checked, under its condition.
fn checks_note(goal: &Goal) -> String {
    format!(
        "Checked when the agent stops · met at p ≥ {:.1} · up to {} \
         continuations and {}",
        tau_goal::MET_AT,
        goal.max_continuations,
        usd(goal.budget)
    )
}
