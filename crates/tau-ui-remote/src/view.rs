//! The run as the interface shows it, kept apart from GPUI so it can be
//! built and tested without a window.
//!
//! A [`RunView`] is fed the same [`RunEvent`]s a [`tau_agent::agent::Run`]
//! streams, through [`RunView::apply`]. Plugin decisions that no event
//! carries yet (a chosen reasoning effort, the notes memory added, which
//! rule blocked a call) come in as a [`RunUpdate`], until plugins report
//! them as events of their own.

use std::time::Duration;

use serde::{Deserialize, Serialize};
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

/// One run, as the transcript, the inspector and the run list show it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// The last run events, newest last, for the Events tab.
    pub log: Vec<LogLine>,
    /// What the chat cost before its latest message: each start of a
    /// resumed run reports only its own cost.
    pub cost_before: f64,
    /// Each plugin's state in the run, as its fold leaves it, by plugin
    /// name (ADR 0017).
    #[serde(default)]
    pub plugin_states:
        std::collections::BTreeMap<String, tau_ui_plugin::PluginValue>,
    /// What each plugin named the context rewrite it is about to make
    /// ([`tau_ui_plugin::RunCx::rewrite`]).
    #[serde(default)]
    pending_rewrites: std::collections::BTreeMap<String, String>,
    /// How a chat under main ended for good, once it landed or was
    /// dropped: it takes no more messages, and its screen is read-only.
    /// `None` for a run that can go on.
    #[serde(default)]
    pub ending: Option<Ending>,
    /// A main chat's: the finished chats waiting to land on it, in the
    /// order they land (ADR 0024). Empty for any other run.
    #[serde(default)]
    pub landing_queue: Vec<crate::queue::Waiting>,
    /// A main chat's: conflicts a turn left on its stack, until it is
    /// clean. Nothing lands on it and no chat forks it meanwhile.
    #[serde(default)]
    pub main_conflicts: Option<crate::queue::MainConflicts>,
    /// What landing it on its parent would do, as the host last worked
    /// it out in the background: a finished fork's. Gone once it goes
    /// on.
    #[serde(default)]
    pub forecast: Option<crate::attention::Forecast>,
}

/// How a chat ended for good: what [`RunView::ending`] holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Ending {
    /// Its changes landed on `on`, its parent: `changes` of them.
    Landed { on: RunId, changes: usize },
    /// It was dropped: its own changes were abandoned.
    Dropped,
}

/// One run event, as the Events tab lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogLine {
    pub turn: u32,
    pub kind: std::borrow::Cow<'static, str>,
    pub text: String,
}

/// How many events a run keeps for the Events tab.
const LOG_LIMIT: usize = 200;

/// Where a run came from.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RunStatus {
    /// Plugins are preparing the run; the session is not open yet.
    Planning,
    Running,
    Finished(StopReason),
    /// tau closed while it ran: it stopped mid-turn, its files as the
    /// cut-off turn left them. Not live; it waits to be resumed. The
    /// sidebar reads it as "Interrupted · tau closed".
    Interrupted,
}

impl RunStatus {
    pub fn is_live(&self) -> bool {
        matches!(self, Self::Planning | Self::Running)
    }
}

/// One entry of the transcript, in the order it happened.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Item {
    User(String),
    /// Assistant text; deltas append to the last one.
    Text(String),
    /// What the model reasoned, and how long it took once it ended: the
    /// seconds from asking the model to its first answer. A model that
    /// keeps its reasoning to itself leaves only the time.
    Thinking {
        text: String,
        secs: Option<u64>,
    },
    /// Boxed: a card is the largest item by far.
    Tool(Box<ToolCard>),
    Plugin(PluginNote),
    /// A plugin's anchor: the plugin draws what goes here, at the
    /// transcript's point (ADR 0017).
    Anchor {
        plugin: String,
        key: String,
    },
    /// A child run's changes landed on this run (ADR 0009).
    Landed(LandedCard),
    /// A fork of this run finished and waits to land or be dropped.
    ForkReady {
        fork: RunId,
    },
    /// A message tau sent to start a turn itself: resolving what a
    /// landing left in conflict (ADR 0014).
    Tau(String),
    /// A plugin rewrote the context: the tokens before and after, when
    /// the run saw it, and what the plugin named it, to draw it.
    Rewrite {
        plugin: String,
        tokens: Option<(u64, u64)>,
        key: Option<String>,
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

/// The plugin name a landing's record is stored under.
pub const LANDING_RECORD: &str = "landing";

/// The plugin name of the record a dropped chat keeps: it was dropped,
/// and takes no more messages.
pub const DROPPED_RECORD: &str = "dropped";

pub use tau_vcs::ui::landed::{LandedCard, LandingRecord};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCard {
    pub call_id: String,
    pub tool: String,
    /// The argument worth reading at a glance: a path, a command, a
    /// pattern.
    pub summary: String,
    pub state: ToolState,
    /// What the call sent and returned, for the tool's plugin to draw.
    /// Shared: cloning a card, and handing it to plugins, is cheap.
    pub data: std::sync::Arc<CallData>,
    /// The plugin that added the tool, when a plugin did.
    pub from_plugin: Option<String>,
    /// What a context rewrite left of the call, when it dropped some.
    pub dropped: Option<Dropped>,
    /// What a plugin cut from the call's result, when it did.
    /// Boxed: most cards have none, and it is the card's largest part.
    pub cut: Option<Box<OutputCut>>,
    /// Characters the call's arguments and result take in the context.
    pub size: usize,
    /// Plugins' anchors on the card, as `(plugin, key)`, in the order
    /// they came: what a plugin draws at the card's points.
    #[serde(default)]
    pub anchors: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ToolState {
    Running,
    Done {
        summary: Option<String>,
    },
    Failed(String),
    /// A plugin refused the call. The reason went back to the model.
    Blocked {
        plugin: String,
        reason: String,
    },
    /// The call ran, but a plugin wants a person to look at it.
    Flagged {
        plugin: String,
    },
}

impl ToolCard {
    /// The arguments the model sent.
    pub fn args(&self) -> &Value {
        &self.data.args
    }
}

/// A plugin speaking in the transcript. Lighter than a tool card, and
/// always named.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginNote {
    pub plugin: String,
    pub text: String,
    /// Cost, latency or confidence, in small print.
    pub detail: Option<String>,
    pub tone: Tone,
}

pub use tau_ui_kit::{
    diff::{DiffKind, DiffLine},
    theme::Tone,
};
pub use tau_ui_plugin::{
    CallData,
    CallResult,
    CardMark,
    Dropped,
    OutputCut,
    PlanField,
    PluginStatus,
};

/// A run as a plugin's fold reaches it. With `anchors`, the plugin's
/// anchors go in; without, the fold only restates the plugin's state:
/// the anchors of what it restates are in the transcript already, or
/// belong to another run.
struct Folding<'a> {
    view: &'a mut RunView,
    plugin: &'a str,
    anchors: bool,
}

impl tau_ui_plugin::RunCx for Folding<'_> {
    fn transcript(&mut self, key: &str) {
        if !self.anchors {
            return;
        }
        self.view.items.push(Item::Anchor {
            plugin: self.plugin.to_owned(),
            key: key.to_owned(),
        });
    }

    /// A nested call's anchor lands on the card of the model's call it
    /// came from.
    fn attach(&mut self, call_id: &str, key: &str) -> bool {
        let plugin = self.plugin.to_owned();
        let Some(card) = self.view.card_of_mut(call_id) else {
            return false;
        };
        if !self.anchors {
            return true;
        }
        let anchor = (plugin, key.to_owned());
        if !card.anchors.contains(&anchor) {
            card.anchors.push(anchor);
        }
        true
    }

    fn mark(&mut self, call_id: &str, mark: CardMark) -> bool {
        mark_card(self.view, self.plugin, call_id, mark)
    }

    fn dropped(&mut self, call_id: &str, dropped: Dropped) -> bool {
        self.view
            .tool_mut(call_id)
            .map(|card| card.dropped = Some(dropped))
            .is_some()
    }

    fn cut(&mut self, call_id: &str, cut: OutputCut) -> bool {
        self.view
            .tool_mut(call_id)
            .map(|card| card.cut = Some(Box::new(cut)))
            .is_some()
    }

    /// History placed the rewrite before its details were folded; live,
    /// the plugin names it before making it.
    fn rewrite(&mut self, key: &str) {
        if !self.anchors {
            return;
        }
        let plugin = self.plugin;
        let placed =
            self.view
                .items
                .iter_mut()
                .rev()
                .find_map(|item| match item {
                    Item::Rewrite {
                        plugin: by, key, ..
                    } if by == plugin => Some(key),
                    _ => None,
                });
        match placed {
            Some(unnamed @ None) => *unnamed = Some(key.to_owned()),
            _ => {
                self.view
                    .pending_rewrites
                    .insert(plugin.to_owned(), key.to_owned());
            }
        }
    }

    fn cards(&self) -> Vec<tau_ui_plugin::CardInfo> {
        Folding::cards_of(self.view)
    }

    fn last_text(&self) -> Option<String> {
        self.view.last_text().map(str::to_owned)
    }

    fn turn(&self) -> u32 {
        self.view.turn
    }
}

/// Marks what `plugin` decided about the call `call_id` on its card. A
/// nested call's mark goes on its outermost card's data, for its row,
/// and leaves the card's own state alone (`tau_ui_plugin::RunCx`).
fn mark_card(
    view: &mut RunView,
    plugin: &str,
    call_id: &str,
    mark: CardMark,
) -> bool {
    if view.tool(call_id).is_none() {
        let Some(card) = view.card_of_mut(call_id) else {
            return false;
        };
        std::sync::Arc::make_mut(&mut card.data)
            .mark_nested(call_id, plugin, mark);
        return true;
    }
    let Some(card) = view.tool_mut(call_id) else {
        return false;
    };
    let plugin = plugin.to_owned();
    card.state = match mark {
        CardMark::Blocked { reason } => ToolState::Blocked { plugin, reason },
        CardMark::Flagged => ToolState::Flagged { plugin },
    };
    true
}

impl Folding<'_> {
    /// `view`'s tool calls, in order, with the turn of each.
    fn cards_of(view: &RunView) -> Vec<tau_ui_plugin::CardInfo> {
        let mut turn = 1;
        let mut cards = Vec::new();
        for item in &view.items {
            match item {
                Item::TurnEnd { turn: ended } => turn = ended + 1,
                Item::Tool(card) => cards.push(tau_ui_plugin::CardInfo {
                    call_id: card.call_id.clone(),
                    tool: card.tool.clone(),
                    args: card.args().clone(),
                    summary: card.summary.clone(),
                    size: card.size,
                    turn,
                }),
                _ => {}
            }
        }
        cards
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Limits {
    pub max_turns: Option<u32>,
    pub max_tokens: Option<u64>,
    pub max_usd: Option<f64>,
    pub timeout: Option<Duration>,
    pub elapsed: Duration,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Totals {
    pub tokens: u64,
    pub cost: f64,
    /// Charged by plugins; part of `cost`.
    pub plugin_cost: f64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ContextWindow {
    pub used: u64,
    pub window: Option<u64>,
    /// The size before the last rewrite, while it is worth showing.
    pub before: Option<u64>,
}

/// What fills the context, estimated at four characters a token and
/// scaled to the model's own count ([`ContextWindow::used`]).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize,
)]
pub struct ContextParts {
    /// What the transcript does not account for: the instructions, the
    /// tool definitions, and the reasoning the model carries between
    /// turns.
    pub fixed: u64,
    /// What the person and the model wrote, tool calls included.
    pub conversation: u64,
    /// What the tools returned.
    pub results: u64,
}

impl ContextParts {
    /// The parts in `used` tokens of context, from what `items` hold
    /// after the last rewrite that replaced them (pruning keeps them,
    /// marked).
    pub fn estimate(items: &[Item], used: u64) -> Self {
        let from = items
            .iter()
            .rposition(|item| {
                matches!(item, Item::Rewrite { plugin, .. }
                    if !crate::plugins::registry()
                        .get(plugin)
                        .is_some_and(|plugin| plugin.rewrites_keep_transcript()))
            })
            .map_or(0, |at| at + 1);
        let tokens = |chars: usize| chars.div_ceil(4) as u64;
        let (mut conversation, mut results) = (0, 0);
        for item in &items[from..] {
            match item {
                Item::User(text) | Item::Text(text) => {
                    conversation += tokens(text.len())
                }
                Item::Tool(card) => {
                    let args = card.args().to_string().len();
                    match card.dropped {
                        Some(Dropped::Call) => {}
                        Some(Dropped::Result) => conversation += tokens(args),
                        _ => {
                            conversation += tokens(args);
                            results += tokens(card.size.saturating_sub(args));
                        }
                    }
                }
                _ => {}
            }
        }
        let seen = conversation + results;
        if seen > used {
            // The estimate runs over the model's count: share the count
            // out in proportion.
            let conversation = conversation * used / seen;
            return Self {
                fixed: 0,
                conversation,
                results: used - conversation,
            };
        }
        Self {
            fixed: used - seen,
            conversation,
            results,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChildRun {
    pub id: RunId,
    pub title: String,
    pub kind: ChildKind,
    pub status: RunStatus,
    /// The parent's tool call that started a live sub-agent.
    #[serde(default)]
    pub call: Option<String>,
}

impl ChildRun {
    /// `child` as its parent lists it: a sub-agent it started (by its
    /// call `call`), or a fork of it. Live and stored runs list them
    /// alike.
    pub fn of(child: &RunView, kind: ChildKind, call: Option<String>) -> Self {
        Self {
            id: child.id.clone(),
            title: child.title.clone(),
            kind,
            status: child.status.clone(),
            call,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChildKind {
    SubAgent,
    Fork,
}

/// How a file changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStat {
    pub path: String,
    pub kind: FileKind,
    pub added: usize,
    pub removed: usize,
}

/// A changed file and its diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    pub stat: FileStat,
    pub lines: Vec<DiffLine>,
}

/// The code of a run and one of its forks, side by side.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BranchCode {
    /// What the run changed after the fork point.
    pub main: Vec<FileStat>,
    /// What the fork changed after the fork point.
    pub fork: Vec<FileStat>,
    /// How the fork's code differs from the run's now, file by file.
    pub between: Vec<FileChange>,
}

/// Where the code of a comparison is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodeState {
    Loading,
    Ready(BranchCode),
    /// The host cannot say: no project, or the diff failed.
    Unavailable(String),
}

/// Anything that changes a [`RunView`]: a run event, or what the host
/// knows that no event carries yet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// The plugin that added a tool, for its card's tag.
    ToolPlugin {
        call_id: String,
        plugin: String,
    },
}

impl From<RunEvent> for RunUpdate {
    fn from(event: RunEvent) -> Self {
        Self::Event(event)
    }
}

/// One entry of a stored run, in order: a message, or a record a
/// plugin kept.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Stored {
    Message(Message),
    Record {
        plugin: String,
        body: Value,
    },
    /// A context rewrite, in place: the details the plugin gave it, and
    /// the context's tokens before and after, when stored.
    Rewrite {
        plugin: String,
        body: Value,
        tokens: Option<(u64, u64)>,
    },
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
            limits: Limits::default(),
            usage: Totals::default(),
            context: ContextWindow::default(),
            plugins: Vec::new(),
            children: Vec::new(),
            origin: Origin::Root,
            started: String::new(),
            log: Vec::new(),
            cost_before: 0.0,
            plugin_states: Default::default(),
            pending_rewrites: Default::default(),
            ending: None,
            landing_queue: Vec::new(),
            main_conflicts: None,
            forecast: None,
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
    /// after the turn that follows it, once its tool cards exist, unless
    /// it says otherwise (`tau_ui_plugin::PLACE`).
    pub fn from_timeline(
        id: RunId,
        title: impl Into<String>,
        agent: impl Into<String>,
        model: impl Into<String>,
        timeline: &[Stored],
    ) -> Self {
        let mut view = Self::new(id, title, agent, model);
        // A rewrite's details mark the cards that follow it, so they come
        // last.
        let mut rewrites: Vec<(&str, &Value)> = Vec::new();
        let mut starting: Vec<(&str, &Value)> = Vec::new();
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
                Stored::Rewrite {
                    plugin,
                    body,
                    tokens,
                } => {
                    view.items.push(Item::Rewrite {
                        plugin: plugin.clone(),
                        tokens: *tokens,
                        key: None,
                    });
                    rewrites.push((plugin, body));
                }
                // What a plugin published as the run started shows after
                // the message it started on; what it says shows now,
                // where it was stored.
                Stored::Record { plugin, body }
                    if body[tau_ui_plugin::PLACE]
                        == tau_ui_plugin::PLACE_MESSAGE =>
                {
                    starting.push((plugin, body));
                }
                Stored::Record { plugin, body }
                    if body[tau_ui_plugin::PLACE]
                        == tau_ui_plugin::PLACE_NOW =>
                {
                    flush(&mut view, &mut during);
                    view.report(plugin, body);
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
                        for (plugin, body) in starting.drain(..) {
                            view.report(plugin, body);
                        }
                    }
                }
            }
        }
        flush(&mut view, &mut during);
        for (plugin, body) in starting {
            view.report(plugin, body);
        }
        for (plugin, details) in rewrites {
            view.fold(
                plugin,
                &serde_json::json!({ tau_ui_plugin::REWRITE: details }),
            );
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
                        AssistantBlock::Thinking(thinking) => {
                            view.items.push(Item::Thinking {
                                text: thinking.thinking.clone(),
                                secs: None,
                            })
                        }
                        AssistantBlock::ToolCall(call) => {
                            let args = Value::Object(call.arguments.clone());
                            view.items.push(Item::Tool(Box::new(ToolCard {
                                call_id: call.id.clone(),
                                tool: call.name.clone(),
                                summary: summarize_args(&args),
                                state: ToolState::Running,
                                data: std::sync::Arc::new(CallData {
                                    args,
                                    ..CallData::default()
                                }),
                                from_plugin: None,
                                dropped: None,
                                cut: None,
                                anchors: Vec::new(),
                                size: 0,
                            })))
                        }
                    }
                }
            }
            Message::ToolResult(result) => {
                let output = ToolOutput {
                    content: result.content.clone(),
                    details: result.details.clone(),
                    structured: None,
                };
                if let Some(card) = view.tool_mut(&result.tool_call_id) {
                    finish_tool(card, &output, result.is_error);
                }
            }
        }
    }

    /// A finished fork of this run waits in its chat, to land or be
    /// dropped: once, and not when it landed already.
    pub fn fork_finished(&mut self, fork: &RunId) {
        let shown = self.items.iter().any(|item| match item {
            Item::ForkReady { fork: ready } => ready == fork,
            Item::Landed(card) => &card.from == fork,
            _ => false,
        });
        if !shown {
            self.items.push(Item::ForkReady { fork: fork.clone() });
        }
    }

    /// The plan a run starts with: its model and effort, what it reaches
    /// models through, and the workspace it works in, which also gets a
    /// line in the plugin list. A live run and its stored view both show
    /// it.
    pub fn set_base_plan(
        &mut self,
        model: &str,
        effort: &str,
        access: &str,
        workspace: Option<&str>,
    ) {
        let field = |name: &str, value: &str, set_by: Option<&str>| PlanField {
            name: name.into(),
            value: value.into(),
            set_by: set_by.map(str::to_owned),
        };
        self.plan = vec![
            field("model", model, None),
            field("reasoning", effort, None),
            field("access", access, None),
            field(
                "workspace",
                workspace.unwrap_or_default(),
                workspace.map(|_| "workspace"),
            ),
        ];
        self.plugins.retain(|status| status.name != "workspace");
        if workspace.is_some() {
            self.plugins.push(PluginStatus {
                name: "workspace".into(),
                state: "a commit per turn".into(),
                tone: tau_ui_kit::theme::Tone::Quiet,
            });
        }
    }

    /// Goes on on `model` at `effort`: the plan says so, and the context
    /// window is the new model's. A chat can change model between
    /// messages; the conversation carries over.
    pub fn switch_model(&mut self, model: &str, effort: &str) {
        self.model = model.to_owned();
        // A run read back from the store has no plan yet: its fields are
        // added, so the choice is still there for the next message.
        for (name, value) in [("reasoning", effort), ("model", model)] {
            match self.plan.iter_mut().find(|field| field.name == name) {
                Some(field) => {
                    field.value = value.to_owned();
                    field.set_by = None;
                }
                None => self.plan.insert(
                    0,
                    PlanField {
                        name: name.into(),
                        value: value.to_owned(),
                        set_by: None,
                    },
                ),
            }
        }
        if let Some(found) = tau_ai::model::find(model) {
            self.context.window = Some(found.context_window);
        }
    }

    /// Ends a rebuilt run: its status, and the stop line.
    /// A stored run's end: `cost` in all, `plugin_cost` of it charged by
    /// plugins.
    pub fn finish_stored(
        &mut self,
        stop: StopReason,
        cost: f64,
        plugin_cost: f64,
    ) {
        self.usage.cost = cost;
        self.usage.plugin_cost = plugin_cost;
        self.status = RunStatus::Finished(stop.clone());
        self.items.push(Item::Stop {
            stop,
            turns: self.turn,
            tokens: self.usage.tokens,
            cost,
            plugin_cost: self.usage.plugin_cost,
        });
    }

    /// Sets a stored run that tau closed while it ran: its costs, and
    /// [`RunStatus::Interrupted`], with no stop in its transcript.
    pub fn interrupted_stored(&mut self, cost: f64, plugin_cost: f64) {
        self.usage.cost = cost;
        self.usage.plugin_cost = plugin_cost;
        self.status = RunStatus::Interrupted;
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

    /// Folds a body `plugin` published (or the host stored for it) into
    /// the plugin's state in the run, through the plugin's own fold. A
    /// plugin the registry does not have folds nothing.
    pub fn fold(&mut self, plugin: &str, body: &Value) {
        let Some(erased) = crate::plugins::registry().get(plugin) else {
            return;
        };
        let mut state = self.plugin_states.remove(plugin).unwrap_or_default();
        erased.apply(
            &mut state,
            body,
            &mut Folding {
                view: self,
                plugin,
                anchors: true,
            },
        );
        self.plugin_states.insert(plugin.to_owned(), state);
    }

    /// `plugin`'s state in the run as `bodies` alone leave it, folded
    /// afresh without placing anchors: what a fork inherits, or what a
    /// change that could not be saved leaves.
    pub fn restate(&mut self, plugin: &str, bodies: &[Value]) {
        let Some(erased) = crate::plugins::registry().get(plugin) else {
            return;
        };
        let mut state = tau_ui_plugin::PluginValue::default();
        for body in bodies {
            let mut folding = Folding {
                view: self,
                plugin,
                anchors: false,
            };
            erased.apply(&mut state, body, &mut folding);
        }
        // Nothing folded: no state, as before anything was published.
        if state.is_null() {
            self.plugin_states.remove(plugin);
        } else {
            self.plugin_states.insert(plugin.to_owned(), state);
        }
    }

    /// The run's tool calls, as a plugin sees them.
    pub fn cards(&self) -> Vec<tau_ui_plugin::CardInfo> {
        Folding::cards_of(self)
    }

    /// The run, as a plugin's UI sees it.
    pub fn info(&self) -> tau_ui_plugin::RunInfo {
        tau_ui_plugin::RunInfo {
            id: self.id.clone(),
            repo: self.repo.clone(),
            live: self.status.is_live(),
            title: self.title.clone(),
            answer: (!self.status.is_live())
                .then(|| self.last_text().map(str::to_owned))
                .flatten(),
            context: self.context.used,
            window: self.context.window,
        }
    }

    pub fn last_text(&self) -> Option<&str> {
        self.items.iter().rev().find_map(|item| match item {
            Item::Text(text) => Some(text.as_str()),
            _ => None,
        })
    }

    /// The last edit that changed a file, with its diff.
    pub fn last_diff(&self) -> Option<(&ToolCard, Vec<DiffLine>)> {
        self.items.iter().rev().find_map(|item| match item {
            Item::Tool(card) => {
                let result = card.data.result.as_ref()?;
                let diff = result.details.as_ref()?.get("diff")?.as_str()?;
                Some((&**card, tau_ui_kit::diff::parse(diff)))
            }
            _ => None,
        })
    }

    /// Plugin notes from before the model's first turn: what each
    /// plugin's `start` decided.
    pub fn start_notes(&self) -> impl Iterator<Item = &PluginNote> {
        self.items
            .iter()
            .take_while(|item| {
                matches!(
                    item,
                    Item::User(_) | Item::Plugin(_) | Item::Anchor { .. }
                )
            })
            .filter_map(|item| match item {
                Item::Plugin(note) => Some(note),
                _ => None,
            })
    }

    /// The latest context rewrite, as `(plugin, before, after, detail)`.
    /// What fills the context now, estimated; `None` before the model
    /// has counted it.
    pub fn context_parts(&self) -> Option<ContextParts> {
        (self.context.used > 0)
            .then(|| ContextParts::estimate(&self.items, self.context.used))
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
                Some(&**card)
            }
            _ => None,
        })
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
            RunUpdate::ToolPlugin { call_id, plugin } => {
                if let Some(card) = self.tool_mut(&call_id) {
                    card.from_plugin = Some(plugin);
                }
            }
        }
    }

    /// Adds the user's message. The run's own events never carry it. A
    /// plugin that reads it as its own draws it (a `/goal`).
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

    pub fn tool(&self, call_id: &str) -> Option<&ToolCard> {
        self.items.iter().find_map(|item| match item {
            Item::Tool(card) if card.call_id == call_id => Some(&**card),
            _ => None,
        })
    }

    pub fn tool_mut(&mut self, call_id: &str) -> Option<&mut ToolCard> {
        self.items.iter_mut().find_map(|item| match item {
            Item::Tool(card) if card.call_id == call_id => Some(&mut **card),
            _ => None,
        })
    }

    /// The card a call shows on: its own, or, for a call a tool made
    /// through the loop (`<parent>/<n>`, at any depth), the card of the
    /// model's call it came from. Nested calls never reach the
    /// transcript, so they never get a card of their own.
    pub fn card_of(&self, call_id: &str) -> Option<&ToolCard> {
        self.items.iter().find_map(|item| match item {
            Item::Tool(card)
                if card.call_id == call_id
                    || tau_ui_plugin::nested_under(call_id, &card.call_id) =>
            {
                Some(&**card)
            }
            _ => None,
        })
    }

    /// [`Self::card_of`], to change.
    pub fn card_of_mut(&mut self, call_id: &str) -> Option<&mut ToolCard> {
        self.items.iter_mut().find_map(|item| match item {
            Item::Tool(card)
                if card.call_id == call_id
                    || tau_ui_plugin::nested_under(call_id, &card.call_id) =>
            {
                Some(&mut **card)
            }
            _ => None,
        })
    }

    /// The tool and arguments of the call `call_id`: a model's call, or
    /// a call nested under one (at any depth) while that call runs. A
    /// nested call is known only until the model's call ends: its card
    /// keeps nothing of it after (ADR 0018).
    pub fn call(&self, call_id: &str) -> Option<(&str, &Value)> {
        let card = self.card_of(call_id)?;
        if card.call_id == call_id {
            return Some((card.tool.as_str(), card.args()));
        }
        card.data
            .nested
            .iter()
            .find(|call| call.id == call_id)
            .map(|call| (call.tool.as_str(), &call.args))
    }

    /// Whether the call `call_id`, which just ended, proposed the run's
    /// landing with `vcs_land` (ADR 0014):
    ///
    /// - a model's `vcs_land` call that succeeded;
    /// - a `vcs_land` nested under a model's call, at any depth, that
    ///   succeeded, while the model's call runs;
    /// - a model's call whose result lists a `vcs_land` that succeeded
    ///   among the calls it made (`details.calls`, as codemode's do).
    ///   That is all a stored run has of its nested calls, and what a
    ///   live card keeps of them once the call ended.
    ///
    /// A run that proposed may be told so more than once, by a nested
    /// call and by the call that made it: the workspace keeps a set.
    pub fn proposes_landing(&self, call_id: &str) -> bool {
        let Some(card) = self.card_of(call_id) else {
            return false;
        };
        if card.call_id != call_id {
            return card.data.nested.iter().any(|call| {
                call.id == call_id
                    && call.tool == LAND
                    && call.result.as_ref().is_some_and(|result| !result.error)
            });
        }
        (card.tool == LAND && matches!(card.state, ToolState::Done { .. }))
            || card
                .data
                .result
                .as_ref()
                .and_then(|result| result.details.as_ref())
                .is_some_and(lists_landing)
    }

    /// Puts the `secs` the model reasoned (`HostUpdate::Reasoned`) on
    /// the reasoning it streamed. A model that streamed none and answered
    /// after a second or more gets a reasoning item of its own, holding
    /// only the time.
    pub fn reasoned(&mut self, secs: u64, answered: bool) {
        match self.items.last_mut() {
            Some(Item::Thinking {
                secs: open @ None, ..
            }) => *open = Some(secs),
            _ if answered && secs > 0 => self.items.push(Item::Thinking {
                text: String::new(),
                secs: Some(secs),
            }),
            _ => {}
        }
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
                // What it would land changes with what it does next.
                self.forecast = None;
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
                    Some(Item::Thinking { text, secs: None }) => {
                        text.push_str(delta)
                    }
                    _ => self.items.push(Item::Thinking {
                        text: delta.clone(),
                        secs: None,
                    }),
                }
            }
            RunEvent::ToolCallDelta { .. } => {}
            // A call a tool made through the loop: it folds into the
            // card of the model's call it came from, and gets none of
            // its own (ADR 0018).
            RunEvent::ToolStart {
                call_id,
                tool,
                args,
                parent: Some(parent),
                ..
            } => {
                if let Some(card) = self.card_of_mut(parent) {
                    std::sync::Arc::make_mut(&mut card.data)
                        .nested_start(call_id, parent, tool, args);
                }
            }
            RunEvent::ToolUpdate {
                call_id,
                partial,
                parent: Some(_),
                ..
            } => {
                if let Some(card) = self.card_of_mut(call_id) {
                    std::sync::Arc::make_mut(&mut card.data)
                        .nested_update(call_id, partial);
                }
            }
            RunEvent::ToolEnd {
                call_id,
                output,
                is_error,
                parent: Some(_),
                ..
            } => {
                if let Some(card) = self.card_of_mut(call_id) {
                    std::sync::Arc::make_mut(&mut card.data)
                        .nested_end(call_id, output, *is_error);
                }
            }
            RunEvent::ToolStart {
                call_id,
                tool,
                args,
                ..
            } => self.items.push(Item::Tool(Box::new(ToolCard {
                call_id: call_id.clone(),
                tool: tool.to_string(),
                summary: summarize_args(args),
                state: ToolState::Running,
                data: std::sync::Arc::new(CallData {
                    args: args.clone(),
                    ..CallData::default()
                }),
                from_plugin: None,
                dropped: None,
                cut: None,
                anchors: Vec::new(),
                size: 0,
            }))),
            // What the call reports while it runs, for its plugin to
            // draw: a command's screen, the output so far.
            RunEvent::ToolUpdate {
                call_id, partial, ..
            } => {
                if let Some(card) = self.tool_mut(call_id) {
                    let data = std::sync::Arc::make_mut(&mut card.data);
                    if let Some(details) = &partial.details {
                        data.updates.push(details.clone());
                    }
                    data.partial = Some(partial.text_content());
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
                    tokens: Some((*tokens_before, *tokens_after)),
                    key: self.pending_rewrites.remove(&**plugin),
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
            // What the person steered with, read by the run.
            RunEvent::Steered { text, .. } => self.push_user(text.clone()),
            // A plugin with its UI says it in what it publishes.
            RunEvent::Continued { plugin, .. }
                if crate::plugins::registry().get(plugin).is_some() => {}
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
            }),
            RunEvent::PluginCharged { usage, .. } => {
                self.usage.plugin_cost += usage.cost.total;
            }
            RunEvent::PluginError {
                plugin, message, ..
            } => self.push_note(PluginNote {
                plugin: plugin.to_string(),
                text: message.clone(),
                detail: Some("error".into()),
                tone: Tone::Danger,
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
            // A call a tool made is listed apart from the model's own.
            RunEvent::ToolStart {
                call_id,
                tool,
                args,
                parent: Some(_),
                ..
            } => (
                "NestedStart",
                format!("{call_id} {tool} {}", summarize_args(args)),
            ),
            RunEvent::ToolEnd {
                call_id,
                is_error,
                parent: Some(_),
                ..
            } => (
                "NestedEnd",
                format!("{call_id} {}", if *is_error { "error" } else { "ok" }),
            ),
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
            kind: kind.into(),
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
                call,
            } if parent == &self.id => {
                self.children.push(ChildRun {
                    id: run.clone(),
                    title: agent.to_string(),
                    kind: ChildKind::SubAgent,
                    status: RunStatus::Running,
                    call: call.as_deref().map(str::to_owned),
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
            _ => ["query", "task", "id", "title"]
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
    /// What a plugin reported: a plugin with its UI folds it; other
    /// plugins' reports only reach the event log.
    pub fn report(&mut self, plugin: &str, body: &Value) {
        if crate::plugins::registry().get(plugin).is_some() {
            self.fold(plugin, body);
        }
    }
}

/// The tool that proposes a run's landing (ADR 0014).
const LAND: &str = "vcs_land";

/// Whether a result's `details.calls`, the calls a tool made through
/// the loop as it lists them (`plugins.md`, "In tau-ui"), hold a
/// `vcs_land` that succeeded.
fn lists_landing(details: &Value) -> bool {
    details
        .get("calls")
        .and_then(Value::as_array)
        .is_some_and(|calls| {
            calls.iter().any(|call| {
                call.get("name").and_then(Value::as_str) == Some(LAND)
                    && call.get("status").and_then(Value::as_str) == Some("ok")
            })
        })
}

fn finish_tool(card: &mut ToolCard, output: &ToolOutput, is_error: bool) {
    let text = output.text_content();
    card.size = card.args().to_string().len() + text.len();
    let reported = output
        .details
        .as_ref()
        .and_then(|details| details.get("summary"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let data = std::sync::Arc::make_mut(&mut card.data);
    data.result = Some(CallResult {
        text: text.clone(),
        details: output.details.clone(),
        error: is_error,
    });
    // What its nested calls left is in its result now, as in a stored
    // run.
    data.end();
    // The tool's plugin says more of the result when it draws the card.
    card.state = if is_error {
        ToolState::Failed(first_line(&text))
    } else {
        // A tool may say how to sum up its result ("5 matches · 9 ms").
        ToolState::Done {
            summary: reported.or_else(|| line_count(&text)),
        }
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

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().to_owned()
}

fn line_count(text: &str) -> Option<String> {
    match text.lines().count() {
        0 => None,
        1 => Some("1 line".into()),
        count => Some(format!("{count} lines")),
    }
}

pub use tau_ui_kit::format::{clock, fine_usd, grouped, tokens, usd};

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;
    use tau_ai::message::{Usage, UsageCost};
    use tau_fast_compaction::ui::Decision;

    use super::*;

    fn run() -> RunId {
        RunId(Arc::from("r1"))
    }

    fn view() -> RunView {
        RunView::new(run(), "retry-after", "coder", "gpt-5.5")
    }

    /// What plugins cost a run is what they charged, whatever they
    /// report: a check's own `cost` in a report is the plugin's figure,
    /// not a second charge.
    #[hegel::test(test_cases = 200)]
    fn plugin_cost_is_what_plugins_charged(tc: hegel::TestCase) {
        use hegel::generators::{self as gs, Generator as _};
        let mut view = view();
        let mut charged = 0.0;
        let steps = tc.draw(gs::vecs(hegel::tuples!(
            gs::booleans(),
            gs::sampled_from(vec![
                tau_goal::NAME,
                tau_constitution::NAME,
                tau_reasoning::NAME,
            ]),
            // Multiples of 1/1024, so sums are exact.
            gs::integers::<u32>()
                .max_value(1024)
                .map(|n| f64::from(n) / 1024.0),
        )));
        for (charge, plugin, cost) in steps {
            if charge {
                let mut usage = Usage::default();
                usage.cost.total = cost;
                charged += cost;
                view.apply(&RunEvent::PluginCharged {
                    run: run(),
                    plugin: plugin.into(),
                    usage,
                });
            } else {
                // A report that carries a cost of its own.
                view.apply(&RunEvent::PluginReport {
                    run: run(),
                    plugin: plugin.into(),
                    body: serde_json::json!({
                        "kind": "checked", "scores": [], "cost": cost,
                    }),
                });
            }
        }
        assert_eq!(view.usage.plugin_cost, charged);
    }

    /// fast-compaction's report on a pruned `bash` output, as it
    /// reports and records it.
    fn output_report(pruned: bool) -> Value {
        json!({
            "kind": "output", "call_id": "c1", "lines": 4810, "chunks": 200,
            "kept": 9, "dropped_lines": 4598, "segments": 1, "requests": 2,
            "tokens_before": 12000, "tokens_after": 900, "pruned": pruned,
            "archive": pruned.then_some("/data/tau/repos/app/archive/tau-output-1.txt")
        })
    }

    /// tau-constitution's state in `view`, as its fold leaves it.
    fn rules_of(view: &RunView) -> tau_constitution::ui::State {
        view.plugin_states
            .get(tau_constitution::NAME)
            .map(|state| serde_json::from_value(state.json().clone()).unwrap())
            .unwrap_or_default()
    }

    /// fast-compaction's state in `view`, as its fold leaves it.
    fn pruning(view: &RunView) -> tau_fast_compaction::ui::State {
        view.plugin_states
            .get(tau_fast_compaction::NAME)
            .map(|state| serde_json::from_value(state.json().clone()).unwrap())
            .unwrap_or_default()
    }

    #[test]
    fn a_pruned_output_shows_on_its_card() {
        let mut view = view();
        view.apply(&RunEvent::ToolStart {
            run: run(),
            call_id: "c1".into(),
            tool: "bash".into(),
            args: json!({"command": "cargo build"}),
            parent: None,
        });
        view.apply(&RunEvent::PluginReport {
            run: run(),
            plugin: tau_fast_compaction::NAME.into(),
            body: output_report(true),
        });
        view.apply(&RunEvent::ToolEnd {
            run: run(),
            call_id: "c1".into(),
            output: Arc::new(ToolOutput::text("pruned")),
            is_error: false,
            parent: None,
        });
        let Some(Item::Tool(card)) = view.items.last() else {
            panic!("a card")
        };
        let cut = card.cut.clone().unwrap();
        assert_eq!(cut.label(), "kept 212 of 4,810 lines · 12k → 900 tokens");
        assert_eq!(cut.archive, "/data/tau/repos/app/archive/tau-output-1.txt");
        // Neither the ledger nor the card's context changed.
        assert!(pruning(&view).ledger.is_empty());
        assert_eq!(card.dropped, None);

        // Jev looked, but the result stayed: nothing to say on the card.
        let mut view = self::view();
        view.apply(&RunEvent::ToolStart {
            run: run(),
            call_id: "c1".into(),
            tool: "bash".into(),
            args: json!({"command": "cargo build"}),
            parent: None,
        });
        view.apply(&RunEvent::PluginReport {
            run: run(),
            plugin: tau_fast_compaction::NAME.into(),
            body: output_report(false),
        });
        let Some(Item::Tool(card)) = view.items.last() else {
            panic!("a card")
        };
        assert_eq!(card.cut, None);
    }

    #[test]
    fn a_pruned_output_shows_on_its_card_from_history() {
        let message = |value: Value| -> Message {
            serde_json::from_value(value).unwrap()
        };
        let usage = json!({"input": 0, "output": 0, "cacheRead": 0,
            "cacheWrite": 0, "totalTokens": 0,
            "cost": {"input": 0, "output": 0, "cacheRead": 0,
                     "cacheWrite": 0, "total": 0}});
        let assistant = |content: Value, stop: &str| {
            message(json!({
                "role": "assistant", "content": content, "api": "responses",
                "provider": "openai", "model": "m", "usage": usage,
                "stopReason": stop, "timestamp": 0
            }))
        };
        // The record is written as the tool runs, before the turn's
        // messages.
        let view = RunView::from_timeline(
            run(),
            "t",
            "a",
            "gpt-6-sol",
            &[
                Stored::Message(message(json!({
                    "role": "user", "content": "fix the build", "timestamp": 0
                }))),
                Stored::Record {
                    plugin: tau_fast_compaction::NAME.into(),
                    body: output_report(true),
                },
                Stored::Message(assistant(
                    json!([{"type": "toolCall", "id": "c1", "name": "bash",
                            "arguments": {"command": "cargo build"}}]),
                    "toolUse",
                )),
                Stored::Message(message(json!({
                    "role": "toolResult", "toolCallId": "c1",
                    "toolName": "bash",
                    "content": [{"type": "text", "text": "pruned"}],
                    "isError": false, "timestamp": 0
                }))),
                Stored::Message(assistant(
                    json!([{"type": "text", "text": "done"}]),
                    "stop",
                )),
            ],
        );
        let cut = view
            .items
            .iter()
            .find_map(|item| match item {
                Item::Tool(card) => card.cut.clone(),
                _ => None,
            })
            .expect("the card says what was cut");
        assert_eq!(cut.label(), "kept 212 of 4,810 lines · 12k → 900 tokens");
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
                parent: None,
            });
            view.apply(&RunEvent::ToolEnd {
                run: run(),
                call_id: call.into(),
                output: Arc::new(ToolOutput::text("x".repeat(4000))),
                is_error: false,
                parent: None,
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
        let state = pruning(&view);
        assert_eq!(state.ledger.len(), 2);
        let first = &state.ledger[0];
        assert_eq!((first.turn, first.decision), (1, Decision::DropResult));
        assert!(first.tokens > 900, "{}", first.tokens);
        assert_eq!(first.matters, Some(0.9));
        assert_eq!(state.ledger[1].decision, Decision::Pinned);
        assert_eq!(state.ledger[1].turn, 2);
        let cards: Vec<(Option<Dropped>, usize)> = view
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Tool(card) => Some((card.dropped, card.anchors.len())),
                _ => None,
            })
            .collect();
        // Both cards carry the pass's mark; the dropped one is out.
        assert_eq!(cards, [(Some(Dropped::Result), 1), (None, 1)]);
        let Some(Item::Rewrite {
            key: Some(key),
            tokens,
            ..
        }) = view.items.last()
        else {
            panic!("a rewrite, named")
        };
        assert_eq!(*tokens, Some((2000, 1100)));
        let detail = &state.passes[key].detail;
        assert!(detail.contains("1 result cut, 0 calls dropped"), "{detail}");
        assert_eq!(state.status().as_deref(), Some("1 pruned · −46%"));
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
            parent: None,
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
            parent: None,
        });
        let Some(Item::Tool(card)) = view.items.last() else {
            panic!("a card")
        };
        // Running to its end does not clear the flag.
        assert_eq!(
            card.state,
            ToolState::Flagged {
                plugin: tau_constitution::NAME.into(),
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
            .filter(|item| {
                matches!(item, Item::Plugin(_) | Item::Anchor { .. })
            })
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
        let state = rules_of(&view);
        assert_eq!(
            state.calls["c1"].scores,
            [("R2".to_owned(), 0.55), ("R4".to_owned(), 0.02)]
        );
        let stats = &state.stats;
        assert_eq!((stats.calls, stats.answers, stats.questions), (1, 1, 3));
        assert_eq!(stats.flagged, ["R2"]);
        assert_eq!(stats.held, ["R6"]);
        assert!((stats.cost - 0.00004).abs() < 1e-12);
        // The run's plugin list says so.
        assert_eq!(state.status().unwrap().0, "1 flagged · 1 held");
        // Other plugins' reports only reach the log.
        let before = view.items.len();
        view.apply(&RunEvent::PluginReport {
            run: run(),
            plugin: "other".into(),
            body: serde_json::json!({"kind": "blocked"}),
        });
        assert_eq!(view.items.len(), before);
    }

    /// A call a tool makes through the loop, at any depth, makes no card:
    /// whatever its events, the transcript holds the model's calls only,
    /// in order, and each nested call shows on the card of the model's
    /// call it came from while that call runs, as it went.
    #[hegel::test(test_cases = 200)]
    fn nested_calls_make_no_cards(tc: hegel::TestCase) {
        use hegel::generators as gs;
        // (top-level card it falls under, depth 1 or 2, error?)
        let calls: Vec<(u8, bool, bool)> = tc.draw(
            gs::vecs(hegel::tuples!(
                gs::integers::<u8>().max_value(2),
                gs::booleans(),
                gs::booleans(),
            ))
            .max_size(12),
        );
        let ended: bool = tc.draw(gs::booleans());
        let mut view = view();
        for top in 0..3 {
            view.apply(&RunEvent::ToolStart {
                run: run(),
                call_id: format!("c{top}"),
                tool: "codemode".into(),
                args: json!({"code": "return 1"}),
                parent: None,
            });
        }
        let mut expected: Vec<Vec<(String, String, Option<bool>)>> =
            vec![Vec::new(); 3];
        for (n, (top, deep, error)) in calls.iter().enumerate() {
            let top = format!("c{top}");
            let parent = match (
                deep,
                expected[top[1..].parse::<usize>().unwrap()].first(),
            ) {
                (true, Some((first, _, _))) => first.clone(),
                _ => top.clone(),
            };
            let id = format!("{parent}/{n}");
            view.apply(&RunEvent::ToolStart {
                run: run(),
                call_id: id.clone(),
                tool: "read".into(),
                args: json!({"path": format!("f{n}")}),
                parent: Some(parent.clone()),
            });
            view.apply(&RunEvent::ToolUpdate {
                run: run(),
                call_id: id.clone(),
                partial: Arc::new(ToolOutput::text("so far")),
                parent: Some(parent.clone()),
            });
            if n % 2 == 0 {
                view.apply(&RunEvent::ToolEnd {
                    run: run(),
                    call_id: id.clone(),
                    output: Arc::new(ToolOutput::text(format!("out {n}"))),
                    is_error: *error,
                    parent: Some(parent.clone()),
                });
            }
            let i: usize = top[1..].parse().unwrap();
            expected[i].push((id, parent, (n % 2 == 0).then_some(*error)));
        }
        if ended {
            view.apply(&RunEvent::ToolEnd {
                run: run(),
                call_id: "c0".into(),
                output: Arc::new(ToolOutput::text("done")),
                is_error: false,
                parent: None,
            });
        }
        let cards: Vec<&ToolCard> = view
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Tool(card) => Some(&**card),
                _ => None,
            })
            .collect();
        let ids: Vec<&str> =
            cards.iter().map(|card| card.call_id.as_str()).collect();
        assert_eq!(ids, ["c0", "c1", "c2"]);
        for (i, card) in cards.iter().enumerate() {
            let nested: Vec<(String, String, Option<bool>)> = card
                .data
                .nested
                .iter()
                .map(|call| {
                    assert_eq!(call.updates, 1);
                    assert_eq!(call.partial.as_deref(), Some("so far"));
                    (
                        call.id.clone(),
                        call.parent.clone(),
                        call.result.as_ref().map(|result| result.error),
                    )
                })
                .collect();
            if ended && i == 0 {
                // Its result is the record now, as in a stored run.
                assert!(nested.is_empty());
                assert_eq!(
                    card.state,
                    ToolState::Done {
                        summary: Some("1 line".into())
                    }
                );
            } else {
                assert_eq!(nested, expected[i]);
                assert_eq!(card.state, ToolState::Running);
            }
        }
        // The Events tab lists them apart from the model's calls.
        let tool_starts = view
            .log
            .iter()
            .filter(|line| line.kind == "ToolStart")
            .count();
        let nested_starts = view
            .log
            .iter()
            .filter(|line| line.kind == "NestedStart")
            .count();
        assert_eq!((tool_starts, nested_starts), (3, calls.len()));
    }

    /// Codemode's store writes fold into its state alike live and from
    /// a stored run's records.
    #[test]
    fn codemode_store_folds_live_and_from_history() {
        let record = json!({ "kind": "store", "set": { "a": 1, "b": [2] }, "delete": [] });
        let deleted = json!({ "kind": "store", "set": {}, "delete": ["a"] });
        let mut live = view();
        for body in [&record, &deleted] {
            live.apply(&RunEvent::PluginReport {
                run: run(),
                plugin: tau_codemode::PLUGIN.into(),
                body: body.clone(),
            });
        }
        let stored = RunView::from_timeline(
            run(),
            "retry-after",
            "coder",
            "gpt-5.5",
            &[record, deleted].map(|body| Stored::Record {
                plugin: tau_codemode::PLUGIN.into(),
                body,
            }),
        );
        let state = |view: &RunView| -> tau_codemode::ui::State {
            serde_json::from_value(
                view.plugin_states[tau_codemode::PLUGIN].json().clone(),
            )
            .unwrap()
        };
        assert_eq!(state(&live), state(&stored));
        assert_eq!(
            state(&live).store,
            [("b".to_owned(), json!([2]))].into_iter().collect()
        );
    }

    /// A verdict folded afresh, as a fork inherits it, still names the
    /// plugin that blocked the call, and places no anchor.
    #[test]
    fn a_restated_verdict_names_its_plugin() {
        let mut view = view();
        view.apply(&RunEvent::ToolStart {
            run: run(),
            call_id: "c1".into(),
            tool: "bash".into(),
            args: json!({"command": "rm -rf /"}),
            parent: None,
        });
        let anchors = view.items.len();
        view.restate(
            tau_constitution::NAME,
            &[json!({
                "kind": "blocked", "rule": "R1", "text": "No rm.",
                "score": 0.95, "call_id": "c1", "tool": "bash",
                "reason": "rule R1"
            })],
        );
        let card = view.tool("c1").unwrap();
        assert!(matches!(
            &card.state,
            ToolState::Blocked { plugin, .. } if plugin == tau_constitution::NAME
        ));
        assert_eq!(view.items.len(), anchors);
        assert!(card.anchors.is_empty());
    }

    /// A plugin's verdict on a nested call marks its row on the card it
    /// shows on, not the card: the script may catch the refusal. Its
    /// anchor lands on that card; a context rewrite cannot drop it.
    #[test]
    fn a_nested_calls_verdict_marks_its_row() {
        let mut view = view();
        view.apply(&RunEvent::ToolStart {
            run: run(),
            call_id: "c1".into(),
            tool: "codemode".into(),
            args: json!({"code": "tools.bash({ command = 'rm -rf /' })"}),
            parent: None,
        });
        view.apply(&RunEvent::ToolStart {
            run: run(),
            call_id: "c1/1".into(),
            tool: "bash".into(),
            args: json!({"command": "rm -rf /"}),
            parent: Some("c1".into()),
        });
        view.apply(&RunEvent::PluginReport {
            run: run(),
            plugin: tau_constitution::NAME.into(),
            body: json!({
                "kind": "blocked", "rule": "R1", "text": "No rm.",
                "score": 0.95, "call_id": "c1/1", "tool": "bash",
                "reason": "rule R1"
            }),
        });
        let card = view.tool("c1").unwrap();
        assert_eq!(card.state, ToolState::Running);
        let marks: Vec<_> = card.data.marks_of("c1/1").collect();
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].plugin, tau_constitution::NAME);
        assert!(matches!(marks[0].mark, CardMark::Blocked { .. }));
        assert!(
            card.anchors
                .contains(&(tau_constitution::NAME.into(), "c1/1".into()))
        );
        {
            use tau_ui_plugin::RunCx as _;
            let mut folding = Folding {
                view: &mut view,
                plugin: tau_fast_compaction::NAME,
                anchors: true,
            };
            assert!(!folding.dropped("c1/1", Dropped::Result));
        }
        assert_eq!(view.tool("c1").unwrap().dropped, None);
        // The script ends: the mark stays, as history places it again.
        view.apply(&RunEvent::ToolEnd {
            run: run(),
            call_id: "c1".into(),
            output: Arc::new(ToolOutput::text("Script completed")),
            is_error: false,
            parent: None,
        });
        let card = view.tool("c1").unwrap();
        assert!(card.data.nested.is_empty());
        assert_eq!(card.data.marks_of("c1/1").count(), 1);
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
            parent: None,
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
            parent: None,
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
            parent: None,
        });
        view.apply(&RunEvent::ToolEnd {
            run: run(),
            call_id: "c1".into(),
            output: Arc::new(ToolOutput {
                details: Some(json!({
                    "diff": "--- a\n+++ b\n@@ -1 +1,2 @@\n-old\n+new\n+more\n",
                })),
                ..ToolOutput::text("Successfully replaced 1 block(s).")
            }),
            is_error: false,
            parent: None,
        });
        let card = view.tool("c1").expect("card");
        assert_eq!(card.summary, "retry.rs");
        assert!(matches!(card.state, ToolState::Done { .. }));
        // tau-tools draws the diff the result carries.
        let lines = tau_tools::ui::diff_of(&card.data).expect("a diff");
        assert_eq!(tau_ui_kit::diff::stat(&lines), "+2 −1");
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
            parent: None,
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
            parent: None,
        });
        let card = view.tool("c1").expect("card");
        // tau-vcs reads the log the result carries.
        let details = card.data.result.as_ref().unwrap().details.as_ref();
        let log = tau_vcs::ui::change_log::ChangeLog::parse(details.unwrap())
            .expect("a log");
        assert_eq!(log.summary(), "1 on the stack · 1 on trunk");
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
                parent: None,
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
                parent: None,
            });
        }
        // tau-vcs reads a show as a commit, a diff as its files.
        let parsed = |id: &str| {
            let card = view.tool(id).expect("card");
            let details = card.data.result.as_ref().unwrap().details.clone();
            tau_vcs::ui::change_diff::ChangeDiff::parse(&details.unwrap())
                .expect("a diff")
        };
        let commit = parsed("c1");
        assert_eq!(commit.stat(), "+2 −1");
        assert_eq!(commit.change.subject, "honor retry-after");
        assert_eq!(parsed("c2").files[0].hunks[0].lines.len(), 3);
    }

    #[test]
    fn events_of_other_runs_only_touch_children() {
        let mut view = view();
        let child = RunId(Arc::from("r2"));
        view.apply(&RunEvent::RunStart {
            run: child.clone(),
            parent: Some(run()),
            agent: Arc::from("reviewer"),
            call: None,
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
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(212), "212");
        assert_eq!(grouped(4_810), "4,810");
        assert_eq!(grouped(1_234_567), "1,234,567");
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

    /// A stored run that fast-compaction rewrote starts with its
    /// ledger: the rewrite shows with what it saved and cost, and the
    /// ledger lists each call, the dropped ones too, marking the cards
    /// that are left.
    #[test]
    fn a_stored_rewrite_brings_its_ledger_back() {
        let message = |value: Value| -> Message {
            serde_json::from_value(value).unwrap()
        };
        let usage = json!({"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0,
                           "totalTokens": 0, "cost": {"input": 0, "output": 0,
                           "cacheRead": 0, "cacheWrite": 0, "total": 0}});
        let call = |id: &str| {
            message(json!({
                "role": "assistant",
                "content": [{"type": "toolCall", "id": id, "name": "read",
                             "arguments": {"path": "a.rs"}}],
                "api": "responses", "provider": "openai", "model": "m",
                "usage": usage, "stopReason": "toolUse", "timestamp": 0
            }))
        };
        let result = |id: &str| {
            message(json!({
                "role": "toolResult", "toolCallId": id, "toolName": "read",
                "content": [{"type": "text", "text": "cut"}],
                "isError": false, "timestamp": 0
            }))
        };
        let decision = |id: &str, action: &str| {
            json!({"call_id": id, "tool": "read", "action": action,
                   "keep_call": 0.6, "keep_result": 0.1})
        };
        let ledger = json!({
            "decisions": [decision("t1", "drop_call"), decision("t2", "drop_result")],
            "stats": {
                "calls": 3, "pinned": 1, "kept": 0, "results_dropped": 1,
                "calls_dropped": 1, "requests": 1, "state_tokens": 2_000,
                "state_stage": "whole", "chars_before": 40_000,
                "chars_after": 8_000, "reduction_ratio": 0.8, "cost": 0.0002,
            },
        });
        let view = RunView::from_timeline(
            run(),
            "t",
            "a",
            "gpt-6-sol",
            &[
                Stored::Rewrite {
                    plugin: tau_fast_compaction::NAME.into(),
                    body: ledger,
                    tokens: None,
                },
                Stored::Message(call("t2")),
                Stored::Message(result("t2")),
                Stored::Message(call("t3")),
                Stored::Message(result("t3")),
            ],
        );
        let Some(Item::Rewrite {
            plugin,
            tokens: None,
            key: Some(key),
        }) = view.items.first()
        else {
            panic!("the rewrite first, named: {:?}", view.items.first())
        };
        assert_eq!(plugin, tau_fast_compaction::NAME);
        let state = pruning(&view);
        let pass = &state.passes[key];
        assert_eq!((pass.before, pass.after), (10_000, 2_000));
        assert!(
            pass.detail.ends_with(", whole · $0.0002"),
            "{}",
            pass.detail
        );
        let decisions: Vec<(&str, Decision)> = state
            .ledger
            .iter()
            .map(|entry| (entry.call_id.as_str(), entry.decision))
            .collect();
        assert_eq!(
            decisions,
            [
                ("t1", Decision::DropCall),
                ("t2", Decision::DropResult),
                ("t3", Decision::Pinned),
            ]
        );
        assert_eq!(
            view.tool("t2").and_then(|card| card.dropped),
            Some(Dropped::Result)
        );
    }

    /// Checks Jev could not answer come back from history, saying what
    /// `on_error` did, and count as not checked.
    #[test]
    fn failed_checks_show_from_history() {
        let record = |body: Value| Stored::Record {
            plugin: tau_constitution::NAME.into(),
            body,
        };
        let view = RunView::from_timeline(
            run(),
            "t",
            "a",
            "gpt-6-sol",
            &[
                record(json!({
                    "kind": "error", "call_id": "c1", "tool": "write",
                    "message": "Jev could not check the call: offline",
                    "on_error": "block",
                })),
                record(json!({
                    "kind": "error",
                    "message": "Jev could not check the final answer: offline",
                    "on_error": "block", "held": true,
                })),
                record(json!({
                    "kind": "error",
                    "message": "Jev could not check the final answer: offline",
                    "on_error": "allow", "held": false,
                })),
            ],
        );
        let state = rules_of(&view);
        let details: Vec<&str> = view
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Anchor { key, .. } => {
                    Some(state.notes[key].detail.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            details,
            [
                "not checked · blocked",
                "not checked · sent back",
                "not checked · the answer stands"
            ]
        );
        assert_eq!(state.stats.failed, 3);
    }

    #[test]
    fn context_parts_split_the_models_count() {
        let mut view = RunView::new(run(), "t", "a", "gpt-6-sol");
        view.items.push(Item::User("x".repeat(400)));
        view.items.push(Item::Text("y".repeat(400)));
        let mut card = ToolCard {
            call_id: "c1".into(),
            tool: "read".into(),
            summary: String::new(),
            state: ToolState::Done { summary: None },
            data: Arc::new(CallData {
                args: json!({"path": "a.rs"}),
                ..CallData::default()
            }),
            from_plugin: None,
            dropped: None,
            cut: None,
            anchors: Vec::new(),
            size: 0,
        };
        let args = card.args().to_string().len();
        card.size = args + 4_000;
        view.items.push(Item::Tool(Box::new(card.clone())));
        view.context.used = 5_000;
        let parts = view.context_parts().unwrap();
        assert_eq!(parts.results, 1_000);
        assert_eq!(parts.conversation, 200 + args.div_ceil(4) as u64);
        assert_eq!(parts.fixed + parts.conversation + parts.results, 5_000);

        // A dropped result counts only its call.
        card.dropped = Some(Dropped::Result);
        view.items[2] = Item::Tool(Box::new(card));
        assert_eq!(view.context_parts().unwrap().results, 0);

        // A summary replaced what came before it; an estimate over the
        // count is scaled down to it.
        view.items.push(Item::Rewrite {
            plugin: tau_compaction::NAME.into(),
            tokens: Some((5_000, 100)),
            key: None,
        });
        view.items.push(Item::Text("z".repeat(800)));
        view.context.used = 100;
        let parts = view.context_parts().unwrap();
        assert_eq!((parts.fixed, parts.conversation), (0, 100));
    }
}
