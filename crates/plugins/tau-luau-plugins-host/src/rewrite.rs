//! A Luau plugin's context rewrite (ADR 0045): the transcript is offered
//! as a summary of its messages, and the plugin answers which to keep
//! and what to put between them. The rewrite is built here, so what the
//! loop checks of one holds: the last message stays, and every tool call
//! stays with its result.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use tau_agent::{context::estimate_message_tokens, plugin::Rewrite};
use tau_ai::message::{
    AssistantBlock,
    InputBlock,
    Message,
    Timestamp,
    UserContent,
    UserMessage,
    text_of,
};

/// `transcript` as `rewrite_context` reads it: each message by its place
/// (from 1), its role, its whole text (ADR 0050, gap B: a transcript too
/// large for the VM is one no plugin can rewrite), its size in bytes,
/// the loop's estimate of its tokens, and, when it has images, how many
/// and its `parts`, its blocks in order.
/// An answer has its thinking, when it has any, and its calls, each with
/// its arguments, `order`, their names in the order the model gave them
/// (a Luau table keeps none), and `json`, the arguments as the model
/// wrote them; a result has its call, name and whether it went well.
pub fn summary(transcript: &[Message]) -> Value {
    let messages: Vec<Value> = transcript
        .iter()
        .enumerate()
        .map(|(at, message)| {
            let (mut out, images) = match message {
                Message::User(user) => (
                    json!({ "text": user.content.text() }),
                    match &user.content {
                        UserContent::Text(_) => 0,
                        UserContent::Blocks(blocks) => images(blocks),
                    },
                ),
                Message::Assistant(assistant) => {
                    let mut out = json!({
                        "text": assistant.text(),
                        "calls": assistant
                            .tool_calls()
                            .map(|call| json!({
                                "id": call.id,
                                "name": call.name,
                                "args": call.arguments,
                                "order": call.arguments.keys().collect::<Vec<_>>(),
                                "json": Value::Object(call.arguments.clone()).to_string(),
                            }))
                            .collect::<Vec<_>>(),
                    });
                    let thinking: Vec<&str> = assistant
                        .content
                        .iter()
                        .filter_map(|block| match block {
                            AssistantBlock::Thinking(thinking) => {
                                Some(thinking.thinking.as_str())
                            }
                            _ => None,
                        })
                        .collect();
                    if !thinking.is_empty() {
                        out["thinking"] = json!(thinking.join("\n"));
                        // Where the thinking sits among the text.
                        out["parts"] = json!(
                            assistant
                                .content
                                .iter()
                                .filter_map(|block| match block {
                                    AssistantBlock::Text(text) => {
                                        Some(json!({ "text": text.text }))
                                    }
                                    AssistantBlock::Thinking(thinking) => Some(
                                        json!({ "thinking": thinking.thinking }),
                                    ),
                                    AssistantBlock::ToolCall(_) => None,
                                })
                                .collect::<Vec<_>>()
                        );
                    }
                    (out, 0)
                }
                Message::ToolResult(result) => (
                    json!({
                        "text": text_of(&result.content),
                        "call": result.tool_call_id,
                        "name": result.tool_name,
                        "ok": !result.is_error,
                    }),
                    images(&result.content),
                ),
            };
            let size = out["text"].as_str().map_or(0, str::len);
            out["index"] = json!(at + 1);
            out["role"] = json!(message.role());
            out["size"] = json!(size);
            out["tokens"] = json!(estimate_message_tokens(message));
            if images > 0 {
                out["images"] = json!(images);
                out["parts"] = json!(parts(message));
            }
            out
        })
        .collect();
    Value::Array(messages)
}

/// `transcript` whole, as `ctx.transcript()` gives it: as
/// `rewrite_context` reads it ([`summary`]).
pub fn transcript(transcript: &[Message]) -> Value {
    summary(transcript)
}

/// A message's blocks in order, `{ text }` or `{ image = true }`: where
/// its images sit among its text.
fn parts(message: &Message) -> Vec<Value> {
    let blocks = match message {
        Message::User(user) => match &user.content {
            UserContent::Text(text) => return vec![json!({ "text": text })],
            UserContent::Blocks(blocks) => blocks,
        },
        Message::ToolResult(result) => &result.content,
        Message::Assistant(_) => return Vec::new(),
    };
    blocks
        .iter()
        .map(|block| match block {
            InputBlock::Text(text) => json!({ "text": text.text }),
            InputBlock::Image(_) => json!({ "image": true }),
        })
        .collect()
}

/// How many of `blocks` are images.
fn images(blocks: &[InputBlock]) -> usize {
    blocks
        .iter()
        .filter(|block| matches!(block, InputBlock::Image(_)))
        .count()
}

/// The rewrite `answer` asks of `transcript`: `{ keep, insert,
/// drop_calls, texts, drops_conversation, details }`, as `tau.rewrite`
/// makes it. `None` when it keeps everything and changes nothing.
///
/// `drop_calls` names calls that go with their results: an answer left
/// with neither text nor a call goes too. `texts`, each `{ index, text }`,
/// replaces the text of a person's message or a result, its images gone.
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
    let (working, gone, changed) = changed(transcript, answer)?;
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
    keep.retain(|at| !gone.contains(at));
    keep_pairs(&working, &mut keep);

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
        let at = place(&working, &keep, before - 1);
        inserts.entry(at).or_default().push(text.to_owned());
    }

    if keep.len() == count && inserts.is_empty() && !changed {
        return Ok(None);
    }
    let mut messages = Vec::new();
    for (at, message) in working.into_iter().enumerate() {
        for text in inserts.remove(&at).into_iter().flatten() {
            messages.push(Message::User(UserMessage {
                content: UserContent::Text(text),
                timestamp: now,
            }));
        }
        if keep.contains(&at) {
            messages.push(message);
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

/// `transcript` with what `answer` drops and replaces: the messages, the
/// places of those that go, and whether any message changed.
fn changed(
    transcript: &[Message],
    answer: &Value,
) -> Result<(Vec<Message>, BTreeSet<usize>, bool), String> {
    let mut working = transcript.to_vec();
    let mut gone = BTreeSet::new();
    let mut changed = false;
    let dropped: BTreeSet<&str> = answer["drop_calls"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    for (at, message) in working.iter_mut().enumerate() {
        match message {
            Message::Assistant(answer) => {
                let before = answer.content.len();
                answer.content.retain(|block| {
                    !matches!(block, AssistantBlock::ToolCall(call)
                        if dropped.contains(call.id.as_str()))
                });
                if answer.content.len() == before {
                    continue;
                }
                changed = true;
                let left = answer.content.iter().any(|block| {
                    matches!(
                        block,
                        AssistantBlock::Text(_) | AssistantBlock::ToolCall(_)
                    )
                });
                if !left {
                    gone.insert(at);
                }
            }
            Message::ToolResult(result)
                if dropped.contains(result.tool_call_id.as_str()) =>
            {
                gone.insert(at);
            }
            _ => {}
        }
    }
    let count = transcript.len();
    for change in answer["texts"].as_array().into_iter().flatten() {
        let index = change["index"].as_u64().map(|i| i as usize);
        let (Some(index), Some(text)) = (
            index.filter(|i| (1..=count).contains(i)),
            change["text"].as_str(),
        ) else {
            return Err(format!(
                "each of `texts` is {{ index, text }}, the index from 1 to {count}"
            ));
        };
        let block = InputBlock::Text(tau_ai::message::TextContent {
            text: text.to_owned(),
            text_signature: None,
        });
        match &mut working[index - 1] {
            Message::ToolResult(result) => result.content = vec![block],
            Message::User(user) => {
                user.content = UserContent::Text(text.to_owned());
            }
            Message::Assistant(_) => {
                return Err(format!(
                    "message {index} is an answer: `texts` replaces a \
                     person's message or a result"
                ));
            }
        }
        changed = true;
    }
    Ok((working, gone, changed))
}

/// How many bytes of JSON `transcript` is, and would be after the
/// rewrite `answer` asks: `{ before, after }`, for `ctx.measure`.
pub fn measure(
    transcript: &[Message],
    answer: &Value,
) -> Result<Value, String> {
    let size = |messages: &[Message]| {
        serde_json::to_string(messages).map_or(0, |text| text.len())
    };
    let before = size(transcript);
    let after = match rebuild(transcript, answer, "", 0)? {
        Some(rewrite) => size(&rewrite.messages),
        None => before,
    };
    Ok(json!({ "before": before, "after": after }))
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
