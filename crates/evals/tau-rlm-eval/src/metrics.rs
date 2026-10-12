//! What a run cost and did, read from its events and outcome.

use std::{collections::BTreeMap, time::Duration};

use serde::Serialize;
use tau_agent::{
    agent::{AgentError, Outcome},
    event::{RunEvent, StopReason},
};
use tau_ai::message::Usage;

use crate::{
    arm::Arm,
    task::{Kind, Score},
};

/// Follows a run's events.
#[derive(Debug, Default)]
pub struct Meter {
    turns: u32,
    calls: Vec<(String, String, bool, bool)>,
    usage: Usage,
}

impl Meter {
    pub fn observe(&mut self, event: &RunEvent) {
        match event {
            RunEvent::TurnEnd { usage, .. } => {
                self.turns += 1;
                self.usage += usage;
            }
            RunEvent::ToolStart {
                call_id,
                tool,
                parent,
                ..
            } => self.calls.push((
                call_id.clone(),
                tool.to_string(),
                parent.is_some(),
                false,
            )),
            RunEvent::ToolEnd {
                call_id, is_error, ..
            } => {
                if let Some(call) =
                    self.calls.iter_mut().rev().find(|c| &c.0 == call_id)
                {
                    call.3 = *is_error;
                }
            }
            _ => {}
        }
    }

    pub fn finish(
        &self,
        outcome: &Result<Outcome, AgentError>,
        wall: Duration,
    ) -> Run {
        let (usage, stop, reply) = match outcome {
            Ok(outcome) => (
                outcome.usage.clone(),
                stop_name(&outcome.stop),
                outcome.text.clone(),
            ),
            Err(error) => {
                (self.usage.clone(), format!("error: {error}"), String::new())
            }
        };
        let tool_calls = |nested: bool| {
            self.calls.iter().filter(|c| c.2 == nested).count() as u32
        };
        let infer: Vec<_> =
            self.calls.iter().filter(|c| c.1 == "infer").collect();
        Run {
            turns: self.turns,
            tool_calls: tool_calls(false),
            nested_calls: tool_calls(true),
            infer_calls: infer.len() as u32,
            infer_failed: infer.iter().filter(|c| c.3).count() as u32,
            input_tokens: usage.input,
            output_tokens: usage.output,
            cached_tokens: usage.cache_read,
            cost_usd: usage.cost.total,
            wall_ms: wall.as_millis() as u64,
            stop,
            reply,
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
pub struct Run {
    pub turns: u32,
    pub tool_calls: u32,
    /// Calls tools made through the loop (a script's).
    pub nested_calls: u32,
    /// Sub-model calls from scripts (`tools.infer`).
    pub infer_calls: u32,
    pub infer_failed: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    /// Everything the run was charged, sub-model calls included.
    pub cost_usd: f64,
    pub wall_ms: u64,
    pub stop: String,
    pub reply: String,
}

/// One trial: a task at a size, under an arm.
#[derive(Debug, Clone, Serialize)]
pub struct Trial {
    pub kind: Kind,
    pub lines: usize,
    pub bytes: usize,
    pub arm: Arm,
    pub trial: u32,
    pub expected: String,
    pub score: Score,
    pub run: Run,
}

/// Means over the trials of one task, size and arm.
#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub kind: Kind,
    pub lines: usize,
    pub arm: Arm,
    pub trials: usize,
    pub exact: f64,
    pub score: f64,
    pub turns: f64,
    pub infer_calls: f64,
    pub input_tokens: f64,
    pub cost_usd: f64,
    pub wall_s: f64,
}

pub fn summarize(trials: &[Trial]) -> Vec<Summary> {
    let mut groups: BTreeMap<(usize, usize, usize), Vec<&Trial>> =
        BTreeMap::new();
    for trial in trials {
        let kind = Kind::ALL.iter().position(|k| *k == trial.kind).unwrap();
        groups
            .entry((kind, trial.lines, trial.arm.rank()))
            .or_default()
            .push(trial);
    }
    groups
        .into_values()
        .map(|group| {
            let n = group.len() as f64;
            let mean = |f: &dyn Fn(&Trial) -> f64| {
                group.iter().map(|t| f(t)).sum::<f64>() / n
            };
            Summary {
                kind: group[0].kind,
                lines: group[0].lines,
                arm: group[0].arm,
                trials: group.len(),
                exact: mean(&|t| f64::from(u8::from(t.score.exact))),
                score: mean(&|t| t.score.score),
                turns: mean(&|t| f64::from(t.run.turns)),
                infer_calls: mean(&|t| f64::from(t.run.infer_calls)),
                input_tokens: mean(&|t| t.run.input_tokens as f64),
                cost_usd: mean(&|t| t.run.cost_usd),
                wall_s: mean(&|t| t.run.wall_ms as f64 / 1000.0),
            }
        })
        .collect()
}

/// The summaries as a table.
pub fn table(summaries: &[Summary]) -> String {
    let mut out = String::from(
        "task    lines  arm            n  exact  score  turns  infer  \
         in-tokens   cost$   time\n",
    );
    for s in summaries {
        out.push_str(&format!(
            "{:<7} {:>5}  {:<13} {:>2}  {:>5.2}  {:>5.2}  {:>5.1}  {:>5.1}  \
             {:>9.0}  {:>6.3}  {:>5.0}s\n",
            s.kind.name(),
            s.lines,
            s.arm.name(),
            s.trials,
            s.exact,
            s.score,
            s.turns,
            s.infer_calls,
            s.input_tokens,
            s.cost_usd,
            s.wall_s,
        ));
    }
    out
}
