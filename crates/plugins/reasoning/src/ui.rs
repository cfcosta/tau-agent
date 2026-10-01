//! tau-reasoning's UI (ADR 0017): its note in the transcript when an
//! effort changes, its line in the plugin list, the plan's reasoning
//! field, its step on the Plan screen, a page with every choice, and its
//! settings on the Models screen.

use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

use gpui::{
    AnyElement,
    App,
    Div,
    Hsla,
    SharedString,
    div,
    prelude::*,
    px,
    relative,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tau_agent::{plugin::Plugin, tool::RunId};
use tau_jev::Jev;
use tau_ui_kit::{
    assets::Icon,
    components::{self as ui, Material as _, NoteHead, heading, link, mono},
    format::usd,
    theme::{Design as _, Theme, Tone, Type, radius, sp},
};
use tau_ui_plugin::{
    HostCx,
    Link,
    Manifest,
    NO_KEY,
    Page,
    PlanField,
    PluginInfo,
    PluginStatus,
    RunCtx,
    RunCx,
    Seam,
    UiPlugin,
    ViewCx,
    needs_jev,
    points::{self, AtAnchor, AtApp, AtRun},
};

use crate::{Choice, DEFAULT_THRESHOLD, Lease, NAME, Reasoning};

/// tau-reasoning with its UI: what tau adds to an agent.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReasoningPlugin;

/// How tau-reasoning picks the effort, as the user set it on the Models
/// screen.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Decide again between a run's steps, when the effort's lease ends,
    /// and not only as each message comes in. Off by default: every
    /// change of effort resends the whole context uncached.
    pub redecide: bool,
    /// How sure Jev must be to change the effort; one of [`THRESHOLDS`].
    pub threshold: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            redecide: false,
            threshold: DEFAULT_THRESHOLD,
        }
    }
}

/// The confidences the Models screen offers, lowest first.
pub const THRESHOLDS: [f64; 5] = [0.5, 0.6, 0.7, 0.8, 0.9];

/// What tau-reasoning did in a run, as its records leave it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// How it stood as the run started: on, or off and why.
    pub starting: Option<String>,
    /// The effort the last message ran at; `None` for the model's
    /// default.
    pub ran_at: Option<String>,
    /// Its line in the plugin list, once it chose: `chose high`.
    pub status: Option<String>,
    /// The plan's reasoning field, once it chose.
    pub plan: Option<String>,
    /// Every choice, in order.
    pub choices: Vec<Choice>,
    /// Each note in the transcript, by its anchor.
    pub notes: BTreeMap<String, Note>,
}

/// One of tau-reasoning's notes in a transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Note {
    /// The effort changed: the choice (by its place in
    /// [`State::choices`]), what the note says, and how it came out.
    Choice {
        choice: usize,
        text: String,
        outcome: String,
    },
    /// Jev could not score the task or the step.
    Failed { message: String, detail: String },
}

/// The notes open in this window, by run and anchor.
#[derive(Debug, Default)]
pub struct Ui {
    open: HashSet<(RunId, String)>,
}

impl State {
    /// Folds one of the plugin's records.
    pub fn apply(&mut self, body: &Value, run: &mut dyn RunCx) {
        if body["kind"] == "starting" {
            self.starting = body["status"].as_str().map(str::to_owned);
            return;
        }
        if body["kind"] == "error" {
            // The message goes on as the last one did; the failure is
            // worth a note all the same.
            let stayed = match body["runs_at"].as_str() {
                Some(effort) => format!("stayed at {effort}"),
                None => "kept the default".into(),
            };
            let key = format!("n{}", self.notes.len());
            self.notes.insert(
                key.clone(),
                Note::Failed {
                    message: body["message"]
                        .as_str()
                        .unwrap_or("failed")
                        .to_owned(),
                    detail: match body["turn"].as_u64() {
                        Some(turn) => format!("turn {turn} · {stayed}"),
                        None => stayed,
                    },
                },
            );
            run.transcript(&key);
            return;
        }
        let Some(choice) = Choice::parse(body) else {
            return;
        };
        let chose = choice.kind == "chose";
        // A choice between turns is for the agent's next step, or for a
        // message the user steered in.
        let what = match (choice.turn, choice.step.as_str()) {
            (None, _) => "this message",
            (Some(_), "user_turn") => "the message steered in",
            (Some(_), _) => "this step",
        };
        // What the message runs at: the pick, else what the last one ran
        // at (a record from before `runs_at` was kept has only the pick).
        let runs_at = choice
            .runs_at
            .clone()
            .or_else(|| chose.then(|| choice.effort.clone()));
        let before = std::mem::replace(&mut self.ran_at, runs_at.clone());
        self.status = Some(match &runs_at {
            Some(effort) if chose => format!("chose {effort}"),
            Some(effort) => format!("stayed at {effort}"),
            None => "kept the default".to_owned(),
        });
        self.plan = Some(runs_at.clone().unwrap_or_else(|| "default".into()));
        self.choices.push(choice.clone());
        // Only a change gets a note: the first pick, or a new effort.
        if runs_at == before {
            return;
        }
        let name = |effort: &Option<String>| match effort {
            Some(effort) => format!("**{effort}**"),
            None => "the model's default".to_owned(),
        };
        let text = match &before {
            None if chose => {
                format!("picked {} reasoning for {what}", name(&runs_at))
            }
            _ => format!("reasoning {} → {}", name(&before), name(&runs_at)),
        };
        let mut outcome = match &runs_at {
            Some(effort) if chose => format!("so {what} runs at {effort}."),
            Some(effort) => format!("so {what} stays at {effort}."),
            None => format!("so {what} runs at the model's default."),
        };
        // With deciding again on, how long Jev said the effort holds.
        if let Some(lease) = choice.lease.as_deref().and_then(Lease::parse) {
            outcome.push_str(&format!(" It holds {}.", lease.holds()));
        }
        let key = format!("n{}", self.notes.len());
        self.notes.insert(
            key.clone(),
            Note::Choice {
                choice: self.choices.len() - 1,
                text,
                outcome,
            },
        );
        run.transcript(&key);
    }

    /// The choice made as the run's last message came in, if Jev scored
    /// it.
    pub fn starting_choice(&self) -> Option<&Choice> {
        self.choices
            .iter()
            .rev()
            .find(|choice| choice.turn.is_none())
    }
}

impl Choice {
    /// What the note under a chart says: the confidence against the
    /// threshold, and how it came out.
    fn verdict(&self, outcome: &str) -> String {
        let comparison = if self.kind == "chose" {
            "above"
        } else {
            "below"
        };
        format!(
            "Confidence {:.2} is {comparison} {:.2}, {outcome}",
            self.confidence, self.threshold
        )
    }

    /// Each level and its probability, lowest first.
    fn bars(&self) -> Vec<(String, f32)> {
        self.levels
            .iter()
            .map(|level| (level.effort.clone(), level.p as f32))
            .collect()
    }
}

impl UiPlugin for ReasoningPlugin {
    type State = State;
    type Data = ();
    type RepoData = ();
    type Settings = Settings;
    type Host = ();
    type Ui = Ui;

    fn name(&self) -> &'static str {
        NAME
    }

    fn host(&self, _cx: &HostCx) -> anyhow::Result<()> {
        Ok(())
    }

    /// On auto, with Jev: an effort picked by hand stands.
    fn agent_plugins(
        &self,
        _host: &(),
        run: &RunCtx,
        settings: &Settings,
    ) -> Vec<Box<dyn Plugin>> {
        let Some(jev) = run.services.get::<Arc<dyn Jev>>() else {
            return Vec::new();
        };
        if run.effort.is_some() {
            return Vec::new();
        }
        vec![Box::new(
            Reasoning::new(jev.clone())
                .redecide(settings.redecide)
                .threshold(settings.threshold),
        )]
    }

    fn starting(
        &self,
        _host: &(),
        run: &RunCtx,
        _settings: &Settings,
    ) -> Vec<Value> {
        let jev = run.services.get::<Arc<dyn Jev>>().is_some();
        let status = match (&run.effort, jev) {
            (Some(effort), _) => format!("off · effort set to {effort}"),
            (None, false) => NO_KEY.to_owned(),
            (None, true) => "picks the effort as the run starts".to_owned(),
        };
        vec![json!({ "kind": "starting", "status": status })]
    }

    fn catalog(
        &self,
        _host: &(),
        cx: &HostCx,
        _settings: &Settings,
    ) -> PluginInfo {
        let jev = cx.services.get::<Arc<dyn Jev>>().is_some();
        PluginInfo {
            name: NAME.into(),
            description: needs_jev(
                jev,
                "Picks each message's reasoning effort on auto",
            ),
            seams: vec![Seam::Start],
            spend: 0.0,
            page: Some(Link::page("choices").param("run", "")),
        }
    }

    fn apply(&self, state: &mut State, body: &Value, run: &mut dyn RunCx) {
        state.apply(body, run);
    }

    fn new_ui(&self, _handle: tau_ui_plugin::Handle, _cx: &mut App) -> Ui {
        Ui::default()
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .page(
                Page::new("choices", choices_page)
                    .title(|_| "Reasoning effort".to_owned()),
            )
            .contribute(points::TRANSCRIPT, transcript_note)
            .contribute_at(points::STATUS, -10, |_: &AtRun, view| {
                let state = view.state?;
                Some(PluginStatus {
                    name: NAME.into(),
                    state: state.status.clone().or(state.starting.clone())?,
                    tone: Tone::Quiet,
                })
            })
            .contribute(points::PLAN, |_: &AtRun, view| {
                Some(PlanField {
                    name: "reasoning".into(),
                    value: view.state?.plan.clone()?,
                    set_by: Some(NAME.into()),
                })
            })
            .contribute(points::PLAN_STEPS, |_: &AtRun, view| {
                let choice = view.state?.starting_choice()?.clone();
                let t = view.theme().clone();
                Some(
                    step(&choice, &t, view.compact)
                        .into_any_element(),
                )
            })
            .contribute(points::MODELS, |_: &AtApp, view| Some(settings_section(view)))
            .contribute(points::PICKER_AUTO, |_: &AtApp, view| {
                Some(if view.jev {
                    "Auto lets tau-reasoning pick the effort for each message, with Jev.".into()
                } else {
                    "Auto leaves the effort to the model: tau-reasoning needs a TypeSafe key (Models).".into()
                })
            })
    }
}

/// The plugin's note at one of its anchors.
fn transcript_note(
    at: &AtAnchor,
    view: &mut ViewCx<'_, ReasoningPlugin>,
) -> Option<AnyElement> {
    let state = view.state?;
    let note = state.notes.get(&at.key)?.clone();
    let t = view.theme().clone();
    let id =
        SharedString::from(format!("reasoning-{}-{}", at.run.id.0, at.key));
    let details = Link::page("choices").param("run", at.run.id.0.to_string());
    let handle = view.handle.clone();
    let details_link = |label: String, id: SharedString| {
        let handle = handle.clone();
        let details = details.clone();
        div()
            .id(id)
            .child(link(label, &t))
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                handle.navigate(details.clone(), cx)
            })
            .into_any_element()
    };
    match note {
        Note::Failed { message, detail } => Some(
            ui::note(
                id,
                NoteHead {
                    plugin: NAME.into(),
                    icon: Icon::Plug,
                    tone: Tone::Danger,
                    text: message,
                    detail: Some(detail),
                },
                None,
                None,
                None,
                None,
                view.compact,
                &t,
            )
            .into_any_element(),
        ),
        Note::Choice {
            choice,
            text,
            outcome,
        } => {
            let choice = state.choices.get(choice)?;
            let key = (at.run.id.clone(), at.key.clone());
            let open = view.read_ui().open.contains(&key);
            let ui_state = view.ui.clone();
            let refresh = view.handle.clone();
            let toggle = Box::new(
                move |_: &gpui::ClickEvent,
                      _: &mut gpui::Window,
                      cx: &mut App| {
                    ui_state.update(cx, |ui, _| {
                        if !ui.open.remove(&key) {
                            ui.open.insert(key.clone());
                        }
                    });
                    refresh.refresh(cx);
                },
            );
            // The phone shows the answer and links to the chart.
            let body = if view.compact {
                let level = choice
                    .levels
                    .get(choice.chosen())
                    .map_or(String::new(), |level| level.effort.clone());
                details_link(
                    format!("Why {level}"),
                    SharedString::from(format!(
                        "why-{}-{}",
                        at.run.id.0, at.key
                    )),
                )
            } else {
                distribution(
                    &choice.bars(),
                    choice.chosen(),
                    &choice.verdict(&outcome),
                    &t,
                    44.,
                )
                .into_any_element()
            };
            Some(
                ui::note(
                    id,
                    NoteHead {
                        plugin: NAME.into(),
                        icon: Icon::Plug,
                        tone: Tone::Info,
                        text,
                        detail: Some(format!("Jev · {}", usd(choice.cost))),
                    },
                    Some(open),
                    Some(toggle),
                    Some(details_link(
                        "Details".into(),
                        SharedString::from(format!(
                            "details-{}-{}",
                            at.run.id.0, at.key
                        )),
                    )),
                    Some(body),
                    view.compact,
                    &t,
                )
                .into_any_element(),
            )
        }
    }
}

/// The page with every choice in a run, newest last.
fn choices_page(view: &mut ViewCx<'_, ReasoningPlugin>) -> AnyElement {
    let t = view.theme().clone();
    let choices: Vec<Choice> = view
        .state
        .map(|state| state.choices.clone())
        .unwrap_or_default();
    let content = div()
        .flex()
        .flex_col()
        .gap(sp(4.5))
        .child(ui::screen_title(
            "Reasoning effort",
            "Each effort tau-reasoning picked for this run, with how sure Jev was.",
            &t,
        ))
        .when(choices.is_empty(), |page| {
            page.child(ui::empty("tau-reasoning has not chosen an effort in this run.", &t))
        })
        .children(choices.iter().map(|choice| step(choice, &t, view.compact)));
    ui::screen("reasoning-choices", view.compact, content).into_any_element()
}

/// One choice at full size: what it was for, then its chart.
fn step(choice: &Choice, t: &Theme, compact: bool) -> Div {
    let what = match (choice.turn, choice.step.as_str()) {
        (None, _) => "As the message came in".to_owned(),
        (Some(turn), "user_turn") => {
            format!("Turn {turn}, a message steered in")
        }
        (Some(turn), _) => format!("Turn {turn}, between steps"),
    };
    let ran = choice
        .runs_at
        .clone()
        .unwrap_or_else(|| "the model's default".into());
    div()
        .flex()
        .flex_col()
        .gap(sp(2.5))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(sp(2.5))
                .child(mono(NAME, Type::SMALL, t.text))
                .child(
                    div()
                        .text_color(t.muted)
                        .child(format!("{what}: runs at {ran}")),
                )
                .child(div().flex_1())
                .child(mono(
                    format!("Jev · {}", usd(choice.cost)),
                    Type::CAPTION,
                    t.muted,
                )),
        )
        .child(chart(choice, t, compact))
}

/// The effort chart at full size: the confidence, then a bar per level
/// with what the level suits.
fn chart(choice: &Choice, t: &Theme, compact: bool) -> Div {
    let max_bar = 104.;
    let chosen = choice.chosen();
    let bars = choice.levels.iter().enumerate().map(|(index, level)| {
        let pick = index == chosen;
        let ink: Hsla = if pick { t.text } else { t.muted };
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_end()
            .gap(sp(1.))
            .child(mono(format!("{:.2}", level.p), Type::MICRO, ink))
            .child(
                div()
                    .w(px(36.))
                    .h(px((level.p as f32 * max_bar).max(3.)))
                    .rounded_t(radius::SMALL)
                    .bg(if pick { t.blue } else { t.bar_idle }),
            )
    });
    let labels = choice.levels.iter().enumerate().map(|(index, level)| {
        let pick = index == chosen;
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .gap(sp(0.5))
            .child(mono(
                level.effort.clone(),
                Type::CAPTION,
                if pick { t.text } else { t.muted },
            ))
            .child(
                div()
                    .typeset(Type::MICRO)
                    .text_color(t.dim)
                    .child(level.suits.clone()),
            )
    });
    let verdict = if choice.confidence >= choice.threshold {
        "is applied"
    } else {
        "is not applied"
    };
    div()
        .flex()
        .when(compact, |chart| chart.flex_col())
        .gap(sp(5.))
        .p(sp(4.))
        .border_1()
        .border_color(t.border)
        .rounded(radius::BOX)
        .well(t)
        .child(
            div()
                .w(px(180.))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .justify_end()
                .gap(sp(1.5))
                .child(
                    div()
                        .typeset(Type::CAPTION)
                        .text_color(t.muted)
                        .child("Confidence"),
                )
                .child(mono(
                    format!("{:.2}", choice.confidence),
                    Type::DISPLAY,
                    t.text,
                ))
                .child(div().typeset(Type::CAPTION).text_color(t.muted).child(
                    format!(
                        "threshold {:.2}, so the choice {verdict}",
                        choice.threshold
                    ),
                )),
        )
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .child(
                    div()
                        .typeset(Type::CAPTION)
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

/// The chart in a note: a bar per level, the chosen one lit, and what
/// it came to.
fn distribution(
    levels: &[(String, f32)],
    chosen: usize,
    note: &str,
    t: &Theme,
    height: f32,
) -> Div {
    let columns = levels.iter().enumerate().map(|(index, (name, p))| {
        let pick = index == chosen;
        let ink = if pick { t.text } else { t.muted };
        div()
            .flex()
            .flex_col()
            .items_center()
            .justify_end()
            .gap(sp(0.75))
            .w(px(58.))
            .child(mono(format!("{p:.2}"), Type::MICRO, ink))
            .child(
                div()
                    .w(px(22.))
                    .h(px((p * height).max(3.)))
                    .rounded_t(radius::BAR)
                    .bg(if pick { t.blue } else { t.bar_idle }),
            )
            .child(
                mono(name.clone(), Type::MICRO, ink)
                    .w_full()
                    .pt(sp(0.75))
                    .border_t_1()
                    .border_color(t.border_strong)
                    .flex()
                    .justify_center(),
            )
    });
    div()
        .flex()
        .flex_wrap()
        .items_end()
        .gap(sp(5.))
        .child(
            div()
                .flex()
                .items_end()
                .gap(sp(1.))
                .h(px(height + 36.))
                .children(columns),
        )
        .child(
            div()
                .max_w(px(280.))
                .typeset(Type::CAPTION)
                .text_color(t.muted)
                .line_height(relative(1.5))
                .child(note.to_owned()),
        )
}

/// The settings on the Models screen: deciding again between steps, and
/// the confidence it takes to change the effort.
fn settings_section(view: &mut ViewCx<'_, ReasoningPlugin>) -> AnyElement {
    let t = view.theme().clone();
    let settings = *view.settings;
    let row = || {
        div()
            .flex()
            .items_center()
            .gap(sp(4.))
            .px(sp(4.))
            .py(sp(3.5))
            .border_b_1()
            .border_color(t.border)
    };
    let what = |name: &str, caption: &str| {
        div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .gap(sp(0.75))
            .child(name.to_owned())
            .child(ui::text(caption.to_owned(), Type::CAPTION, t.muted))
    };
    let thresholds = THRESHOLDS.into_iter().map(|threshold| {
        let on = (settings.threshold - threshold).abs() < 1e-9;
        let handle = view.handle.clone();
        div()
            .id(SharedString::from(format!("threshold-{threshold}")))
            .h(px(28.))
            .px(sp(2.5))
            .flex()
            .items_center()
            .justify_center()
            .rounded(radius::CONTROL)
            .typeset(Type::CAPTION)
            .cursor_pointer()
            .text_color(if on { t.text } else { t.muted })
            .when(on, |segment| segment.key(&t))
            .child(format!("{threshold:.1}"))
            .on_click(move |_, _, cx| {
                handle.save_settings(
                    &Settings {
                        threshold,
                        ..settings
                    },
                    cx,
                )
            })
    });
    let handle = view.handle.clone();
    div()
        .flex()
        .flex_col()
        .gap(sp(2.))
        .child(heading("Reasoning on auto", &t))
        .child(
            ui::card(&t)
                .when(!view.jev, |card| {
                    card.child(row().child(ui::text(
                        "tau-reasoning needs a TypeSafe key (Accounts): until then, auto is the \
                         model's own default.",
                        Type::CAPTION,
                        t.accent,
                    )))
                })
                .child(
                    row()
                        .child(what(
                            "Decide again between steps",
                            "Jev looks again when a step's effort runs out, not only as each \
                             message comes in. Each change resends the whole context uncached, \
                             so it pays only on long tool chains.",
                        ))
                        .child(
                            div()
                                .id("reasoning-redecide")
                                .child(ui::switch(settings.redecide, &t))
                                .on_click(move |_, _, cx| {
                                    handle.save_settings(
                                        &Settings {
                                            redecide: !settings.redecide,
                                            ..settings
                                        },
                                        cx,
                                    )
                                }),
                        ),
                )
                .child(
                    row()
                        .child(what(
                            "Confidence to change the effort",
                            "Below it, a message runs at the effort the last one ran at.",
                        ))
                        .child(
                            div()
                                .flex()
                                .gap(sp(0.5))
                                .p(sp(0.5))
                                .rounded(radius::BOX)
                                .well(&t)
                                .children(thresholds),
                        ),
                ),
        )
        .into_any_element()
}
