//! Run events: what a run reports while it works
//! (`docs/reference/agent-loop.md`, "Events").
//!
//! A run's events follow the grammar
//! `RunStart (TurnStart … TurnEnd)* RunEnd`, and nothing follows
//! `RunEnd`. Events from a child run carry `parent`, so one subscriber can
//! follow a whole workflow tree.

use std::{sync::Arc, time::Duration};

use serde_json::Value;
use tau_ai::message::{StopReason as MessageStop, Usage};

use crate::tool::{RunId, ToolOutput};

/// Why a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    /// The model finished.
    Stop,
    /// A limit was reached.
    Limit(LimitKind),
    Cancelled,
    /// The model's last response failed, or the loop could not continue.
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LimitKind {
    Turns,
    Tokens,
    Usd,
    Time,
}

impl StopReason {
    /// The run's stop reason for a model response that ended the run.
    pub fn from_message(stop: MessageStop, error: Option<&str>) -> Self {
        match stop {
            MessageStop::Aborted => Self::Cancelled,
            MessageStop::Error => {
                Self::Error(error.unwrap_or("model error").to_owned())
            }
            _ => Self::Stop,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RunEvent {
    RunStart {
        run: RunId,
        parent: Option<RunId>,
        agent: Arc<str>,
    },
    TurnStart {
        run: RunId,
        turn: u32,
    },
    TextDelta {
        run: RunId,
        parent: Option<RunId>,
        delta: String,
    },
    ThinkingDelta {
        run: RunId,
        delta: String,
    },
    ToolCallDelta {
        run: RunId,
        call_id: String,
        json_fragment: String,
    },
    ToolStart {
        run: RunId,
        call_id: String,
        tool: Arc<str>,
        args: Value,
    },
    ToolUpdate {
        run: RunId,
        call_id: String,
        partial: Arc<ToolOutput>,
    },
    ToolEnd {
        run: RunId,
        call_id: String,
        output: Arc<ToolOutput>,
        is_error: bool,
    },
    TurnEnd {
        run: RunId,
        turn: u32,
        usage: Usage,
    },
    /// A plugin rewrote the context; the next request goes in full.
    ContextRewritten {
        run: RunId,
        plugin: Arc<str>,
        tokens_before: u64,
        tokens_after: u64,
    },
    /// A response failed in a way worth retrying; attempt `attempt`
    /// starts after `delay`. Comes inside the turn it retries.
    Retry {
        run: RunId,
        turn: u32,
        attempt: u32,
        delay: Duration,
        error: String,
    },
    /// A plugin kept the run from stopping: `message` goes to the model
    /// as the user's, and another turn starts. Comes after the turn's
    /// `TurnEnd`.
    Continued {
        run: RunId,
        plugin: Arc<str>,
        message: String,
    },
    /// What a plugin decided, for interfaces to show and review: a
    /// blocked call, a flagged one, a held stop. `body` is the plugin's
    /// own JSON. Comes before the event it explains, such as the
    /// `ToolEnd` of a call it blocked.
    PluginReport {
        run: RunId,
        plugin: Arc<str>,
        body: Value,
    },
    /// A plugin failed at a seam where failing does not end the run.
    PluginError {
        run: RunId,
        plugin: Arc<str>,
        message: String,
    },
    RunEnd {
        run: RunId,
        parent: Option<RunId>,
        stop: StopReason,
        cost: f64,
    },
}

impl RunEvent {
    /// The run the event is about.
    pub fn run(&self) -> &RunId {
        match self {
            Self::RunStart { run, .. }
            | Self::TurnStart { run, .. }
            | Self::TextDelta { run, .. }
            | Self::ThinkingDelta { run, .. }
            | Self::ToolCallDelta { run, .. }
            | Self::ToolStart { run, .. }
            | Self::ToolUpdate { run, .. }
            | Self::ToolEnd { run, .. }
            | Self::TurnEnd { run, .. }
            | Self::ContextRewritten { run, .. }
            | Self::Retry { run, .. }
            | Self::Continued { run, .. }
            | Self::PluginReport { run, .. }
            | Self::PluginError { run, .. }
            | Self::RunEnd { run, .. } => run,
        }
    }
}
