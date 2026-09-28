//! A client for Jev, TypeSafe's System One model
//! (<https://docs.typesafe.ai>), for tau plugins.
//!
//! Jev does not generate text. It takes a `state` (text or JSON) and a
//! map of typed questions, and answers each one with a structured value:
//!
//! - [`Question::Noul`]: the probability that a yes/no statement holds;
//! - [`Question::Choice`]: one option from a set, with a probability for
//!   each and a confidence;
//! - [`Question::Score`]: a position on ordered levels, with a
//!   probability for each level and a confidence.
//!
//! Plugins depend on the [`Jev`] trait, not on the HTTP client, so tests
//! answer with [`fake::FakeJev`] and production uses [`TypeSafe`].
//! Every answer is checked before a plugin sees it: a missing answer, an
//! answer of the wrong kind, or a probability outside `[0, 1]` is an
//! error, never a default.

mod client;
pub mod fake;

use std::collections::BTreeMap;

use async_trait::async_trait;
pub use client::{API_KEY_VAR, MissingApiKey, TypeSafe, api_key};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_ai::message::{Usage, UsageCost};

/// The model most requests should use: TypeSafe's latest stable Jev.
pub const DEFAULT_MODEL: &str = "jev-latest";

/// The System One endpoint.
pub const SYSTEM_ONE_URL: &str = "https://api.typesafe.ai/v1/systemone";

/// US dollars per million input tokens for Jev 1.13. Output tokens are
/// free. Used for every model: it is the only price TypeSafe publishes.
pub const PRICE_PER_MILLION_INPUT: f64 = 0.042;

/// Asks Jev questions about a state.
#[async_trait]
pub trait Jev: Send + Sync + 'static {
    async fn ask(&self, request: &Request) -> Result<Response, JevError>;
}

/// One request: a state, and the questions to answer about it. The
/// client adds the model.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Request {
    pub state: Value,
    /// Question ids are yours; the model never sees them.
    pub questions: BTreeMap<String, Question>,
}

impl Request {
    pub fn new(state: impl Into<Value>) -> Self {
        Self {
            state: state.into(),
            questions: BTreeMap::new(),
        }
    }

    /// Adds a question under `id`.
    pub fn question(
        mut self,
        id: impl Into<String>,
        question: Question,
    ) -> Self {
        self.questions.insert(id.into(), question);
        self
    }
}

/// A typed question. `instructions` and the criteria may be text or
/// JSON (TypeSafe's "structured instructions").
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Question {
    Noul {
        instructions: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        criteria: Option<NoulCriteria>,
    },
    Choice {
        instructions: Value,
        /// Option name to its description (`null` for none). At most
        /// 255 options.
        criteria: BTreeMap<String, Value>,
    },
    Score {
        instructions: Value,
        /// Level descriptions, lowest first. Two to ten.
        criteria: Vec<Value>,
    },
}

/// What counts as yes and as no for a [`Question::Noul`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NoulCriteria {
    #[serde(rename = "true", skip_serializing_if = "Option::is_none")]
    pub yes: Option<Value>,
    #[serde(rename = "false", skip_serializing_if = "Option::is_none")]
    pub no: Option<Value>,
}

impl Question {
    /// A yes/no question.
    pub fn noul(instructions: impl Into<Value>) -> Self {
        Self::Noul {
            instructions: instructions.into(),
            criteria: None,
        }
    }

    /// A choice among `options`: name and description pairs.
    pub fn choice(
        instructions: impl Into<Value>,
        options: impl IntoIterator<Item = (impl Into<String>, impl Into<Value>)>,
    ) -> Self {
        Self::Choice {
            instructions: instructions.into(),
            criteria: options
                .into_iter()
                .map(|(name, description)| (name.into(), description.into()))
                .collect(),
        }
    }

    /// A score over `levels`, lowest first.
    pub fn score(
        instructions: impl Into<Value>,
        levels: impl IntoIterator<Item = impl Into<Value>>,
    ) -> Self {
        Self::Score {
            instructions: instructions.into(),
            criteria: levels.into_iter().map(Into::into).collect(),
        }
    }
}

/// Jev's answers to one request.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Response {
    /// The versioned model that answered, such as `jev-1.13.0`.
    pub model: String,
    pub answers: BTreeMap<String, Answer>,
    pub usage: JevUsage,
}

/// One answer, of the kind its question asked for.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Answer {
    Noul {
        noul: f64,
    },
    Choice {
        choice: String,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
    },
    Score {
        score: f64,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub struct JevUsage {
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
}

impl Response {
    /// The probability of yes for the Noul under `id`.
    pub fn noul(&self, id: &str) -> Result<f64, JevError> {
        match self.answer(id)? {
            Answer::Noul { noul } => probability(id, *noul),
            _ => Err(JevError::WrongAnswer {
                id: id.to_owned(),
                expected: "noul",
            }),
        }
    }

    /// The option picked for the Choice under `id`, and its confidence.
    pub fn choice(&self, id: &str) -> Result<(&str, f64), JevError> {
        match self.answer(id)? {
            Answer::Choice {
                choice, confidence, ..
            } => Ok((choice.as_str(), probability(id, *confidence)?)),
            _ => Err(JevError::WrongAnswer {
                id: id.to_owned(),
                expected: "choice",
            }),
        }
    }

    /// The score for the Score under `id` (0 for the lowest level), and
    /// its confidence. JSON has no non-finite numbers, so a score is
    /// always a number.
    pub fn score(&self, id: &str) -> Result<(f64, f64), JevError> {
        match self.answer(id)? {
            Answer::Score {
                score, confidence, ..
            } => Ok((*score, probability(id, *confidence)?)),
            _ => Err(JevError::WrongAnswer {
                id: id.to_owned(),
                expected: "score",
            }),
        }
    }

    fn answer(&self, id: &str) -> Result<&Answer, JevError> {
        self.answers
            .get(id)
            .ok_or_else(|| JevError::MissingAnswer(id.to_owned()))
    }

    /// The request's usage as tau counts it: input tokens, and their
    /// cost at [`PRICE_PER_MILLION_INPUT`], for `PluginCtx::charge`.
    pub fn usage(&self) -> Usage {
        let cost = self.usage.input_tokens as f64 * PRICE_PER_MILLION_INPUT
            / 1_000_000.0;
        Usage {
            input: self.usage.input_tokens,
            output: self.usage.output_tokens,
            total_tokens: self.usage.input_tokens + self.usage.output_tokens,
            cost: UsageCost {
                input: cost,
                total: cost,
                ..UsageCost::default()
            },
            ..Usage::default()
        }
    }
}

fn probability(id: &str, value: f64) -> Result<f64, JevError> {
    if (0.0..=1.0).contains(&value) {
        Ok(value)
    } else {
        Err(JevError::OutOfRange {
            id: id.to_owned(),
            value,
        })
    }
}

/// Why Jev gave no usable answer.
#[derive(Debug, Clone, PartialEq)]
pub enum JevError {
    /// The request could not be sent, or no response came back.
    Transport(String),
    /// TypeSafe answered with an error status. The body is not kept: it
    /// can echo the request.
    Status(u16),
    /// The response is not the JSON TypeSafe documents.
    Malformed(String),
    MissingAnswer(String),
    WrongAnswer {
        id: String,
        expected: &'static str,
    },
    /// A probability outside `[0, 1]`.
    OutOfRange {
        id: String,
        value: f64,
    },
}

impl std::fmt::Display for JevError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(message) => {
                write!(f, "Jev request failed: {message}")
            }
            Self::Status(status) => {
                write!(f, "Jev answered with status {status}")
            }
            Self::Malformed(message) => {
                write!(f, "Jev's response is malformed: {message}")
            }
            Self::MissingAnswer(id) => write!(f, "Jev gave no answer for {id}"),
            Self::WrongAnswer { id, expected } => {
                write!(f, "Jev's answer for {id} is not a {expected}")
            }
            Self::OutOfRange { id, value } => {
                write!(f, "Jev's answer for {id} is out of range: {value}")
            }
        }
    }
}

impl std::error::Error for JevError {}
