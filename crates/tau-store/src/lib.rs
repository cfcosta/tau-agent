//! Storage for runs, messages and fork transcripts: what the agent loop
//! and plugins keep and read back, behind the [`Backend`] a host picks.
//!
//! See `docs/reference/storage.md`. The desktop's backend is SQLite
//! (`tau-store-sqlite`); this crate holds only the shapes and the
//! interface, so what uses a store builds without a database driver.
//!
//! Message bodies are opaque JSON here: a store keeps what the agent loop
//! gives it and knows nothing about message shapes beyond their role.

use std::{error::Error, fmt::Debug, sync::Arc};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Database(Box<dyn Error + Send + Sync>),
    #[error("stored JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unknown run {0}")]
    UnknownRun(String),
    #[error("run {0} is still running")]
    StillRunning(String),
    #[error("retained run history is malformed: {0}")]
    CorruptHistory(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// A store, shared: every clone reads and writes the same runs.
pub type Store = Arc<dyn Backend>;

/// How a run relates to other runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunKind {
    Root,
    /// Inherits `parent`'s messages with `seq <= fork_seq`.
    Fork {
        parent: String,
        fork_seq: i64,
    },
    /// Started by `parent` as a tool. With a `fork_seq`, it inherits
    /// `parent`'s messages up to it, as a fork does; without, nothing.
    Subagent {
        parent: String,
        fork_seq: Option<i64>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRun<'a> {
    pub id: &'a str,
    pub workflow_id: Option<&'a str>,
    pub agent: &'a str,
    pub kind: RunKind,
    pub model: &'a str,
    /// Turns the run starts after: a fork's, inherited from its parent.
    pub turns: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    Running,
    Done,
    Failed,
    Cancelled,
    Limit,
    /// The process that ran it closed while it ran: it was left
    /// `running`, and [`Backend::interrupt_running`] said so.
    Interrupted,
}

/// One transcript entry. Its body is JSON text, which the store keeps
/// as given: callers serialize straight to text and parse straight from
/// it, with no `Value` in between.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    Message {
        role: String,
        body: String,
    },
    /// A context rewrite by `plugin`, with its details in `body`. The
    /// transcript it leaves is `layout`, in order: `Some(i)` keeps the
    /// message at index `i` of the transcript before it, by reference;
    /// `None` takes the next message stored after the rewrite, one it
    /// made or changed. Messages stored after those come after them.
    Context {
        plugin: String,
        body: String,
        layout: Vec<Option<usize>>,
        /// What the rewrite did to the context. Every rewrite the loop
        /// stores has it; `None` only for a row from before it was kept.
        stats: Option<RewriteStats>,
    },
    /// A record `plugin` keeps with the run. It is never part of the
    /// transcript; [`Backend::records`] reads it back, and
    /// [`Backend::timeline`] shows it in place.
    Plugin {
        plugin: String,
        body: String,
    },
}

/// What a context rewrite did, for measuring it: the loop's token
/// estimate of the context before and after, and what triggered it
/// (`turn_end`, `start` or `overflow`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewriteStats {
    pub tokens_before: u64,
    pub tokens_after: u64,
    pub trigger: String,
}

/// Token and cost totals added by one turn.
///
/// Token counts are unsigned and 32-bit: a turn never has negative or
/// billions of tokens, and the run totals, stored as SQLite's 64-bit
/// INTEGER, cannot overflow from any realistic number of turns.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TurnUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cost_usd: f64,
    /// Model turns the write ends: 1 for a turn's reply and results, 0
    /// for anything else.
    pub turns: u32,
}

/// A run as stored.
#[derive(Debug, Clone, PartialEq)]
pub struct RunRecord {
    pub id: String,
    pub workflow_id: Option<String>,
    pub agent: String,
    pub kind: RunKind,
    pub model: String,
    pub status: Status,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
    /// Model turns so far, across every time the run was resumed.
    pub turns: i64,
    pub result: Option<String>,
    pub error: Option<String>,
    /// A model's short name for the run, once one is written.
    pub title: Option<String>,
    /// When the run started, as SQLite's `strftime` writes it
    /// (`2026-09-28T14:03:11.402Z`).
    pub created_at: String,
}

/// What one plugin charged to a run.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginCost {
    pub plugin: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
}

/// The cost of one agent's runs in a workflow.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentCost {
    pub agent: String,
    pub runs: i64,
    pub usd: f64,
}

/// What a query returned, as text.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize,
)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// There were more rows than the limit.
    pub truncated: bool,
}

/// What a store does. Its methods are what a [`Store`] answers.
#[async_trait]
pub trait Backend: Debug + Send + Sync {
    /// Runs a query someone typed, such as History's query box, and
    /// returns at most `limit` rows, every value as text. The query only
    /// reads: the store refuses anything that would write. It is SQL from
    /// a person, not from tau's code.
    async fn query(&self, sql: &str, limit: usize) -> Result<Table>;

    /// Records a new run with status `running`.
    async fn create_run(&self, run: &NewRun<'_>) -> Result<()>;

    /// Appends one turn's entries and adds its usage to the run's totals,
    /// in one write transaction: either all of it is stored or none.
    /// Returns the `seq` of the run's last entry afterwards, or -1 when
    /// the run has none.
    async fn append_turn(
        &self,
        run: &str,
        entries: &[Entry],
        usage: TurnUsage,
    ) -> Result<i64> {
        self.append_charged(run, entries, usage, &[]).await
    }

    /// [`Backend::append_turn`], with what each plugin charged among
    /// `usage`: added to the run's per-plugin costs in the same
    /// transaction. `usage` already counts it; `plugins` only says whose
    /// it is.
    async fn append_charged(
        &self,
        run: &str,
        entries: &[Entry],
        usage: TurnUsage,
        plugins: &[(&str, TurnUsage)],
    ) -> Result<i64>;

    /// What each plugin charged to the run itself, by plugin name.
    async fn plugin_costs(&self, run: &str) -> Result<Vec<PluginCost>>;

    /// Each plugin's cost over the runs started at `since` or later (a
    /// time as [`RunRecord::created_at`] writes it), by plugin name.
    async fn plugin_spend(&self, since: &str) -> Result<Vec<(String, f64)>>;

    /// The run's transcript: the inherited messages of its fork chain,
    /// then its own, as its context rewrites left them. It starts with
    /// the latest rewrite, if any, then the messages that rewrite kept
    /// or made, then the ones stored after. Plugin records are not part
    /// of it.
    async fn transcript(&self, run: &str) -> Result<Vec<Entry>>;

    /// The body of the run's own first user message, not one it
    /// inherited: what a fork was started with.
    async fn first_prompt(&self, run: &str) -> Result<Option<String>>;

    /// Everything the run's fork chain holds, in the order it was
    /// written, plugin records included: what an interface needs to show
    /// a stored run as it happened. Each context rewrite is in place,
    /// without the messages it made or changed, which nobody saw happen.
    async fn timeline(&self, run: &str) -> Result<Vec<Entry>>;

    /// The bodies of `plugin`'s records along the run's fork chain, oldest
    /// first: the inherited ones, then the run's own.
    async fn records(&self, run: &str, plugin: &str) -> Result<Vec<String>>;

    /// Sets a run's final status and result or error.
    async fn finish_run(
        &self,
        run: &str,
        status: Status,
        result: Option<&str>,
        error: Option<&str>,
    ) -> Result<()>;

    /// Marks every run still `running` as `interrupted`, and returns
    /// their ids, sorted: what a process finds as it opens a store the
    /// last one left, whose runs it no longer runs. Call it before any
    /// run starts. When they were last active stays as it was.
    async fn interrupt_running(&self) -> Result<Vec<String>>;

    /// Names the run `title`, in place of any name it had.
    async fn set_title(&self, run: &str, title: &str) -> Result<()>;

    /// Opens a finished run again, to go on from where it stopped on
    /// `model`: it is `running` again, without its result or error, and
    /// keeps its transcript, usage and turn count. Returns it as it now
    /// is.
    async fn reopen_run(&self, run: &str, model: &str) -> Result<RunRecord>;

    /// The sub-agents `parent` called, oldest first.
    /// [`Backend::recent_runs`] leaves them out: they belong under their
    /// parent.
    async fn subagents(&self, parent: &str) -> Result<Vec<RunRecord>>;

    /// The run `run`, if the store has it.
    async fn run(&self, run: &str) -> Result<Option<RunRecord>>;

    /// The latest `limit` runs that are not sub-agents, the most
    /// recently active first: a resumed run comes back to the top.
    async fn recent_runs(&self, limit: u32) -> Result<Vec<RunRecord>>;

    /// Every persisted run, including subagents. Retention maintenance must
    /// never use the paginated sidebar history as its root inventory.
    async fn retained_runs(&self) -> Result<Vec<RunRecord>>;

    /// All original message bodies in one run, including those hidden by a
    /// later context rewrite. Callers must inspect them as opaque JSON.
    async fn retention_entries(&self, run: &str) -> Result<Vec<(i64, String)>>;

    /// `plugin`'s records in the run itself, not inherited, with the
    /// `seq` each was stored at, oldest first. A fork at a record's `seq`
    /// inherits that record and everything before it.
    async fn plugin_entries(
        &self,
        run: &str,
        plugin: &str,
    ) -> Result<Vec<(i64, String)>>;

    /// `plugin`'s records in every run, each in the run that stored it
    /// (not inherited), as `(run, body)`: by run, oldest first within
    /// one. For what a plugin did across a store's history.
    async fn plugin_entries_everywhere(
        &self,
        plugin: &str,
    ) -> Result<Vec<(String, String)>>;

    /// Runs and cost per agent in a workflow.
    async fn workflow_cost(&self, workflow: &str) -> Result<Vec<AgentCost>>;
}
