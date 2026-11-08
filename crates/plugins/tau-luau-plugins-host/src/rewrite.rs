//! A Luau plugin's context rewrite (ADR 0045): the transcript is offered
//! as a summary of its messages, and the plugin answers which to keep
//! and what to put between them. The rewrite is built here, so what the
//! loop checks of one holds: the last message stays, and every tool call
//! stays with its result.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use tau_agent::plugin::Rewrite;
use tau_ai::message::{
    InputBlock,
    Message,
    Timestamp,
    UserContent,
    UserMessage,
};

/// `transcript` as `rewrite_context` reads it: each message by its place
/// (from 1), its role, its whole text and its size, and the tool calls
/// it makes or answers (ADR 0050, gap B: a transcript too large for the
/// VM is one no plugin can rewrite).
pub fn summary(transcript: &[Message]) -> Value {
    let messages: Vec<Value> = transcript
        .iter()
        .enumerate()
        .map(|(at, message)| {
            let mut summary = match message {
                Message::User(user) => json!({ "text": user.content.text() }),
                Message::Assistant(assistant) => json!({
                    "text": assistant.text(),
                    "calls": assistant
                        .tool_calls()
                        .map(|call| json!({ "id": call.id, "name": call.name }))
                        .collect::<Vec<_>>(),
                }),
                Message::ToolResult(result) => json!({
                    "text": blocks_text(&result.content),
                    "call": result.tool_call_id,
                    "name": result.tool_name,
                    "ok": !result.is_error,
                }),
            };
            let size = summary["text"].as_str().map_or(0, str::len);
            summary["index"] = json!(at + 1);
            summary["role"] = json!(message.role());
            summary["size"] = json!(size);
            summary
        })
        .collect();
    Value::Array(messages)
}

/// `transcript` whole, as `ctx.transcript()` gives it: each message by
/// its place (from 1), its role and its full text; an answer's calls
/// with their arguments and, as a Luau table keeps no order, `order`,
/// their names in the order the model gave them; a result's call, name
/// and whether it went well.
pub fn transcript(transcript: &[Message]) -> Value {
    let messages: Vec<Value> = transcript
        .iter()
        .enumerate()
        .map(|(at, message)| {
            let mut out = match message {
                Message::User(user) => json!({ "text": user.content.text() }),
                Message::Assistant(assistant) => json!({
                    "text": assistant.text(),
                    "calls": assistant
                        .tool_calls()
                        .map(|call| json!({
                            "id": call.id,
                            "name": call.name,
                            "args": call.arguments,
                            "order": call.arguments.keys().collect::<Vec<_>>(),
                        }))
                        .collect::<Vec<_>>(),
                }),
                Message::ToolResult(result) => json!({
                    "text": blocks_text(&result.content),
                    "call": result.tool_call_id,
                    "name": result.tool_name,
                    "ok": !result.is_error,
                }),
            };
            out["index"] = json!(at + 1);
            out["role"] = json!(message.role());
            out
        })
        .collect();
    Value::Array(messages)
}

fn blocks_text(blocks: &[InputBlock]) -> String {
    blocks.iter().filter_map(InputBlock::as_text).collect()
}

/// The rewrite `answer` asks of `transcript`: `{ keep, insert,
/// drops_conversation, details }`, as `tau.rewrite` makes it. `None` when
/// it keeps everything and adds nothing.
pub fn rebuild(
    transcript: &[Message],
    answer: &Value,
    plugin: &str,
    now: Timestamp,
) -> Result<Option<Rewrite>, String> {
    let count = transcript.len();
    if count == 0 {
        return Ok(None);
    }
    let mut keep: BTreeSet<usize> = BTreeSet::new();
    for index in answer["keep"].as_array().into_iter().flatten() {
        match index.as_u64().map(|i| i as usize) {
            Some(i) if (1..=count).contains(&i) => {
                keep.insert(i - 1);
            }
            _ => return Err(format!("`keep` holds {index}, not a message")),
        }
    }
    // The message the next request answers stays.
    keep.insert(count - 1);
    keep_pairs(transcript, &mut keep);

    let mut inserts: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for insert in answer["insert"].as_array().into_iter().flatten() {
        let before = insert["before"].as_u64().map(|i| i as usize);
        let Some(text) = insert["text"].as_str() else {
            return Err("each of `insert` needs its `text`".into());
        };
        let Some(before) = before.filter(|i| (1..=count).contains(i)) else {
            return Err(format!(
                "an insert goes `before` a message, from 1 to {count}"
            ));
        };
        let at = place(transcript, &keep, before - 1);
        inserts.entry(at).or_default().push(text.to_owned());
    }

    if keep.len() == count && inserts.is_empty() {
        return Ok(None);
    }
    let mut messages = Vec::new();
    for (at, message) in transcript.iter().enumerate() {
        for text in inserts.remove(&at).into_iter().flatten() {
            messages.push(Message::User(UserMessage {
                content: UserContent::Text(text),
                timestamp: now,
            }));
        }
        if keep.contains(&at) {
            messages.push(message.clone());
        }
    }
    Ok(Some(Rewrite {
        messages,
        drops_conversation: answer["drops_conversation"]
            .as_bool()
            .unwrap_or(false),
        details: json!({ "plugin": plugin, "details": answer["details"] }),
    }))
}

/// Keeps every tool call's message with its result's: a kept call keeps
/// its results, and a kept result its call.
fn keep_pairs(transcript: &[Message], keep: &mut BTreeSet<usize>) {
    let mut call_at: BTreeMap<&str, usize> = BTreeMap::new();
    let mut results_of: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (at, message) in transcript.iter().enumerate() {
        match message {
            Message::Assistant(assistant) => {
                for call in assistant.tool_calls() {
                    call_at.insert(&call.id, at);
                }
            }
            Message::ToolResult(result) => {
                if let Some(&call) = call_at.get(result.tool_call_id.as_str()) {
                    results_of.entry(call).or_default().push(at);
                }
            }
            Message::User(_) => {}
        }
    }
    loop {
        let before = keep.len();
        for (&call, results) in &results_of {
            if keep.contains(&call) || results.iter().any(|r| keep.contains(r))
            {
                keep.insert(call);
                keep.extend(results.iter().copied());
            }
        }
        if keep.len() == before {
            return;
        }
    }
}

/// Where an insert before the message at `before` goes: before the next
/// kept message from there, and never between a call and its results,
/// whose call it goes before.
fn place(
    transcript: &[Message],
    keep: &BTreeSet<usize>,
    before: usize,
) -> usize {
    let mut at = keep
        .range(before..)
        .next()
        .copied()
        .unwrap_or(transcript.len() - 1);
    while at > 0 && matches!(transcript[at], Message::ToolResult(_)) {
        at -= 1;
    }
    at
}
