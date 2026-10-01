//! What a run cost and did, read from its events and outcome, and the
//! means over many runs, by arm and variant.

use std::{collections::BTreeMap, time::Duration};

use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use tau_agent::{
    agent::{AgentError, Outcome},
    event::{RunEvent, StopReason},
};
use tau_ai::message::Usage;

use crate::{arm::Arm, scenario::Variant};

/// One tool call a run made: the model's, or one a tool made through the
/// loop (a codemode script's), which names the call it came from.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Call {
    pub tool: String,
    pub args: Value,
    pub is_error: bool,
    /// The call that made it, for a nested call.
    pub parent: Option<String>,
}

/// Follows a run's events: its turns, its tool calls and their usage.
#[derive(Debug, Default)]
pub struct Meter {
    turns: u32,
    calls: Vec<(String, Call)>,
    usage: Usage,
}

impl Meter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn observe(&mut self, event: &RunEvent) {
        match event {
            RunEvent::TurnEnd { usage, .. } => {
                self.turns += 1;
                self.usage += usage;
            }
            RunEvent::ToolStart {
                call_id,
                tool,
                args,
                parent,
                ..
            } => self.calls.push((
                call_id.clone(),
                Call {
                    tool: tool.to_string(),
                    args: args.clone(),
                    is_error: false,
                    parent: parent.clone(),
                },
            )),
            RunEvent::ToolEnd {
                call_id, is_error, ..
            } => {
                if let Some((_, call)) =
                    self.calls.iter_mut().rev().find(|(id, _)| id == call_id)
                {
                    call.is_error = *is_error;
                }
            }
            _ => {}
        }
    }

    /// The calls seen so far, in order, nested ones included: a script
    /// that reads memory reads it as much as the model's own call does.
    pub fn calls(&self) -> Vec<Call> {
        self.calls.iter().map(|(_, call)| call.clone()).collect()
    }

    /// The run's figures. The outcome's usage counts what plugins asked
    /// the model for too (a consolidation pass); a run that failed
    /// without one falls back on its turns' usage.
    pub fn finish(
        &self,
        outcome: &Result<Outcome, AgentError>,
        wall: Duration,
    ) -> RunMetrics {
        let (usage, stop) = match outcome {
            Ok(outcome) => (outcome.usage.clone(), stop_name(&outcome.stop)),
            Err(error) => (self.usage.clone(), format!("error: {error}")),
        };
        let count = |nested: bool, failed: bool| {
            self.calls
                .iter()
                .filter(|(_, call)| call.parent.is_some() == nested)
                .filter(|(_, call)| !failed || call.is_error)
                .count() as u32
        };
        RunMetrics {
            success: None,
            turns: self.turns,
            tool_calls: count(false, false),
            failed_calls: count(false, true),
            nested_calls: count(true, false),
            input_tokens: usage.input,
            output_tokens: usage.output,
            cached_tokens: usage.cache_read,
            cost_usd: usage.cost.total,
            wall_ms: wall.as_millis() as u64,
            stop,
        }
    }
}

fn stop_name(stop: &StopReason) -> String {
    match stop {
        StopReason::Stop => "stop".into(),
        StopReason::Limit(kind) => format!("limit: {kind:?}").to_lowercase(),
        StopReason::Cancelled => "cancelled".into(),
        StopReason::Error(message) => format!("error: {message}"),
    }
}

/// What one run did and cost.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct RunMetrics {
    /// Whether its task's check passed, once checked.
    pub success: Option<bool>,
    /// Model responses.
    pub turns: u32,
    /// The model's tool calls.
    pub tool_calls: u32,
    /// The model's tool calls that returned an error.
    pub failed_calls: u32,
    /// Calls tools made through the loop (a codemode script's), apart
    /// from the model's.
    pub nested_calls: u32,
    /// Input tokens, as the provider counts them.
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Input tokens read from the provider's cache.
    pub cached_tokens: u64,
    pub cost_usd: f64,
    pub wall_ms: u64,
    /// How the run ended.
    pub stop: String,
}

/// One trial: a scenario's two runs under one arm.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Trial {
    pub scenario: String,
    pub variant: Variant,
    pub arm: Arm,
    /// Which repetition, from 0.
    pub trial: u32,
    pub first: RunMetrics,
    pub second: RunMetrics,
    /// Whether the first run left memory behind: notes, a `MEMORY.md`,
    /// or a transcript.
    pub memory_saved: bool,
    /// Whether the second run started with memory in its context.
    pub memory_given: bool,
    /// The second run's calls that read memory: memory_search and
    /// memory_read, or reading `MEMORY.md`.
    pub memory_calls: u32,
    /// In the changed variant, whether the second run acted on the old
    /// fact; `None` in the stable one.
    pub stale_used: Option<bool>,
    /// The second run's tool calls, for reading what it did.
    pub calls: Vec<Call>,
}

impl Trial {
    /// Whether the second run read memory at all.
    pub fn read_memory(&self) -> bool {
        self.memory_given || self.memory_calls > 0
    }

    pub fn success(&self) -> bool {
        self.second.success == Some(true)
    }

    pub fn cost_usd(&self) -> f64 {
        self.first.cost_usd + self.second.cost_usd
    }
}

/// Tools that read memory, as the memory arms offer them.
const MEMORY_READS: [&str; 2] = ["memory_search", "memory_read"];

/// How many of `calls` read memory: the memory tools, or `MEMORY.md`
/// read with a tool or a command.
pub fn memory_calls(calls: &[Call]) -> u32 {
    calls
        .iter()
        .filter(|call| {
            MEMORY_READS.contains(&call.tool.as_str())
                || match call.tool.as_str() {
                    "read" => arg(call, "path").ends_with("MEMORY.md"),
                    "bash" => arg(call, "command").contains("MEMORY.md"),
                    _ => false,
                }
        })
        .count() as u32
}

/// Whether any of `calls` acted on the old fact that `stale` matches: in
/// a command, a path read, or an edit or write to a file other than
/// `MEMORY.md` (where noting the change is fine). Memory tools do not
/// count: writing down that a fact changed is not using it.
pub fn stale_used(calls: &[Call], stale: &Regex) -> bool {
    calls.iter().any(|call| match call.tool.as_str() {
        "bash" => stale.is_match(arg(call, "command")),
        "read" => stale.is_match(arg(call, "path")),
        "edit" | "write" => {
            !arg(call, "path").ends_with("MEMORY.md")
                && stale.is_match(&call.args.to_string())
        }
        _ => false,
    })
}

fn arg<'a>(call: &'a Call, name: &str) -> &'a str {
    call.args.get(name).and_then(Value::as_str).unwrap_or("")
}

/// The means over one arm's trials in one variant.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Summary {
    pub arm: Arm,
    pub variant: Variant,
    pub trials: usize,
    /// Share of second runs whose check passed.
    pub success_rate: f64,
    /// Share of first runs whose check passed.
    pub first_success_rate: f64,
    /// Means over the second runs.
    pub tool_calls: f64,
    pub turns: f64,
    pub input_tokens: f64,
    pub output_tokens: f64,
    pub cached_tokens: f64,
    pub cost_usd: f64,
    pub wall_s: f64,
    /// Mean cost of both runs together.
    pub trial_cost_usd: f64,
    /// Share of second runs that acted on the old fact, over the trials
    /// where it changed; `None` when none did.
    pub stale_rate: Option<f64>,
    /// Share of second runs that read memory.
    pub read_memory_rate: f64,
}

/// The summaries of `trials`, one per arm and variant present, in
/// [`Arm::ALL`] order, then by variant.
pub fn summarize(trials: &[Trial]) -> Vec<Summary> {
    let mut groups: BTreeMap<(usize, Variant), Vec<&Trial>> = BTreeMap::new();
    for trial in trials {
        groups
            .entry((trial.arm.rank(), trial.variant))
            .or_default()
            .push(trial);
    }
    groups.into_values().map(|group| summary(&group)).collect()
}

fn summary(group: &[&Trial]) -> Summary {
    let n = group.len() as f64;
    let mean = |f: &dyn Fn(&Trial) -> f64| {
        group.iter().map(|trial| f(trial)).sum::<f64>() / n
    };
    let rate = |f: &dyn Fn(&Trial) -> bool| {
        mean(&|trial| if f(trial) { 1.0 } else { 0.0 })
    };
    let changed: Vec<bool> =
        group.iter().filter_map(|trial| trial.stale_used).collect();
    Summary {
        arm: group[0].arm,
        variant: group[0].variant,
        trials: group.len(),
        success_rate: rate(&Trial::success),
        first_success_rate: rate(&|trial| trial.first.success == Some(true)),
        tool_calls: mean(&|trial| f64::from(trial.second.tool_calls)),
        turns: mean(&|trial| f64::from(trial.second.turns)),
        input_tokens: mean(&|trial| trial.second.input_tokens as f64),
        output_tokens: mean(&|trial| trial.second.output_tokens as f64),
        cached_tokens: mean(&|trial| trial.second.cached_tokens as f64),
        cost_usd: mean(&|trial| trial.second.cost_usd),
        wall_s: mean(&|trial| trial.second.wall_ms as f64 / 1000.0),
        trial_cost_usd: mean(&Trial::cost_usd),
        stale_rate: (!changed.is_empty()).then(|| {
            changed.iter().filter(|used| **used).count() as f64
                / changed.len() as f64
        }),
        read_memory_rate: rate(&Trial::read_memory),
    }
}

/// The summaries as a plain-text table. Calls, turns, tokens, cost and
/// time are means over second runs; `trial $` is both runs.
pub fn table(summaries: &[Summary]) -> String {
    let mut out = format!(
        "{:<19} {:<8} {:>3} {:>5} {:>5} {:>6} {:>5} {:>8} {:>7} {:>8} \
         {:>8} {:>6} {:>8} {:>6} {:>6}\n",
        "arm",
        "variant",
        "n",
        "ok",
        "ok1",
        "calls",
        "turns",
        "input",
        "output",
        "cached",
        "cost $",
        "time s",
        "trial $",
        "stale",
        "read",
    );
    for s in summaries {
        let stale = s
            .stale_rate
            .map_or_else(|| "-".to_owned(), |rate| format!("{rate:.2}"));
        out.push_str(&format!(
            "{:<19} {:<8} {:>3} {:>5.2} {:>5.2} {:>6.1} {:>5.1} {:>8.0} \
             {:>7.0} {:>8.0} {:>8.4} {:>6.1} {:>8.4} {:>6} {:>6.2}\n",
            s.arm.name(),
            s.variant.name(),
            s.trials,
            s.success_rate,
            s.first_success_rate,
            s.tool_calls,
            s.turns,
            s.input_tokens,
            s.output_tokens,
            s.cached_tokens,
            s.cost_usd,
            s.wall_s,
            s.trial_cost_usd,
            stale,
            s.read_memory_rate,
        ));
    }
    out
}
