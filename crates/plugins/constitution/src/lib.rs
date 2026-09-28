//! Checks a run's tool calls and final answer against a constitution,
//! asking Jev how likely each applicable rule is broken
//! (`docs/reference/plugins.md`, `tau-constitution`).
//!
//! - **Before a tool call**, the rules that name one of the call's
//!   arguments (`edit.newText`, `bash.command`) are checked, all in one
//!   request. Jev sees only those arguments, as the model wrote them,
//!   never tool output or file content. A rule at or past its `block`
//!   probability refuses the call, and the reason (the rule, quoted)
//!   goes back to the model so it can fix the call. Between `review`
//!   and `block`, the call runs and is flagged for a person.
//! - **Before the run stops**, the rules on the final answer are checked
//!   the same way. A broken one sends the answer back with the rule, up
//!   to `max_holds` times per run.
//! - Every decision is reported
//!   ([`PluginCtx::report`](tau_agent::plugin::PluginCtx::report)) and
//!   recorded with the run, as a [`Verdict`], so interfaces can show and
//!   review them, now and in history.
//! - When Jev gives no answer, `on_error` decides: `allow` (the default)
//!   lets the call run and reports it; `block` refuses it.

pub mod rules;

use std::{path::PathBuf, sync::Arc};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tau_agent::{
    hook::{Decision, ToolCall},
    plugin::{Plugin, PluginCtx, PluginRun, RunPlan, StopDecision},
};
use tau_ai::message::{AssistantBlock, AssistantMessage};
use tau_jev::{Jev, NoulCriteria, Question, Request};

pub use crate::rules::{Constitution, OnError, Rule, Target};

/// The name the plugin goes by in events, reports and records.
pub const NAME: &str = "tau-constitution";

/// What the plugin decided about one rule, as it reports and records
/// it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    pub kind: VerdictKind,
    /// The rule, and its text.
    pub rule: String,
    pub text: String,
    /// Jev's probability that the rule is broken.
    pub score: f64,
    /// The call it is about; `None` for the final answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// What the model was told, for a block or a hold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// For a hold: which one this is, and how many the run may have.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_holds: Option<u32>,
}

/// One check: every applicable rule's score, whatever it decided, as
/// the plugin reports and records it (`"kind": "checked"`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Check {
    /// The call checked; `None` for the final answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Each rule asked about, and its violation probability.
    pub scores: Vec<Score>,
    /// What Jev charged, in US dollars.
    pub cost: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Score {
    pub rule: String,
    pub score: f64,
}

impl Check {
    /// Reads a check back from a report or record body.
    pub fn parse(body: &Value) -> Option<Self> {
        (body["kind"] == "checked")
            .then(|| serde_json::from_value(body.clone()).ok())
            .flatten()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VerdictKind {
    /// The call was refused.
    Blocked,
    /// The call ran (or the answer stood); a person should look.
    Flagged,
    /// The final answer was sent back.
    Held,
}

impl Verdict {
    /// Reads a verdict back from a report or record body.
    pub fn parse(body: &Value) -> Option<Self> {
        serde_json::from_value(body.clone()).ok()
    }
}

/// Where the plugin finds its constitution.
#[derive(Debug, Clone)]
enum Source {
    Rules(Arc<Constitution>),
    /// Read when each run starts, so a run sees the file as its
    /// workspace has it.
    File(PathBuf),
}

/// The plugin. Share one across an agent's runs.
#[derive(Clone)]
pub struct ConstitutionPlugin {
    jev: Arc<dyn Jev>,
    source: Source,
}

impl ConstitutionPlugin {
    pub fn new(jev: Arc<dyn Jev>, constitution: Constitution) -> Self {
        Self {
            jev,
            source: Source::Rules(Arc::new(constitution)),
        }
    }

    /// Reads the constitution from `path` when each run starts. No file
    /// there is no rules; a file that does not parse fails the run, so
    /// a broken constitution is never silently ignored.
    pub fn from_file(jev: Arc<dyn Jev>, path: impl Into<PathBuf>) -> Self {
        Self {
            jev,
            source: Source::File(path.into()),
        }
    }
}

#[async_trait]
impl Plugin for ConstitutionPlugin {
    fn name(&self) -> &str {
        NAME
    }

    async fn start(
        &self,
        _plan: &mut RunPlan,
        _ctx: &PluginCtx,
    ) -> anyhow::Result<Box<dyn PluginRun>> {
        let constitution = match &self.source {
            Source::Rules(rules) => rules.clone(),
            Source::File(path) => Arc::new(Constitution::load(path)?),
        };
        Ok(Box::new(Checks {
            jev: self.jev.clone(),
            constitution,
            holds: 0,
        }))
    }
}

/// A run's checks.
struct Checks {
    jev: Arc<dyn Jev>,
    constitution: Arc<Constitution>,
    /// Final answers sent back so far.
    holds: u32,
}

impl Checks {
    /// Asks Jev about `rules` on `state`, one yes/no question each, and
    /// returns each rule's violation probability, in order.
    /// Reports and records the check, with every score.
    async fn ask(
        &self,
        state: Value,
        rules: &[(&Rule, String)],
        about: (Option<&str>, Option<&str>),
        ctx: &PluginCtx,
    ) -> Result<Vec<f64>, String> {
        let mut request = Request::new(state);
        for (n, (rule, what)) in rules.iter().enumerate() {
            request = request.question(
                question_id(n),
                Question::Noul {
                    instructions: format!(
                        "Does {what} break this rule? Judge only what is \
                         shown.\n\nRule {}: {}",
                        rule.id, rule.text
                    )
                    .into(),
                    criteria: Some(NoulCriteria {
                        yes: Some(format!("{what} breaks the rule.").into()),
                        no: Some(
                            format!(
                                "{what} follows the rule, or the rule is not \
                                 about it."
                            )
                            .into(),
                        ),
                    }),
                },
            );
        }
        let response =
            self.jev.ask(&request).await.map_err(|e| e.to_string())?;
        let usage = response.usage();
        ctx.charge(&usage);
        let scores = (0..rules.len())
            .map(|n| response.noul(&question_id(n)).map_err(|e| e.to_string()))
            .collect::<Result<Vec<f64>, String>>()?;
        let check = Check {
            call_id: about.0.map(str::to_owned),
            tool: about.1.map(str::to_owned),
            scores: rules
                .iter()
                .zip(&scores)
                .map(|((rule, _), score)| Score {
                    rule: rule.id.clone(),
                    score: *score,
                })
                .collect(),
            cost: usage.cost.total,
        };
        let mut body = serde_json::to_value(&check).unwrap_or_default();
        body["kind"] = "checked".into();
        ctx.report(body.clone());
        let _ = ctx.record(&body).await;
        Ok(scores)
    }

    async fn tell(&self, verdict: &Verdict, ctx: &PluginCtx) {
        let body = serde_json::to_value(verdict).unwrap_or_default();
        ctx.report(body.clone());
        // History reads it back; a failed write loses only the review.
        let _ = ctx.record(&body).await;
    }
}

fn question_id(n: usize) -> String {
    format!("rule-{n}")
}

#[async_trait]
impl PluginRun for Checks {
    async fn before_tool(
        &mut self,
        call: &mut ToolCall,
        ctx: &PluginCtx,
    ) -> anyhow::Result<Decision> {
        let found = self.constitution.for_call(&call.name, &call.args);
        if found.is_empty() {
            return Ok(Decision::Allow);
        }
        let rules: Vec<(&Rule, String)> = found
            .iter()
            .map(|(rule, fields)| {
                let names: Vec<&str> =
                    fields.keys().map(String::as_str).collect();
                (
                    *rule,
                    format!(
                        "the `{}` of this `{}` call",
                        names.join("` and `"),
                        call.name
                    ),
                )
            })
            .collect();
        // One state for all: the tool, and only the fields rules name.
        let mut fields = Map::new();
        for (_, found) in &found {
            fields.extend(found.clone());
        }
        let state = json!({ "tool": call.name, "arguments": fields });
        let about = (Some(call.id.as_str()), Some(call.name.as_str()));
        let scores = match self.ask(state, &rules, about, ctx).await {
            Ok(scores) => scores,
            Err(error) => {
                let message = format!("Jev could not check the call: {error}");
                ctx.report(json!({
                    "kind": "error",
                    "call_id": call.id,
                    "tool": call.name,
                    "message": message,
                }));
                return Ok(match self.constitution.on_error {
                    OnError::Allow => Decision::Allow,
                    OnError::Block => Decision::Block(format!(
                        "Blocked by {NAME}: {message}, and it blocks what it \
                         cannot check. Try again."
                    )),
                });
            }
        };
        let broken: Vec<(&Rule, f64)> = rules
            .iter()
            .zip(&scores)
            .filter(|((rule, _), score)| **score >= rule.block)
            .map(|((rule, _), score)| (*rule, *score))
            .collect();
        let reason = (!broken.is_empty()).then(|| {
            let quoted: Vec<String> = broken
                .iter()
                .map(|(rule, score)| {
                    format!(
                        "rule {} (\"{}\"), violation probability {score:.2}",
                        rule.id, rule.text
                    )
                })
                .collect();
            format!(
                "Blocked by {NAME}: this call breaks {}. Change the call so \
                 it follows the rule, then try again.",
                quoted.join("; and ")
            )
        });
        for ((rule, _), score) in rules.iter().zip(&scores) {
            let kind = if *score >= rule.block {
                VerdictKind::Blocked
            } else if *score >= rule.review {
                VerdictKind::Flagged
            } else {
                continue;
            };
            let verdict = Verdict {
                kind,
                rule: rule.id.clone(),
                text: rule.text.clone(),
                score: *score,
                call_id: Some(call.id.clone()),
                tool: Some(call.name.clone()),
                reason: (kind == VerdictKind::Blocked)
                    .then(|| reason.clone())
                    .flatten(),
                hold: None,
                max_holds: None,
            };
            self.tell(&verdict, ctx).await;
        }
        Ok(match reason {
            Some(reason) => Decision::Block(reason),
            None => Decision::Allow,
        })
    }

    async fn before_stop(
        &mut self,
        message: &AssistantMessage,
        ctx: &PluginCtx,
    ) -> anyhow::Result<StopDecision> {
        let rules: Vec<(&Rule, String)> = self
            .constitution
            .for_final_answer()
            .into_iter()
            .map(|rule| (rule, "this final answer".to_owned()))
            .collect();
        if rules.is_empty() {
            return Ok(StopDecision::Stop);
        }
        let answer: String = message
            .content
            .iter()
            .filter_map(|block| match block {
                AssistantBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        let scores = self
            .ask(json!({ "final_answer": answer }), &rules, (None, None), ctx)
            .await
            .map_err(|error| {
                anyhow::anyhow!("Jev could not check the final answer: {error}")
            })?;
        let mut held: Vec<String> = Vec::new();
        for ((rule, _), score) in rules.iter().zip(&scores) {
            let broken = *score >= rule.block;
            let kind = if broken && self.holds < self.constitution.max_holds {
                VerdictKind::Held
            } else if *score >= rule.review {
                // Broken past the cap, or only doubtful: a person looks.
                VerdictKind::Flagged
            } else {
                continue;
            };
            let reason = (kind == VerdictKind::Held).then(|| {
                format!(
                    "Your answer breaks rule {} (\"{}\"). Revise it so it \
                     follows the rule, then answer again.",
                    rule.id, rule.text
                )
            });
            held.extend(reason.clone());
            let held = kind == VerdictKind::Held;
            let verdict = Verdict {
                kind,
                rule: rule.id.clone(),
                text: rule.text.clone(),
                score: *score,
                call_id: None,
                tool: None,
                reason,
                hold: held.then_some(self.holds + 1),
                max_holds: held.then_some(self.constitution.max_holds),
            };
            self.tell(&verdict, ctx).await;
        }
        if held.is_empty() {
            return Ok(StopDecision::Stop);
        }
        self.holds += 1;
        Ok(StopDecision::Continue(held.join("\n")))
    }
}
