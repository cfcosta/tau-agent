//! Picks a run's reasoning effort from its task, asking Jev once before
//! the run's session opens (`docs/reference/plugins.md`,
//! `tau-reasoning`).
//!
//! Only runs that left the effort open (`plan.reasoning` is `None`, the
//! "auto" effort) are scored; an effort someone chose stands. Jev gets
//! the task and the start of the instructions, and one Score question
//! whose levels say what each effort suits. The most likely level is
//! used when Jev's confidence reaches the threshold. Below it, or when
//! the request fails, the message goes on at the effort the run's last
//! message ran at, if the model takes it, else at the model's default.
//! Either way the plugin reports and records what Jev answered and what
//! the message runs at, as a [`Choice`], so interfaces can show why.
//!
//! The effort stays fixed while the run works: changing it mid-run would
//! break the conversation's chain each time. Each new message of a chat
//! is scored again. A failed request is reported and never fails the
//! run.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tau_agent::plugin::{Plugin, PluginCtx, PluginRun, RunPlan};
use tau_ai::responses::request::ReasoningEffort;
use tau_jev::{Answer, Jev, Question, Request};

/// The name the plugin goes by in events, reports and records.
pub const NAME: &str = "tau-reasoning";

/// How confident Jev must be for its level to be used.
pub const DEFAULT_THRESHOLD: f64 = 0.7;

/// How much of the instructions Jev sees: the start says what the agent
/// is for, and the rest costs tokens for little.
const INSTRUCTIONS_SEEN: usize = 1_000;

/// An effort, and the work it suits, in words Jev reads.
#[derive(Debug, Clone, PartialEq)]
pub struct Level {
    pub effort: ReasoningEffort,
    pub suits: String,
}

/// What each effort suits, in words Jev reads, lowest first.
fn suits(effort: ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::None => "answers that need no thought",
        ReasoningEffort::Minimal => "lookups, and answers that need no code",
        ReasoningEffort::Low => "small, clear edits in one place",
        ReasoningEffort::Medium => "routine code across a few files",
        ReasoningEffort::High => {
            "bugs to track down, refactors, designs across many files"
        }
        ReasoningEffort::Xhigh => {
            "audits, proofs, subtle concurrency or invariants"
        }
        ReasoningEffort::Max => {
            "the hardest problems, where more thought still pays"
        }
    }
}

/// The efforts `model` takes, lowest first, each with the work it
/// suits.
pub fn levels_for(model: &str) -> Vec<Level> {
    tau_ai::model::efforts(model)
        .into_iter()
        .map(|effort| Level {
            effort,
            suits: suits(effort).to_owned(),
        })
        .collect()
}

/// What Jev answered, as the plugin reports and records it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    /// `chose` when the run uses `effort`; `kept` when Jev was not sure
    /// enough, and the run keeps its default.
    pub kind: String,
    /// The most likely effort.
    pub effort: String,
    pub confidence: f64,
    pub threshold: f64,
    /// Each level, lowest first.
    pub levels: Vec<Scored>,
    /// What Jev cost, in US dollars.
    pub cost: f64,
    /// The effort the message runs at: `effort` when chosen, else the
    /// last message's; `None` for the model's default.
    #[serde(default)]
    pub runs_at: Option<String>,
}

impl Choice {
    /// The effort this record says its message ran at.
    fn ran_at(&self) -> Option<&str> {
        self.runs_at
            .as_deref()
            .or((self.kind == "chose").then_some(self.effort.as_str()))
    }
}

/// The effort the run's last scored message ran at, from the plugin's
/// records, if one of `levels` still takes it.
fn previous(records: &[Value], levels: &[Level]) -> Option<ReasoningEffort> {
    let last = records.iter().rev().find_map(Choice::parse)?;
    let effort = ReasoningEffort::parse(last.ran_at()?)?;
    levels
        .iter()
        .any(|level| level.effort == effort)
        .then_some(effort)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scored {
    pub effort: String,
    pub suits: String,
    pub p: f64,
}

impl Choice {
    /// Reads a choice back from a report or record body.
    pub fn parse(body: &Value) -> Option<Self> {
        serde_json::from_value(body.clone()).ok()
    }

    /// The level Jev found most likely, by index.
    pub fn chosen(&self) -> usize {
        self.levels
            .iter()
            .position(|level| level.effort == self.effort)
            .unwrap_or_default()
    }
}

/// The plugin. Share one across an agent's runs.
#[derive(Clone)]
pub struct Reasoning {
    jev: Arc<dyn Jev>,
    /// The efforts to choose from; `None` takes the run's model's.
    levels: Option<Vec<Level>>,
    threshold: f64,
}

impl Reasoning {
    pub fn new(jev: Arc<dyn Jev>) -> Self {
        Self {
            jev,
            levels: None,
            threshold: DEFAULT_THRESHOLD,
        }
    }

    /// The efforts to choose from, lowest first, in place of those the
    /// run's model takes.
    pub fn levels(mut self, levels: Vec<Level>) -> Self {
        self.levels = Some(levels);
        self
    }

    pub fn threshold(mut self, threshold: f64) -> Self {
        self.threshold = threshold;
        self
    }

    async fn choose(
        &self,
        plan: &RunPlan,
        levels: &[Level],
        ctx: &PluginCtx,
    ) -> Result<(Choice, ReasoningEffort), String> {
        let instructions: String = plan
            .instructions
            .as_deref()
            .unwrap_or_default()
            .chars()
            .take(INSTRUCTIONS_SEEN)
            .collect();
        let request = Request::new(json!({
            "task": plan.input,
            "agent_instructions": instructions,
        }))
        .question(
            "effort",
            Question::score(
                "How much reasoning does this task need from a coding \
                 agent? Pick the least that does it well.",
                levels.iter().map(|level| level.suits.clone()),
            ),
        );
        let response =
            self.jev.ask(&request).await.map_err(|e| e.to_string())?;
        let usage = response.usage();
        ctx.charge(&usage);
        let Some(Answer::Score {
            probabilities,
            confidence,
            ..
        }) = response.answers.get("effort")
        else {
            return Err("Jev sent no score".into());
        };
        let p = |n: usize| {
            probabilities.get(&n.to_string()).copied().unwrap_or(0.0)
        };
        let best = (0..levels.len())
            .max_by(|a, b| p(*a).total_cmp(&p(*b)))
            .ok_or("no levels to choose from")?;
        let effort = levels[best].effort;
        let choice = Choice {
            kind: if *confidence >= self.threshold {
                "chose".into()
            } else {
                "kept".into()
            },
            effort: effort.as_str().into(),
            confidence: *confidence,
            threshold: self.threshold,
            levels: levels
                .iter()
                .enumerate()
                .map(|(n, level)| Scored {
                    effort: level.effort.as_str().into(),
                    suits: level.suits.clone(),
                    p: p(n),
                })
                .collect(),
            cost: usage.cost.total,
            runs_at: None,
        };
        Ok((choice, effort))
    }
}

#[async_trait]
impl Plugin for Reasoning {
    fn name(&self) -> &str {
        NAME
    }

    async fn start(
        &self,
        plan: &mut RunPlan,
        ctx: &PluginCtx,
    ) -> anyhow::Result<Box<dyn PluginRun>> {
        // An effort someone chose stands.
        if plan.reasoning.is_some() {
            return Ok(Box::new(()));
        }
        let levels = self
            .levels
            .clone()
            .unwrap_or_else(|| levels_for(plan.model()));
        // A model that does not reason has nothing to choose.
        if levels.is_empty() {
            return Ok(Box::new(()));
        }
        // Unsure or failed, the message goes on as the last one did.
        let previous = previous(plan.records(), &levels);
        match self.choose(plan, &levels, ctx).await {
            Ok((mut choice, effort)) => {
                plan.reasoning = if choice.kind == "chose" {
                    Some(effort)
                } else {
                    previous
                };
                choice.runs_at =
                    plan.reasoning.map(|effort| effort.as_str().to_owned());
                let body = serde_json::to_value(&choice).unwrap_or_default();
                ctx.report(body.clone());
                let _ = ctx.record(&body).await;
            }
            Err(error) => {
                plan.reasoning = previous;
                ctx.report(json!({
                    "kind": "error",
                    "message": format!("Jev could not score the task: {error}"),
                    "runs_at": previous.map(ReasoningEffort::as_str),
                }))
            }
        }
        Ok(Box::new(()))
    }
}
