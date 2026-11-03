//! tau-goal's UI (ADR 0017): the banner above a run with what can be
//! done about its goal, the phone's bar, the goal's section of the
//! inspector, its line in the sidebar and the plugin list, its notes in
//! the transcript, how a `/goal` message and a continuation read, and the
//! `/goal` command with its popover. Each drawn one is a view tree (ADR
//! 0045): the popover's limits are inputs whose drafts are the
//! interface's state.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use gpui::{AnyElement, App};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_agent::plugin::Plugin;
use tau_jev::Jev;
use tau_ui_kit::{assets::Icon, format::usd, theme::Tone};
use tau_ui_plugin::{
    Fold,
    Handle,
    HostCx,
    HostHalf,
    Manifest,
    PluginInfo,
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
    tree::{self, KeyValue, Kind, Node, draft_key},
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
    pub tone: tree::Tone,
}

/// The `/goal` popover's inputs, by id: their drafts are the limits.
pub const CONTINUATIONS: &str = "goal-continuations";
pub const BUDGET: &str = "goal-budget";

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
                        tone: tree::Tone::Good,
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
                        tone: tree::Tone::Warn,
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
                    tone: tree::Tone::Bad,
                })
            }
            Record::Error { message } => Some(Note {
                text: message.clone(),
                detail: "not checked".into(),
                tone: tree::Tone::Bad,
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

/// The goal's tone, as a tree's pieces read it.
fn tree_tone(goal: &Goal) -> tree::Tone {
    match goal.status {
        Status::Active | Status::Paused => tree::Tone::Warn,
        Status::Met => tree::Tone::Good,
        Status::Stopped(_) => tree::Tone::Bad,
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
    type Ui = ();

    fn name(&self) -> &'static str {
        NAME
    }

    fn read_prompt(&self, prompt: &str) -> Option<String> {
        set_message(prompt)
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .contribute(points::RUN_BANNER, banner)
            .contribute_at(points::INSPECTOR, -100, |at: &AtRun, view| {
                let goal = view.state?.goal.clone()?;
                let tree = section(&goal, at.run.live);
                let id = format!("goal-inspector-{}", at.run.id.0);
                Some(view.tree(&id, &tree, |_, _, _| {}))
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
                let note = view.state?.notes.get(&at.key)?;
                let tree = note_tree(note.tone, &note.text, &note.detail);
                let id = format!("goal-{}-{}", at.run.id.0, at.key);
                Some(view.tree(&id, &tree, |_, _, _| {}))
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

/// tau-goal on the host.
#[derive(Debug, Clone, Copy, Default)]
pub struct GoalHost;

impl HostHalf for GoalHost {
    type Plugin = GoalUi;
    type Host = ();

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
            group: tau_ui_plugin::Group::Rules,
            description: needs_jev(
                jev,
                "Keeps a conversation going until its /goal holds",
            ),
            seams: vec![Seam::Start, Seam::AfterTool, Seam::BeforeStop],
            page: None,
            ..Default::default()
        }
    }
}

/// tau-goal's mark: its target, read as `tone`.
fn target(tone: tree::Tone) -> Node {
    Node::Icon {
        name: "target".into(),
        tone,
    }
}

/// One of tau-goal's notes: what happened, and the small print.
fn note_tree(tone: tree::Tone, text: &str, detail: &str) -> Node {
    Node::Card {
        children: vec![Node::row([
            target(tone),
            Node::Mono { text: NAME.into() },
            Node::text(text),
            Node::Spacer,
            Node::Mono {
                text: detail.into(),
            },
        ])],
    }
}

/// A person's message, when tau-goal reads it as its own: a `/goal`
/// shows as the goal it set, and a continuation as tau-goal's note,
/// unless the check that sent it says it already.
fn user_message(
    at: &AtMessage,
    view: &mut ViewCx<'_, GoalUi>,
) -> Option<AnyElement> {
    let id = format!("goal-said-{}-{}", at.run.id.0, at.index);
    let tree = message_tree(&at.text, view.state)?;
    Some(view.tree(&id, &tree, |_, _, _| {}))
}

/// The tree a person's message `text` draws as, when tau-goal reads it
/// as its own; nothing at all for a continuation a check's note says.
pub fn message_tree(text: &str, state: Option<&State>) -> Option<Node> {
    if let Some(condition) = set_message(text) {
        let goal = state.and_then(|state| state.goal.as_ref());
        return Some(goal_set(goal, &condition));
    }
    let said = text.strip_prefix(CONTINUATION_PREFIX)?;
    let held = continuation_of(text)
        .is_some_and(|n| state.is_some_and(|state| state.held.contains(&n)));
    if held {
        return Some(Node::stack([]));
    }
    let first = said.lines().next().unwrap_or_default();
    Some(note_tree(
        tree::Tone::Warn,
        first.trim_end_matches('.'),
        "before_stop",
    ))
}

/// The person setting a goal: the condition, and the goal's limits while
/// it is the conversation's goal.
fn goal_set(goal: Option<&Goal>, condition: &str) -> Node {
    let mut children = vec![Node::row([
        target(tree::Tone::Info),
        Node::Mono {
            text: "/goal".into(),
        },
        Node::Rich {
            text: condition.into(),
        },
    ])];
    if let Some(goal) = goal.filter(|goal| goal.condition == condition) {
        children.push(Node::row(
            [
                format!("up to {} continuations", goal.max_continuations),
                format!("{} budget", usd(goal.budget)),
                "checked by Jev".to_owned(),
            ]
            .map(|text| Node::Badge {
                text,
                tone: tree::Tone::Neutral,
            }),
        ));
    }
    Node::Card { children }
}

/// What the goal's buttons do; a button's arguments hold one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

/// Every banner button's action: carry out the [`Act`] its arguments
/// hold.
const ACT: &str = "act";

/// The phone's bar's action: open the run's details.
const DETAILS: &str = "details";

/// Above the transcript while the conversation has a goal; on a phone, a
/// bar that opens the run's details.
fn banner(at: &AtRun, view: &mut ViewCx<'_, GoalUi>) -> Option<AnyElement> {
    let state = view.state?;
    let goal = state.goal.clone()?;
    let why = unchecked(&goal, view.jev, at.run.live, state.checks);
    let tree = banner_tree(&goal, why, view.compact);
    let (run, jev, handle) = (at.run.clone(), view.jev, view.handle.clone());
    let id = format!("goal-banner-{}", at.run.id.0);
    Some(view.tree(&id, &tree, move |action, args, cx| match action {
        DETAILS => handle.run_details(cx),
        ACT => {
            if let Ok(what) = serde_json::from_value::<Act>(args.clone()) {
                act(what, &run, &goal, jev, &handle, cx);
            }
        }
        _ => {}
    }))
}

/// The banner's tree: the goal, where it stands, why nothing checks it
/// when nothing does, and its buttons; on a phone, a bar with a button
/// that opens the run's details.
pub fn banner_tree(goal: &Goal, why: Option<&str>, compact: bool) -> Node {
    let tone = tree_tone(goal);
    if compact {
        return Node::Card {
            children: vec![Node::row([
                target(tone),
                Node::stack([
                    Node::text(goal.condition.clone()),
                    Node::Mono {
                        text: status_line(goal),
                    },
                ]),
                Node::Spacer,
                Node::Button {
                    label: String::new(),
                    action: DETAILS.into(),
                    args: Value::Null,
                    kind: Kind::Secondary,
                    icon: Some("chevron".into()),
                },
            ])],
        };
    }
    let button = |label: &str, kind: Kind, what: Act| Node::Button {
        label: label.into(),
        action: ACT.into(),
        args: serde_json::to_value(what).unwrap_or(Value::Null),
        kind,
        icon: None,
    };
    let actions = match goal.status {
        Status::Active => vec![
            button("Pause", Kind::Secondary, Act::Pause),
            button("Edit", Kind::Secondary, Act::Edit),
            button("Clear", Kind::Secondary, Act::Clear),
        ],
        Status::Paused => vec![
            button("Resume", Kind::Primary, Act::KeepGoing),
            button("Edit", Kind::Secondary, Act::Edit),
            button("Clear", Kind::Secondary, Act::Clear),
        ],
        Status::Met => vec![
            button("New goal", Kind::Secondary, Act::Edit),
            button("Clear", Kind::Secondary, Act::Clear),
        ],
        Status::Stopped(_) => vec![
            button("Keep going · +10", Kind::Primary, Act::KeepGoing),
            button("Edit", Kind::Secondary, Act::Edit),
            button("Clear", Kind::Secondary, Act::Clear),
        ],
    };
    let mut about = vec![
        Node::row([
            Node::Badge {
                text: "goal".into(),
                tone,
            },
            Node::text(goal.condition.clone()),
        ]),
        Node::Mono {
            text: status_line(goal),
        },
    ];
    if let Some(why) = why {
        about.push(Node::Status {
            text: why.into(),
            detail: None,
            tone: tree::Tone::Warn,
        });
    }
    let mut row = vec![target(tone), Node::Stack { children: about }];
    row.push(Node::Spacer);
    row.extend(actions);
    Node::Card {
        children: vec![Node::Row { children: row }],
    }
}

/// The goal's section of the inspector: the condition, where it stands,
/// its limits, and every check.
pub fn section(goal: &Goal, live: bool) -> Node {
    let status = match goal.status {
        Status::Active => "working toward it",
        Status::Paused => "paused",
        Status::Met => "met",
        Status::Stopped(_) => "stopped · not met",
    };
    let share = |used: f64, of: f64| {
        if of > 0.0 {
            (used / of).clamp(0.0, 1.0)
        } else {
            1.0
        }
    };
    let working = live && goal.status == Status::Active;
    let mut checks = Vec::new();
    if working {
        checks.push(Node::row([
            Node::Icon {
                name: "spinner".into(),
                tone: tree::Tone::Info,
            },
            Node::text("working · checked when it stops"),
        ]));
    }
    checks.extend(goal.checks.iter().rev().map(|check| {
        let (verdict, tone) = if check.met {
            ("met", tree::Tone::Good)
        } else {
            ("not met", tree::Tone::Warn)
        };
        let then = match check.continuation {
            Some(n) => format!("sent back, {n} of {}", goal.max_continuations),
            None if check.met => "the run stopped".to_owned(),
            None => "stopped here".to_owned(),
        };
        Node::row([
            Node::Mono {
                text: format!("#{}", check.n),
            },
            Node::Badge {
                text: verdict.into(),
                tone,
            },
            Node::text(then),
            Node::Spacer,
            Node::Mono {
                text: format!("turn {} · p {:.2}", check.turn, check.p),
            },
        ])
    }));
    if checks.is_empty() {
        checks.push(Node::Empty {
            text: "Not checked yet.".into(),
        });
    }
    let cost: f64 = goal.checks.iter().map(|check| check.cost).sum();
    let mut children = vec![
        Node::row([
            Node::Heading {
                text: "Goal".into(),
            },
            Node::Spacer,
            Node::Pill {
                text: status.into(),
                tone: tree_tone(goal),
            },
        ]),
        Node::Card {
            children: vec![Node::Rich {
                text: goal.condition.clone(),
            }],
        },
        Node::text(format!(
            "Checked when the agent stops · met at p ≥ {MET_AT:.1} · up to {} \
             continuations and {}",
            goal.max_continuations,
            usd(goal.budget)
        )),
        Node::Heading {
            text: "Limits".into(),
        },
        Node::Progress {
            share: share(
                f64::from(goal.continuations),
                f64::from(goal.max_continuations),
            ),
            label: Some(format!(
                "Continuations · {} / {}",
                goal.continuations, goal.max_continuations
            )),
        },
        Node::Progress {
            share: share(goal.spent, goal.budget),
            label: Some(format!(
                "Cost · {} / {}",
                usd(goal.spent),
                usd(goal.budget)
            )),
        },
        Node::Heading {
            text: "Checks".into(),
        },
        Node::Card { children: checks },
    ];
    if let Some(error) = &goal.error {
        children.push(Node::Status {
            text: error.clone(),
            detail: None,
            tone: tree::Tone::Bad,
        });
    }
    children.push(Node::Rows {
        rows: vec![
            KeyValue {
                key: "checked by".into(),
                value: "tau-goal · Jev".into(),
            },
            KeyValue {
                key: "Jev cost".into(),
                value: format!("${cost:.5}"),
            },
        ],
    });
    children.push(Node::text(
        "Jev reads the goal, the agent's last answer and its last tool results. When \
         they fall short, the agent is sent back.",
    ));
    Node::Stack { children }
}

/// The limits the `/goal` popover's drafts say, or the defaults where
/// one does not read as a number.
pub fn limits(continuations: &str, budget: &str) -> (u32, f64) {
    (
        continuations
            .trim()
            .parse()
            .unwrap_or(DEFAULT_CONTINUATIONS),
        budget
            .trim()
            .trim_start_matches('$')
            .parse()
            .unwrap_or(DEFAULT_BUDGET),
    )
}

/// The draft of the popover's input `id`, as the interface keeps it.
fn draft<'a>(view: &ViewCx<'a, GoalUi>, id: &str) -> &'a str {
    view.ui_value(&draft_key(id))
        .and_then(Value::as_str)
        .unwrap_or_default()
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
        limits(draft(view, CONTINUATIONS), draft(view, BUDGET))
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

/// Enter in one of the popover's limits, by its action: send the goal,
/// as Enter in the composer would.
const SUBMIT: &str = "submit-";

/// The `/goal` popover: what a goal does, and its limits.
fn popover(view: &mut ViewCx<'_, GoalUi>) -> AnyElement {
    let tree = popover_tree(view.jev, view.compact);
    let handle = view.handle.clone();
    view.tree("goal-popover", &tree, move |action, args, cx| {
        let Some(id) = action.strip_prefix(SUBMIT) else {
            return;
        };
        // Enter clears an input's draft; a limit keeps what it says.
        let text = args.get("text").and_then(Value::as_str).unwrap_or("");
        let value = (!text.is_empty()).then(|| Value::from(text));
        handle.set_ui(draft_key(id), value, cx);
        handle.request(Request::Submit, cx);
    })
}

/// The popover's tree: the command, what it does or what it needs, and
/// an input for each limit, drafted by the interface.
pub fn popover_tree(jev: bool, compact: bool) -> Node {
    let limit = |id: &str, placeholder: String| Node::Input {
        id: id.into(),
        placeholder: Some(placeholder),
        multiline: false,
        submit: Some(format!("{SUBMIT}{id}")),
    };
    let about = if jev {
        Node::text(
            "The agent keeps going until this holds. Each time it would stop, \
             tau-goal asks Jev whether the goal is met; if not, the agent goes on.",
        )
    } else {
        Node::Status {
            text:
                "Goals are checked with Jev: add a TypeSafe key on the Models \
                   screen first."
                    .into(),
            detail: None,
            tone: tree::Tone::Warn,
        }
    };
    let mut children = vec![
        Node::row([
            target(tree::Tone::Info),
            Node::Mono {
                text: "/goal".into(),
            },
            Node::Mono {
                text: "<condition>".into(),
            },
        ]),
        about,
        Node::Rule,
        Node::row([
            Node::text("Stop after"),
            limit(CONTINUATIONS, DEFAULT_CONTINUATIONS.to_string()),
            Node::text("continuations"),
            Node::text("Budget $"),
            limit(BUDGET, format!("{DEFAULT_BUDGET:.2}")),
        ]),
    ];
    if !compact {
        children.push(Node::Rich {
            text: "`Enter` set the goal and start · `Esc` close".into(),
        });
    }
    Node::Stack { children }
}
