//! tau-constitution's interface half: a repository's rules, what the
//! checks decided as they record it, and how every interface draws both.
//! The checks themselves, and the database the rules are kept in, are
//! `tau-constitution-host`'s (ADR 0030).

pub mod rules;
pub mod ui;

use serde::{Deserialize, Serialize};
use serde_json::Value;
pub use ui::ConstitutionUi;

pub use crate::rules::{
    Constitution,
    OnError,
    Rule,
    RuleError,
    StoredConstitution,
    StoredRule,
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
