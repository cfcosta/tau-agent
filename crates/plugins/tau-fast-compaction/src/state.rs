//! The transcript as the history stage shows it to Jev: tool calls with
//! their inputs and the size of their results, never the results
//! themselves; and the token estimate both stages use.
//!
//! Ported from `tamaratran/fast-jev-compaction` by way of
//! `joelhooks/pi-fast-jev-compaction` (`src/core/state.ts`), with the
//! estimate of `tamaratran/jev-pruner` (`src/jev.ts`; see
//! `THIRD_PARTY_NOTICES.md`). Lengths count characters, where the
//! originals count UTF-16 units; they differ only outside the Basic
//! Multilingual Plane.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Map, Value};
use tau_ai::message::{AssistantBlock, InputBlock, Message, UserContent};

use crate::history::{CallRecord, Record};

/// What the state tells Jev about itself.
pub const STATE_CONTEXT: &str = "A coding assistant conversation is being compacted to free context. `history` is the conversation so far, oldest first, verbatim except that tool outputs are replaced by a short `result` note. A long conversation is split into ordered segments that are asked about separately; then `history` is one segment, `calls` lists the tool calls asked about, and a keep in any segment keeps the call. Oversized fields continue across entries labeled `part`, with their field name and character offset. Each question asks whether one tool call, or the full output of that call, still needs to stay in the history verbatim. Whatever is not kept is deleted from model context: a cut output keeps its head and the path of a file holding it whole, and a dropped call goes with its output, though the assistant can re-run a tool or re-read a file.";

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
                    .tool_calls()
                    .map(|call| ToolUse {
                        call_id: call.id.clone(),
                        tool: call.name.clone(),
                        input: call.arguments.clone(),
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

/// Whether `c` is whitespace to JavaScript's `\s`, which the estimate
/// skips (Rust's `char::is_whitespace` differs on U+0085 and U+FEFF).
fn js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n'
            | '\u{b}'
            | '\u{c}'
            | '\r'
            | ' '
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

/// The estimate of `text` in tenths of a token, with `digit` tenths per
/// digit: a word of ASCII letters costs ten, plus ten per six letters
/// after its first; any other character that is not a space costs
/// nine per UTF-16 unit, as the original's regular expression matches
/// units.
pub fn tenths(text: &str, digit: usize) -> usize {
    let mut tenths = 0usize;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_alphabetic() {
            let mut len = 1usize;
            while chars.next_if(char::is_ascii_alphabetic).is_some() {
                len += 1;
            }
            tenths += 10 * (1 + (len - 1) / 6);
        } else if c.is_ascii_digit() {
            tenths += digit;
        } else if !js_space(c) {
            tenths += 9 * c.len_utf16();
        }
    }
    tenths
}

/// Tenths per digit in [`estimate_tokens`].
pub const DIGIT_TENTHS: usize = 5;
/// Tenths per digit in [`estimate_state_tokens`]: half a token more.
pub const STATE_DIGIT_TENTHS: usize = 10;

/// A token count without a tokenizer, `estimateTokens` of
/// `tamaratran/jev-pruner` (`src/jev.ts`): a word of ASCII letters is
/// one token per six letters, rounded up; a digit half a token; any
/// other character that is not a space nine tenths of a token. There it
/// was calibrated against the usage Jev reports for real transcripts,
/// landing 2–18% above the true count.
///
/// Counted exactly, in tenths of a token, and rounded up once: the
/// original sums floats, so ten punctuation characters come to
/// 9.000000000000002 there and round up to 10; here they are 9, on
/// purpose.
pub fn estimate_tokens(text: &str) -> usize {
    tenths(text, DIGIT_TENTHS).div_ceil(10)
}

/// The estimate of a Jev state or question set, jev-pruner's
/// `estimateStateTokens`: [`estimate_tokens`] and half a token more per
/// digit, since the numbers in JSON tokenize worse than prose. Rounded
/// up once, where the original rounds the first part up and then adds
/// the halves.
pub fn estimate_state_tokens(text: &str) -> usize {
    tenths(text, STATE_DIGIT_TENTHS).div_ceil(10)
}

/// [`estimate_state_tokens`] of `value` as JSON, in tenths.
pub fn json_tenths(value: &impl Serialize) -> usize {
    tenths(
        &serde_json::to_string(value).expect("states serialize"),
        STATE_DIGIT_TENTHS,
    )
}

/// `text` cut to `limit` characters, the last one an ellipsis.
pub fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let kept: String = text.chars().take(limit.saturating_sub(1)).collect();
    format!("{kept}…")
}

fn result_note(call: &Call) -> String {
    format!(
        "{}, {} chars (omitted)",
        if call.is_error { "error" } else { "ok" },
        call.result_chars
    )
}

/// `call` as the state shows it: its id, tool, whole input, and a note
/// of its result's status and size.
pub fn call_record(call: &Call) -> CallRecord {
    CallRecord {
        id: call.id.clone(),
        tool: call.tool.clone(),
        input: serde_json::to_string(&call.input)
            .expect("JSON objects serialize"),
        result: Some(result_note(call)),
    }
}

/// The history of the state: every message with text or tool calls,
/// in order, whole. Tool calls come with their input and a note of
/// their result; the results themselves never do.
pub fn history_records(entries: &[Entry], calls: &[Call]) -> Vec<Record> {
    let mut by_message: BTreeMap<usize, Vec<&Call>> = BTreeMap::new();
    for call in calls {
        by_message.entry(call.call_index).or_default().push(call);
    }
    entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            let tool_calls: Vec<CallRecord> = by_message
                .get(&index)
                .map(|calls| {
                    calls.iter().map(|call| call_record(call)).collect()
                })
                .unwrap_or_default();
            if entry.text.trim().is_empty() && tool_calls.is_empty() {
                return None;
            }
            Some(Record {
                i: index,
                role: entry.role,
                text: entry.text.clone(),
                tool_calls,
                tool_results: Vec::new(),
                part: None,
            })
        })
        .collect()
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

/// `goal` when there is one, otherwise [`goal_from`] `entries`.
pub fn goal_or_prompts(goal: Option<&str>, entries: &[Entry]) -> String {
    goal.filter(|goal| !goal.is_empty())
        .map_or_else(|| goal_from(entries), str::to_owned)
}

/// The state sent to Jev: the whole history, or one segment of it with
/// the calls asked about beside it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct State {
    pub context: &'static str,
    pub goal: String,
    pub history: Vec<Record>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calls: Option<Vec<CallRecord>>,
}
