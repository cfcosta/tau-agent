//! Picks a run's reasoning effort from its task, asking Jev once before
//! the run's session opens (`docs/reference/plugins.md`,
//! `tau-reasoning`).
//!
//! Only runs that left the effort open (`plan.reasoning` is `None`, the
//! "auto" effort) are scored; an effort someone chose stands. Jev gets
//! the task and the start of the instructions, and one Score question
//! whose levels say what each effort suits. The most likely level is
//! used when Jev's confidence reaches the threshold; below it, the run
//! keeps the model's default. Either way the plugin reports and records
//! what Jev answered, as a [`Choice`], so interfaces can show why.
//!
//! The effort stays fixed for the run: changing it mid-run would break
//! the conversation's chain each time. A failed request leaves the
//! default, reported, and never fails the run.

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

/// The efforts Codex models support, lowest first.
pub fn default_levels() -> Vec<Level> {
    [
        (
            ReasoningEffort::Minimal,
            "lookups, and answers that need no code",
        ),
        (ReasoningEffort::Low, "small, clear edits in one place"),
        (ReasoningEffort::Medium, "routine code across a few files"),
        (
            ReasoningEffort::High,
            "bugs to track down, refactors, designs across many files",
        ),
        (
            ReasoningEffort::Xhigh,
            "audits, proofs, subtle concurrency or invariants",
        ),
    ]
    .into_iter()
    .map(|(effort, suits)| Level {
        effort,
        suits: suits.to_owned(),
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
    levels: Vec<Level>,
    threshold: f64,
}

impl Reasoning {
    pub fn new(jev: Arc<dyn Jev>) -> Self {
        Self {
            jev,
            levels: default_levels(),
            threshold: DEFAULT_THRESHOLD,
        }
    }

    /// The efforts to choose from, lowest first: those the model
    /// supports.
    pub fn levels(mut self, levels: Vec<Level>) -> Self {
        self.levels = levels;
        self
    }

    pub fn threshold(mut self, threshold: f64) -> Self {
        self.threshold = threshold;
        self
    }

    async fn choose(
        &self,
        plan: &RunPlan,
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
                self.levels.iter().map(|level| level.suits.clone()),
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
        let best = (0..self.levels.len())
            .max_by(|a, b| p(*a).total_cmp(&p(*b)))
            .ok_or("no levels to choose from")?;
        let effort = self.levels[best].effort;
        let choice = Choice {
            kind: if *confidence >= self.threshold {
                "chose".into()
            } else {
                "kept".into()
            },
            effort: effort.as_str().into(),
            confidence: *confidence,
            threshold: self.threshold,
            levels: self
                .levels
                .iter()
                .enumerate()
                .map(|(n, level)| Scored {
                    effort: level.effort.as_str().into(),
                    suits: level.suits.clone(),
                    p: p(n),
                })
                .collect(),
            cost: usage.cost.total,
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
        match self.choose(plan, ctx).await {
            Ok((choice, effort)) => {
                if choice.kind == "chose" {
                    plan.reasoning = Some(effort);
                }
                let body = serde_json::to_value(&choice).unwrap_or_default();
                ctx.report(body.clone());
                let _ = ctx.record(&body).await;
            }
            Err(error) => ctx.report(json!({
                "kind": "error",
                "message": format!("Jev could not score the task: {error}"),
            })),
        }
        Ok(Box::new(()))
    }
}
