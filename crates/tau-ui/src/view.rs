//! The run as the interface shows it, kept apart from GPUI so it can be
//! built and tested without a window.
//!
//! A [`RunView`] is fed the same [`RunEvent`]s a [`tau_agent::agent::Run`]
//! streams, through [`RunView::apply`]. Plugin decisions that no event
//! carries yet (a chosen reasoning effort, the notes memory added, which
//! rule blocked a call) come in as a [`RunUpdate`], until plugins report
//! them as events of their own.

use std::time::Duration;

use serde_json::Value;
use tau_agent::{
    event::{LimitKind, RunEvent, StopReason},
    tool::{RunId, ToolOutput},
};
use tau_ai::message::{InputBlock, Usage};

/// One run, as the transcript, the inspector and the run list show it.
#[derive(Debug, Clone, PartialEq)]
pub struct RunView {
    pub id: RunId,
    /// What the run list calls it.
    pub title: String,
    pub agent: String,
    pub model: String,
    pub status: RunStatus,
    pub turn: u32,
    pub items: Vec<Item>,
    /// The `RunPlan` fields worth showing, after every plugin's `start`.
    pub plan: Vec<PlanField>,
    pub limits: Limits,
    pub usage: Totals,
    pub context: ContextWindow,
    pub plugins: Vec<PluginStatus>,
    /// Sub-agents and forks started from this run.
    pub children: Vec<ChildRun>,
    /// Where the run came from.
    pub origin: Origin,
    /// When it started, as the run list shows it.
    pub started: String,
    /// The latest context pruning's decision for each tool call.
    pub ledger: Vec<LedgerEntry>,
}

/// Where a run came from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Origin {
    #[default]
    Root,
    /// Forked from `from` at the checkpoint after `turn`.
    Fork {
        from: RunId,
        turn: u32,
    },
    SubAgent {
        parent: RunId,
    },
}

impl Origin {
    pub fn parent(&self) -> Option<&RunId> {
        match self {
            Self::Root => None,
            Self::Fork { from, .. } => Some(from),
            Self::SubAgent { parent } => Some(parent),
        }
    }
}

/// One tool call as context pruning judged it.
#[derive(Debug, Clone, PartialEq)]
pub struct LedgerEntry {
    pub call_id: String,
    pub turn: u32,
    pub tool: String,
    pub input: String,
    /// Tokens the call and its result take.
    pub tokens: u64,
    /// The probability that knowing the call still matters.
    pub matters: Option<f32>,
    /// The probability that its full output must stay verbatim.
    pub verbatim: Option<f32>,
    pub decision: Decision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Pinned,
    Keep,
    DropResult,
    DropCall,
}

impl Decision {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pinned => "pinned",
            Self::Keep => "keep",
            Self::DropResult => "drop result",
            Self::DropCall => "drop call",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RunStatus {
    /// Plugins are preparing the run; the session is not open yet.
    Planning,
    Running,
    Finished(StopReason),
}

impl RunStatus {
    pub fn is_live(&self) -> bool {
        !matches!(self, Self::Finished(_))
    }
}

/// One entry of the transcript, in the order it happened.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    User(String),
    /// Assistant text; deltas append to the last one.
    Text(String),
    Thinking(String),
    Tool(ToolCard),
    Plugin(PluginNote),
    /// A plugin rewrote the context.
    Rewrite {
        plugin: String,
        tokens_before: u64,
        tokens_after: u64,
        detail: Option<String>,
    },
    Retry {
        attempt: u32,
        delay: Duration,
        error: String,
    },
    /// The run ended.
    Stop {
        stop: StopReason,
        turns: u32,
        tokens: u64,
        cost: f64,
        plugin_cost: f64,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCard {
    pub call_id: String,
    pub tool: String,
    /// The argument worth reading at a glance: a path, a command, a
    /// pattern.
    pub summary: String,
    /// The arguments the model sent.
    pub args: Value,
    pub state: ToolState,
    pub body: ToolBody,
    /// The plugin that added the tool, when a plugin did.
    pub from_plugin: Option<String>,
    /// Rule scores that passed, shown quietly next to the result.
    pub checks: Vec<String>,
    /// What context pruning did to this call, if anything.
    pub pruned: Option<Pruned>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ToolState {
    Running,
    Done {
        summary: Option<String>,
    },
    Failed(String),
    /// A plugin refused the call. The reason went back to the model.
    Blocked {
        plugin: String,
        rule: String,
        reason: String,
        score: String,
    },
    /// The call ran, but a plugin wants a person to look at it.
    Flagged {
        plugin: String,
        rule: String,
        score: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pruned {
    Kept,
    ResultDropped,
    CallDropped,
}

impl Pruned {
    pub fn label(self) -> &'static str {
        match self {
            Self::Kept => "kept",
            Self::ResultDropped => "result dropped",
            Self::CallDropped => "call dropped",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum ToolBody {
    #[default]
    None,
    Diff(Vec<DiffLine>),
    Output(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    Context,
    Added,
    Removed,
}

/// A plugin speaking in the transcript. Lighter than a tool card, and
/// always named.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginNote {
    pub plugin: String,
    pub text: String,
    /// Cost, latency or confidence, in small print.
    pub detail: Option<String>,
    pub tone: Tone,
    pub body: NoteBody,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum NoteBody {
    #[default]
    None,
    Chips(Vec<String>),
    /// A choice over ordered levels, such as reasoning effort.
    Distribution {
        levels: Vec<(String, f32)>,
        chosen: usize,
        note: String,
    },
    /// Notes a plugin suggests keeping.
    Proposals(Vec<Proposal>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    pub title: String,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tone {
    #[default]
    Info,
    Warn,
    Danger,
    Good,
    Quiet,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanField {
    pub name: String,
    pub value: String,
    /// The plugin that set it.
    pub set_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Limits {
    pub max_turns: Option<u32>,
    pub max_tokens: Option<u64>,
    pub max_usd: Option<f64>,
    pub timeout: Option<Duration>,
    pub elapsed: Duration,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Totals {
    pub tokens: u64,
    pub cost: f64,
    /// Charged by plugins; part of `cost`.
    pub plugin_cost: f64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ContextWindow {
    pub used: u64,
    pub window: Option<u64>,
    /// Share of the window where the first context plugin steps in.
    pub trigger: Option<f32>,
    /// The size before the last rewrite, while it is worth showing.
    pub before: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginStatus {
    pub name: String,
    pub state: String,
    pub tone: Tone,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChildRun {
    pub id: RunId,
    pub title: String,
    pub kind: ChildKind,
    pub status: RunStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildKind {
    SubAgent,
    Fork,
}

/// Anything that changes a [`RunView`]: a run event, or what the host
/// knows that no event carries yet.
#[derive(Debug, Clone, PartialEq)]
pub enum RunUpdate {
    Event(RunEvent),
    /// The user's message. Run events never carry it.
    User(String),
    Note(PluginNote),
    Plan(Vec<PlanField>),
    Plugins(Vec<PluginStatus>),
    Limits(Limits),
    Context(ContextWindow),
    Tool {
        call_id: String,
        state: ToolState,
    },
    Checks {
        call_id: String,
        checks: Vec<String>,
    },
    Pruned {
        call_id: String,
        pruned: Pruned,
    },
    /// The plugin that added a tool, for its card's tag.
    ToolPlugin {
        call_id: String,
        plugin: String,
    },
    /// Extra detail on the last context rewrite.
    RewriteDetail(String),
    PluginCost(f64),
    /// The pruning plugin's decisions, replacing the last ledger.
    Ledger(Vec<LedgerEntry>),
}

impl From<RunEvent> for RunUpdate {
    fn from(event: RunEvent) -> Self {
        Self::Event(event)
    }
}

impl RunView {
    pub fn new(
        id: RunId,
        title: impl Into<String>,
        agent: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            id,
            title: title.into(),
            agent: agent.into(),
            model: model.into(),
            status: RunStatus::Planning,
            turn: 0,
            items: Vec::new(),
            plan: Vec::new(),
            limits: Limits::default(),
            usage: Totals::default(),
            context: ContextWindow::default(),
            plugins: Vec::new(),
            children: Vec::new(),
            origin: Origin::Root,
            started: String::new(),
            ledger: Vec::new(),
        }
    }

    pub fn with_origin(mut self, origin: Origin) -> Self {
        self.origin = origin;
        self
    }

    pub fn started(mut self, started: impl Into<String>) -> Self {
        self.started = started.into();
        self
    }

    /// The last thing the model said, if anything.
    pub fn last_text(&self) -> Option<&str> {
        self.items.iter().rev().find_map(|item| match item {
            Item::Text(text) => Some(text.as_str()),
            _ => None,
        })
    }

    /// The last edit that changed a file, with its diff.
    pub fn last_diff(&self) -> Option<(&ToolCard, &[DiffLine])> {
        self.items.iter().rev().find_map(|item| match item {
            Item::Tool(card) => match &card.body {
                ToolBody::Diff(lines) => Some((card, lines.as_slice())),
                _ => None,
            },
            _ => None,
        })
    }

    /// Plugin notes from before the model's first turn: what each
    /// plugin's `start` decided.
    pub fn start_notes(&self) -> impl Iterator<Item = &PluginNote> {
        self.items
            .iter()
            .take_while(|item| matches!(item, Item::User(_) | Item::Plugin(_)))
            .filter_map(|item| match item {
                Item::Plugin(note) => Some(note),
                _ => None,
            })
    }

    /// The latest context rewrite, as `(plugin, before, after, detail)`.
    pub fn last_rewrite(&self) -> Option<(&str, u64, u64, Option<&str>)> {
        self.items.iter().rev().find_map(|item| match item {
            Item::Rewrite {
                plugin,
                tokens_before,
                tokens_after,
                detail,
            } => Some((
                plugin.as_str(),
                *tokens_before,
                *tokens_after,
                detail.as_deref(),
            )),
            _ => None,
        })
    }

    /// Tool calls a plugin blocked or flagged.
    pub fn reviews(&self) -> impl Iterator<Item = &ToolCard> {
        self.items.iter().filter_map(|item| match item {
            Item::Tool(card)
                if matches!(
                    card.state,
                    ToolState::Blocked { .. } | ToolState::Flagged { .. }
                ) =>
            {
                Some(card)
            }
            _ => None,
        })
    }

    /// Notes a plugin suggested keeping.
    pub fn proposals(&self) -> impl Iterator<Item = &Proposal> {
        self.items
            .iter()
            .filter_map(|item| match item {
                Item::Plugin(PluginNote {
                    body: NoteBody::Proposals(proposals),
                    ..
                }) => Some(proposals.iter()),
                _ => None,
            })
            .flatten()
    }

    /// Applies any update. Unknown call ids are ignored: the card may
    /// belong to a run the view has not seen start.
    pub fn update(&mut self, update: RunUpdate) {
        match update {
            RunUpdate::Event(event) => self.apply(&event),
            RunUpdate::User(text) => self.push_user(text),
            RunUpdate::Note(note) => self.push_note(note),
            RunUpdate::Plan(plan) => self.set_plan(plan),
            RunUpdate::Plugins(plugins) => self.set_plugins(plugins),
            RunUpdate::Limits(limits) => self.limits = limits,
            RunUpdate::Context(context) => self.context = context,
            RunUpdate::Tool { call_id, state } => {
                self.mark_tool(&call_id, state);
            }
            RunUpdate::Checks { call_id, checks } => {
                if let Some(card) = self.tool_mut(&call_id) {
                    card.checks = checks;
                }
            }
            RunUpdate::Pruned { call_id, pruned } => {
                self.mark_pruned(&call_id, pruned);
            }
            RunUpdate::ToolPlugin { call_id, plugin } => {
                if let Some(card) = self.tool_mut(&call_id) {
                    card.from_plugin = Some(plugin);
                }
            }
            RunUpdate::RewriteDetail(text) => {
                if let Some(Item::Rewrite { detail, .. }) = self
                    .items
                    .iter_mut()
                    .rev()
                    .find(|item| matches!(item, Item::Rewrite { .. }))
                {
                    *detail = Some(text);
                }
            }
            RunUpdate::PluginCost(cost) => self.usage.plugin_cost += cost,
            RunUpdate::Ledger(ledger) => {
                for entry in &ledger {
                    let pruned = match entry.decision {
                        Decision::Pinned | Decision::Keep => Pruned::Kept,
                        Decision::DropResult => Pruned::ResultDropped,
                        Decision::DropCall => Pruned::CallDropped,
                    };
                    self.mark_pruned(&entry.call_id, pruned);
                }
                self.ledger = ledger;
            }
        }
    }

    /// Adds the user's message. The run's own events never carry it.
    pub fn push_user(&mut self, text: impl Into<String>) {
        self.items.push(Item::User(text.into()));
    }

    pub fn push_note(&mut self, note: PluginNote) {
        self.items.push(Item::Plugin(note));
    }

    pub fn set_plan(&mut self, plan: Vec<PlanField>) {
        self.plan = plan;
    }

    pub fn set_plugins(&mut self, plugins: Vec<PluginStatus>) {
        self.plugins = plugins;
    }

    /// Changes a tool card's state, for decisions no event reports yet.
    /// Returns false if no card has that call id.
    pub fn mark_tool(&mut self, call_id: &str, state: ToolState) -> bool {
        self.tool_mut(call_id)
            .map(|card| card.state = state)
            .is_some()
    }

    pub fn mark_pruned(&mut self, call_id: &str, pruned: Pruned) -> bool {
        self.tool_mut(call_id)
            .map(|card| card.pruned = Some(pruned))
            .is_some()
    }

    pub fn tool(&self, call_id: &str) -> Option<&ToolCard> {
        self.items.iter().find_map(|item| match item {
            Item::Tool(card) if card.call_id == call_id => Some(card),
            _ => None,
        })
    }

    pub fn tool_mut(&mut self, call_id: &str) -> Option<&mut ToolCard> {
        self.items.iter_mut().find_map(|item| match item {
            Item::Tool(card) if card.call_id == call_id => Some(card),
            _ => None,
        })
    }

    /// Folds one run event into the view. Events of other runs are
    /// ignored, except a child's start and end, which update
    /// [`RunView::children`].
    pub fn apply(&mut self, event: &RunEvent) {
        if !self.owns(event) {
            self.apply_child(event);
            return;
        }
        match event {
            RunEvent::RunStart { agent, .. } => {
                self.agent = agent.to_string();
                self.status = RunStatus::Running;
            }
            RunEvent::TurnStart { turn, .. } => {
                self.status = RunStatus::Running;
                self.turn = *turn;
            }
            RunEvent::TextDelta { delta, .. } => match self.items.last_mut() {
                Some(Item::Text(text)) => text.push_str(delta),
                _ => self.items.push(Item::Text(delta.clone())),
            },
            RunEvent::ThinkingDelta { delta, .. } => {
                match self.items.last_mut() {
                    Some(Item::Thinking(text)) => text.push_str(delta),
                    _ => self.items.push(Item::Thinking(delta.clone())),
                }
            }
            RunEvent::ToolCallDelta { .. } => {}
            RunEvent::ToolStart {
                call_id,
                tool,
                args,
                ..
            } => self.items.push(Item::Tool(ToolCard {
                call_id: call_id.clone(),
                tool: tool.to_string(),
                summary: summarize_args(args),
                args: args.clone(),
                state: ToolState::Running,
                body: ToolBody::None,
                from_plugin: None,
                checks: Vec::new(),
                pruned: None,
            })),
            RunEvent::ToolUpdate {
                call_id, partial, ..
            } => {
                if let Some(card) = self.tool_mut(call_id) {
                    card.body = ToolBody::Output(tail(&text_of(partial), 6));
                }
            }
            RunEvent::ToolEnd {
                call_id,
                output,
                is_error,
                ..
            } => {
                if let Some(card) = self.tool_mut(call_id) {
                    finish_tool(card, output, *is_error);
                }
            }
            RunEvent::TurnEnd { turn, usage, .. } => {
                self.turn = *turn;
                self.add_usage(usage);
            }
            RunEvent::ContextRewritten {
                plugin,
                tokens_before,
                tokens_after,
                ..
            } => {
                self.context.before = Some(*tokens_before);
                self.context.used = *tokens_after;
                self.items.push(Item::Rewrite {
                    plugin: plugin.to_string(),
                    tokens_before: *tokens_before,
                    tokens_after: *tokens_after,
                    detail: None,
                });
            }
            RunEvent::Retry {
                attempt,
                delay,
                error,
                ..
            } => self.items.push(Item::Retry {
                attempt: *attempt,
                delay: *delay,
                error: error.clone(),
            }),
            RunEvent::Continued {
                plugin, message, ..
            } => self.push_note(PluginNote {
                plugin: plugin.to_string(),
                text: format!("held the stop: {message}"),
                detail: Some("before_stop".into()),
                tone: Tone::Warn,
                body: NoteBody::None,
            }),
            RunEvent::PluginError {
                plugin, message, ..
            } => self.push_note(PluginNote {
                plugin: plugin.to_string(),
                text: message.clone(),
                detail: Some("error".into()),
                tone: Tone::Danger,
                body: NoteBody::None,
            }),
            RunEvent::RunEnd { stop, cost, .. } => {
                self.usage.cost = *cost;
                self.status = RunStatus::Finished(stop.clone());
                self.items.push(Item::Stop {
                    stop: stop.clone(),
                    turns: self.turn,
                    tokens: self.usage.tokens,
                    cost: *cost,
                    plugin_cost: self.usage.plugin_cost,
                });
            }
        }
    }

    fn owns(&self, event: &RunEvent) -> bool {
        event_run(event) == &self.id
    }

    fn apply_child(&mut self, event: &RunEvent) {
        match event {
            RunEvent::RunStart {
                run,
                parent: Some(parent),
                agent,
            } if parent == &self.id => {
                self.children.push(ChildRun {
                    id: run.clone(),
                    title: agent.to_string(),
                    kind: ChildKind::SubAgent,
                    status: RunStatus::Running,
                });
            }
            RunEvent::RunEnd {
                run,
                parent: Some(parent),
                stop,
                ..
            } if parent == &self.id => {
                if let Some(child) =
                    self.children.iter_mut().find(|child| &child.id == run)
                {
                    child.status = RunStatus::Finished(stop.clone());
                }
            }
            _ => {}
        }
    }

    fn add_usage(&mut self, usage: &Usage) {
        self.usage.tokens += usage.total_tokens;
        self.usage.cost += usage.cost.total;
        // The last request's input is the best measure of the context.
        self.context.used = usage.input + usage.cache_read;
    }

    /// One meter per limit the run has.
    pub fn meters(&self) -> Vec<Meter> {
        let mut meters = Vec::new();
        if let Some(max) = self.limits.max_turns {
            meters.push(Meter::new(
                LimitKind::Turns,
                format!("{} / {max}", self.turn),
                self.turn as f32 / max as f32,
            ));
        }
        if let Some(max) = self.limits.max_tokens {
            meters.push(Meter::new(
                LimitKind::Tokens,
                format!("{} / {}", tokens(self.usage.tokens), tokens(max)),
                self.usage.tokens as f32 / max as f32,
            ));
        }
        if let Some(max) = self.limits.max_usd {
            meters.push(Meter::new(
                LimitKind::Usd,
                format!("{} / {}", usd(self.usage.cost), usd(max)),
                (self.usage.cost / max) as f32,
            ));
        }
        if let Some(max) = self.limits.timeout {
            meters.push(Meter::new(
                LimitKind::Time,
                format!("{} / {}", clock(self.limits.elapsed), clock(max)),
                self.limits.elapsed.as_secs_f32() / max.as_secs_f32(),
            ));
        }
        meters
    }
}

/// One limit, ready to draw.
#[derive(Debug, Clone, PartialEq)]
pub struct Meter {
    pub kind: LimitKind,
    pub label: &'static str,
    pub value: String,
    /// Used share, clamped to `0..=1`.
    pub share: f32,
}

impl Meter {
    fn new(kind: LimitKind, value: String, share: f32) -> Self {
        let label = match kind {
            LimitKind::Turns => "Turns",
            LimitKind::Tokens => "Tokens",
            LimitKind::Usd => "Cost",
            LimitKind::Time => "Time",
        };
        Self {
            kind,
            label,
            value,
            share: share.clamp(0.0, 1.0),
        }
    }
}

fn event_run(event: &RunEvent) -> &RunId {
    match event {
        RunEvent::RunStart { run, .. }
        | RunEvent::TurnStart { run, .. }
        | RunEvent::TextDelta { run, .. }
        | RunEvent::ThinkingDelta { run, .. }
        | RunEvent::ToolCallDelta { run, .. }
        | RunEvent::ToolStart { run, .. }
        | RunEvent::ToolUpdate { run, .. }
        | RunEvent::ToolEnd { run, .. }
        | RunEvent::TurnEnd { run, .. }
        | RunEvent::ContextRewritten { run, .. }
        | RunEvent::Retry { run, .. }
        | RunEvent::Continued { run, .. }
        | RunEvent::PluginError { run, .. }
        | RunEvent::RunEnd { run, .. } => run,
    }
}

/// Picks the argument a person would want to see first.
pub fn summarize_args(args: &Value) -> String {
    let text = |key: &str| args.get(key).and_then(Value::as_str);
    if let Some(command) = text("command") {
        return command.to_owned();
    }
    // grep and find read best as `"pattern" path`.
    match (text("pattern"), text("path")) {
        (Some(pattern), Some(path)) => format!("\"{pattern}\" {path}"),
        (Some(pattern), None) => format!("\"{pattern}\""),
        (None, Some(path)) => path.to_owned(),
        (None, None) => ["query", "id", "title"]
            .into_iter()
            .find_map(text)
            .unwrap_or_default()
            .to_owned(),
    }
}

/// The text an `edit` or `write` call would add, for showing a call that
/// never ran.
pub fn proposed_text(args: &Value) -> Vec<String> {
    let edits = args
        .get("edits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|edit| edit.get("newText")?.as_str());
    let content = args.get("content").and_then(Value::as_str);
    edits
        .chain(content)
        .flat_map(str::lines)
        .map(str::to_owned)
        .collect()
}

fn finish_tool(card: &mut ToolCard, output: &ToolOutput, is_error: bool) {
    let text = text_of(output);
    if is_error {
        card.state = ToolState::Failed(first_line(&text));
        return;
    }
    let diff = output
        .details
        .as_ref()
        .and_then(|details| details.get("diff"))
        .and_then(Value::as_str)
        .map(parse_diff);
    card.state = ToolState::Done {
        summary: match &diff {
            Some(lines) => Some(diff_stat(lines)),
            None => line_count(&text),
        },
    };
    card.body = match diff {
        Some(lines) => ToolBody::Diff(lines),
        None if card.tool == "bash" => ToolBody::Output(tail(&text, 4)),
        None => ToolBody::None,
    };
}

fn text_of(output: &ToolOutput) -> String {
    output
        .content
        .iter()
        .filter_map(|block| match block {
            InputBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().to_owned()
}

fn tail(text: &str, count: usize) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(count);
    lines[start..]
        .iter()
        .map(|line| (*line).to_owned())
        .collect()
}

fn line_count(text: &str) -> Option<String> {
    match text.lines().count() {
        0 => None,
        1 => Some("1 line".into()),
        count => Some(format!("{count} lines")),
    }
}

/// Reads a unified diff into lines, dropping the file and hunk headers.
pub fn parse_diff(diff: &str) -> Vec<DiffLine> {
    diff.lines()
        .filter(|line| {
            !(line.starts_with("---")
                || line.starts_with("+++")
                || line.starts_with("@@"))
        })
        .map(|line| {
            let (kind, rest) = match line.chars().next() {
                Some('+') => (DiffKind::Added, &line[1..]),
                Some('-') => (DiffKind::Removed, &line[1..]),
                Some(' ') => (DiffKind::Context, &line[1..]),
                _ => (DiffKind::Context, line),
            };
            DiffLine {
                kind,
                text: rest.to_owned(),
            }
        })
        .collect()
}

pub fn diff_stat(lines: &[DiffLine]) -> String {
    let added = lines.iter().filter(|l| l.kind == DiffKind::Added).count();
    let removed = lines.iter().filter(|l| l.kind == DiffKind::Removed).count();
    format!("+{added} −{removed}")
}

/// `184000` as `184k`.
pub fn tokens(count: u64) -> String {
    match count {
        0..1_000 => count.to_string(),
        1_000..1_000_000 => format!("{}k", count / 1_000),
        _ => format!("{:.1}M", count as f64 / 1_000_000.0),
    }
}

pub fn usd(amount: f64) -> String {
    if amount < 1.0 {
        format!("${amount:.3}")
    } else {
        format!("${amount:.2}")
    }
}

pub fn clock(duration: Duration) -> String {
    let secs = duration.as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;
    use tau_ai::message::UsageCost;

    use super::*;

    fn run() -> RunId {
        RunId(Arc::from("r1"))
    }

    fn view() -> RunView {
        RunView::new(run(), "retry-after", "coder", "gpt-5.5")
    }

    #[test]
    fn text_deltas_join_until_something_else_happens() {
        let mut view = view();
        for delta in ["Hel", "lo"] {
            view.apply(&RunEvent::TextDelta {
                run: run(),
                parent: None,
                delta: delta.into(),
            });
        }
        view.apply(&RunEvent::ToolStart {
            run: run(),
            call_id: "c1".into(),
            tool: Arc::from("read"),
            args: json!({ "path": "src/lib.rs" }),
        });
        view.apply(&RunEvent::TextDelta {
            run: run(),
            parent: None,
            delta: "Done".into(),
        });
        assert_eq!(view.items.len(), 3);
        assert_eq!(view.items[0], Item::Text("Hello".into()));
        assert_eq!(view.items[2], Item::Text("Done".into()));
    }

    #[test]
    fn an_edit_shows_its_diff() {
        let mut view = view();
        view.apply(&RunEvent::ToolStart {
            run: run(),
            call_id: "c1".into(),
            tool: Arc::from("edit"),
            args: json!({ "path": "retry.rs", "edits": [] }),
        });
        view.apply(&RunEvent::ToolEnd {
            run: run(),
            call_id: "c1".into(),
            output: Arc::new(ToolOutput {
                details: Some(json!({
                    "diff": "--- a\n+++ b\n@@ -1 +1 @@\n-old\n+new\n+more\n",
                })),
                ..ToolOutput::text("Successfully replaced 1 block(s).")
            }),
            is_error: false,
        });
        let card = view.tool("c1").expect("card");
        assert_eq!(card.summary, "retry.rs");
        assert_eq!(
            card.state,
            ToolState::Done {
                summary: Some("+2 −1".into())
            }
        );
        let ToolBody::Diff(lines) = &card.body else {
            panic!("expected a diff, got {:?}", card.body);
        };
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].kind, DiffKind::Removed);
    }

    #[test]
    fn events_of_other_runs_only_touch_children() {
        let mut view = view();
        let child = RunId(Arc::from("r2"));
        view.apply(&RunEvent::RunStart {
            run: child.clone(),
            parent: Some(run()),
            agent: Arc::from("reviewer"),
        });
        view.apply(&RunEvent::TextDelta {
            run: child.clone(),
            parent: Some(run()),
            delta: "looks fine".into(),
        });
        view.apply(&RunEvent::RunEnd {
            run: child,
            parent: Some(run()),
            stop: StopReason::Stop,
            cost: 0.01,
        });
        assert!(view.items.is_empty());
        assert_eq!(view.children.len(), 1);
        assert_eq!(
            view.children[0].status,
            RunStatus::Finished(StopReason::Stop)
        );
    }

    #[test]
    fn usage_and_the_end_are_recorded() {
        let mut view = view();
        view.apply(&RunEvent::TurnEnd {
            run: run(),
            turn: 1,
            usage: Usage {
                input: 1_200,
                cache_read: 800,
                total_tokens: 2_300,
                cost: UsageCost {
                    total: 0.01,
                    ..UsageCost::default()
                },
                ..Usage::default()
            },
        });
        view.apply(&RunEvent::RunEnd {
            run: run(),
            parent: None,
            stop: StopReason::Stop,
            cost: 0.012,
        });
        assert_eq!(view.context.used, 2_000);
        assert_eq!(view.status, RunStatus::Finished(StopReason::Stop));
        assert!(matches!(
            view.items.last(),
            Some(Item::Stop {
                tokens: 2_300,
                turns: 1,
                ..
            })
        ));
    }

    #[test]
    fn grep_arguments_read_as_pattern_then_path() {
        assert_eq!(
            summarize_args(&json!({ "pattern": "retry", "path": "src" })),
            "\"retry\" src"
        );
        assert_eq!(
            summarize_args(&json!({ "command": "cargo test" })),
            "cargo test"
        );
    }

    #[test]
    fn numbers_read_the_way_the_mockups_write_them() {
        assert_eq!(tokens(184_000), "184k");
        assert_eq!(usd(0.184), "$0.184");
        assert_eq!(usd(2.0), "$2.00");
        assert_eq!(clock(Duration::from_secs(192)), "3:12");
    }
}
