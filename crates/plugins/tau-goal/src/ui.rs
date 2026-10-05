//! tau-goal's UI (ADR 0017): the banner above a run with what can be
//! done about its goal, the phone's bar, the goal's section of the
//! inspector, its line in the sidebar and the plugin list, its notes in
//! the transcript, how a `/goal` message and a continuation read, and the
//! `/goal` command with its popover.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use gpui::{
    AnyElement,
    App,
    AppContext as _,
    Div,
    Entity,
    Hsla,
    SharedString,
    div,
    prelude::*,
    rems,
};
use serde::{Deserialize, Serialize};
use tau_agent::plugin::Plugin;
use tau_jev::Jev;
use tau_ui_kit::{
    assets::Icon,
    components::{self as ui, ButtonKind, Material as _, NoteHead, mono},
    format::usd,
    input::{InputEvent, TextInput},
    prose::rich,
    theme::{Design as _, IconSize, Theme, Tone, Type, radius, sp},
};
use tau_ui_plugin::{
    Fold,
    Handle,
    HostCx,
    Manifest,
    PluginInfo,
    PluginUi,
    Request,
    RowNote,
    RunCtx,
    RunCx,
    RunInfo,
    RunKind,
    Seam,
    SlashCommand,
    UiPlugin,
    ViewCx,
    needs_jev,
    points::{self, AtAnchor, AtMessage, AtRun},
};

use crate::{
    CONTINUATION_PREFIX,
    Command,
    DEFAULT_BUDGET,
    DEFAULT_CONTINUATIONS,
    Exhausted,
    Goal,
    GoalPlugin,
    MET_AT,
    NAME,
    Record,
    Status,
    set_input,
    set_message,
};

/// What a person sends to go on toward the goal.
pub const KEEP_GOING: &str = "Keep going toward the goal.";

/// tau-goal with its UI: what tau adds to an agent.
#[derive(Debug, Clone, Copy, Default)]
pub struct GoalUi;

/// A conversation's goal, as tau-goal's records leave it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub goal: Option<Goal>,
    /// Whether tau-goal runs with the run as it goes now, and checks its
    /// goal: there was a TypeSafe key when it started, and it is not a
    /// sub-agent. A stored run is checked again only once it goes on.
    pub checks: bool,
    /// Each note in the transcript, by its anchor.
    pub notes: BTreeMap<String, Note>,
    /// The continuations a check's note says sent the model back: their
    /// messages say nothing more.
    pub held: BTreeSet<u32>,
}

/// One of tau-goal's notes in a transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub text: String,
    pub detail: String,
    pub tone: Tone,
}

/// The `/goal` popover's limits, in this window.
pub struct Ui {
    continuations: Entity<TextInput>,
    budget: Entity<TextInput>,
}

impl Fold for State {
    type Record = Record;

    /// Folds one of the plugin's records, or what it says as a run
    /// starts.
    fn apply(&mut self, record: Record, run: &mut dyn RunCx) {
        if let Record::Starting { checks } = record {
            self.checks = checks;
            return;
        }
        Goal::apply(&mut self.goal, &record);
        let note = match &record {
            Record::Check(check) => {
                let max = self.goal.as_ref().map_or(0, |g| g.max_continuations);
                if let Some(n) = check.continuation {
                    self.held.insert(n);
                }
                Some(if check.met {
                    Note {
                        text: "the goal is met".into(),
                        detail: format!(
                            "check {} · turn {} · p {:.2} · the run stops",
                            check.n, check.turn, check.p
                        ),
                        tone: Tone::Good,
                    }
                } else {
                    let next = match check.continuation {
                        Some(n) => format!("continuing {n} of {max}"),
                        None => "not continuing".to_owned(),
                    };
                    Note {
                        text: "the goal is not met yet".into(),
                        detail: format!(
                            "check {} · turn {} · p {:.2} · {next}",
                            check.n, check.turn, check.p
                        ),
                        tone: Tone::Warn,
                    }
                })
            }
            Record::Stopped { why } => {
                let why = match why {
                    Exhausted::Continuations => "out of continuations",
                    Exhausted::Budget => "out of budget",
                };
                Some(Note {
                    text: format!("stopped the goal: {why}, and it is not met"),
                    detail: "before_stop".into(),
                    tone: Tone::Danger,
                })
            }
            Record::Error { message } => Some(Note {
                text: message.clone(),
                detail: "not checked".into(),
                tone: Tone::Danger,
            }),
            _ => None,
        };
        if let Some(note) = note {
            let key = format!("g{}", self.notes.len());
            self.notes.insert(key.clone(), note);
            run.transcript(&key);
        }
    }
}

impl State {
    /// The plugin's line in the run's plugin list.
    pub fn status(&self) -> Option<String> {
        let goal = self.goal.as_ref()?;
        Some(match goal.status {
            Status::Active => format!(
                "{} of {} continuations",
                goal.continuations, goal.max_continuations
            ),
            Status::Paused => "paused".to_owned(),
            Status::Met => format!("met at check {}", goal.checks.len()),
            Status::Stopped(_) => "stopped, not met".to_owned(),
        })
    }
}

/// The goal's tone: working, met, or stopped.
pub fn tone(goal: &Goal) -> Tone {
    match goal.status {
        Status::Active | Status::Paused => Tone::Warn,
        Status::Met => Tone::Good,
        Status::Stopped(_) => Tone::Danger,
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
        Status::Active => format!("check {checks} not met · {used} · {spent}"),
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

/// Why nothing checks a goal that is still to be met, if nothing does:
/// there is no TypeSafe key, or the run going on started without
/// tau-goal.
pub fn unchecked(
    goal: &Goal,
    jev: bool,
    live: bool,
    checks: bool,
) -> Option<&'static str> {
    if !matches!(goal.status, Status::Active | Status::Paused) {
        return None;
    }
    if !jev {
        return Some("Not checked: tau-goal needs a TypeSafe key (Models).");
    }
    (live && !checks).then_some(
        "Not checked while this run goes on: it started without tau-goal. It is \
         checked from the next message.",
    )
}

/// The goal as the sidebar and the runs list say it: `2/10`, `met`.
pub fn badge(goal: &Goal) -> String {
    match goal.status {
        Status::Active | Status::Stopped(_) => {
            format!("{}/{}", goal.continuations, goal.max_continuations)
        }
        Status::Paused => "paused".into(),
        Status::Met => "met".into(),
    }
}

/// The continuation a message sends the model back for, when tau-goal
/// wrote it.
fn continuation_of(text: &str) -> Option<u32> {
    let rest = text.strip_prefix(CONTINUATION_PREFIX)?;
    let at = rest.find("Continuation ")? + "Continuation ".len();
    rest[at..].split_whitespace().next()?.parse().ok()
}

impl UiPlugin for GoalUi {
    type State = State;
    type Data = ();
    type RepoData = ();
    type Settings = ();
    type Host = ();
    type Ui = Ui;

    fn name(&self) -> &'static str {
        NAME
    }

    /// A run's goal, checked with Jev; a sub-agent has none.
    async fn agent_plugins(
        &self,
        _host: &(),
        run: &RunCtx,
        _settings: &(),
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        Ok(match run.services.get::<Arc<dyn Jev>>() {
            Some(jev) if run.kind != RunKind::SubAgent => {
                vec![Box::new(GoalPlugin::new(jev.clone()))]
            }
            _ => Vec::new(),
        })
    }

    async fn starting(
        &self,
        _host: &(),
        run: &RunCtx,
        _settings: &(),
    ) -> Vec<Record> {
        let checks = run.services.get::<Arc<dyn Jev>>().is_some()
            && run.kind != RunKind::SubAgent;
        vec![Record::Starting { checks }]
    }

    async fn catalog(
        &self,
        _host: &(),
        cx: &HostCx,
        _settings: &(),
    ) -> PluginInfo {
        let jev = cx.services.get::<Arc<dyn Jev>>().is_some();
        PluginInfo {
            description: needs_jev(
                jev,
                "Keeps a conversation going until its /goal holds",
            ),
            seams: vec![Seam::Start, Seam::AfterTool, Seam::BeforeStop],
            page: None,
            ..Default::default()
        }
    }

    fn read_prompt(&self, prompt: &str) -> Option<String> {
        set_message(prompt)
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .contribute(points::RUN_BANNER, banner)
            .contribute_at(points::INSPECTOR, -100, |at: &AtRun, view| {
                let goal = view.state?.goal.clone()?;
                let t = view.theme().clone();
                Some(section(&goal, at.run.live, &t).into_any_element())
            })
            .status(State::status)
            .contribute(points::RUN_ROW, |_: &AtRun, view| {
                let goal = view.state?.goal.as_ref()?;
                let line = match goal.status {
                    Status::Met => format!("goal met · {}", goal.condition),
                    Status::Stopped(_) => {
                        format!("goal not met · {}", goal.condition)
                    }
                    _ => format!("goal · {}", goal.condition),
                };
                Some(RowNote {
                    line: Some(line),
                    phone_line: Some(format!("goal · {}", status_line(goal))),
                    count: Some(badge(goal)),
                    icon: Icon::Target,
                    tone: tone(goal),
                })
            })
            .contribute(points::TRANSCRIPT, |at: &AtAnchor, view| {
                let note = view.state?.notes.get(&at.key)?.clone();
                let t = view.theme().clone();
                Some(
                    ui::note(
                        SharedString::from(format!(
                            "goal-{}-{}",
                            at.run.id.0, at.key
                        )),
                        NoteHead {
                            plugin: NAME.into(),
                            icon: Icon::Target,
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
            })
            .contribute(points::USER_MESSAGE, user_message)
            .command(
                SlashCommand::new(
                    "goal",
                    "Keep working until a condition holds",
                    run_command,
                )
                .args("<condition>")
                .icon(Icon::Target)
                .popover(|_, view| popover(view)),
            )
    }
}

impl PluginUi for Ui {
    fn new(handle: Handle, cx: &mut Context<Self>) -> Self {
        let limit = |text: String, cx: &mut gpui::Context<Ui>| {
            let input = cx.new(|cx| {
                let mut input = TextInput::new("", cx).keep_on_submit();
                input.set_text(text, cx);
                input
            });
            // Enter in a limit sends the goal, as Enter in the composer.
            let handle = handle.clone();
            cx.subscribe(&input, move |_, _, _: &InputEvent, cx| {
                handle.request(Request::Submit, cx)
            })
            .detach();
            input
        };
        Ui {
            continuations: limit(DEFAULT_CONTINUATIONS.to_string(), cx),
            budget: limit(format!("{DEFAULT_BUDGET:.2}"), cx),
        }
    }
}

/// A person's message, when tau-goal reads it as its own: a `/goal`
/// shows as the goal it set, and a continuation as tau-goal's note,
/// unless the check that sent it says it already.
fn user_message(
    at: &AtMessage,
    view: &mut ViewCx<'_, GoalUi>,
) -> Option<AnyElement> {
    let t = view.theme().clone();
    if let Some(condition) = set_message(&at.text) {
        let goal = view.state.and_then(|state| state.goal.as_ref());
        return Some(
            goal_set(goal, &condition, &t, view.compact).into_any_element(),
        );
    }
    let said = at.text.strip_prefix(CONTINUATION_PREFIX)?;
    let held = continuation_of(&at.text).is_some_and(|n| {
        view.state.is_some_and(|state| state.held.contains(&n))
    });
    if held {
        return Some(div().into_any_element());
    }
    let first = said.lines().next().unwrap_or_default();
    Some(
        ui::note(
            SharedString::from(format!(
                "goal-said-{}-{}",
                at.run.id.0, at.index
            )),
            NoteHead {
                plugin: NAME.into(),
                icon: Icon::Target,
                tone: Tone::Warn,
                text: first.trim_end_matches('.').to_owned(),
                detail: Some("before_stop".into()),
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

/// The person setting a goal: the condition, and the goal's limits while
/// it is the conversation's goal.
fn goal_set(
    goal: Option<&Goal>,
    condition: &str,
    t: &Theme,
    compact: bool,
) -> Div {
    let limits = goal.filter(|goal| goal.condition == condition).map(|goal| {
        [
            format!("up to {} continuations", goal.max_continuations),
            format!("{} budget", usd(goal.budget)),
            "checked by Jev".to_owned(),
        ]
    });
    div().flex().justify_end().child(
        ui::bubble(t)
            .border_color(t.accent_border)
            .max_w(rems((if compact { 300. } else { 620. }) / 16.))
            .flex()
            .flex_col()
            .gap(sp(2.))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(sp(2.))
                    .child(ui::icon(Icon::Target, IconSize::BASE, t.accent))
                    .child(mono("/goal", Type::CAPTION, t.accent))
                    .child(div().flex_1().child(rich(condition, t.text, t))),
            )
            .when_some(limits, |bubble, limits| {
                bubble.child(div().flex().flex_wrap().gap(sp(1.5)).children(
                    limits.into_iter().map(|limit| {
                        mono(limit, Type::MICRO, t.muted)
                            .px(sp(2.))
                            .py(sp(0.5))
                            .border_1()
                            .border_color(t.border)
                            .rounded(radius::LARGE)
                    }),
                ))
            }),
    )
}

/// What the goal's buttons do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Pause,
    Edit,
    Clear,
    KeepGoing,
}

/// Carries out a goal button's `act` on `run`.
pub fn act(
    act: Act,
    run: &RunInfo,
    goal: &Goal,
    jev: bool,
    handle: &Handle,
    cx: &mut App,
) {
    match act {
        Act::Pause => handle.record(&run.id, Record::Paused, cx),
        Act::Clear => handle.record(&run.id, Record::Cleared, cx),
        Act::Edit => {
            let condition = if goal.status == Status::Met {
                ""
            } else {
                &goal.condition
            };
            handle.composer(format!("/goal {condition}"), cx);
        }
        // Resume, or Keep going: the goal is active again (a stopped one
        // with more continuations), and a finished conversation goes on.
        // Without a key nothing would check it, so it says so instead.
        Act::KeepGoing => {
            if !jev {
                handle.alert(
                    "Goals need Jev",
                    "tau-goal checks goals with Jev. Add a TypeSafe key on the Models \
                     screen, then go on.",
                    cx,
                );
                return;
            }
            match goal.status {
                Status::Paused => handle.record(&run.id, Record::Resumed, cx),
                Status::Stopped(_) => handle.record(
                    &run.id,
                    Record::Extended {
                        by: DEFAULT_CONTINUATIONS,
                    },
                    cx,
                ),
                _ => {}
            }
            if !run.live {
                handle.send(Some(&run.id), KEEP_GOING, cx);
            }
        }
    }
}

/// Above the transcript while the conversation has a goal; on a phone, a
/// bar that opens the run's details.
fn banner(at: &AtRun, view: &mut ViewCx<'_, GoalUi>) -> Option<AnyElement> {
    let state = view.state?;
    let goal = state.goal.clone()?;
    let t = view.theme().clone();
    let tone_color = t.tone(tone(&goal));
    if view.compact {
        let handle = view.handle.clone();
        return Some(
            div()
                .id("phone-goal")
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(sp(2.5))
                .min_h(rems(3.))
                .px(sp(3.5))
                .py(sp(1.5))
                .bg(t.accent_soft)
                .border_b_1()
                .border_color(t.accent_border)
                .cursor_pointer()
                .child(ui::icon(Icon::Target, IconSize::BASE, tone_color))
                .child(
                    div()
                        .flex_1()
                        .min_w(rems(0.))
                        .flex()
                        .flex_col()
                        .gap(sp(0.5))
                        .child(div().truncate().child(goal.condition.clone()))
                        .child(
                            mono(status_line(&goal), Type::MICRO, tone_color)
                                .truncate(),
                        ),
                )
                .child(ui::icon(Icon::Chevron, IconSize::BASE, t.muted))
                .on_click(move |_, _, cx| handle.run_details(cx))
                .into_any_element(),
        );
    }
    let (bg, border): (Hsla, Hsla) = match goal.status {
        Status::Met => (t.green_soft, t.green.opacity(0.35)),
        Status::Stopped(_) => (t.red_soft, t.red_border),
        _ => (t.accent_soft, t.accent_border),
    };
    let button = |label: &'static str, kind: ButtonKind, what: Act| {
        let (run, goal, jev, handle) =
            (at.run.clone(), goal.clone(), view.jev, view.handle.clone());
        div()
            .id(SharedString::from(format!("goal-{label}")))
            .child(ui::button(label, kind, &t))
            .on_click(move |_, _, cx| act(what, &run, &goal, jev, &handle, cx))
    };
    let actions = match goal.status {
        Status::Active => vec![
            button("Pause", ButtonKind::Secondary, Act::Pause),
            button("Edit", ButtonKind::Secondary, Act::Edit),
            button("Clear", ButtonKind::Secondary, Act::Clear),
        ],
        Status::Paused => vec![
            button("Resume", ButtonKind::Primary, Act::KeepGoing),
            button("Edit", ButtonKind::Secondary, Act::Edit),
            button("Clear", ButtonKind::Secondary, Act::Clear),
        ],
        Status::Met => vec![
            button("New goal", ButtonKind::Secondary, Act::Edit),
            button("Clear", ButtonKind::Secondary, Act::Clear),
        ],
        Status::Stopped(_) => vec![
            button("Keep going · +10", ButtonKind::Primary, Act::KeepGoing),
            button("Edit", ButtonKind::Secondary, Act::Edit),
            button("Clear", ButtonKind::Secondary, Act::Clear),
        ],
    };
    let why = unchecked(&goal, view.jev, at.run.live, state.checks);
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
            .lit(bg, &t)
            .border_1()
            .border_color(border)
            .child(ui::icon(Icon::Target, IconSize::BASE, tone_color))
            .child(
                div()
                    .flex_1()
                    .min_w(rems(0.))
                    .flex()
                    .flex_col()
                    .gap(sp(0.75))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(sp(2.))
                            .child(mono("goal", Type::MICRO, tone_color))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(rems(0.))
                                    .truncate()
                                    .text_color(t.text)
                                    .child(goal.condition.clone()),
                            ),
                    )
                    .child(mono(status_line(&goal), Type::CAPTION, t.muted))
                    .children(
                        why.map(|why| ui::text(why, Type::CAPTION, t.accent)),
                    ),
            )
            .children(actions)
            .into_any_element(),
    )
}

/// The goal's section of the inspector: the condition, where it stands,
/// its limits, and every check.
fn section(goal: &Goal, live: bool, t: &Theme) -> Div {
    let tone_color = t.tone(tone(goal));
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
                    .child(mono(value, Type::CAPTION, t.muted)),
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
                mono(format!("#{}", check.n), Type::MICRO, t.dim)
                    .w(rems(1.375)),
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
                            .child(mono(verdict, Type::CAPTION, color))
                            .child(div().flex_1())
                            .child(mono(
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
    let working = live && goal.status == Status::Active;
    let cost: f64 = goal.checks.iter().map(|check| check.cost).sum();
    div()
        .flex()
        .flex_col()
        .gap(sp(3.))
        .child(
            div()
                .flex()
                .items_center()
                .child(div().flex_1().child(ui::heading("Goal", t)))
                .child(ui::pill(status, tone_color, t.raised)),
        )
        .child(
            div()
                .px(sp(3.))
                .py(sp(2.5))
                .rounded(radius::BOX)
                .border_1()
                .border_color(t.border)
                .raised(t)
                .child(rich(&goal.condition, t.text, t)),
        )
        .child(ui::text(
            format!(
                "Checked when the agent stops · met at p ≥ {MET_AT:.1} · up to {} \
                 continuations and {}",
                goal.max_continuations,
                usd(goal.budget)
            ),
            Type::CAPTION,
            t.dim,
        ))
        .child(ui::heading("Limits", t))
        .child(meter(
            "Continuations",
            format!("{} / {}", goal.continuations, goal.max_continuations),
            share(goal.continuations as f64, goal.max_continuations as f64),
            tone_color,
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
                            .child(ui::icon(Icon::Spinner, IconSize::SMALL, t.accent))
                            .child(ui::text("working · checked when it stops", Type::CAPTION, t.muted)),
                    )
                })
                .children(checks)
                .when(goal.checks.is_empty() && !working, |list| {
                    list.child(
                        div()
                            .px(sp(3.))
                            .py(sp(2.25))
                            .child(ui::text("Not checked yet.", Type::CAPTION, t.muted)),
                    )
                }),
        )
        .when_some(goal.error.clone(), |body, error| {
            body.child(ui::text(error, Type::CAPTION, t.red))
        })
        .child(ui::key_values(
            [
                ("checked by".into(), mono("tau-goal · Jev", Type::CAPTION, t.text)),
                ("Jev cost".into(), mono(format!("${cost:.5}"), Type::CAPTION, t.text)),
            ],
            t,
        ))
        .child(ui::text(
            "Jev reads the goal, the agent's last answer and its last tool results. When \
             they fall short, the agent is sent back.",
            Type::CAPTION,
            t.dim,
        ))
}

/// The limits in the `/goal` popover, or the defaults where a field
/// does not read as a number.
pub fn limits(ui: &Ui, cx: &App) -> (u32, f64) {
    let continuations = ui
        .continuations
        .read(cx)
        .text()
        .trim()
        .parse()
        .unwrap_or(DEFAULT_CONTINUATIONS);
    let budget = ui
        .budget
        .read(cx)
        .text()
        .trim()
        .trim_start_matches('$')
        .parse()
        .unwrap_or(DEFAULT_BUDGET);
    (continuations, budget)
}

/// `/goal …`: sets the open conversation's goal, or a new run's, with the
/// popover's limits unless the command gave its own. `/goal clear`
/// clears it.
fn run_command(args: &str, view: &mut ViewCx<'_, GoalUi>) {
    let text = format!("/goal {args}");
    let Some(command) = Command::parse(&text) else {
        // `/goal` alone is not a goal yet: it stays, to be written.
        view.handle.composer("/goal ", view.cx);
        return;
    };
    let run = view.run.cloned();
    let Command::Set {
        condition,
        continuations,
        budget,
    } = command
    else {
        if let Some(run) = run {
            view.handle.record(&run.id, Record::Cleared, view.cx);
        }
        return;
    };
    if !view.jev {
        view.handle.alert(
            "Goals need Jev",
            "tau-goal checks goals with Jev. Add a TypeSafe key on the Models screen, then \
             set the goal again.",
            view.cx,
        );
        view.handle.composer(format!("/goal {condition}"), view.cx);
        return;
    }
    // Limits typed with the command win over the popover's.
    let typed = text.contains("--continuations") || text.contains("--budget");
    let (continuations, budget) = if typed {
        (continuations, budget)
    } else {
        let ui = view.ui.clone();
        limits(ui.read(view.cx), view.cx)
    };
    let checks = view.state.is_some_and(|state| state.checks);
    let set = Record::Set {
        goal: condition.clone(),
        continuations,
        budget,
    };
    match run {
        // A run going on without tau-goal, started before the key or as a
        // sub-agent: the goal is kept for when it goes on, and nothing
        // tells the model it is checked now.
        Some(run) if run.live && !checks => {
            view.handle.record(&run.id, set, view.cx);
            view.handle.alert(
                "The goal is checked from the next message",
                "This run started without tau-goal, so nothing checks the goal while it \
                 goes on. It is kept, and checked once the conversation goes on.",
                view.cx,
            );
        }
        // A run going on takes it at its next stop, and the model is told.
        Some(run) if run.live => {
            view.handle.record(&run.id, set, view.cx);
            view.handle.steer(&run.id, set_input(&condition), view.cx);
        }
        // A finished conversation goes on with it; a new run starts.
        _ => view.handle.send(
            run.as_ref().map(|run| &run.id),
            format!("/goal --continuations {continuations} --budget {budget:.2} {condition}"),
            view.cx,
        ),
    }
}

/// The `/goal` popover: what a goal does, and its limits.
fn popover(view: &mut ViewCx<'_, GoalUi>) -> AnyElement {
    let t = view.theme().clone();
    let jev = view.jev;
    let ui = view.ui.read(view.cx);
    let (continuations, budget) = (ui.continuations.clone(), ui.budget.clone());
    let limit =
        |label: &'static str, input: Entity<TextInput>, unit: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(sp(2.))
                .typeset(Type::CAPTION)
                .text_color(t.muted)
                .child(label)
                .child(
                    div()
                        .w(rems(4.))
                        .h(rems(1.75))
                        .flex()
                        .items_center()
                        .px(sp(2.))
                        .rounded(radius::CONTROL)
                        .well(&t)
                        .typeset(Type::CAPTION.mono())
                        .text_color(t.text)
                        .child(input),
                )
                .when(!unit.is_empty(), |row| row.child(unit))
        };
    div()
        .flex()
        .flex_col()
        .gap(sp(0.5))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(1.5))
                .px(sp(2.5))
                .pt(sp(2.))
                .pb(sp(2.5))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(2.))
                        .child(ui::icon(Icon::Target, IconSize::BASE, t.accent))
                        .child(mono("/goal", Type::SMALL, t.accent))
                        .child(mono("<condition>", Type::CAPTION, t.dim)),
                )
                .child(ui::text(
                    if jev {
                        "The agent keeps going until this holds. Each time it would stop, \
                         tau-goal asks Jev whether the goal is met; if not, the agent goes on."
                    } else {
                        "Goals are checked with Jev: add a TypeSafe key on the Models screen \
                         first."
                    },
                    Type::CAPTION,
                    if jev { t.muted } else { t.accent },
                )),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(sp(4.))
                .px(sp(2.5))
                .py(sp(2.))
                .border_t_1()
                .border_b_1()
                .border_color(t.border)
                .child(limit("Stop after", continuations, "continuations"))
                .child(limit("Budget $", budget, "")),
        )
        .when(!view.compact, |popover| {
            popover.child(ui::hints(&[("Enter", "set the goal and start"), ("Esc", "close")], &t))
        })
        .into_any_element()
}
