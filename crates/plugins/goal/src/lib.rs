//! Keeps a run going until a goal holds (`docs/reference/goal.md`).
//!
//! - **Setting a goal**: a run whose input is `/goal <condition>` sets
//!   it, with optional `--continuations N` and `--budget USD` before the
//!   condition. The input the model gets is the condition and how the
//!   goal works. A goal belongs to the conversation: resuming the run
//!   keeps it, until it is met, runs out, or is cleared.
//! - **Checking it**: each time the model would stop, Jev is asked
//!   whether the goal holds, from the model's last answer and its recent
//!   tool results. Not yet: the model is sent back with the goal, up to
//!   the goal's continuations, and while what the goal cost stays under
//!   its budget.
//! - **State in records**: every change is a [`Record`], stored with the
//!   run and reported, so the goal outlives the process and interfaces
//!   can show it. An interface controls a goal by storing records too
//!   (pause, resume, extend, clear): the plugin reads its records again
//!   at each check.

#[cfg(feature = "demo")]
pub mod demo;
pub mod ui;

use std::collections::VecDeque;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tau_agent::{
    error::PluginError,
    event::RunEvent,
    plugin::{
        Plugin,
        PluginCtx,
        PluginRun,
        RunPlan,
        StopDecision,
        ToolResultView,
    },
    tool::ToolOutput,
};
use tau_ai::message::{AssistantMessage, InputBlock};
use tau_jev::{Jev, NoulCriteria, Question, Request};

/// The name the plugin goes by in events, reports and records.
pub const NAME: &str = "tau-goal";

pub use ui::GoalUi;

/// Continuations a goal allows when `/goal` does not say.
pub const DEFAULT_CONTINUATIONS: u32 = 10;

/// US dollars a goal may cost when `/goal` does not say: the run's
/// turns and the checks, from when it was set.
pub const DEFAULT_BUDGET: f64 = 2.0;

/// The probability at or past which the goal counts as met.
pub const MET_AT: f64 = 0.7;

/// How many of the latest tool results Jev sees.
const EVIDENCE: usize = 6;

/// How much of a tool result Jev sees: its end, where results and
/// summaries are.
const OUTPUT_CHARS: usize = 1500;

/// How the model is told about its goal, after the condition.
pub const INSTRUCTIONS: &str = "This is your goal. Keep working until it \
    holds. Each time you stop, tau-goal checks it against your last answer \
    and your recent tool results, and sends you back if it does not hold \
    yet, so run what proves it before you stop.";

/// How a continuation starts, so interfaces can tell it from a person's
/// message.
pub const CONTINUATION_PREFIX: &str = "tau-goal: ";

/// A `/goal` command.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Set {
        condition: String,
        continuations: u32,
        budget: f64,
    },
    Clear,
}

impl Command {
    /// Reads `/goal [--continuations N] [--budget USD] <condition>` or
    /// `/goal clear`. Anything else is not a command, a budget that is
    /// not a finite amount of at least zero included.
    ///
    /// A condition is its words joined by single spaces, unless it is
    /// one quoted string, `"..."`: that is read exactly as written, with
    /// `\"`, `\\`, `\n` and `\r` for a quote, a backslash, a line feed
    /// and a carriage return. `/goal "clear"` sets the goal `clear`.
    pub fn parse(input: &str) -> Option<Self> {
        let rest = input.trim().strip_prefix("/goal")?;
        if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
            return None;
        }
        let mut rest = rest.trim_start();
        let mut continuations = DEFAULT_CONTINUATIONS;
        let mut budget = DEFAULT_BUDGET;
        loop {
            let (word, after) = next_word(rest);
            match word {
                "--continuations" => {
                    let (n, after) = next_word(after);
                    continuations = n.parse().ok()?;
                    rest = after;
                }
                "--budget" => {
                    let (usd, after) = next_word(after);
                    budget = usd.trim_start_matches('$').parse().ok().filter(
                        |budget: &f64| budget.is_finite() && *budget >= 0.0,
                    )?;
                    rest = after;
                }
                _ => break,
            }
        }
        if let Some(condition) = unquote(rest) {
            if condition.trim().is_empty() {
                return None;
            }
            return Some(Self::Set {
                condition,
                continuations,
                budget,
            });
        }
        let condition = rest.split_whitespace().collect::<Vec<_>>().join(" ");
        match condition.as_str() {
            "" => None,
            "clear" => Some(Self::Clear),
            _ => Some(Self::Set {
                condition,
                continuations,
                budget,
            }),
        }
    }
}

/// The first whitespace-separated word of `text`, and what follows it,
/// its leading whitespace gone.
fn next_word(text: &str) -> (&str, &str) {
    let end = text.find(char::is_whitespace).unwrap_or(text.len());
    (&text[..end], text[end..].trim_start())
}

/// What a quoted string says, when `text` is exactly one: `"`, then
/// characters with `\"`, `\\`, `\n` and `\r` escaped, then `"`.
fn unquote(text: &str) -> Option<String> {
    let mut chars = text.strip_prefix('"')?.chars();
    let mut out = String::new();
    loop {
        match chars.next()? {
            '"' => return chars.as_str().is_empty().then_some(out),
            '\\' => out.push(match chars.next()? {
                '"' => '"',
                '\\' => '\\',
                'n' => '\n',
                'r' => '\r',
                _ => return None,
            }),
            c => out.push(c),
        }
    }
}

/// `condition` as a quoted string [`unquote`] reads back.
fn quote(condition: &str) -> String {
    let mut out = String::from("\"");
    for c in condition.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The input the model gets for a goal: the condition, then how goals
/// work. Interfaces read the condition back with [`set_message`].
///
/// The condition is written as is when it reads back that way, and
/// quoted otherwise: when it would read as `clear` or as limits, spans
/// lines, has runs of whitespace or is itself one quoted string.
pub fn set_input(condition: &str) -> String {
    let plain = format!("/goal {condition}\n\n{INSTRUCTIONS}");
    if set_message(&plain).as_deref() == Some(condition) {
        return plain;
    }
    format!("/goal {}\n\n{INSTRUCTIONS}", quote(condition))
}

/// The condition of a message that set a goal: `/goal ...`, as typed or
/// as [`set_input`] wrote it.
pub fn set_message(text: &str) -> Option<String> {
    let first = text.lines().next()?;
    match Command::parse(first)? {
        Command::Set { condition, .. } => Some(condition),
        Command::Clear => None,
    }
}

/// Why a goal stopped before it was met.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Exhausted {
    Continuations,
    Budget,
}

/// One check of a goal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Check {
    /// 1 for the goal's first check.
    pub n: u32,
    pub met: bool,
    /// Jev's probability that the goal holds.
    pub p: f64,
    /// The turn the model stopped at.
    pub turn: u32,
    /// Which continuation it sent the model back for, if it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<u32>,
    /// What the check cost, in US dollars.
    pub cost: f64,
    /// What the goal cost so far, this check included.
    pub spent: f64,
}

/// Everything that happens to a goal, as stored and reported.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Record {
    Set {
        goal: String,
        continuations: u32,
        budget: f64,
    },
    Check(Check),
    /// Not met, and out of continuations or budget.
    Stopped {
        why: Exhausted,
    },
    /// More continuations, and the goal active again.
    Extended {
        by: u32,
    },
    Paused,
    Resumed,
    Cleared,
    /// Jev gave no answer; the run stopped unchecked.
    Error {
        message: String,
    },
    /// What the interface folds as a run starts, never stored: whether
    /// the run's stops are checked.
    Starting {
        checks: bool,
    },
}

impl Record {
    /// The record `body` holds, or none, said the first time.
    pub fn parse(body: &Value) -> Option<Self> {
        tau_agent::plugin::read_record(NAME, body)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    /// Checked each time the model stops.
    Active,
    /// Kept, but not checked, until resumed.
    Paused,
    Met,
    Stopped(Exhausted),
}

/// A conversation's goal, folded from its records.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Goal {
    pub condition: String,
    pub max_continuations: u32,
    pub budget: f64,
    pub status: Status,
    /// Continuations used.
    pub continuations: u32,
    /// What the goal cost so far, at its last check.
    pub spent: f64,
    /// Every check, oldest first.
    pub checks: Vec<Check>,
    /// The last time Jev could not check it.
    pub error: Option<String>,
}

impl Goal {
    /// The goal `records` leave, if any.
    pub fn fold<'a>(
        records: impl IntoIterator<Item = &'a Value>,
    ) -> Option<Self> {
        let mut goal = None;
        for body in records {
            if let Some(record) = Record::parse(body) {
                Self::apply(&mut goal, &record);
            }
        }
        goal
    }

    /// Applies one record to a conversation's goal.
    pub fn apply(goal: &mut Option<Self>, record: &Record) {
        if let Record::Set {
            goal: condition,
            continuations,
            budget,
        } = record
        {
            *goal = Some(Self {
                condition: condition.clone(),
                max_continuations: *continuations,
                budget: *budget,
                status: Status::Active,
                continuations: 0,
                spent: 0.0,
                checks: Vec::new(),
                error: None,
            });
            return;
        }
        if *record == Record::Cleared {
            *goal = None;
            return;
        }
        let Some(goal) = goal else { return };
        match record {
            Record::Check(check) => {
                goal.spent = check.spent;
                goal.error = None;
                if let Some(continuation) = check.continuation {
                    goal.continuations = continuation;
                }
                if check.met {
                    goal.status = Status::Met;
                }
                goal.checks.push(check.clone());
            }
            Record::Stopped { why } => goal.status = Status::Stopped(*why),
            Record::Extended { by } => {
                goal.max_continuations =
                    goal.continuations.max(goal.max_continuations) + by;
                goal.status = Status::Active;
            }
            Record::Paused if goal.status == Status::Active => {
                goal.status = Status::Paused;
            }
            Record::Resumed if goal.status == Status::Paused => {
                goal.status = Status::Active;
            }
            Record::Error { message } => goal.error = Some(message.clone()),
            _ => {}
        }
    }
}

/// The plugin. Share one across an agent's runs.
#[derive(Clone)]
pub struct GoalPlugin {
    jev: std::sync::Arc<dyn Jev>,
}

impl GoalPlugin {
    pub fn new(jev: std::sync::Arc<dyn Jev>) -> Self {
        Self { jev }
    }
}

#[async_trait]
impl Plugin for GoalPlugin {
    fn name(&self) -> &str {
        NAME
    }

    async fn start(
        &self,
        plan: &mut RunPlan,
        ctx: &PluginCtx,
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        if let Some(command) = Command::parse(&plan.input) {
            let record = match command {
                Command::Set {
                    condition,
                    continuations,
                    budget,
                } => {
                    plan.input = set_input(&condition);
                    Record::Set {
                        goal: condition,
                        continuations,
                        budget,
                    }
                }
                Command::Clear => {
                    plan.input = "The goal is cleared.".into();
                    Record::Cleared
                }
            };
            tell(&record, ctx).await;
        }
        Ok(Box::new(Pursuit {
            jev: self.jev.clone(),
            evidence: VecDeque::new(),
            unchecked_cost: 0.0,
            turn: 0,
        }))
    }
}

/// Reports and records `record`. A failed write loses it from history,
/// and a stop may be checked again after a restart: not worth failing
/// the run for.
async fn tell(record: &Record, ctx: &PluginCtx) {
    ctx.publish(record).await;
}

/// A run's pursuit of its conversation's goal.
struct Pursuit {
    jev: std::sync::Arc<dyn Jev>,
    /// The latest tool results, oldest first.
    evidence: VecDeque<Value>,
    /// What the run cost since the goal's last check.
    unchecked_cost: f64,
    turn: u32,
}

impl Pursuit {
    /// Jev's probability that `goal` holds, given `answer` and the
    /// evidence, and what asking cost.
    async fn ask(
        &self,
        goal: &str,
        answer: &str,
        ctx: &PluginCtx,
    ) -> Result<(f64, f64), String> {
        let state = json!({
            "goal": goal,
            "final_answer": answer,
            "recent_tool_results": self.evidence.iter().collect::<Vec<_>>(),
        });
        let request = Request::new(state).question(
            "met",
            Question::Noul {
                instructions: "Is the goal met? Judge from the tool results: \
                    the answer's own claims count only where a result backs \
                    them."
                    .into(),
                criteria: Some(NoulCriteria {
                    yes: Some(
                        "The tool results show that the goal holds.".into(),
                    ),
                    no: Some(
                        "The goal does not hold yet, or nothing shown proves \
                         it."
                        .into(),
                    ),
                }),
            },
        );
        let response =
            self.jev.ask(&request).await.map_err(|e| e.to_string())?;
        let usage = response.usage();
        ctx.charge(&usage);
        let p = response.noul("met").map_err(|e| e.to_string())?;
        Ok((p, usage.cost.total))
    }
}

#[async_trait]
impl PluginRun for Pursuit {
    async fn after_tool_result(
        &mut self,
        view: &ToolResultView<'_>,
        output: &mut ToolOutput,
        _ctx: &PluginCtx,
    ) -> Result<(), PluginError> {
        let call = view.call;
        let text: String = output
            .content
            .iter()
            .filter_map(|block| match block {
                InputBlock::Text(text) => Some(text.text.as_str()),
                InputBlock::Image(_) => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        self.evidence.push_back(json!({
            "tool": call.name,
            "arguments": call.args,
            "output": tail(&text, OUTPUT_CHARS),
        }));
        if self.evidence.len() > EVIDENCE {
            self.evidence.pop_front();
        }
        Ok(())
    }

    async fn on_event(&mut self, event: &RunEvent, _ctx: &PluginCtx) {
        if let RunEvent::TurnEnd { turn, usage, .. } = event {
            self.turn = *turn;
            self.unchecked_cost += usage.cost.total;
        }
    }

    async fn before_stop(
        &mut self,
        message: &AssistantMessage,
        ctx: &PluginCtx,
    ) -> Result<StopDecision, PluginError> {
        // Read again: an interface may have paused, extended or cleared
        // the goal since the run started.
        let records = ctx.records().await?;
        let Some(goal) = Goal::fold(&records) else {
            return Ok(StopDecision::Stop);
        };
        if goal.status != Status::Active {
            return Ok(StopDecision::Stop);
        }
        let answer: String = message.text();
        let (p, cost) = match self.ask(&goal.condition, &answer, ctx).await {
            Ok(answer) => answer,
            Err(error) => {
                let message = format!("Jev could not check the goal: {error}");
                tell(&Record::Error { message }, ctx).await;
                return Ok(StopDecision::Stop);
            }
        };
        let spent = goal.spent + self.unchecked_cost + cost;
        self.unchecked_cost = 0.0;
        let met = p >= MET_AT;
        let exhausted = if met {
            None
        } else if goal.continuations >= goal.max_continuations {
            Some(Exhausted::Continuations)
        } else if spent >= goal.budget {
            Some(Exhausted::Budget)
        } else {
            None
        };
        let continuation =
            (!met && exhausted.is_none()).then_some(goal.continuations + 1);
        let n = goal.checks.len() as u32 + 1;
        tell(
            &Record::Check(Check {
                n,
                met,
                p,
                turn: self.turn,
                continuation,
                cost,
                spent,
            }),
            ctx,
        )
        .await;
        if let Some(why) = exhausted {
            tell(&Record::Stopped { why }, ctx).await;
        }
        let Some(continuation) = continuation else {
            return Ok(StopDecision::Stop);
        };
        Ok(StopDecision::Continue(format!(
            "{CONTINUATION_PREFIX}the goal is not met yet (check {n}, \
             probability {p:.2}).\nGoal: {}\n\nKeep working toward it, and \
             run what proves it. Continuation {continuation} of {}.",
            goal.condition, goal.max_continuations
        )))
    }
}

/// The last `max` characters of `text`, on a char boundary.
fn tail(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_owned();
    }
    let skipped: String = text.chars().skip(count - max).collect();
    format!("…{skipped}")
}
