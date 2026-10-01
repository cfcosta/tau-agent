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
//!   lets the call run, or the answer stand; `block` refuses the call,
//!   or sends the answer back while holds are left. Either way the
//!   failure is reported and recorded (`"kind": "error"`).
//! - The rules are read again at every check, from a [`Live`] handle the
//!   host updates when they are edited: an edit applies from the next
//!   tool call, in runs already going too.

pub mod db;
pub mod rules;
pub mod ui;

use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tau_agent::{
    error::PluginError,
    plugin::{
        Decision,
        Plugin,
        PluginCtx,
        PluginRun,
        RunPlan,
        StopDecision,
        ToolCall,
    },
};
use tau_ai::message::AssistantMessage;
use tau_jev::{Jev, NoulCriteria, Question, Request};

pub use crate::rules::{
    Constitution,
    ConstitutionError,
    OnError,
    Rule,
    RuleError,
    Target,
};

/// The name the plugin goes by in events, reports and records.
pub const NAME: &str = "tau-constitution";

/// What the plugin decided about one rule, as it reports and records
/// it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    /// Stored as the record's own kind ([`Record`]).
    #[serde(skip)]
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

/// A check Jev could not answer, and what `on_error` did about it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Failure {
    /// The call it was about; `None` for the final answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    pub message: String,
    /// `block` or `allow`.
    #[serde(default)]
    pub on_error: String,
    /// For the final answer: whether it was sent back.
    #[serde(default)]
    pub held: bool,
}

/// What tau-constitution publishes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", from = "Wire")]
pub enum Record {
    /// Every applicable rule's score on a call or the final answer.
    Checked(Check),
    Blocked(Verdict),
    Flagged(Verdict),
    Held(Verdict),
    Error(Failure),
    /// What the interface folds as a run starts, never stored: on, or
    /// off and why.
    Starting {
        status: Option<String>,
    },
}

/// [`Record`] as stored: a verdict's kind is the record's.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum Wire {
    Checked(Check),
    Blocked(Verdict),
    Flagged(Verdict),
    Held(Verdict),
    Error(Failure),
    Starting { status: Option<String> },
}

impl From<Wire> for Record {
    fn from(wire: Wire) -> Self {
        let verdict = |kind, verdict| Verdict { kind, ..verdict };
        match wire {
            Wire::Checked(check) => Self::Checked(check),
            Wire::Blocked(v) => Self::Blocked(verdict(VerdictKind::Blocked, v)),
            Wire::Flagged(v) => Self::Flagged(verdict(VerdictKind::Flagged, v)),
            Wire::Held(v) => Self::Held(verdict(VerdictKind::Held, v)),
            Wire::Error(failure) => Self::Error(failure),
            Wire::Starting { status } => Self::Starting { status },
        }
    }
}

impl Record {
    /// The record `body` holds, or none, said the first time.
    pub fn parse(body: &Value) -> Option<Self> {
        tau_agent::plugin::read_record(NAME, body)
    }

    /// `verdict` as the record of its kind.
    pub fn verdict(verdict: Verdict) -> Self {
        match verdict.kind {
            VerdictKind::Blocked => Self::Blocked(verdict),
            VerdictKind::Flagged => Self::Flagged(verdict),
            VerdictKind::Held => Self::Held(verdict),
        }
    }
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum VerdictKind {
    /// The call was refused.
    Blocked,
    /// The call ran (or the answer stood); a person should look.
    #[default]
    Flagged,
    /// The final answer was sent back.
    Held,
}

/// A constitution that can change while runs use it. Share one per
/// repository: [`Live::set`] replaces the rules, and every run's next
/// check reads the new ones.
#[derive(Debug, Clone, Default)]
pub struct Live(Arc<RwLock<Arc<Constitution>>>);

impl Live {
    pub fn new(constitution: Constitution) -> Self {
        Self(Arc::new(RwLock::new(Arc::new(constitution))))
    }

    /// The rules as they are now.
    pub fn get(&self) -> Arc<Constitution> {
        self.0.read().expect("not poisoned").clone()
    }

    pub fn set(&self, constitution: Constitution) {
        *self.0.write().expect("not poisoned") = Arc::new(constitution);
    }
}

/// The plugin. Share one across an agent's runs.
#[derive(Clone)]
pub struct ConstitutionPlugin {
    jev: Arc<dyn Jev>,
    rules: Live,
}

impl ConstitutionPlugin {
    /// Checks against rules that never change.
    pub fn new(jev: Arc<dyn Jev>, constitution: Constitution) -> Self {
        Self::live(jev, Live::new(constitution))
    }

    /// Checks against `rules` as they are at each check.
    pub fn live(jev: Arc<dyn Jev>, rules: Live) -> Self {
        Self { jev, rules }
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
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        Ok(Box::new(Checks {
            jev: self.jev.clone(),
            rules: self.rules.clone(),
            holds: 0,
        }))
    }
}

/// A run's checks.
struct Checks {
    jev: Arc<dyn Jev>,
    rules: Live,
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
            request = request.question(question_id(n), question(rule, what));
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
        ctx.publish(&Record::Checked(check)).await;
        Ok(scores)
    }

    /// Reports and records a check Jev could not answer: history shows it,
    /// and the Constitution screen counts it.
    async fn failed(&self, failure: Failure, ctx: &PluginCtx) {
        ctx.publish(&Record::Error(failure)).await;
    }

    async fn tell(&self, verdict: Verdict, ctx: &PluginCtx) {
        ctx.publish(&Record::verdict(verdict)).await;
    }
}

fn question_id(n: usize) -> String {
    format!("rule-{n}")
}

/// The question a check asks about `rule` on `what` (such as "the
/// `command` of this `bash` call").
fn question(rule: &Rule, what: &str) -> Question {
    Question::Noul {
        instructions: format!(
            "Does {what} break this rule? Judge only what is shown.\n\n\
             Rule {}: {}",
            rule.id, rule.text
        )
        .into(),
        criteria: Some(NoulCriteria {
            yes: Some(format!("{what} breaks the rule.").into()),
            no: Some(
                format!(
                    "{what} follows the rule, or the rule is not about it."
                )
                .into(),
            ),
        }),
    }
}

/// What a call's check shows Jev, and what the question calls it.
fn call_state(tool: &str, fields: &Map<String, Value>) -> (Value, String) {
    let names: Vec<&str> = fields.keys().map(String::as_str).collect();
    (
        json!({ "tool": tool, "arguments": fields }),
        format!("the `{}` of this `{tool}` call", names.join("` and `")),
    )
}

/// Something a rule was tried on, and Jev's probability that it breaks
/// the rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trial {
    /// The tool, or `None` for a final answer.
    pub tool: Option<String>,
    /// What the rule reads of it: the fields' values, or the answer.
    pub shown: String,
    pub score: f64,
}

/// Tries `rule` on past calls (tool, arguments) and final answers, as a
/// check would ask about each: those it does not apply to are skipped.
/// Returns each trial, and what Jev cost. Nothing runs again; Jev sees
/// only what a check would show it.
pub async fn try_rule(
    jev: &dyn Jev,
    rule: &Rule,
    calls: &[(String, Value)],
    answers: &[String],
) -> Result<(Vec<Trial>, f64), String> {
    let one = Constitution {
        rules: vec![rule.clone()],
        ..Constitution::default()
    };
    let mut asks: Vec<(Option<String>, String, Value, String)> = Vec::new();
    for (tool, args) in calls {
        if let Some((_, fields)) = one.for_call(tool, args).into_iter().next() {
            let shown = fields
                .values()
                .map(|value| match value {
                    Value::String(text) => text.clone(),
                    other => other.to_string(),
                })
                .collect::<Vec<_>>()
                .join(" · ");
            let (state, what) = call_state(tool, &fields);
            asks.push((Some(tool.clone()), shown, state, what));
        }
    }
    if !one.for_final_answer().is_empty() {
        for answer in answers {
            asks.push((
                None,
                answer.clone(),
                json!({ "final_answer": answer }),
                "this final answer".to_owned(),
            ));
        }
    }
    let mut trials = Vec::with_capacity(asks.len());
    let mut cost = 0.0;
    for (tool, shown, state, what) in asks {
        let request =
            Request::new(state).question(question_id(0), question(rule, &what));
        let response = jev.ask(&request).await.map_err(|e| e.to_string())?;
        cost += response.usage().cost.total;
        let score =
            response.noul(&question_id(0)).map_err(|e| e.to_string())?;
        trials.push(Trial { tool, shown, score });
    }
    Ok((trials, cost))
}

#[async_trait]
impl PluginRun for Checks {
    async fn before_tool(
        &mut self,
        call: &mut ToolCall,
        ctx: &PluginCtx,
    ) -> Result<Decision, PluginError> {
        let constitution = self.rules.get();
        let found = constitution.for_call(&call.name, &call.args);
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
                self.failed(
                    Failure {
                        call_id: Some(call.id.clone()),
                        tool: Some(call.name.clone()),
                        message: message.clone(),
                        on_error: constitution.on_error.as_str().to_owned(),
                        held: false,
                    },
                    ctx,
                )
                .await;
                return Ok(match constitution.on_error {
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
            self.tell(verdict, ctx).await;
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
    ) -> Result<StopDecision, PluginError> {
        let constitution = self.rules.get();
        let rules: Vec<(&Rule, String)> = constitution
            .for_final_answer()
            .into_iter()
            .map(|rule| (rule, "this final answer".to_owned()))
            .collect();
        if rules.is_empty() {
            return Ok(StopDecision::Stop);
        }
        let answer: String = message.text();
        let scores = match self
            .ask(json!({ "final_answer": answer }), &rules, (None, None), ctx)
            .await
        {
            Ok(scores) => scores,
            Err(error) => {
                // `on_error` decides, as for a call: `block` sends the
                // answer back while holds are left; past them, or with
                // `allow`, it stands.
                let message =
                    format!("Jev could not check the final answer: {error}");
                let held = constitution.on_error == OnError::Block
                    && self.holds < constitution.max_holds;
                self.failed(
                    Failure {
                        call_id: None,
                        tool: None,
                        message: message.clone(),
                        on_error: constitution.on_error.as_str().to_owned(),
                        held,
                    },
                    ctx,
                )
                .await;
                if !held {
                    return Ok(StopDecision::Stop);
                }
                self.holds += 1;
                return Ok(StopDecision::Continue(format!(
                    "{NAME} could not check your answer against this \
                     repository's rules ({message}), and it does not let \
                     an unchecked answer stand. Answer again."
                )));
            }
        };
        let mut held: Vec<String> = Vec::new();
        for ((rule, _), score) in rules.iter().zip(&scores) {
            let broken = *score >= rule.block;
            let kind = if broken && self.holds < constitution.max_holds {
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
                max_holds: held.then_some(constitution.max_holds),
            };
            self.tell(verdict, ctx).await;
        }
        if held.is_empty() {
            return Ok(StopDecision::Stop);
        }
        self.holds += 1;
        Ok(StopDecision::Continue(held.join("\n")))
    }
}
