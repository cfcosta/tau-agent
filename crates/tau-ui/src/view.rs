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
use tau_ai::message::{
    AssistantBlock,
    InputBlock,
    Message,
    Usage,
    UserContent,
};

use crate::{
    change_diff::{self, ChangeDiff},
    change_log::{self, ChangeLog},
};

/// One run, as the transcript, the inspector and the run list show it.
#[derive(Debug, Clone, PartialEq)]
pub struct RunView {
    pub id: RunId,
    /// What the run list calls it.
    pub title: String,
    pub agent: String,
    pub model: String,
    /// The repository the run works on; empty when the host did not say.
    pub repo: String,
    pub status: RunStatus,
    pub turn: u32,
    pub items: Vec<Item>,
    /// The `RunPlan` fields worth showing, after every plugin's `start`.
    pub plan: Vec<PlanField>,
    /// The effort tau-reasoning last ran a message at; `None` for the
    /// model's default. A note shows only when it changes.
    pub ran_at: Option<String>,
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
    /// The last run events, newest last, for the Events tab.
    pub log: Vec<LogLine>,
    /// What the chat cost before its latest message: each start of a
    /// resumed run reports only its own cost.
    pub cost_before: f64,
    /// What tau-constitution checked and decided in the run.
    pub constitution: ConstitutionStats,
    /// The conversation's goal, from tau-goal's reports and records.
    pub goal: Option<tau_goal::Goal>,
    /// What the pruning plugin said about its pass, for the rewrite it
    /// explains, which comes right after.
    pending_rewrite: Option<String>,
}

/// tau-constitution's work in one run, from its reports.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConstitutionStats {
    /// Tool calls checked, and final answers checked.
    pub calls: u32,
    pub answers: u32,
    /// Questions asked of Jev: one per rule per check.
    pub questions: u32,
    /// What Jev cost, in US dollars.
    pub cost: f64,
    /// The rules behind each block, flag and hold, in order.
    pub blocked: Vec<String>,
    pub flagged: Vec<String>,
    pub held: Vec<String>,
    /// How many holds a run may have.
    pub max_holds: Option<u32>,
    /// Final answers that stood but were flagged for a person, with the
    /// rule and its score.
    pub flagged_answers: Vec<FlaggedAnswer>,
}

/// A final answer flagged for review.
#[derive(Debug, Clone, PartialEq)]
pub struct FlaggedAnswer {
    pub rule: String,
    pub text: String,
    pub score: f64,
    pub answer: String,
}

impl ConstitutionStats {
    pub fn is_empty(&self) -> bool {
        self.calls == 0 && self.answers == 0
    }

    /// The plugin's state in a line: `1 blocked · 1 flagged`.
    pub fn summary(&self) -> String {
        let parts: Vec<String> = [
            (self.blocked.len(), "blocked"),
            (self.flagged.len(), "flagged"),
            (self.held.len(), "held"),
        ]
        .into_iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, what)| format!("{count} {what}"))
        .collect();
        if parts.is_empty() {
            let checks = self.calls + self.answers;
            format!(
                "{checks} {}, all clear",
                if checks == 1 { "check" } else { "checks" }
            )
        } else {
            parts.join(" · ")
        }
    }
}

/// One run event, as the Events tab lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub turn: u32,
    pub kind: &'static str,
    pub text: String,
}

/// How many events a run keeps for the Events tab.
const LOG_LIMIT: usize = 200;

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
    /// The person set a goal (`/goal`).
    Goal(String),
    /// Assistant text; deltas append to the last one.
    Text(String),
    Thinking(String),
    Tool(ToolCard),
    Plugin(PluginNote),
    /// A child run's changes landed on this run (ADR 0009).
    Landed(LandedCard),
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
    /// A turn ended here: the point a fork can start from.
    TurnEnd {
        turn: u32,
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

/// The record a landing leaves in its parent, under [`LANDING_RECORD`]:
/// enough to draw its card again when the parent comes back from
/// history.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LandingRecord {
    pub from: String,
    pub title: String,
    pub landing: tau_vcs::Landing,
}

/// The plugin name a landing's record is stored under.
pub const LANDING_RECORD: &str = "landing";

impl LandedCard {
    pub fn from_record(record: LandingRecord) -> Self {
        Self {
            from: RunId(record.from.into()),
            title: record.title,
            changes: record
                .landing
                .changes
                .into_iter()
                .map(crate::change_log::Change::new)
                .collect(),
            conflicts: record.landing.conflicts,
        }
    }
}

/// A child run that landed on this run: what it brought.
#[derive(Debug, Clone, PartialEq)]
pub struct LandedCard {
    pub from: RunId,
    /// The child's title, as the run list called it.
    pub title: String,
    /// Its changes on this run's stack, newest first.
    pub changes: Vec<crate::change_log::Change>,
    /// Paths left with conflict markers for this run's next turn.
    pub conflicts: Vec<String>,
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
    /// Characters the call's arguments and result take in the context.
    pub size: usize,
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
    /// A `vcs_log` result, shown as the stack over trunk.
    Log(Box<ChangeLog>),
    /// A `vcs_diff` result: the files, each opening to its hunks.
    Files(Box<ChangeDiff>),
    /// A `vcs_show` result: the message, ids and parent, then the files.
    Commit(Box<ChangeDiff>),
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
        /// The answer's confidence and the threshold it had to pass.
        confidence: Option<(f32, f32)>,
        /// What each level suits, in the order of `levels`.
        hints: Vec<String>,
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

/// How a file changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Added,
    Modified,
    Removed,
}

impl FileKind {
    /// The letter `jj status` shows.
    pub fn letter(self) -> char {
        match self {
            Self::Added => 'A',
            Self::Modified => 'M',
            Self::Removed => 'D',
        }
    }
}

/// A changed file, with its line counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStat {
    pub path: String,
    pub kind: FileKind,
    pub added: usize,
    pub removed: usize,
}

/// A changed file and its diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub stat: FileStat,
    pub lines: Vec<DiffLine>,
}

/// The code of a run and one of its forks, side by side.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BranchCode {
    /// What the run changed after the fork point.
    pub main: Vec<FileStat>,
    /// What the fork changed after the fork point.
    pub fork: Vec<FileStat>,
    /// How the fork's code differs from the run's now, file by file.
    pub between: Vec<FileChange>,
}

/// Where the code of a comparison is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeState {
    Loading,
    Ready(BranchCode),
    /// The host cannot say: no project, or the diff failed.
    Unavailable(String),
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

/// One entry of a stored run, in order: a message, or a record a
/// plugin kept.
#[derive(Debug, Clone, PartialEq)]
pub enum Stored {
    Message(Message),
    Record { plugin: String, body: Value },
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
            repo: String::new(),
            status: RunStatus::Planning,
            turn: 0,
            items: Vec::new(),
            plan: Vec::new(),
            ran_at: None,
            limits: Limits::default(),
            usage: Totals::default(),
            context: ContextWindow::default(),
            plugins: Vec::new(),
            children: Vec::new(),
            origin: Origin::Root,
            started: String::new(),
            ledger: Vec::new(),
            log: Vec::new(),
            cost_before: 0.0,
            constitution: ConstitutionStats::default(),
            goal: None,
            pending_rewrite: None,
        }
    }

    /// A stored run, rebuilt from its transcript: the user's messages,
    /// the model's text and tool calls with their results. The caller
    /// sets the status and adds the stop.
    pub fn from_messages(
        id: RunId,
        title: impl Into<String>,
        agent: impl Into<String>,
        model: impl Into<String>,
        messages: &[Message],
    ) -> Self {
        let mut view = Self::new(id, title, agent, model);
        for message in messages {
            view.push_message(message);
        }
        view.end_stored_turn();
        view
    }

    /// [`Self::from_messages`], with the plugins' records shown where
    /// they happened. The store writes a turn's messages when the turn
    /// ends, after what plugins recorded during it, so a record shows
    /// after the turn that follows it, once its tool cards exist.
    /// tau-reasoning's is written as a message starts, so it shows
    /// right after that message, as it does live.
    pub fn from_timeline(
        id: RunId,
        title: impl Into<String>,
        agent: impl Into<String>,
        model: impl Into<String>,
        timeline: &[Stored],
    ) -> Self {
        let mut view = Self::new(id, title, agent, model);
        let mut starting: Vec<&Value> = Vec::new();
        let mut during: Vec<(&str, &Value)> = Vec::new();
        // Whether the turn the records in `during` belong to has begun.
        let mut begun = false;
        let flush = |view: &mut Self, during: &mut Vec<(&str, &Value)>| {
            for (plugin, body) in during.drain(..) {
                view.report(plugin, body);
            }
        };
        for entry in timeline {
            match entry {
                Stored::Record { plugin, body }
                    if plugin == tau_reasoning::NAME =>
                {
                    starting.push(body);
                }
                // A child landed between turns: its card follows the
                // turn it came after.
                Stored::Record { plugin, body } if plugin == LANDING_RECORD => {
                    flush(&mut view, &mut during);
                    if let Ok(record) =
                        serde_json::from_value::<LandingRecord>(body.clone())
                    {
                        view.end_turn();
                        view.items.push(Item::Landed(LandedCard::from_record(
                            record,
                        )));
                    }
                }
                Stored::Record { plugin, body } => {
                    if during.is_empty() {
                        begun = false;
                    }
                    during.push((plugin, body));
                }
                Stored::Message(message) => {
                    match message {
                        Message::User(_) => flush(&mut view, &mut during),
                        Message::Assistant(_) if begun => {
                            flush(&mut view, &mut during)
                        }
                        Message::Assistant(_) => begun = true,
                        Message::ToolResult(_) => {}
                    }
                    view.push_message(message);
                    if matches!(message, Message::User(_)) {
                        for body in starting.drain(..) {
                            view.report(tau_reasoning::NAME, body);
                        }
                    }
                }
            }
        }
        flush(&mut view, &mut during);
        for body in starting {
            view.report(tau_reasoning::NAME, body);
        }
        view.end_stored_turn();
        view
    }

    fn end_stored_turn(&mut self) {
        self.end_turn();
    }

    /// Marks the end of the current turn, once.
    fn end_turn(&mut self) {
        let ended = self.items.iter().rev().find_map(|item| match item {
            Item::TurnEnd { turn } => Some(*turn),
            _ => None,
        });
        if self.turn > 0 && ended != Some(self.turn) {
            self.items.push(Item::TurnEnd { turn: self.turn });
        }
    }

    fn push_message(&mut self, message: &Message) {
        let view = self;
        match message {
            Message::User(user) => view.push_user(user_words(&user.content)),
            Message::Assistant(reply) => {
                // A turn is a reply and the results of its calls; the
                // next reply starts the next one.
                view.end_turn();
                view.turn += 1;
                view.add_usage(&reply.usage);
                for block in &reply.content {
                    match block {
                        AssistantBlock::Text(text) => {
                            view.items.push(Item::Text(text.text.clone()))
                        }
                        AssistantBlock::Thinking(thinking) => view
                            .items
                            .push(Item::Thinking(thinking.thinking.clone())),
                        AssistantBlock::ToolCall(call) => {
                            let args = Value::Object(call.arguments.clone());
                            view.items.push(Item::Tool(ToolCard {
                                call_id: call.id.clone(),
                                tool: call.name.clone(),
                                summary: summarize_args(&args),
                                args,
                                state: ToolState::Running,
                                body: ToolBody::None,
                                from_plugin: None,
                                checks: Vec::new(),
                                pruned: None,
                                size: 0,
                            }))
                        }
                    }
                }
            }
            Message::ToolResult(result) => {
                let output = ToolOutput {
                    content: result.content.clone(),
                    details: result.details.clone(),
                };
                if let Some(card) = view.tool_mut(&result.tool_call_id) {
                    finish_tool(card, &output, result.is_error);
                }
            }
        }
    }

    /// Goes on on `model` at `effort`: the plan says so, and the context
    /// window is the new model's. A chat can change model between
    /// messages; the conversation carries over.
    pub fn switch_model(&mut self, model: &str, effort: &str) {
        self.model = model.to_owned();
        for field in &mut self.plan {
            match field.name.as_str() {
                "model" => field.value = model.to_owned(),
                "reasoning" => {
                    field.value = effort.to_owned();
                    field.set_by = None;
                }
                _ => {}
            }
        }
        if let Some(found) = tau_ai::model::find(model) {
            self.context.window = Some(found.context_window);
        }
    }

    /// Ends a rebuilt run: its status, and the stop line.
    pub fn finish_stored(&mut self, stop: StopReason, cost: f64) {
        self.usage.cost = cost;
        self.status = RunStatus::Finished(stop.clone());
        self.items.push(Item::Stop {
            stop,
            turns: self.turn,
            tokens: self.usage.tokens,
            cost,
            plugin_cost: self.usage.plugin_cost,
        });
    }

    pub fn with_origin(mut self, origin: Origin) -> Self {
        self.origin = origin;
        self
    }

    pub fn in_repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
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
            .take_while(|item| {
                matches!(item, Item::User(_) | Item::Goal(_) | Item::Plugin(_))
            })
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
    /// A person's message. A `/goal` shows as the goal it set; tau-goal
    /// sending the model back, which history stores as a user message,
    /// shows as tau-goal's note.
    pub fn push_user(&mut self, text: impl Into<String>) {
        let text = text.into();
        if let Some(condition) = tau_goal::set_message(&text) {
            self.items.push(Item::Goal(condition));
        } else if let Some(said) =
            text.strip_prefix(tau_goal::CONTINUATION_PREFIX)
        {
            let first = said.lines().next().unwrap_or_default();
            self.push_note(PluginNote {
                plugin: tau_goal::NAME.into(),
                text: first.trim_end_matches('.').to_owned(),
                detail: Some("before_stop".into()),
                tone: Tone::Warn,
                body: NoteBody::None,
            });
        } else {
            self.items.push(Item::User(text));
        }
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
        self.log(event);
        match event {
            RunEvent::RunStart { agent, .. } => {
                self.agent = agent.to_string();
                self.status = RunStatus::Running;
                // A resumed chat starts again with what it cost so far.
                self.cost_before = self.usage.cost;
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
                size: 0,
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
                    // A plugin's verdict on the call outlasts its end: a
                    // blocked call stays blocked, a flagged one flagged.
                    match card.state.clone() {
                        ToolState::Blocked { .. } => {}
                        flagged @ ToolState::Flagged { .. } => {
                            finish_tool(card, output, *is_error);
                            card.state = flagged;
                        }
                        _ => finish_tool(card, output, *is_error),
                    }
                }
            }
            RunEvent::TurnEnd { turn, usage, .. } => {
                self.turn = *turn;
                self.add_usage(usage);
                self.items.push(Item::TurnEnd { turn: *turn });
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
                    detail: self.pending_rewrite.take(),
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
            // The constitution's report of the hold already says it.
            RunEvent::Continued { plugin, .. }
                if &**plugin == tau_constitution::NAME
                    || &**plugin == tau_goal::NAME => {}
            RunEvent::PluginReport { plugin, body, .. } => {
                self.report(plugin, body)
            }
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
                self.usage.cost = self.cost_before + cost;
                self.status = RunStatus::Finished(stop.clone());
                self.items.push(Item::Stop {
                    stop: stop.clone(),
                    turns: self.turn,
                    tokens: self.usage.tokens,
                    cost: self.usage.cost,
                    plugin_cost: self.usage.plugin_cost,
                });
            }
        }
    }

    fn log(&mut self, event: &RunEvent) {
        let (kind, text) = match event {
            RunEvent::RunStart { agent, .. } => ("RunStart", agent.to_string()),
            RunEvent::TurnStart { turn, .. } => {
                ("TurnStart", format!("turn {turn}"))
            }
            RunEvent::ToolStart { tool, args, .. } => {
                ("ToolStart", format!("{tool} {}", summarize_args(args)))
            }
            RunEvent::ToolEnd {
                call_id, is_error, ..
            } => (
                "ToolEnd",
                format!("{call_id} {}", if *is_error { "error" } else { "ok" }),
            ),
            RunEvent::TurnEnd { usage, .. } => (
                "Usage",
                format!(
                    "in {} out {} {}",
                    tokens(usage.input),
                    tokens(usage.output),
                    usd(usage.cost.total)
                ),
            ),
            RunEvent::ContextRewritten {
                plugin,
                tokens_before,
                tokens_after,
                ..
            } => (
                "Rewrite",
                format!(
                    "{plugin} {} → {}",
                    tokens(*tokens_before),
                    tokens(*tokens_after)
                ),
            ),
            RunEvent::Retry { attempt, error, .. } => {
                ("Retry", format!("attempt {attempt}: {error}"))
            }
            RunEvent::Continued { plugin, .. } => {
                ("Continued", plugin.to_string())
            }
            RunEvent::PluginReport { plugin, body, .. } => {
                ("Report", format!("{plugin} {body}"))
            }
            RunEvent::PluginError {
                plugin, message, ..
            } => ("PluginError", format!("{plugin}: {message}")),
            RunEvent::RunEnd { stop, cost, .. } => {
                ("RunEnd", format!("{stop:?} {}", usd(*cost)))
            }
            // Deltas are too many to list.
            _ => return,
        };
        self.log.push(LogLine {
            turn: self.turn,
            kind,
            text,
        });
        if self.log.len() > LOG_LIMIT {
            self.log.remove(0);
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
            // A fork's events name no parent; its entry is found by id.
            RunEvent::RunEnd { run, stop, .. } => {
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
    event.run()
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
        (None, None) => match (text("id"), text("title")) {
            (Some(id), Some(title)) => format!("{id} · {title}"),
            _ => ["query", "id", "title"]
                .into_iter()
                .find_map(text)
                .unwrap_or_default()
                .to_owned(),
        },
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

impl RunView {
    /// What a plugin reported. The constitution's verdicts mark the call
    /// they are about, or note what it did with the final answer; other
    /// plugins' reports only reach the event log.
    pub fn report(&mut self, plugin: &str, body: &Value) {
        use tau_constitution::{Verdict, VerdictKind};
        if plugin == tau_reasoning::NAME {
            self.reasoning_report(body);
            return;
        }
        if plugin == tau_fast_compaction::NAME {
            self.ledger_report(body);
            return;
        }
        if plugin == tau_goal::NAME {
            self.goal_report(body);
            return;
        }
        if plugin != tau_constitution::NAME {
            return;
        }
        if body["kind"] == "error" {
            self.push_note(PluginNote {
                plugin: plugin.to_owned(),
                text: body["message"].as_str().unwrap_or("failed").to_owned(),
                detail: Some("not checked".into()),
                tone: Tone::Danger,
                body: NoteBody::None,
            });
            return;
        }
        if let Some(check) = tau_constitution::Check::parse(body) {
            let stats = &mut self.constitution;
            match check.call_id {
                Some(_) => stats.calls += 1,
                None => stats.answers += 1,
            }
            stats.questions += check.scores.len() as u32;
            stats.cost += check.cost;
            self.usage.plugin_cost += check.cost;
            // Every score shows on the call's card, passed or not.
            if let Some(call_id) = &check.call_id
                && let Some(card) = self.tool_mut(call_id)
            {
                card.checks = check
                    .scores
                    .iter()
                    .map(|score| format!("{} {:.2}", score.rule, score.score))
                    .collect();
            }
            self.sync_constitution_status();
            return;
        }
        let Some(verdict) = Verdict::parse(body) else {
            return;
        };
        if verdict.call_id.is_none() && verdict.kind == VerdictKind::Flagged {
            let answer = self.last_text().unwrap_or_default().to_owned();
            self.constitution.flagged_answers.push(FlaggedAnswer {
                rule: verdict.rule.clone(),
                text: verdict.text.clone(),
                score: verdict.score,
                answer,
            });
        }
        let stats = &mut self.constitution;
        match verdict.kind {
            VerdictKind::Blocked => stats.blocked.push(verdict.rule.clone()),
            VerdictKind::Flagged => stats.flagged.push(verdict.rule.clone()),
            VerdictKind::Held => {
                stats.held.push(verdict.rule.clone());
                stats.max_holds = verdict.max_holds.or(stats.max_holds);
            }
        }
        self.sync_constitution_status();
        let score = format!("{:.2}", verdict.score);
        if let Some(call_id) = &verdict.call_id {
            if let Some(card) = self.tool_mut(call_id) {
                card.state = match verdict.kind {
                    VerdictKind::Blocked => ToolState::Blocked {
                        plugin: plugin.to_owned(),
                        rule: verdict.rule,
                        reason: verdict.reason.unwrap_or(verdict.text),
                        score,
                    },
                    _ => ToolState::Flagged {
                        plugin: plugin.to_owned(),
                        rule: verdict.rule,
                        score,
                    },
                };
            }
            return;
        }
        let (text, tone) = match verdict.kind {
            VerdictKind::Held => (
                format!(
                    "held the stop: the answer breaks {} (\"{}\")",
                    verdict.rule, verdict.text
                ),
                Tone::Warn,
            ),
            _ => (
                format!(
                    "flagged the answer for review: {} (\"{}\")",
                    verdict.rule, verdict.text
                ),
                Tone::Warn,
            ),
        };
        let detail = match (verdict.hold, verdict.max_holds) {
            (Some(hold), Some(max)) => {
                format!("before_stop · continuation {hold} / {max}")
            }
            _ => format!("before_stop · {score}"),
        };
        self.push_note(PluginNote {
            plugin: plugin.to_owned(),
            text,
            detail: Some(detail),
            tone,
            body: NoteBody::None,
        });
    }

    /// What tau-reasoning chose, as a note with its distribution, the
    /// plan's reasoning, and its line in the plugin list.
    fn reasoning_report(&mut self, body: &Value) {
        let plugin = tau_reasoning::NAME;
        if body["kind"] == "error" {
            // The message goes on as the last one did; the failure is
            // worth a note all the same.
            self.push_note(PluginNote {
                plugin: plugin.to_owned(),
                text: body["message"].as_str().unwrap_or("failed").to_owned(),
                detail: Some(match body["runs_at"].as_str() {
                    Some(effort) => format!("stayed at {effort}"),
                    None => "kept the default".into(),
                }),
                tone: Tone::Danger,
                body: NoteBody::None,
            });
            return;
        }
        let Some(choice) = tau_reasoning::Choice::parse(body) else {
            return;
        };
        let chose = choice.kind == "chose";
        // What the message runs at: the pick, else what the last one ran
        // at (a record from before `runs_at` was kept has only the pick).
        let runs_at = choice
            .runs_at
            .clone()
            .or_else(|| chose.then(|| choice.effort.clone()));
        let before = std::mem::replace(&mut self.ran_at, runs_at.clone());
        self.usage.plugin_cost += choice.cost;
        let state = match &runs_at {
            Some(effort) if chose => format!("chose {effort}"),
            Some(effort) => format!("stayed at {effort}"),
            None => "kept the default".to_owned(),
        };
        match self.plugins.iter_mut().find(|status| status.name == plugin) {
            Some(status) => status.state = state,
            None => self.plugins.insert(
                0,
                PluginStatus {
                    name: plugin.to_owned(),
                    state,
                    tone: Tone::Quiet,
                },
            ),
        }
        let value = runs_at.clone().unwrap_or_else(|| "default".into());
        match self.plan.iter_mut().find(|field| field.name == "reasoning") {
            Some(field) => {
                field.value = value;
                field.set_by = Some(plugin.to_owned());
            }
            None => self.plan.insert(
                0,
                PlanField {
                    name: "reasoning".into(),
                    value,
                    set_by: Some(plugin.to_owned()),
                },
            ),
        }
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
                format!("picked {} reasoning for this message", name(&runs_at))
            }
            _ => format!("reasoning {} → {}", name(&before), name(&runs_at)),
        };
        let comparison = if chose { "above" } else { "below" };
        let outcome = match &runs_at {
            Some(effort) if chose => {
                format!("so this message runs at {effort}.")
            }
            Some(effort) => format!("so this message stays at {effort}."),
            None => "so this message runs at the model's default.".to_owned(),
        };
        self.push_note(PluginNote {
            plugin: plugin.to_owned(),
            text,
            detail: Some(format!("Jev · {}", usd(choice.cost))),
            tone: Tone::Info,
            body: NoteBody::Distribution {
                levels: choice
                    .levels
                    .iter()
                    .map(|level| (level.effort.clone(), level.p as f32))
                    .collect(),
                chosen: choice.chosen(),
                note: format!(
                    "Confidence {:.2} is {comparison} {:.2}, {outcome}",
                    choice.confidence, choice.threshold
                ),
                confidence: Some((
                    choice.confidence as f32,
                    choice.threshold as f32,
                )),
                hints: choice
                    .levels
                    .iter()
                    .map(|level| level.suits.clone())
                    .collect(),
            },
        });
    }

    /// The pruning plugin's ledger: each tool call in the run, with what
    /// it decided (recent calls it never judged are pinned).
    fn ledger_report(&mut self, body: &Value) {
        use tau_fast_compaction::{Action, Details};
        let Ok(details) = serde_json::from_value::<Details>(body.clone())
        else {
            return;
        };
        let mut turn = 1;
        let mut entries = Vec::new();
        for item in &self.items {
            match item {
                Item::TurnEnd { turn: ended } => turn = ended + 1,
                Item::Tool(card) => {
                    let decided = details
                        .decisions
                        .iter()
                        .find(|decision| decision.call_id == card.call_id);
                    entries.push(LedgerEntry {
                        call_id: card.call_id.clone(),
                        turn,
                        tool: card.tool.clone(),
                        input: card.summary.clone(),
                        tokens: (card.size / 4) as u64,
                        matters: decided.map(|d| d.keep_call as f32),
                        verbatim: decided.map(|d| d.keep_result as f32),
                        decision: match decided.map(|d| d.action) {
                            None => Decision::Pinned,
                            Some(Action::Keep) => Decision::Keep,
                            Some(Action::DropResult) => Decision::DropResult,
                            Some(Action::DropCall) => Decision::DropCall,
                        },
                    });
                }
                _ => {}
            }
        }
        let stats = &details.stats;
        let count = |n: usize, one: &str, many: &str| {
            format!("{n} {}", if n == 1 { one } else { many })
        };
        self.pending_rewrite = Some(format!(
            "{} judged in {} · {} cut, {} dropped · −{:.0}%",
            count(stats.calls - stats.pinned, "call", "calls"),
            count(stats.requests, "Jev request", "Jev requests"),
            count(stats.results_dropped, "result", "results"),
            count(stats.calls_dropped, "call", "calls"),
            stats.reduction_ratio * 100.0
        ));
        let state = format!(
            "{} pruned · −{:.0}%",
            stats.results_dropped + stats.calls_dropped,
            stats.reduction_ratio * 100.0
        );
        match self
            .plugins
            .iter_mut()
            .find(|status| status.name == tau_fast_compaction::NAME)
        {
            Some(status) => status.state = state,
            None => self.plugins.push(PluginStatus {
                name: tau_fast_compaction::NAME.into(),
                state,
                tone: Tone::Quiet,
            }),
        }
        self.update(RunUpdate::Ledger(entries));
    }

    /// The goal as `records` leave it, from history: its checks show in
    /// the Goal tab, and the continuations in the transcript already.
    pub fn set_goal_records(&mut self, records: &[Value]) {
        self.goal = tau_goal::Goal::fold(records);
        for check in self.goal.iter().flat_map(|goal| &goal.checks) {
            self.usage.plugin_cost += check.cost;
        }
        self.sync_goal_status();
    }

    /// What tau-goal did: the goal changes, and checks and stops show in
    /// the transcript.
    pub fn goal_report(&mut self, body: &Value) {
        use tau_goal::{Exhausted, Goal, Record};
        let Some(record) = Record::parse(body) else {
            return;
        };
        Goal::apply(&mut self.goal, &record);
        let plugin = tau_goal::NAME.to_owned();
        let note = |text: String, detail: String, tone| PluginNote {
            plugin: plugin.clone(),
            text,
            detail: Some(detail),
            tone,
            body: NoteBody::None,
        };
        match &record {
            Record::Check(check) => {
                self.usage.plugin_cost += check.cost;
                let max = self.goal.as_ref().map_or(0, |g| g.max_continuations);
                let (text, detail, tone) = if check.met {
                    (
                        "the goal is met".to_owned(),
                        format!(
                            "check {} · p {:.2} · the run stops",
                            check.n, check.p
                        ),
                        Tone::Good,
                    )
                } else {
                    let next = match check.continuation {
                        Some(n) => format!("continuing {n} of {max}"),
                        None => "not continuing".to_owned(),
                    };
                    (
                        "the goal is not met yet".to_owned(),
                        format!(
                            "check {} · p {:.2} · {next}",
                            check.n, check.p
                        ),
                        Tone::Warn,
                    )
                };
                self.push_note(note(text, detail, tone));
            }
            Record::Stopped { why } => {
                let why = match why {
                    Exhausted::Continuations => "out of continuations",
                    Exhausted::Budget => "out of budget",
                };
                self.push_note(note(
                    format!("stopped the goal: {why}, and it is not met"),
                    "before_stop".into(),
                    Tone::Danger,
                ));
            }
            Record::Error { message } => self.push_note(note(
                message.clone(),
                "not checked".into(),
                Tone::Danger,
            )),
            _ => {}
        }
        self.sync_goal_status();
    }

    /// A change to the goal made here (pause, extend, clear), before
    /// tau-goal stores and reports it.
    pub fn apply_goal(&mut self, record: &tau_goal::Record) {
        tau_goal::Goal::apply(&mut self.goal, record);
        self.sync_goal_status();
    }

    /// The run's plugin list says where the goal stands.
    fn sync_goal_status(&mut self) {
        use tau_goal::Status;
        let state = self.goal.as_ref().map(|goal| match goal.status {
            Status::Active => format!(
                "{} of {} continuations",
                goal.continuations, goal.max_continuations
            ),
            Status::Paused => "paused".to_owned(),
            Status::Met => format!("met at check {}", goal.checks.len()),
            Status::Stopped(_) => "stopped, not met".to_owned(),
        });
        let found = self
            .plugins
            .iter()
            .position(|status| status.name == tau_goal::NAME);
        match (state, found) {
            (Some(state), Some(at)) => self.plugins[at].state = state,
            (Some(state), None) => self.plugins.push(PluginStatus {
                name: tau_goal::NAME.into(),
                state,
                tone: Tone::Quiet,
            }),
            (None, Some(at)) => {
                self.plugins.remove(at);
            }
            (None, None) => {}
        }
    }

    /// The run's plugin list says what the constitution did so far.
    fn sync_constitution_status(&mut self) {
        let state = self.constitution.summary();
        let tone = if self.constitution.blocked.is_empty() {
            Tone::Quiet
        } else {
            Tone::Danger
        };
        match self
            .plugins
            .iter_mut()
            .find(|status| status.name == tau_constitution::NAME)
        {
            Some(status) => {
                status.state = state;
                status.tone = tone;
            }
            None => self.plugins.push(PluginStatus {
                name: tau_constitution::NAME.into(),
                state,
                tone,
            }),
        }
    }
}

fn finish_tool(card: &mut ToolCard, output: &ToolOutput, is_error: bool) {
    let text = text_of(output);
    card.size = card.args.to_string().len() + text.len();
    if is_error {
        card.state = ToolState::Failed(first_line(&text));
        return;
    }
    let vcs = [change_diff::DIFF_TOOL, change_diff::SHOW_TOOL]
        .contains(&card.tool.as_str())
        .then_some(output.details.as_ref())
        .flatten()
        .and_then(ChangeDiff::parse);
    if let Some(diff) = vcs {
        card.state = ToolState::Done {
            summary: Some(diff.stat()),
        };
        card.body = if card.tool == change_diff::SHOW_TOOL {
            ToolBody::Commit(Box::new(diff))
        } else {
            ToolBody::Files(Box::new(diff))
        };
        return;
    }
    let diff = output
        .details
        .as_ref()
        .and_then(|details| details.get("diff"))
        .and_then(Value::as_str)
        .map(parse_diff);
    let log = (card.tool == change_log::TOOL)
        .then_some(output.details.as_ref())
        .flatten()
        .and_then(ChangeLog::parse);
    if let Some(log) = log {
        card.state = ToolState::Done {
            summary: Some(log.summary()),
        };
        card.body = ToolBody::Log(Box::new(log));
        return;
    }
    // A tool may say how to sum up its result ("5 matches · 9 ms").
    let reported = output
        .details
        .as_ref()
        .and_then(|details| details.get("summary"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    card.state = ToolState::Done {
        summary: match (&diff, reported) {
            (Some(lines), _) => Some(diff_stat(lines)),
            (None, Some(summary)) => Some(summary),
            (None, None) => line_count(&text),
        },
    };
    card.body = match diff {
        Some(lines) => ToolBody::Diff(lines),
        None if card.tool == "bash" => ToolBody::Output(tail(&text, 4)),
        None => ToolBody::None,
    };
}

/// What the user wrote in a user message. A run's first message carries
/// plugins' context (memory, for one) as text blocks before the input,
/// so the user's words are its last text block.
pub fn user_words(content: &UserContent) -> String {
    match content {
        UserContent::Text(text) => text.clone(),
        UserContent::Blocks(blocks) => blocks
            .iter()
            .rev()
            .find_map(|block| match block {
                InputBlock::Text(text) => Some(text.text.clone()),
                InputBlock::Image(_) => None,
            })
            .unwrap_or_default(),
    }
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
    const HEADERS: [&str; 9] = [
        "---",
        "+++",
        "@@",
        "diff --git ",
        "new file mode",
        "deleted file mode",
        "old mode",
        "new mode",
        "\\ No newline",
    ];
    diff.lines()
        .filter(|line| !HEADERS.iter().any(|header| line.starts_with(header)))
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

/// Dollars to the cent, or to a tenth of one under a dollar: `$0.042`,
/// `$1.00`, `$12.34`. The choice is made on the rounded amount, so
/// `0.9996` reads `$1.00`, never `$1.000`.
pub fn usd(amount: f64) -> String {
    let fine = format!("{amount:.3}");
    if fine.parse::<f64>().is_ok_and(|rounded| rounded < 1.0) {
        format!("${fine}")
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
    fn goals_show_as_their_card_notes_and_state() {
        use tau_goal::{Check, Record, Status};
        let mut view = view();
        // As typed, and as history stores what the model got.
        view.push_user("/goal --continuations 3 tests pass");
        view.push_user(tau_goal::set_input("tests pass"));
        assert_eq!(
            view.items[..2],
            [
                Item::Goal("tests pass".into()),
                Item::Goal("tests pass".into())
            ]
        );
        let report = |view: &mut RunView, record: Record| {
            view.apply(&RunEvent::PluginReport {
                run: run(),
                plugin: tau_goal::NAME.into(),
                body: record.to_value(),
            })
        };
        report(
            &mut view,
            Record::Set {
                goal: "tests pass".into(),
                continuations: 3,
                budget: 2.0,
            },
        );
        let check = |n, met, continuation| {
            Record::Check(Check {
                n,
                met,
                p: if met { 0.9 } else { 0.1 },
                turn: n,
                continuation,
                cost: 0.001,
                spent: 0.1,
            })
        };
        report(&mut view, check(1, false, Some(1)));
        // The continuation's event says nothing more than the check.
        view.apply(&RunEvent::Continued {
            run: run(),
            plugin: tau_goal::NAME.into(),
            message: "tau-goal: the goal is not met yet".into(),
        });
        report(&mut view, check(2, true, None));
        let notes: Vec<(&str, Tone)> = view
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Plugin(note) => Some((note.text.as_str(), note.tone)),
                _ => None,
            })
            .collect();
        assert_eq!(
            notes,
            [
                ("the goal is not met yet", Tone::Warn),
                ("the goal is met", Tone::Good)
            ]
        );
        let goal = view.goal.clone().unwrap();
        assert_eq!(goal.status, Status::Met);
        assert_eq!(goal.continuations, 1);
        assert!((view.usage.plugin_cost - 0.002).abs() < 1e-9);
        let status = view
            .plugins
            .iter()
            .find(|status| status.name == tau_goal::NAME)
            .unwrap();
        assert_eq!(status.state, "met at check 2");

        // History: the continuation is tau-goal's note, not the person's.
        let mut stored = self::view();
        stored.push_user(format!(
            "{}the goal is not met yet (check 1, probability 0.10).\nGoal: x",
            tau_goal::CONTINUATION_PREFIX
        ));
        assert!(matches!(&stored.items[0], Item::Plugin(note)
            if note.plugin == tau_goal::NAME
                && note.text == "the goal is not met yet (check 1, probability 0.10)"));
        // Clearing takes the goal and its plugin line away.
        view.apply_goal(&Record::Cleared);
        assert!(view.goal.is_none());
        assert!(view.plugins.iter().all(|s| s.name != tau_goal::NAME));
    }

    #[test]
    fn a_pruning_ledger_fills_the_ledger_and_marks_its_calls() {
        let mut view = view();
        for (call, turn) in [("c1", 1), ("c2", 2)] {
            view.apply(&RunEvent::ToolStart {
                run: run(),
                call_id: call.into(),
                tool: "read".into(),
                args: json!({"path": format!("{call}.rs")}),
            });
            view.apply(&RunEvent::ToolEnd {
                run: run(),
                call_id: call.into(),
                output: Arc::new(ToolOutput::text("x".repeat(4000))),
                is_error: false,
            });
            view.apply(&RunEvent::TurnEnd {
                run: run(),
                turn,
                usage: Usage::default(),
            });
        }
        view.apply(&RunEvent::PluginReport {
            run: run(),
            plugin: tau_fast_compaction::NAME.into(),
            body: json!({
                "kind": "ledger",
                "decisions": [{
                    "call_id": "c1", "tool": "read", "action": "drop_result",
                    "keep_call": 0.9, "keep_result": 0.1
                }],
                "stats": {
                    "calls": 2, "pinned": 1, "kept": 0, "results_dropped": 1,
                    "calls_dropped": 0, "requests": 1, "state_tokens": 900,
                    "state_stage": "full", "chars_before": 8100,
                    "chars_after": 4400, "reduction_ratio": 0.46
                }
            }),
        });
        view.apply(&RunEvent::ContextRewritten {
            run: run(),
            plugin: tau_fast_compaction::NAME.into(),
            tokens_before: 2000,
            tokens_after: 1100,
        });
        assert_eq!(view.ledger.len(), 2);
        let first = &view.ledger[0];
        assert_eq!((first.turn, first.decision), (1, Decision::DropResult));
        assert!(first.tokens > 900, "{}", first.tokens);
        assert_eq!(first.matters, Some(0.9));
        assert_eq!(view.ledger[1].decision, Decision::Pinned);
        assert_eq!(view.ledger[1].turn, 2);
        let cards: Vec<Option<Pruned>> = view
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Tool(card) => Some(card.pruned),
                _ => None,
            })
            .collect();
        assert_eq!(cards, [Some(Pruned::ResultDropped), Some(Pruned::Kept)]);
        let Some(Item::Rewrite { detail, .. }) = view.items.last() else {
            panic!("a rewrite")
        };
        assert!(
            detail
                .as_deref()
                .unwrap()
                .contains("1 result cut, 0 calls dropped"),
            "{detail:?}"
        );
        assert!(
            view.plugins
                .iter()
                .any(|status| status.state == "1 pruned · −46%")
        );
    }

    #[test]
    fn constitution_verdicts_mark_their_calls_and_answers() {
        let mut view = view();
        let report = |body: Value| RunEvent::PluginReport {
            run: run(),
            plugin: tau_constitution::NAME.into(),
            body,
        };
        view.apply(&RunEvent::ToolStart {
            run: run(),
            call_id: "c1".into(),
            tool: "write".into(),
            args: serde_json::json!({"path": "a", "content": "maybe"}),
        });
        view.apply(&report(serde_json::json!({
            "kind": "flagged", "rule": "R2", "text": "No unwrap.",
            "score": 0.55, "call_id": "c1", "tool": "write"
        })));
        view.apply(&RunEvent::ToolEnd {
            run: run(),
            call_id: "c1".into(),
            output: Arc::new(ToolOutput::text("written")),
            is_error: false,
        });
        let Some(Item::Tool(card)) = view.items.last() else {
            panic!("a card")
        };
        // Running to its end does not clear the flag.
        assert_eq!(
            card.state,
            ToolState::Flagged {
                plugin: tau_constitution::NAME.into(),
                rule: "R2".into(),
                score: "0.55".into(),
            }
        );
        // A held answer is one note, not two.
        view.apply(&report(serde_json::json!({
            "kind": "held", "rule": "R6", "text": "Name the tests.",
            "score": 0.9, "reason": "Your answer breaks rule R6"
        })));
        view.apply(&RunEvent::Continued {
            run: run(),
            plugin: tau_constitution::NAME.into(),
            message: "Your answer breaks rule R6".into(),
        });
        let notes = view
            .items
            .iter()
            .filter(|item| matches!(item, Item::Plugin(_)))
            .count();
        assert_eq!(notes, 1);
        // Every check counts; passing scores show on their call's card.
        view.apply(&report(serde_json::json!({
            "kind": "checked", "call_id": "c1", "tool": "write",
            "scores": [{"rule": "R2", "score": 0.55}, {"rule": "R4", "score": 0.02}],
            "cost": 0.00003
        })));
        view.apply(&report(serde_json::json!({
            "kind": "checked", "scores": [{"rule": "R6", "score": 0.9}], "cost": 0.00001
        })));
        let Some(Item::Tool(card)) =
            view.items.iter().find(|item| matches!(item, Item::Tool(_)))
        else {
            panic!("a card")
        };
        assert_eq!(card.checks, ["R2 0.55", "R4 0.02"]);
        let stats = &view.constitution;
        assert_eq!((stats.calls, stats.answers, stats.questions), (1, 1, 3));
        assert_eq!(stats.flagged, ["R2"]);
        assert_eq!(stats.held, ["R6"]);
        assert!((stats.cost - 0.00004).abs() < 1e-12);
        // The run's plugin list says so.
        let status = view
            .plugins
            .iter()
            .find(|status| status.name == tau_constitution::NAME)
            .unwrap();
        assert_eq!(status.state, "1 flagged · 1 held");
        // Other plugins' reports only reach the log.
        let before = view.items.len();
        view.apply(&RunEvent::PluginReport {
            run: run(),
            plugin: "other".into(),
            body: serde_json::json!({"kind": "blocked"}),
        });
        assert_eq!(view.items.len(), before);
    }

    /// An amount shows three decimals exactly when it reads under a
    /// dollar, and two otherwise, and is what it rounds to.
    #[hegel::test(test_cases = 500)]
    fn usd_shows_cents_from_a_dollar_up(tc: hegel::TestCase) {
        use hegel::generators as gs;
        let amount = tc.draw(hegel::one_of!(
            gs::floats::<f64>().min_value(0.0).max_value(2.0),
            gs::floats::<f64>().min_value(0.99).max_value(1.0),
            gs::floats::<f64>().min_value(0.0).max_value(1e6),
        ));
        let text = usd(amount);
        let number = text.strip_prefix('$').expect("a dollar sign");
        let (_, decimals) = number.split_once('.').expect("decimals");
        let shown: f64 = number.parse().unwrap();
        if shown < 1.0 {
            assert_eq!(decimals.len(), 3, "{amount} as {text}");
            assert!((shown - amount).abs() <= 0.0005 + 1e-9, "{text}");
        } else {
            assert_eq!(decimals.len(), 2, "{amount} as {text}");
            assert!((shown - amount).abs() <= 0.005 + 1e-9, "{text}");
        }
    }

    #[test]
    fn usd_rounds_up_to_a_dollar_with_cents() {
        assert_eq!(usd(0.9996), "$1.00");
        assert_eq!(usd(0.9994), "$0.999");
        assert_eq!(usd(1.0), "$1.00");
        assert_eq!(usd(0.042), "$0.042");
    }

    /// Text however it streams in, split anywhere, shows as the text in
    /// one delta would: pieces join until something else happens.
    #[hegel::test]
    fn text_split_anywhere_shows_as_one_delta(tc: hegel::TestCase) {
        use hegel::generators as gs;
        // Text, or a tool starting (`None`).
        let parts: Vec<Option<String>> = tc
            .draw(gs::vecs(gs::optional(gs::text().max_size(12))).max_size(6));
        let tool = |n: usize| RunEvent::ToolStart {
            run: run(),
            call_id: format!("c{n}"),
            tool: Arc::from("read"),
            args: json!({ "path": "src/lib.rs" }),
        };
        let text = |delta: &str| RunEvent::TextDelta {
            run: run(),
            parent: None,
            delta: delta.into(),
        };
        let (mut whole, mut split) = (view(), view());
        for (n, part) in parts.iter().enumerate() {
            let Some(part) = part else {
                whole.apply(&tool(n));
                split.apply(&tool(n));
                continue;
            };
            whole.apply(&text(part));
            // Cut at char boundaries, some pieces empty.
            let bounds: Vec<usize> = (0..=part.len())
                .filter(|at| part.is_char_boundary(*at))
                .collect();
            let mut cuts =
                tc.draw(gs::vecs(gs::sampled_from(bounds)).max_size(4));
            cuts.sort();
            let mut from = 0;
            for cut in cuts.into_iter().chain([part.len()]) {
                split.apply(&text(&part[from..cut]));
                from = cut;
            }
        }
        assert_eq!(split.items, whole.items);
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
    fn a_log_reads_as_the_stack_over_trunk() {
        let mut view = view();
        view.apply(&RunEvent::ToolStart {
            run: run(),
            call_id: "c1".into(),
            tool: Arc::from("vcs_log"),
            args: json!({}),
        });
        let change = |id: &str, description: &str, immutable: bool| {
            json!({
                "change_id": id, "commit_id": "0123", "description": description,
                "empty": false, "conflict": false, "immutable": immutable,
                "working_copy": false, "divergent": false, "bookmarks": [],
            })
        };
        view.apply(&RunEvent::ToolEnd {
            run: run(),
            call_id: "c1".into(),
            output: Arc::new(ToolOutput {
                details: Some(json!({
                    "changes": [
                        change("a", "feat(tau-ui): a card", false),
                        change("b", "docs: a page", true),
                    ],
                    "more": false,
                })),
                ..ToolOutput::text(
                    "a 0123 feat(tau-ui): a card\nb 0123 docs: a page",
                )
            }),
            is_error: false,
        });
        let card = view.tool("c1").expect("card");
        assert_eq!(
            card.state,
            ToolState::Done {
                summary: Some("1 on the stack · 1 on trunk".into())
            }
        );
        let ToolBody::Log(log) = &card.body else {
            panic!("expected a log, got {:?}", card.body);
        };
        assert_eq!(log.stack[0].changes[0].subject, "a card");
        assert_eq!(log.trunk[0].scope.as_deref(), Some("docs"));
    }

    #[test]
    fn a_show_reads_as_a_commit_and_a_diff_as_files() {
        let mut view = view();
        let change = json!({
            "change_id": "onvkmqwo", "commit_id": "28b5b7a7",
            "description": "feat(tau-ai): honor retry-after\n",
            "empty": false, "conflict": false, "immutable": false,
            "working_copy": false, "divergent": false, "bookmarks": [],
        });
        let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n\
                    @@ -1 +1,2 @@\n-old\n+new\n+more\n";
        for (id, tool) in [("c1", "vcs_show"), ("c2", "vcs_diff")] {
            view.apply(&RunEvent::ToolStart {
                run: run(),
                call_id: id.into(),
                tool: Arc::from(tool),
                args: json!({ "change": "onvkmqwo" }),
            });
            view.apply(&RunEvent::ToolEnd {
                run: run(),
                call_id: id.into(),
                output: Arc::new(ToolOutput {
                    details: Some(json!({
                        "change": change,
                        "parents": [],
                        "author": { "name": "tau", "email": "tau@localhost" },
                        "files": [{ "path": "a.rs", "kind": "modified" }],
                        "diff": diff,
                        "truncated": false,
                    })),
                    ..ToolOutput::text(diff)
                }),
                is_error: false,
            });
        }
        let show = view.tool("c1").expect("card");
        assert_eq!(
            show.state,
            ToolState::Done {
                summary: Some("+2 −1".into())
            }
        );
        let ToolBody::Commit(commit) = &show.body else {
            panic!("expected a commit, got {:?}", show.body);
        };
        assert_eq!(commit.change.subject, "honor retry-after");
        assert!(matches!(
            &view.tool("c2").expect("card").body,
            ToolBody::Files(files) if files.files[0].hunks[0].lines.len() == 3
        ));
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

    /// A choice as tau-reasoning reports it, on gpt-5.5's levels.
    fn choice(kind: &str, effort: &str, runs_at: Option<&str>) -> Value {
        let levels: Vec<Value> = ["none", "low", "medium", "high", "xhigh"]
            .iter()
            .map(|effort| json!({ "effort": effort, "suits": "", "p": 0.2 }))
            .collect();
        json!({
            "kind": kind, "effort": effort, "confidence": 0.8,
            "threshold": 0.7, "levels": levels, "cost": 0.0,
            "runs_at": runs_at,
        })
    }

    #[test]
    fn a_reasoning_note_shows_only_when_the_effort_changes() {
        let mut view = RunView::new(RunId("r".into()), "t", "coder", "gpt-5.5");
        let notes = |view: &RunView| -> Vec<String> {
            view.items
                .iter()
                .filter_map(|item| match item {
                    Item::Plugin(note)
                        if note.plugin == tau_reasoning::NAME =>
                    {
                        Some(note.text.clone())
                    }
                    _ => None,
                })
                .collect()
        };
        // Unsure with nothing before: the default, no note.
        view.report(tau_reasoning::NAME, &choice("kept", "low", None));
        view.report(
            tau_reasoning::NAME,
            &choice("chose", "high", Some("high")),
        );
        // Unsure, staying at high; then sure of high again.
        view.report(tau_reasoning::NAME, &choice("kept", "low", Some("high")));
        view.report(
            tau_reasoning::NAME,
            &choice("chose", "high", Some("high")),
        );
        view.report(tau_reasoning::NAME, &choice("chose", "low", Some("low")));
        assert_eq!(
            notes(&view),
            [
                "picked **high** reasoning for this message",
                "reasoning **high** → **low**",
            ]
        );
        assert_eq!(view.ran_at.as_deref(), Some("low"));
        let reasoning =
            view.plan.iter().find(|f| f.name == "reasoning").unwrap();
        assert_eq!(reasoning.value, "low");
    }

    #[test]
    fn numbers_read_the_way_the_mockups_write_them() {
        assert_eq!(tokens(184_000), "184k");
        assert_eq!(usd(0.184), "$0.184");
        assert_eq!(usd(2.0), "$2.00");
        assert_eq!(clock(Duration::from_secs(192)), "3:12");
    }

    #[test]
    fn turns_end_with_a_marker_to_fork_from() {
        let mut view = view();
        view.apply(&RunEvent::TurnEnd {
            run: run(),
            turn: 1,
            usage: Usage::default(),
        });
        assert_eq!(view.items.last(), Some(&Item::TurnEnd { turn: 1 }));
    }

    #[test]
    fn stored_runs_get_a_marker_after_each_turn() {
        let user: Message = serde_json::from_value(json!({
            "role": "user", "content": "go", "timestamp": 0
        }))
        .unwrap();
        let reply = |text: &str| -> Message {
            serde_json::from_value(json!({
                "role": "assistant",
                "content": [{"type": "text", "text": text}],
                "api": "responses", "provider": "openai", "model": "m",
                "usage": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0,
                          "totalTokens": 0,
                          "cost": {"input": 0, "output": 0, "cacheRead": 0,
                                   "cacheWrite": 0, "total": 0}},
                "stopReason": "stop", "timestamp": 0
            }))
            .unwrap()
        };
        let view = RunView::from_messages(
            run(),
            "t",
            "a",
            "m",
            &[user, reply("one"), reply("two")],
        );
        let markers: Vec<u32> = view
            .items
            .iter()
            .filter_map(|item| match item {
                Item::TurnEnd { turn } => Some(*turn),
                _ => None,
            })
            .collect();
        assert_eq!(markers, [1, 2]);
        assert_eq!(view.turn, 2);
    }
}
