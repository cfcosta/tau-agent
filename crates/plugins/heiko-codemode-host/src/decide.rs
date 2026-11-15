//! The `decide` global's requests and answers, as JSON.
//!
//! - `decide.predicate({ state, question, yes?, no? })` → `{ probability }`
//! - `decide.choice({ state, question, options })` → `{ choice, confidence,
//!   probabilities }`
//! - `decide.score({ state, question, levels })` → `{ score, confidence,
//!   probabilities }`
//! - `decide.ask({ state, questions = { id = { kind, question, ... } } })` →
//!   `{ id = answer }`, in one request.
//!
//! `state_json`, JSON text, may stand for `state`: its keys keep their
//! order.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};
use heiko_decide::{Answer, DecideError, PredicateCriteria, Question, Request, Response};

/// Decider requests a script may have in flight at once, as pi limits its
/// classifier calls.
pub const MAX_IN_FLIGHT: usize = 4;

/// The most options a choice may have (heiko-decide's limit).
pub const MAX_OPTIONS: usize = 255;

/// A score has between this many levels...
pub const MIN_LEVELS: usize = 2;
/// ...and this many.
pub const MAX_LEVELS: usize = 10;

/// The id of the one question `predicate`, `choice` and `score` ask.
const SINGLE: &str = "answer";

/// A question's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Predicate,
    Choice,
    Score,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Predicate => "predicate",
            Self::Choice => "choice",
            Self::Score => "score",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        match name {
            "predicate" => Some(Self::Predicate),
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

/// One `decide.<function>(args)` call as a request. `function` is `predicate`,
/// `choice`, `score` or `ask`.
pub fn request(
    function: &str,
    args: &Value,
) -> Result<(Request, Shape), String> {
    let Value::Object(map) = args else {
        return Err(format!("decide.{function} takes one table"));
    };
    // A state given as JSON text keeps its keys in the order written: a
    // Luau table keeps none.
    let state = match map.get("state_json") {
        Some(Value::String(text)) => {
            serde_json::from_str(text).map_err(|error| {
                format!("decide.{function}: `state_json` is not JSON: {error}")
            })?
        }
        Some(_) => {
            return Err(format!("decide.{function}: `state_json` is a text"));
        }
        None => map.get("state").cloned().unwrap_or(Value::Null),
    };
    if function == "ask" {
        let Some(Value::Object(questions)) = map.get("questions") else {
            return Err(
                "decide.ask needs `questions`, a table of questions by id".into(),
            );
        };
        if questions.is_empty() {
            return Err("decide.ask needs at least one question".into());
        }
        let mut request = Request::new(state);
        let mut kinds = BTreeMap::new();
        for (id, spec) in questions {
            let Value::Object(spec) = spec else {
                return Err(format!(
                    "decide.ask: question `{id}` must be a table"
                ));
            };
            let kind = spec
                .get("kind")
                .and_then(Value::as_str)
                .and_then(Kind::parse)
                .ok_or_else(|| {
                    format!(
                        "decide.ask: question `{id}` needs `kind`: \"predicate\", \
                         \"choice\" or \"score\""
                    )
                })?;
            let question = question(kind, spec).map_err(|error| {
                format!("decide.ask: question `{id}`: {error}")
            })?;
            request = request.question(id.clone(), question);
            kinds.insert(id.clone(), kind);
        }
        return Ok((request, Shape::Many(kinds)));
    }
    let kind = Kind::parse(function)
        .ok_or_else(|| format!("decide has no function `{function}`"))?;
    let question = question(kind, map)
        .map_err(|error| format!("decide.{function}: {error}"))?;
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
        Kind::Predicate => {
            let yes = spec.get("yes").filter(|v| !v.is_null()).cloned();
            let no = spec.get("no").filter(|v| !v.is_null()).cloned();
            Ok(Question::Predicate {
                instructions,
                criteria: (yes.is_some() || no.is_some())
                    .then_some(PredicateCriteria { yes, no }),
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

/// The script's value for `response`, checked as heiko-decide checks it.
pub fn answer(shape: &Shape, response: &Response) -> Result<Value, DecideError> {
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

fn one(kind: Kind, id: &str, response: &Response) -> Result<Value, DecideError> {
    let probabilities = |id: &str| match response.answers.get(id) {
        Some(
            Answer::Choice { probabilities, .. }
            | Answer::Score { probabilities, .. },
        ) => json!(probabilities),
        _ => Value::Null,
    };
    Ok(match kind {
        Kind::Predicate => json!({ "probability": response.predicate(id)? }),
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
