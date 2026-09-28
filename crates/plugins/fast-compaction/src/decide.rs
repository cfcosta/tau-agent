//! Asking Jev what each tool call is still worth.
//!
//! Ported from `tamaratran/fast-jev-compaction` by way of
//! `joelhooks/pi-fast-jev-compaction` (`src/core/decide.ts`; see
//! `THIRD_PARTY_NOTICES.md`).

use std::collections::BTreeMap;

use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use tau_ai::message::Usage;
use tau_jev::{Jev, Question, Request};

use crate::state::{Call, Fitted, estimate_tokens, to_value};

/// Tokens a request takes besides its state and questions.
const REQUEST_OVERHEAD_TOKENS: usize = 20;

/// What happens to a tool call. Decisions only ever escalate, in this
/// order.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// The call and its result stay.
    Keep,
    /// The call stays; its result is cut to its head and a note.
    DropResult,
    /// The call and its result go.
    DropCall,
}

/// The decision for one call, with Jev's answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub call_id: String,
    pub tool: String,
    pub action: Action,
    /// Jev's probability that the call should stay.
    pub keep_call: f64,
    /// Jev's probability that the full result should stay.
    pub keep_result: f64,
}

/// The two questions for one call.
pub fn questions_for(call: &Call) -> [(String, Question); 2] {
    [
        (
            format!("call_{}", call.id),
            Question::noul(format!(
                "Tool call {} ({}) should stay in the history: knowing this call was made, with its input, still matters for what the assistant does next",
                call.id, call.tool
            )),
        ),
        (
            format!("result_{}", call.id),
            Question::noul(format!(
                "The full output of tool call {} ({}, {} chars) should stay in the history verbatim: the assistant still needs its contents and re-running the tool would not do",
                call.id, call.tool, call.result_chars
            )),
        ),
    ]
}

/// Splits `calls` into batches whose questions, with the state, fit
/// `max_request_tokens`.
pub fn batch_calls<'a>(
    calls: &[&'a Call],
    state_tokens: usize,
    max_request_tokens: usize,
) -> Result<Vec<Vec<&'a Call>>, String> {
    let budget = max_request_tokens
        .saturating_sub(state_tokens)
        .saturating_sub(REQUEST_OVERHEAD_TOKENS);
    let mut batches = Vec::new();
    let mut current: Vec<&Call> = Vec::new();
    let mut current_tokens = 0;
    for call in calls {
        let questions: BTreeMap<String, Question> =
            questions_for(call).into_iter().collect();
        let tokens = estimate_tokens(
            &serde_json::to_string(&questions).expect("questions serialize"),
        );
        if !current.is_empty() && current_tokens + tokens > budget {
            batches.push(std::mem::take(&mut current));
            current_tokens = 0;
        }
        if current.is_empty() && tokens > budget {
            return Err(format!(
                "the state leaves no room for questions (~{state_tokens} of {max_request_tokens} tokens)"
            ));
        }
        current.push(call);
        current_tokens += tokens;
    }
    if !current.is_empty() {
        batches.push(current);
    }
    Ok(batches)
}

/// The decision for one call: pinned calls and calls whose full result
/// still matters are kept; a call that matters without its result keeps
/// only the result's head; anything else goes.
pub fn decide_call(
    call: &Call,
    keep_call: f64,
    keep_result: f64,
    threshold: f64,
) -> Decision {
    let action = if call.pinned || keep_result >= threshold {
        Action::Keep
    } else if keep_call >= threshold {
        Action::DropResult
    } else {
        Action::DropCall
    };
    Decision {
        call_id: call.call_id.clone(),
        tool: call.tool.clone(),
        action,
        keep_call,
        keep_result,
    }
}

/// Decisions for every call, and how many requests they took.
#[derive(Debug, Clone, PartialEq)]
pub struct Decided {
    pub decisions: Vec<Decision>,
    pub requests: usize,
}

/// Asks about every unpinned call, in batches sent together, and decides
/// every call. A pinned call is kept without asking. Every response is
/// passed to `charge`, including when another batch failed, so no
/// answered request goes unpaid.
pub async fn decide(
    jev: &dyn Jev,
    calls: &[Call],
    fitted: &Fitted,
    max_request_tokens: usize,
    threshold: f64,
    charge: impl Fn(&Usage),
) -> anyhow::Result<Decided> {
    let candidates: Vec<&Call> =
        calls.iter().filter(|call| !call.pinned).collect();
    let batches = batch_calls(&candidates, fitted.tokens, max_request_tokens)
        .map_err(anyhow::Error::msg)?;
    let state = to_value(&fitted.state);
    let responses = join_all(batches.iter().map(|batch| {
        let request =
            batch
                .iter()
                .fold(Request::new(state.clone()), |request, call| {
                    questions_for(call).into_iter().fold(
                        request,
                        |request, (id, question)| {
                            request.question(id, question)
                        },
                    )
                });
        async move { jev.ask(&request).await }
    }))
    .await;
    for response in responses.iter().flatten() {
        charge(&response.usage());
    }
    let responses = responses.into_iter().collect::<Result<Vec<_>, _>>()?;

    let mut answers: BTreeMap<&str, (f64, f64)> = BTreeMap::new();
    for (batch, response) in batches.iter().zip(&responses) {
        for call in batch {
            let keep_call = response.noul(&format!("call_{}", call.id))?;
            let keep_result = response.noul(&format!("result_{}", call.id))?;
            answers.insert(call.id.as_str(), (keep_call, keep_result));
        }
    }
    let decisions = calls
        .iter()
        .map(|call| {
            let (keep_call, keep_result) =
                answers.get(call.id.as_str()).copied().unwrap_or((1.0, 1.0));
            decide_call(call, keep_call, keep_result, threshold)
        })
        .collect();
    Ok(Decided {
        decisions,
        requests: batches.len(),
    })
}
