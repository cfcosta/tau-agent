//! Asking Jev what each tool call is still worth.
//!
//! Ported from `tamaratran/fast-jev-compaction` by way of
//! `joelhooks/pi-fast-jev-compaction` (`src/core/decide.ts`), with the
//! partitioned history of `tamaratran/jev-pruner` (`src/history.ts`; see
//! `THIRD_PARTY_NOTICES.md`).

use std::collections::BTreeMap;

use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tau_ai::message::Usage;
use tau_jev::{Jev, Question, Request};

use crate::{
    Settings,
    history::split_history,
    plan::{self, Item, Tally},
    state::{
        Call,
        Entry,
        STATE_CONTEXT,
        State,
        call_record,
        goal_or_prompts,
        history_records,
        json_tenths,
    },
};

/// Tenths of a token for a comma between items.
const COMMA_TENTHS: usize = 9;

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
    /// The call stays; its result is cut to its head and a note naming
    /// the archive that holds it whole.
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
    /// Where the result was archived when it was cut.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive: Option<String>,
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
                "The full output of tool call {} ({}, {} chars) should stay in the history verbatim: the assistant still needs its contents and reading them back from an archive or re-running the tool would not do",
                call.id, call.tool, call.result_chars
            )),
        ),
    ]
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
        archive: None,
    }
}

/// The requests for a pass: each with the segment its state holds and
/// the unpinned calls it asks about, by index into `calls`.
#[derive(Debug, Clone, PartialEq)]
pub struct Requests {
    pub requests: Vec<(Request, usize, Vec<usize>)>,
    /// How many segments the history came in: 1 when it fit whole.
    pub segments: usize,
    /// The largest state's estimated tokens.
    pub state_tokens: usize,
}

/// Plans the requests that ask about every unpinned call. When the
/// whole history fits `max_state_tokens` with the goal, every request
/// shares that one state, as in pi. Otherwise the history is split in
/// order into segments (never abridged; see
/// [`crate::history::split_history`]), each state holds one segment and
/// the calls it asks about, and every call is asked about against every
/// segment. Questions go in batches that fit `max_request_tokens` with
/// their state.
pub fn requests(
    entries: &[Entry],
    calls: &[Call],
    goal: Option<&str>,
    max_state_tokens: usize,
    max_request_tokens: usize,
) -> Result<Requests, String> {
    let goal = goal_or_prompts(goal, entries);
    let records = history_records(entries, calls);
    let candidates: Vec<usize> = calls
        .iter()
        .enumerate()
        .filter(|(_, call)| !call.pinned)
        .map(|(index, _)| index)
        .collect();
    let state_of = |history, calls| State {
        context: STATE_CONTEXT,
        goal: goal.clone(),
        history,
        calls,
    };
    let question_tenths = |call: &Call| {
        json_tenths(
            &questions_for(call).into_iter().collect::<BTreeMap<_, _>>(),
        )
    };
    let whole = state_of(records.clone(), None);
    let (segments, alongside) = if json_tenths(&whole) <= max_state_tokens * 10
    {
        (vec![records], false)
    } else {
        let every: Vec<_> = candidates
            .iter()
            .map(|&index| call_record(&calls[index]))
            .collect();
        let fixed =
            json_tenths(&state_of(Vec::new(), Some(Vec::new()))).div_ceil(10);
        let all = json_tenths(&state_of(Vec::new(), Some(every.clone())))
            .div_ceil(10);
        let largest = every
            .iter()
            .map(|record| (json_tenths(record) + COMMA_TENTHS).div_ceil(10))
            .filter(|&tokens| fixed + tokens <= max_state_tokens)
            .max()
            .unwrap_or(0);
        let reserve = plan::reserve(fixed, all, largest, max_state_tokens);
        let segments = split_history(
            &records,
            max_state_tokens
                .checked_sub(reserve)
                .filter(|room| *room > 0)
                .ok_or_else(|| {
                    format!(
                        "the goal leaves no room for the history (~{fixed} of {max_state_tokens} tokens)"
                    )
                })?,
        )?;
        (segments, true)
    };
    let items: Vec<Item> = candidates
        .iter()
        .map(|&index| Item {
            state_tenths: if alongside {
                json_tenths(&call_record(&calls[index])) + COMMA_TENTHS
            } else {
                0
            },
            question_tenths: question_tenths(&calls[index]),
        })
        .collect();
    let empty = alongside.then(Vec::new);
    let bases: Vec<usize> = segments
        .iter()
        .map(|segment| json_tenths(&state_of(segment.clone(), empty.clone())))
        .collect();
    let planned =
        plan::plan(&bases, &items, max_state_tokens, max_request_tokens);
    let mut state_tokens = 0;
    let requests = planned
        .into_iter()
        .map(|planned| {
            let state = state_of(
                segments[planned.segment].clone(),
                alongside.then(|| {
                    planned
                        .group
                        .iter()
                        .map(|&item| call_record(&calls[candidates[item]]))
                        .collect()
                }),
            );
            state_tokens = state_tokens.max(json_tenths(&state).div_ceil(10));
            let asked: Vec<usize> =
                planned.batch.iter().map(|&item| candidates[item]).collect();
            let request = asked.iter().fold(
                Request::new(json!(state)),
                |request, &index| {
                    questions_for(&calls[index]).into_iter().fold(
                        request,
                        |request, (id, question)| {
                            request.question(id, question)
                        },
                    )
                },
            );
            (request, planned.segment, asked)
        })
        .collect();
    Ok(Requests {
        requests,
        segments: segments.len(),
        state_tokens,
    })
}

/// Decisions for every call, and how the asking went.
#[derive(Debug, Clone, PartialEq)]
pub struct Decided {
    pub decisions: Vec<Decision>,
    pub requests: usize,
    pub segments: usize,
    pub state_tokens: usize,
}

/// Asks about every unpinned call, in requests sent together (see
/// [`requests`]), and decides every call. A pinned call is kept without
/// asking; so is a call some segment was not asked about. Otherwise each
/// answer is its largest over the segments: a keep in any segment
/// keeps. Every response is passed to `charge`, including when another
/// request failed, so no answered request goes unpaid; any failure
/// fails the whole.
pub async fn decide(
    jev: &dyn Jev,
    entries: &[Entry],
    calls: &[Call],
    settings: &Settings,
    charge: impl Fn(&Usage),
) -> anyhow::Result<Decided> {
    let planned = requests(
        entries,
        calls,
        settings.goal.as_deref(),
        settings.max_state_tokens,
        settings.max_request_tokens,
    )
    .map_err(anyhow::Error::msg)?;
    let responses = join_all(
        planned
            .requests
            .iter()
            .map(|(request, _, _)| jev.ask(request)),
    )
    .await;
    for response in responses.iter().flatten() {
        charge(&response.usage());
    }
    let responses = responses.into_iter().collect::<Result<Vec<_>, _>>()?;

    let mut tally = Tally::new(calls.len(), planned.segments);
    for ((_, segment, asked), response) in
        planned.requests.iter().zip(&responses)
    {
        for &index in asked {
            let id = &calls[index].id;
            let keep_call = response.noul(&format!("call_{id}"))?;
            let keep_result = response.noul(&format!("result_{id}"))?;
            tally.record(index, *segment, vec![keep_call, keep_result]);
        }
    }
    let decisions = calls
        .iter()
        .enumerate()
        .map(|(index, call)| {
            let (keep_call, keep_result) = match tally.complete(index) {
                Some(answers) => (answers[0], answers[1]),
                None => (1.0, 1.0),
            };
            decide_call(call, keep_call, keep_result, settings.keep_threshold)
        })
        .collect();
    Ok(Decided {
        decisions,
        requests: planned.requests.len(),
        segments: planned.segments,
        state_tokens: planned.state_tokens,
    })
}
