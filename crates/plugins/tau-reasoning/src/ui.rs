//! tau-reasoning's UI (ADR 0017): its note in the transcript when an
//! effort changes, its line in the plugin list, the plan's reasoning
//! field, its step on the Plan screen, a page with every choice, and its
//! settings on the Models screen. Each is a view tree (ADR 0045): a
//! note's chart opens and closes as a section, its state the
//! interface's.

use std::{collections::BTreeMap, sync::Arc};

use gpui::AnyElement;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tau_agent::plugin::Plugin;
use tau_jev::Jev;
use tau_ui_kit::{format::usd, theme::Tone};
use tau_ui_plugin::{
    Fold,
    HostCx,
    HostHalf,
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
    tree::{self, KeyValue, Node},
};

use crate::{Choice, DEFAULT_THRESHOLD, Lease, NAME, Reasoning, Record};

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

impl Fold for State {
    type Record = Record;

    /// Folds one of the plugin's records.
    fn apply(&mut self, record: Record, run: &mut dyn RunCx) {
        let choice = match record {
            Record::Starting { status } => {
                self.starting = status;
                return;
            }
            Record::Error {
                message,
                runs_at,
                turn,
                ..
            } => {
                // The message goes on as the last one did; the failure is
                // worth a note all the same.
                let stayed = match runs_at {
                    Some(effort) => format!("stayed at {effort}"),
                    None => "kept the default".into(),
                };
                let key = format!("n{}", self.notes.len());
                self.notes.insert(
                    key.clone(),
                    Note::Failed {
                        message,
                        detail: match turn {
                            Some(turn) => format!("turn {turn} · {stayed}"),
                            None => stayed,
                        },
                    },
                );
                run.transcript(&key);
                return;
            }
            Record::Context(_) => return,
            Record::Choice(choice) => choice,
        };
        let chose = choice.chose();
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
        let kept = choice.kept_for_cache;
        self.status = Some(match &runs_at {
            Some(effort) if kept.is_some() => format!("kept {effort}"),
            Some(effort) if chose => format!("chose {effort}"),
            Some(effort) => format!("stayed at {effort}"),
            None => "kept the default".to_owned(),
        });
        self.plan = Some(runs_at.clone().unwrap_or_else(|| "default".into()));
        self.choices.push(choice.clone());
        // Only a change gets a note: the first pick, or a new effort; and
        // an effort kept over Jev's, to keep the cache.
        if runs_at == before && kept.is_none() {
            return;
        }
        let name = |effort: &Option<String>| match effort {
            Some(effort) => format!("**{effort}**"),
            None => "the model's default".to_owned(),
        };
        let text = match &before {
            _ if kept.is_some() => {
                format!("kept {} reasoning for {what}", name(&runs_at))
            }
            None if chose => {
                format!("picked {} reasoning for {what}", name(&runs_at))
            }
            _ => format!("reasoning {} → {}", name(&before), name(&runs_at)),
        };
        let stays = match &runs_at {
            Some(effort) => format!("stays at {effort}"),
            None => "stays at the model's default".to_owned(),
        };
        let mut outcome = match (&runs_at, kept) {
            (_, Some(tokens)) => format!(
                "but {what} {stays}: switching to {} would resend {} \
                 tokens uncached.",
                choice.effort,
                tokens_text(tokens)
            ),
            (Some(effort), None) if chose => {
                format!("so {what} runs at {effort}.")
            }
            (Some(_), None) => format!("so {what} {stays}."),
            (None, None) => format!("so {what} runs at the model's default."),
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
}

/// A count of tokens as a note says it: `48k`, or `900` under a
/// thousand.
pub fn tokens_text(tokens: u64) -> String {
    if tokens < 1_000 {
        tokens.to_string()
    } else {
        format!("{}k", (tokens + 500) / 1_000)
    }
}

impl State {
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
        let comparison = if self.chose() { "above" } else { "below" };
        format!(
            "Confidence {:.2} is {comparison} {:.2}, {outcome}",
            self.confidence, self.threshold
        )
    }
}

impl UiPlugin for ReasoningPlugin {
    type State = State;
    type Data = ();
    type RepoData = ();
    type Settings = Settings;
    type Ui = ();

    fn name(&self) -> &'static str {
        NAME
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
                let choice = view.state?.starting_choice()?;
                let tree = step(choice, view.compact);
                Some(view.tree("reasoning-plan-step", &tree, |_, _, _| {}))
            })
            .settings(settings_pane)
            .contribute(points::PICKER_AUTO, |_: &AtApp, view| {
                Some(if view.jev {
                    "Auto lets tau-reasoning pick the effort for each message, with Jev.".into()
                } else {
                    "Auto leaves the effort to the model: tau-reasoning needs a TypeSafe key (Models).".into()
                })
            })
    }
}

/// tau-reasoning on the host.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReasoningHost;

impl HostHalf for ReasoningHost {
    type Plugin = ReasoningPlugin;
    type Host = ();

    /// On auto, with Jev: an effort picked by hand stands.
    async fn agent_plugins(
        &self,
        _host: &(),
        run: &RunCtx,
        settings: &Settings,
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        let Some(jev) = run.services.get::<Arc<dyn Jev>>() else {
            return Ok(Vec::new());
        };
        if run.effort.is_some() {
            return Ok(Vec::new());
        }
        Ok(vec![Box::new(
            Reasoning::new(jev.clone())
                .redecide(settings.redecide)
                .threshold(settings.threshold),
        )])
    }

    async fn starting(
        &self,
        _host: &(),
        run: &RunCtx,
        _settings: &Settings,
    ) -> Vec<Record> {
        let jev = run.services.get::<Arc<dyn Jev>>().is_some();
        let status = match (&run.effort, jev) {
            (Some(effort), _) => format!("off · effort set to {effort}"),
            (None, false) => NO_KEY.to_owned(),
            (None, true) => "picks the effort as the run starts".to_owned(),
        };
        vec![Record::Starting {
            status: Some(status),
        }]
    }

    async fn catalog(
        &self,
        _host: &(),
        cx: &HostCx,
        _settings: &Settings,
    ) -> PluginInfo {
        let jev = cx.services.get::<Arc<dyn Jev>>().is_some();
        PluginInfo {
            group: tau_ui_plugin::Group::Context,
            description: needs_jev(
                jev,
                "Picks each message's reasoning effort on auto",
            ),
            seams: vec![Seam::Start],
            page: Some(Link::page("choices").param("run", "")),
            ..Default::default()
        }
    }
}

/// The plugin's mark: its plug, read as `tone`.
fn plug(tone: tree::Tone) -> Node {
    Node::Icon {
        name: "plug".into(),
        tone,
    }
}

/// The plugin's note at one of its anchors, as a view tree (ADR 0045).
/// Whether a choice's chart is open is the interface's, under the
/// note's section.
fn transcript_note(
    at: &AtAnchor,
    view: &mut ViewCx<'_, ReasoningPlugin>,
) -> Option<AnyElement> {
    let state = view.state?;
    let note = state.notes.get(&at.key)?;
    let id = format!("reasoning-{}-{}", at.run.id.0, at.key);
    let details = Link::page("choices").param("run", at.run.id.0.to_string());
    let tree = match note {
        Note::Failed { message, detail } => Node::Card {
            children: vec![Node::row([
                plug(tree::Tone::Bad),
                Node::Mono { text: NAME.into() },
                Node::text(message.clone()),
                Node::Spacer,
                Node::Mono {
                    text: detail.clone(),
                },
            ])],
        },
        Note::Choice {
            choice,
            text,
            outcome,
        } => {
            let choice = state.choices.get(*choice)?;
            let head = Node::row([
                plug(tree::Tone::Info),
                Node::Mono { text: NAME.into() },
                Node::Rich { text: text.clone() },
                Node::Spacer,
                Node::Mono {
                    text: format!("Jev · {}", usd(choice.cost)),
                },
                Node::Link {
                    label: "Details".into(),
                    to: details.clone(),
                },
            ]);
            // The phone shows the answer and links to the chart.
            let body = if view.compact {
                let level = choice
                    .levels
                    .get(choice.chosen())
                    .map_or(String::new(), |level| level.effort.clone());
                Node::Link {
                    label: format!("Why {level}"),
                    to: details,
                }
            } else {
                Node::Section {
                    id: format!("note-{}-{}", at.run.id.0, at.key),
                    title: "Probability of each effort level".into(),
                    open: false,
                    children: vec![
                        levels(choice, false),
                        Node::text(choice.verdict(outcome)),
                    ],
                }
            };
            Node::Card {
                children: vec![head, body],
            }
        }
    };
    Some(view.tree(&id, &tree, |_, _, _| {}))
}

/// The page with every choice in a run, newest last.
fn choices_page(view: &mut ViewCx<'_, ReasoningPlugin>) -> AnyElement {
    let compact = view.compact;
    let choices = view
        .state
        .map(|state| state.choices.as_slice())
        .unwrap_or_default();
    let mut page = vec![
        Node::Heading {
            text: "Reasoning effort".into(),
        },
        Node::text(
            "Each effort tau-reasoning picked for this run, with how sure \
             Jev was.",
        ),
    ];
    if choices.is_empty() {
        page.push(Node::Empty {
            text: "tau-reasoning has not chosen an effort in this run.".into(),
        });
    }
    page.extend(choices.iter().map(|choice| step(choice, compact)));
    view.tree("reasoning-choices", &Node::stack(page), |_, _, _| {})
}

/// One choice at full size: what it was for, then its chart.
fn step(choice: &Choice, compact: bool) -> Node {
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
    Node::stack([
        Node::row([
            Node::Mono { text: NAME.into() },
            Node::text(format!("{what}: runs at {ran}")),
            Node::Spacer,
            Node::Mono {
                text: format!("Jev · {}", usd(choice.cost)),
            },
        ]),
        chart(choice, compact),
    ])
}

/// The effort chart at full size: the confidence against the
/// threshold, then each level's probability with what the level suits.
fn chart(choice: &Choice, compact: bool) -> Node {
    let verdict = match choice.kept_for_cache {
        Some(tokens) => format!(
            "is held back: it would resend {} tokens uncached",
            tokens_text(tokens)
        ),
        None if choice.confidence >= choice.threshold => "is applied".into(),
        None => "is not applied".into(),
    };
    let confidence = Node::stack([
        Node::Rows {
            rows: vec![
                KeyValue {
                    key: "Confidence".into(),
                    value: format!("{:.2}", choice.confidence),
                },
                KeyValue {
                    key: "Threshold".into(),
                    value: format!("{:.2}", choice.threshold),
                },
            ],
        },
        Node::text(format!("So the choice {verdict}.")),
    ]);
    let levels = Node::stack([
        Node::text("Probability of each effort level"),
        levels(choice, !compact),
    ]);
    Node::Card {
        children: vec![confidence, Node::Rule, levels],
    }
}

/// A row per level: its name, its probability as a bar and a number,
/// what it suits when `suits`, and a badge on the one Jev found most
/// likely.
fn levels(choice: &Choice, suits: bool) -> Node {
    let chosen = choice.chosen();
    let mut head = vec!["Effort".to_owned(), "Probability".to_owned()];
    if suits {
        head.push("Suits".into());
    }
    head.push(String::new());
    let rows = choice
        .levels
        .iter()
        .enumerate()
        .map(|(index, level)| {
            let mut row = vec![
                Node::Mono {
                    text: level.effort.clone(),
                },
                Node::Progress {
                    share: level.p,
                    label: Some(format!("{:.2}", level.p)),
                },
            ];
            if suits {
                row.push(Node::text(level.suits.clone()));
            }
            row.push(if index == chosen {
                Node::Badge {
                    text: "most likely".into(),
                    tone: tree::Tone::Info,
                }
            } else {
                Node::text("")
            });
            row
        })
        .collect();
    Node::Table {
        head,
        rows,
        widths: Vec::new(),
    }
}

/// The settings pane's actions: switch deciding again, and set the
/// threshold its arguments name.
const REDECIDE: &str = "redecide";
const THRESHOLD: &str = "threshold";

/// Its settings pane (ADR 0029): whether to decide again between steps,
/// and how sure Jev must be, saved in the scope the pane shows.
fn settings_pane(view: &mut ViewCx<'_, ReasoningPlugin>) -> AnyElement {
    let settings = *view.settings;
    let scope = view.scope().map(str::to_owned);
    let mut card = Vec::new();
    if !view.jev {
        card.push(Node::Status {
            text: "tau-reasoning needs a TypeSafe key (Accounts): until \
                   then, auto is the model's own default."
                .into(),
            detail: None,
            tone: tree::Tone::Warn,
        });
    }
    card.push(Node::Switch {
        label: "Decide again between steps".into(),
        detail: Some(
            "Jev looks again when a step's effort runs out, not only as \
             each message comes in. Each change resends the whole context \
             uncached, so it pays only on long tool chains."
                .into(),
        ),
        on: settings.redecide,
        action: REDECIDE.into(),
        args: Value::Null,
    });
    card.push(Node::stack([
        Node::text("Confidence to change the effort"),
        Node::text(
            "Below it, a message runs at the effort the last one ran at.",
        ),
        Node::row(THRESHOLDS.into_iter().map(|threshold| {
            let on = (settings.threshold - threshold).abs() < 1e-9;
            Node::Button {
                label: format!("{threshold:.1}"),
                action: THRESHOLD.into(),
                args: json!({ "threshold": threshold }),
                kind: if on {
                    tree::Kind::Primary
                } else {
                    tree::Kind::Secondary
                },
                icon: None,
            }
        })),
    ]));
    let handle = view.handle.clone();
    view.tree(
        "reasoning-settings",
        &Node::Card { children: card },
        move |action, args, cx| {
            let settings = match action {
                REDECIDE => Settings {
                    redecide: !settings.redecide,
                    ..settings
                },
                THRESHOLD => {
                    let Some(threshold) =
                        args.get("threshold").and_then(Value::as_f64)
                    else {
                        return;
                    };
                    Settings {
                        threshold,
                        ..settings
                    }
                }
                _ => return,
            };
            handle.save_settings_in(scope.as_deref(), &settings, cx);
        },
    )
}
