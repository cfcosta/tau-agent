//! The transcript as Jev sees it: tool calls with their inputs and the
//! size of their results, never the results themselves, fitted to a
//! token budget.
//!
//! Ported from `tamaratran/fast-jev-compaction` by way of
//! `joelhooks/pi-fast-jev-compaction` (`src/core/state.ts`; see
//! `THIRD_PARTY_NOTICES.md`). Lengths count characters, where the
//! original counts UTF-16 units; they differ only outside the Basic
//! Multilingual Plane.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::{Map, Value, json};
use tau_ai::message::{AssistantBlock, InputBlock, Message, UserContent};

/// What the state tells Jev about itself.
pub const STATE_CONTEXT: &str = "A coding assistant conversation is being compacted to free context. `history` is the whole conversation so far, oldest first; tool outputs are replaced by a short `result` note and long texts may be abridged. Each question asks whether one tool call, or the full output of that call, still needs to stay in the history verbatim. Whatever is not kept is deleted from model context, but the assistant can re-run a tool or re-read a file.";

/// Tool inputs are cut to the first of these that fits, in order.
const INPUT_CHARS: [usize; 3] = [1000, 200, 60];
const TEXT_HEAD: usize = 400;
const TEXT_TAIL: usize = 150;

/// Who a message is from, as the state names it. Tool results count as
/// the user's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

/// A tool call the model made.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolUse {
    pub call_id: String,
    pub tool: String,
    pub input: Map<String, Value>,
}

/// A tool call's result.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutcome {
    pub call_id: String,
    pub text: String,
    pub is_error: bool,
}

/// One transcript message, reduced to what the decision needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub role: Role,
    pub text: String,
    pub uses: Vec<ToolUse>,
    pub outcomes: Vec<ToolOutcome>,
}

/// Reduces a transcript: text and thinking, tool calls with their
/// arguments, and tool results as text.
pub fn entries(transcript: &[Message]) -> Vec<Entry> {
    transcript
        .iter()
        .map(|message| match message {
            Message::User(user) => Entry {
                role: Role::User,
                text: match &user.content {
                    UserContent::Text(text) => text.clone(),
                    UserContent::Blocks(blocks) => block_text(blocks),
                },
                uses: Vec::new(),
                outcomes: Vec::new(),
            },
            Message::Assistant(assistant) => Entry {
                role: Role::Assistant,
                text: assistant
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        AssistantBlock::Text(text) => Some(text.text.clone()),
                        AssistantBlock::Thinking(thinking) => {
                            Some(format!("[thinking]\n{}", thinking.thinking))
                        }
                        AssistantBlock::ToolCall(_) => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
                uses: assistant
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        AssistantBlock::ToolCall(call) => Some(ToolUse {
                            call_id: call.id.clone(),
                            tool: call.name.clone(),
                            input: call.arguments.clone(),
                        }),
                        _ => None,
                    })
                    .collect(),
                outcomes: Vec::new(),
            },
            Message::ToolResult(result) => Entry {
                role: Role::User,
                text: String::new(),
                uses: Vec::new(),
                outcomes: vec![ToolOutcome {
                    call_id: result.tool_call_id.clone(),
                    text: block_text(&result.content),
                    is_error: result.is_error,
                }],
            },
        })
        .collect()
}

/// The text blocks of `blocks`, joined by newlines; images are left out.
pub fn block_text(blocks: &[InputBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            InputBlock::Text(text) => Some(text.text.as_str()),
            InputBlock::Image(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A tool call with a result, as the decision sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    /// `t1`, `t2`, …: the id questions use, short and free of the
    /// provider's id.
    pub id: String,
    pub call_id: String,
    pub tool: String,
    pub input: Map<String, Value>,
    pub call_index: usize,
    pub result_index: usize,
    pub result_chars: usize,
    pub is_error: bool,
    /// Pinned calls are always kept.
    pub pinned: bool,
}

/// Whether the message at `index` is pinned: the first one, or one of
/// the last `preserve_recent`.
pub fn is_pinned(index: usize, total: usize, preserve_recent: usize) -> bool {
    index == 0 || index + preserve_recent >= total
}

/// Every tool call that has a result, in order, pinned when its call or
/// its result is.
pub fn collect_calls(entries: &[Entry], preserve_recent: usize) -> Vec<Call> {
    let mut results: BTreeMap<&str, (usize, &ToolOutcome)> = BTreeMap::new();
    for (index, entry) in entries.iter().enumerate() {
        for outcome in &entry.outcomes {
            results.insert(outcome.call_id.as_str(), (index, outcome));
        }
    }
    let mut calls = Vec::new();
    for (call_index, entry) in entries.iter().enumerate() {
        for tool in &entry.uses {
            let Some((result_index, outcome)) =
                results.get(tool.call_id.as_str())
            else {
                continue;
            };
            calls.push(Call {
                id: format!("t{}", calls.len() + 1),
                call_id: tool.call_id.clone(),
                tool: tool.tool.clone(),
                input: tool.input.clone(),
                call_index,
                result_index: *result_index,
                result_chars: outcome.text.chars().count(),
                is_error: outcome.is_error,
                pinned: is_pinned(call_index, entries.len(), preserve_recent)
                    || is_pinned(*result_index, entries.len(), preserve_recent),
            });
        }
    }
    calls
}

/// A rough token count: a word of ASCII letters is one token per six
/// letters (rounded up), a run of digits half a token per digit, and any
/// other non-space character nine tenths of a token; the sum rounds up.
///
/// Counted exactly, in tenths of a token. pi's `estimateTokens` sums the
/// same weights as floats, so ten punctuation characters come to
/// 9.000000000000002 there and round up to 10; here they are 9, on
/// purpose.
pub fn estimate_tokens(text: &str) -> usize {
    let mut tenths = 0usize;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_alphabetic() {
            let mut len = 1usize;
            while chars.next_if(char::is_ascii_alphabetic).is_some() {
                len += 1;
            }
            tenths += 10 * len.div_ceil(6);
        } else if c.is_ascii_digit() {
            tenths += 5;
        } else if !c.is_whitespace() {
            tenths += 9;
        }
    }
    tenths.div_ceil(10)
}

/// `text` cut to `limit` characters, the last one an ellipsis.
pub fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let kept: String = text.chars().take(limit.saturating_sub(1)).collect();
    format!("{kept}…")
}

fn abridge(text: &str, head: usize, tail: usize) -> String {
    let len = text.chars().count();
    if len <= head + tail + 40 {
        return text.to_owned();
    }
    let start: String = text.chars().take(head).collect();
    let end: String = text.chars().skip(len - tail).collect();
    format!("{start}\n[… {} chars omitted …]\n{end}", len - head - tail)
}

/// One message in the state.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HistoryEntry {
    pub i: usize,
    pub role: Role,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<HistoryCalls>,
}

/// A message's tool calls: in full, or compacted to one line each.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum HistoryCalls {
    Full(Vec<HistoryCall>),
    Compact(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HistoryCall {
    pub id: String,
    pub tool: String,
    pub input: String,
    pub result: String,
}

/// The state sent to Jev.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct State {
    pub context: &'static str,
    pub goal: String,
    pub history: Vec<HistoryEntry>,
}

/// A state that fits the budget: its estimated tokens, and how much it
/// had to shrink to fit (`full`, or the last stage applied).
#[derive(Debug, Clone, PartialEq)]
pub struct Fitted {
    pub state: State,
    pub tokens: usize,
    pub stage: &'static str,
}

fn input_text(input: &Map<String, Value>, limit: usize) -> String {
    truncate(
        &serde_json::to_string(input).expect("JSON objects serialize"),
        limit,
    )
}

fn result_note(call: &Call) -> String {
    format!(
        "{}, {} chars (omitted)",
        if call.is_error { "error" } else { "ok" },
        call.result_chars
    )
}

fn compact_call(call: &Call) -> String {
    let input = call
        .input
        .iter()
        .map(|(key, value)| {
            let text = match value {
                Value::String(text) => text.clone(),
                other => {
                    let mut single = Map::new();
                    single.insert(key.clone(), other.clone());
                    input_text(&single, 200)
                }
            };
            let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            format!("{key}={text}")
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{} {} {} → {} {}ch",
        call.id,
        call.tool,
        truncate(&input, INPUT_CHARS[2]),
        if call.is_error { "error" } else { "ok" },
        call.result_chars
    )
}

fn calls_by_message(calls: &[Call]) -> BTreeMap<usize, Vec<&Call>> {
    let mut by_message: BTreeMap<usize, Vec<&Call>> = BTreeMap::new();
    for call in calls {
        by_message.entry(call.call_index).or_default().push(call);
    }
    by_message
}

fn history_entries(
    entries: &[Entry],
    calls: &[Call],
    input_chars: usize,
) -> Vec<HistoryEntry> {
    let by_message = calls_by_message(calls);
    let mut history = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let tool_calls: Vec<HistoryCall> = by_message
            .get(&index)
            .map(|calls| {
                calls
                    .iter()
                    .map(|call| HistoryCall {
                        id: call.id.clone(),
                        tool: call.tool.clone(),
                        input: input_text(&call.input, input_chars),
                        result: result_note(call),
                    })
                    .collect()
            })
            .unwrap_or_default();
        if entry.text.trim().is_empty() && tool_calls.is_empty() {
            continue;
        }
        history.push(HistoryEntry {
            i: index,
            role: entry.role,
            text: entry.text.clone(),
            tool_calls: (!tool_calls.is_empty())
                .then_some(HistoryCalls::Full(tool_calls)),
        });
    }
    history
}

/// The user's last three prompts, each cut to 500 characters: the goal
/// when none is given.
pub fn goal_from(entries: &[Entry]) -> String {
    let prompts: Vec<String> = entries
        .iter()
        .filter(|entry| {
            entry.role == Role::User
                && !entry.text.trim().is_empty()
                && entry.outcomes.is_empty()
        })
        .map(|entry| truncate(&entry.text, 500))
        .collect();
    prompts[prompts.len().saturating_sub(3)..].join("\n")
}

fn entry_tokens(entry: &HistoryEntry) -> usize {
    estimate_tokens(&serde_json::to_string(entry).expect("entries serialize"))
        + 1
}

/// Folds runs of adjacent compacted-call-only entries by one role into
/// one entry.
fn merge_call_runs(
    history: Vec<HistoryEntry>,
    pinned: impl Fn(&HistoryEntry) -> bool,
) -> Vec<HistoryEntry> {
    let foldable = |entry: &HistoryEntry| {
        !pinned(entry)
            && entry.text.is_empty()
            && matches!(entry.tool_calls, Some(HistoryCalls::Compact(_)))
    };
    let mut merged: Vec<HistoryEntry> = Vec::new();
    for entry in history {
        if let Some(previous) = merged.last_mut()
            && foldable(previous)
            && foldable(&entry)
            && previous.role == entry.role
            && let (
                Some(HistoryCalls::Compact(into)),
                Some(HistoryCalls::Compact(from)),
            ) = (&mut previous.tool_calls, &entry.tool_calls)
        {
            into.extend(from.iter().cloned());
            continue;
        }
        merged.push(entry);
    }
    merged
}

/// Builds the state and shrinks it until its estimate fits `max_tokens`,
/// in stages: shorter tool inputs, abridged long texts, collapsed old
/// texts, compacted old calls, old messages without calls left out, and
/// last, runs of old calls merged. Pinned messages shrink last, and are
/// never collapsed or left out.
pub fn fit_state(
    entries: &[Entry],
    calls: &[Call],
    goal: Option<&str>,
    max_tokens: usize,
    preserve_recent: usize,
) -> Result<Fitted, String> {
    let goal = goal
        .filter(|goal| !goal.is_empty())
        .map_or_else(|| goal_from(entries), str::to_owned);
    let state_of = |history: Vec<HistoryEntry>| State {
        context: STATE_CONTEXT,
        goal: goal.clone(),
        history,
    };
    let base = estimate_tokens(
        &serde_json::to_string(&state_of(Vec::new()))
            .expect("states serialize"),
    );
    let fits = |tokens: usize| tokens <= max_tokens;
    let fitted = |history, tokens, stage| Fitted {
        state: state_of(history),
        tokens,
        stage,
    };
    let rebuild = |input_chars| {
        let history = history_entries(entries, calls, input_chars);
        let per: Vec<usize> = history.iter().map(entry_tokens).collect();
        let tokens = base + per.iter().sum::<usize>();
        (history, per, tokens)
    };

    let (mut history, mut per, mut tokens) = rebuild(INPUT_CHARS[0]);
    if fits(tokens) {
        return Ok(fitted(history, tokens, "full"));
    }
    for (limit, stage) in [
        (INPUT_CHARS[1], "inputs<=200"),
        (INPUT_CHARS[2], "inputs<=60"),
    ] {
        (history, per, tokens) = rebuild(limit);
        if fits(tokens) {
            return Ok(fitted(history, tokens, stage));
        }
    }

    let pinned = |entry: &HistoryEntry| {
        is_pinned(entry.i, entries.len(), preserve_recent)
    };
    let order: Vec<usize> = (0..history.len())
        .filter(|&index| !pinned(&history[index]))
        .chain((0..history.len()).filter(|&index| pinned(&history[index])))
        .collect();
    let mut shrink = |history: &mut Vec<HistoryEntry>,
                      tokens: &mut usize,
                      index: usize,
                      change: &dyn Fn(&mut HistoryEntry)| {
        change(&mut history[index]);
        let now = entry_tokens(&history[index]);
        *tokens = *tokens + now - per[index];
        per[index] = now;
    };

    for &index in &order {
        if history[index].text.chars().count() <= TEXT_HEAD + TEXT_TAIL + 40 {
            continue;
        }
        shrink(&mut history, &mut tokens, index, &|entry| {
            entry.text = abridge(&entry.text, TEXT_HEAD, TEXT_TAIL);
        });
        if fits(tokens) {
            return Ok(fitted(history, tokens, "texts abridged"));
        }
    }

    for &index in &order {
        let entry = &history[index];
        if pinned(entry) || entry.text.is_empty() {
            continue;
        }
        let original = entries[entry.i].text.chars().count();
        shrink(&mut history, &mut tokens, index, &|entry| {
            entry.text = format!("[… {original} chars omitted …]");
        });
        if fits(tokens) {
            return Ok(fitted(history, tokens, "old messages collapsed"));
        }
    }

    let by_message = calls_by_message(calls);
    for &index in &order {
        let entry = &history[index];
        let Some(own) = by_message.get(&entry.i) else {
            continue;
        };
        if pinned(entry) {
            continue;
        }
        let compact: Vec<String> =
            own.iter().map(|call| compact_call(call)).collect();
        shrink(&mut history, &mut tokens, index, &|entry| {
            entry.tool_calls = Some(HistoryCalls::Compact(compact.clone()));
        });
        if fits(tokens) {
            return Ok(fitted(history, tokens, "old calls compacted"));
        }
    }

    let mut left_out = BTreeSet::new();
    for &index in &order {
        let entry = &history[index];
        if pinned(entry) || entry.tool_calls.is_some() {
            continue;
        }
        left_out.insert(index);
        tokens -= per[index];
        if fits(tokens) {
            let kept = history
                .into_iter()
                .enumerate()
                .filter(|(index, _)| !left_out.contains(index))
                .map(|(_, entry)| entry)
                .collect();
            return Ok(fitted(kept, tokens, "old messages left out"));
        }
    }

    let kept: Vec<HistoryEntry> = history
        .into_iter()
        .enumerate()
        .filter(|(index, _)| !left_out.contains(index))
        .map(|(_, entry)| entry)
        .collect();
    let merged = merge_call_runs(kept, pinned);
    let tokens = base + merged.iter().map(entry_tokens).sum::<usize>();
    if fits(tokens) {
        return Ok(fitted(merged, tokens, "old calls merged"));
    }
    Err(format!(
        "history too large for Jev (~{tokens} tokens after shrinking, limit {max_tokens})"
    ))
}

/// The state as JSON, for a request.
pub fn to_value(state: &State) -> Value {
    json!(state)
}
