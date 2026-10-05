//! The `jev` global's requests and answers, as JSON.
//!
//! - `jev.noul({ state, question, yes?, no? })` → `{ probability }`
//! - `jev.choice({ state, question, options })` → `{ choice, confidence,
//!   probabilities }`
//! - `jev.score({ state, question, levels })` → `{ score, confidence,
//!   probabilities }`
//! - `jev.ask({ state, questions = { id = { kind, question, ... } } })` →
//!   `{ id = answer }`, in one request.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};
use tau_jev::{Answer, JevError, NoulCriteria, Question, Request, Response};

/// Jev requests a script may have in flight at once, as pi limits its
/// classifier calls.
pub const MAX_IN_FLIGHT: usize = 4;

/// The most options a choice may have (tau-jev's limit).
pub const MAX_OPTIONS: usize = 255;

/// A score has between this many levels...
pub const MIN_LEVELS: usize = 2;
/// ...and this many.
pub const MAX_LEVELS: usize = 10;

/// The id of the one question `noul`, `choice` and `score` ask.
const SINGLE: &str = "answer";

/// A question's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Noul,
    Choice,
    Score,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Noul => "noul",
            Self::Choice => "choice",
            Self::Score => "score",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        match name {
            "noul" => Some(Self::Noul),
            "choice" => Some(Self::Choice),
            "score" => Some(Self::Score),
            _ => None,
        }
    }
}

/// What a request asked, to shape its answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shape {
    One(Kind),
    Many(BTreeMap<String, Kind>),
}

/// One `jev.<function>(args)` call as a request. `function` is `noul`,
/// `choice`, `score` or `ask`.
pub fn request(
    function: &str,
    args: &Value,
) -> Result<(Request, Shape), String> {
    let Value::Object(map) = args else {
        return Err(format!("jev.{function} takes one table"));
    };
    let state = map.get("state").cloned().unwrap_or(Value::Null);
    if function == "ask" {
        let Some(Value::Object(questions)) = map.get("questions") else {
            return Err(
                "jev.ask needs `questions`, a table of questions by id".into(),
            );
        };
        if questions.is_empty() {
            return Err("jev.ask needs at least one question".into());
        }
        let mut request = Request::new(state);
        let mut kinds = BTreeMap::new();
        for (id, spec) in questions {
            let Value::Object(spec) = spec else {
                return Err(format!(
                    "jev.ask: question `{id}` must be a table"
                ));
            };
            let kind = spec
                .get("kind")
                .and_then(Value::as_str)
                .and_then(Kind::parse)
                .ok_or_else(|| {
                    format!(
                        "jev.ask: question `{id}` needs `kind`: \"noul\", \
                         \"choice\" or \"score\""
                    )
                })?;
            let question = question(kind, spec).map_err(|error| {
                format!("jev.ask: question `{id}`: {error}")
            })?;
            request = request.question(id.clone(), question);
            kinds.insert(id.clone(), kind);
        }
        return Ok((request, Shape::Many(kinds)));
    }
    let kind = Kind::parse(function)
        .ok_or_else(|| format!("jev has no function `{function}`"))?;
    let question = question(kind, map)
        .map_err(|error| format!("jev.{function}: {error}"))?;
    Ok((
        Request::new(state).question(SINGLE, question),
        Shape::One(kind),
    ))
}

fn question(kind: Kind, spec: &Map<String, Value>) -> Result<Question, String> {
    let instructions = match spec.get("question") {
        Some(Value::Null) | None => {
            return Err("`question` is missing".into());
        }
        Some(value) => value.clone(),
    };
    match kind {
        Kind::Noul => {
            let yes = spec.get("yes").filter(|v| !v.is_null()).cloned();
            let no = spec.get("no").filter(|v| !v.is_null()).cloned();
            Ok(Question::Noul {
                instructions,
                criteria: (yes.is_some() || no.is_some())
                    .then_some(NoulCriteria { yes, no }),
            })
        }
        Kind::Choice => {
            let options: BTreeMap<String, Value> = match spec.get("options") {
                Some(Value::Object(map)) => map.clone().into_iter().collect(),
                Some(Value::Array(names)) => names
                    .iter()
                    .map(|name| {
                        name.as_str()
                            .map(|name| (name.to_owned(), Value::Null))
                            .ok_or("`options` names must be strings")
                    })
                    .collect::<Result<_, _>>()?,
                _ => {
                    return Err("`options` must be a table of option \
                                descriptions by name"
                        .into());
                }
            };
            if options.is_empty() || options.len() > MAX_OPTIONS {
                return Err(format!(
                    "`options` must have 1 to {MAX_OPTIONS} options; it has {}",
                    options.len()
                ));
            }
            Ok(Question::Choice {
                instructions,
                criteria: options,
            })
        }
        Kind::Score => {
            let Some(Value::Array(levels)) = spec.get("levels") else {
                return Err("`levels` must be a list, lowest first".into());
            };
            if !(MIN_LEVELS..=MAX_LEVELS).contains(&levels.len()) {
                return Err(format!(
                    "`levels` must have {MIN_LEVELS} to {MAX_LEVELS} levels; \
                     it has {}",
                    levels.len()
                ));
            }
            Ok(Question::Score {
                instructions,
                criteria: levels.clone(),
            })
        }
    }
}

/// The script's value for `response`, checked as tau-jev checks it.
pub fn answer(shape: &Shape, response: &Response) -> Result<Value, JevError> {
    match shape {
        Shape::One(kind) => one(*kind, SINGLE, response),
        Shape::Many(kinds) => {
            let mut out = Map::new();
            for (id, kind) in kinds {
                out.insert(id.clone(), one(*kind, id, response)?);
            }
            Ok(Value::Object(out))
        }
    }
}

fn one(kind: Kind, id: &str, response: &Response) -> Result<Value, JevError> {
    let probabilities = |id: &str| match response.answers.get(id) {
        Some(
            Answer::Choice { probabilities, .. }
            | Answer::Score { probabilities, .. },
        ) => json!(probabilities),
        _ => Value::Null,
    };
    Ok(match kind {
        Kind::Noul => json!({ "probability": response.noul(id)? }),
        Kind::Choice => {
            let (choice, confidence) = response.choice(id)?;
            json!({
                "choice": choice,
                "confidence": confidence,
                "probabilities": probabilities(id),
            })
        }
        Kind::Score => {
            let (score, confidence) = response.score(id)?;
            json!({
                "score": score,
                "confidence": confidence,
                "probabilities": probabilities(id),
            })
        }
    })
}
