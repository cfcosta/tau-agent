//! Renders assistant messages as the `response.*` frames a real OpenAI
//! Responses WebSocket connection would send for them, for testing
//! `tau_ai::responses::stream::StreamProcessor`.
//!
//! [`draw_response_frames`] is the wire-format reference: it is meant to
//! be faithful enough to be reused later by a fake OpenAI server (see
//! `docs/reference/testing.md`'s "Fake OpenAI server"), so every frame
//! shape here is cited against a pi test fixture or `openai-responses-shared.ts`.
//!
//! ## The wire-realistic subset
//!
//! Not every [`AssistantMessage`] a generic generator could build is
//! something OpenAI could actually have produced. [`wire_assistant_message`]
//! only builds messages in the subset `StreamProcessor` round-trips
//! exactly:
//!
//! - a text block's `text_signature` is always
//!   [`encode_text_signature_v1`] of a message item id (and, sometimes, a
//!   `phase`) — the form `StreamProcessor` itself produces, not the bare
//!   legacy id form `tau_testing::generators::text_content` draws (that
//!   form is for testing `responses::input`'s replay of *foreign or
//!   legacy* signatures, not fresh stream output);
//! - a thinking block's `thinking_signature` is always
//!   [`generators::reasoning_item_json`], a serialized reasoning item
//!   shaped `{"id", "type": "reasoning", "summary": [], "encrypted_content"?}`.
//!   Its `summary` is always empty, matching encrypted-reasoning mode
//!   (tau-agent always requests `reasoning.encrypted_content`;
//!   `docs/reference/openai-websocket.md`), so `StreamProcessor` falls
//!   back to the streamed text, never to a summary/content array baked
//!   into the signature;
//! - a tool call's id is always `call_id|item_id`
//!   ([`generators::tool_call`]'s shape already matches);
//! - `StopReason::Stop` never coincides with a tool call in the content
//!   (that combination isn't reachable: `StreamProcessor` always upgrades
//!   it to `ToolUse`), and `StopReason::ToolUse` always has at least one;
//! - `StopReason::Aborted` is never produced, because nothing in the
//!   server-frame vocabulary this module renders maps to it —
//!   `StreamProcessor` only ever produces `ErrorReason::Error`. Cancelling
//!   a turn is a lane-level concept, above this module's scope.
//! - `usage.cost` is always [`UsageCost::default`] (all zero): the stream
//!   processor never prices usage, `tau_ai::cost` does.

use hegel::{
    TestCase,
    generators::{self as gs, Generator as _, PrintableGenerator},
};
use serde_json::{Value, json};
use tau_ai::{
    message::{
        API,
        AssistantBlock,
        AssistantMessage,
        PROVIDER,
        StopReason,
        TextContent,
        ThinkingContent,
        Usage,
    },
    responses::stream::encode_text_signature_v1,
};

use crate::{generators, stream::draw_split};

/// A text block a real `response.output_item.done` `message` item could
/// produce: `text_signature` decodes back to the item's id (and `phase`,
/// when present).
pub fn wire_text_content() -> impl PrintableGenerator<TextContent> {
    // TextContent is tau's own type, so its drawn values print through Debug.
    wire_text_content_unprinted().print_as_debug()
}

#[hegel::composite]
fn wire_text_content_unprinted(tc: &TestCase) -> TextContent {
    let item_id = tc.draw(generators::id("msg_"));
    let phase = tc.draw(gs::optional(gs::sampled_from(vec![
        "commentary".to_owned(),
        "final_answer".to_owned(),
    ])));
    TextContent {
        text: tc.draw(generators::text(40)),
        text_signature: Some(encode_text_signature_v1(
            &item_id,
            phase.as_deref(),
        )),
    }
}

/// A thinking block a real `response.output_item.done` `reasoning` item
/// could produce, in tau-agent's always-encrypted-reasoning
/// configuration: see the module docs for why `summary` is empty.
pub fn wire_thinking_content() -> impl PrintableGenerator<ThinkingContent> {
    // ThinkingContent is tau's own type, so its drawn values print through Debug.
    wire_thinking_content_unprinted().print_as_debug()
}

#[hegel::composite]
fn wire_thinking_content_unprinted(tc: &TestCase) -> ThinkingContent {
    ThinkingContent {
        thinking: tc.draw(generators::text(40)),
        thinking_signature: Some(tc.draw(generators::reasoning_item_json())),
        redacted: None,
    }
}

/// An [`AssistantMessage`] built only from the wire-realistic blocks
/// above, with a stop reason `StreamProcessor` can actually produce (see
/// the module docs). `usage` never exceeds `max_tokens` per field, and
/// `usage.cost` is always zero.
pub fn wire_assistant_message() -> impl PrintableGenerator<AssistantMessage> {
    // AssistantMessage is tau's own type, so its drawn values print through Debug.
    wire_assistant_message_unprinted().print_as_debug()
}

#[hegel::composite]
fn wire_assistant_message_unprinted(tc: &TestCase) -> AssistantMessage {
    let stop_reason = tc.draw(
        gs::sampled_from(vec![
            StopReason::Stop,
            StopReason::Length,
            StopReason::ToolUse,
            StopReason::Error,
        ])
        .print_as_debug(),
    );
    let block_count =
        tc.draw(gs::integers::<usize>().min_value(1).max_value(4));
    let force_tool_call_at = (stop_reason == StopReason::ToolUse)
        .then(|| tc.draw(gs::integers::<usize>().max_value(block_count - 1)));
    let no_tool_call = stop_reason == StopReason::Stop;
    let mut content = Vec::with_capacity(block_count);
    for i in 0..block_count {
        let block = if force_tool_call_at == Some(i) {
            AssistantBlock::ToolCall(tc.draw(generators::tool_call()))
        } else {
            let max = if no_tool_call { 1 } else { 2 };
            match tc.draw(gs::integers::<u8>().max_value(max)) {
                0 => AssistantBlock::Text(tc.draw(wire_text_content())),
                1 => AssistantBlock::Thinking(tc.draw(wire_thinking_content())),
                _ => AssistantBlock::ToolCall(tc.draw(generators::tool_call())),
            }
        };
        content.push(block);
    }
    // `StreamProcessor` always encodes a `response.failed`/`error` frame's
    // fields as `format!("Error Code {code}: {message}")`
    // (`crates/tau-ai/src/responses/stream.rs`, from pi's
    // `Error Code ${event.code}: ${event.message}`,
    // `openai-responses-shared.ts:695`); using one fixed code here keeps
    // `draw_response_frames` able to recover it losslessly (see
    // `error_frame_parts`) without needing a delimiter-safe encoding.
    let error_message = (stop_reason == StopReason::Error).then(|| {
        format!(
            "Error Code {WIRE_ERROR_CODE}: {}",
            tc.draw(generators::text(30))
        )
    });
    AssistantMessage {
        content,
        api: API.to_owned(),
        provider: PROVIDER.to_owned(),
        model: tc.draw(gs::sampled_from(vec![
            "gpt-5.5".to_owned(),
            "gpt-5.5-mini".to_owned(),
        ])),
        response_id: Some(tc.draw(generators::id("resp_"))),
        // A raw `error` frame never carries usage (`StreamProcessor`
        // always reports `Usage::default()` for it, matching pi and the
        // "usage on abort" rule in `docs/reference/openai-websocket.md`),
        // so a wire-realistic `Error` message must have zero usage too.
        usage: if stop_reason == StopReason::Error {
            Usage::default()
        } else {
            tc.draw(generators::usage_with_max_tokens(1 << 20))
        },
        stop_reason,
        error_message,
        timestamp: tc.draw(generators::timestamp()),
    }
}

/// The fixed error `code` [`wire_assistant_message`] uses whenever it
/// draws `StopReason::Error`. `previous_response_not_found` and other
/// real codes are covered by dedicated example tests instead of this
/// generator; see `crates/tau-ai/tests/responses_stream.rs`.
const WIRE_ERROR_CODE: &str = "invalid_request_error";

/// Recovers `(code, message)` from a `message.error_message` built by
/// [`wire_assistant_message`], the exact inverse of the `format!` there.
fn error_frame_parts(error_message: &str) -> (&str, &str) {
    let rest = error_message.strip_prefix("Error Code ").expect(
        "wire-realistic error_message always has the Error Code prefix",
    );
    rest.strip_prefix(WIRE_ERROR_CODE)
        .and_then(|rest| rest.strip_prefix(": "))
        .map(|message| (WIRE_ERROR_CODE, message))
        .expect("wire-realistic error_message always uses WIRE_ERROR_CODE")
}

/// Renders `message` as the `response.*` frames a real OpenAI Responses
/// WebSocket connection would send to produce it: `response.created`,
/// `output_item.added`/`output_item.done` with delta events for each
/// block (split at arbitrary points via [`draw_split`]), and one terminal
/// frame matching `message.stop_reason`.
///
/// `message` must be in the wire-realistic subset (see the module docs);
/// [`wire_assistant_message`] always draws one.
pub fn draw_response_frames(
    tc: &TestCase,
    message: &AssistantMessage,
) -> Vec<Value> {
    let response_id = message
        .response_id
        .clone()
        .unwrap_or_else(|| "resp_missing".to_owned());
    let mut frames = vec![json!({
        "type": "response.created",
        "response": { "id": response_id },
    })];

    for (output_index, block) in message.content.iter().enumerate() {
        match block {
            AssistantBlock::Text(text) => {
                push_text_frames(tc, &mut frames, output_index, text)
            }
            AssistantBlock::Thinking(thinking) => {
                push_thinking_frames(tc, &mut frames, output_index, thinking)
            }
            AssistantBlock::ToolCall(call) => {
                push_tool_call_frames(tc, &mut frames, output_index, call)
            }
        }
    }

    frames.push(terminal_frame(&response_id, message));
    frames
}

fn push_text_frames(
    tc: &TestCase,
    frames: &mut Vec<Value>,
    output_index: usize,
    text: &TextContent,
) {
    let signature = text
        .text_signature
        .as_deref()
        .expect("wire-realistic text blocks always have a signature");
    let signature: Value =
        serde_json::from_str(signature).expect("a V1 text signature is JSON");
    let item_id = signature["id"].as_str().expect("a V1 signature has an id");
    let phase = signature.get("phase").and_then(Value::as_str);

    let mut added_item = json!({
        "type": "message",
        "id": item_id,
        "role": "assistant",
        "status": "in_progress",
        "content": [],
    });
    if let Some(phase) = phase {
        added_item["phase"] = json!(phase);
    }
    frames.push(json!({
        "type": "response.output_item.added",
        "output_index": output_index,
        "item": added_item,
    }));
    for delta in draw_split(tc, &text.text) {
        frames.push(json!({
            "type": "response.output_text.delta",
            "output_index": output_index,
            "delta": delta,
        }));
    }
    let mut done_item = json!({
        "type": "message",
        "id": item_id,
        "role": "assistant",
        "status": "completed",
        "content": [{ "type": "output_text", "text": text.text, "annotations": [] }],
    });
    if let Some(phase) = phase {
        done_item["phase"] = json!(phase);
    }
    frames.push(json!({
        "type": "response.output_item.done",
        "output_index": output_index,
        "item": done_item,
    }));
}

fn push_thinking_frames(
    tc: &TestCase,
    frames: &mut Vec<Value>,
    output_index: usize,
    thinking: &ThinkingContent,
) {
    let signature = thinking
        .thinking_signature
        .as_deref()
        .expect("wire-realistic thinking blocks always have a signature");
    let item: Value =
        serde_json::from_str(signature).expect("a reasoning signature is JSON");
    let item_id = item.get("id").and_then(Value::as_str).unwrap_or_default();

    frames.push(json!({
        "type": "response.output_item.added",
        "output_index": output_index,
        "item": { "type": "reasoning", "id": item_id, "summary": [] },
    }));
    for delta in draw_split(tc, &thinking.thinking) {
        frames.push(json!({
            "type": "response.reasoning_summary_text.delta",
            "output_index": output_index,
            "delta": delta,
        }));
    }
    frames.push(json!({
        "type": "response.output_item.done",
        "output_index": output_index,
        "item": item,
    }));
}

fn push_tool_call_frames(
    tc: &TestCase,
    frames: &mut Vec<Value>,
    output_index: usize,
    call: &tau_ai::message::ToolCall,
) {
    let (call_id, item_id) = call
        .id
        .split_once('|')
        .map(|(call_id, item_id)| (call_id, Some(item_id)))
        .unwrap_or((call.id.as_str(), None));
    let mut added_item = json!({
        "type": "function_call",
        "call_id": call_id,
        "name": call.name,
        "arguments": "",
    });
    if let Some(item_id) = item_id {
        added_item["id"] = json!(item_id);
    }
    frames.push(json!({
        "type": "response.output_item.added",
        "output_index": output_index,
        "item": added_item,
    }));

    let full_args =
        serde_json::to_string(&Value::Object(call.arguments.clone()))
            .expect("a JSON object always serializes");
    for delta in draw_split(tc, &full_args) {
        frames.push(json!({
            "type": "response.function_call_arguments.delta",
            "output_index": output_index,
            "delta": delta,
        }));
    }
    frames.push(json!({
        "type": "response.function_call_arguments.done",
        "output_index": output_index,
        "arguments": full_args,
    }));

    let mut done_item = json!({
        "type": "function_call",
        "call_id": call_id,
        "name": call.name,
        "arguments": full_args,
    });
    if let Some(item_id) = item_id {
        done_item["id"] = json!(item_id);
    }
    frames.push(json!({
        "type": "response.output_item.done",
        "output_index": output_index,
        "item": done_item,
    }));
}

/// The single terminal frame for `message`, per
/// `crates/tau-ai/src/responses/stream.rs`'s stop-reason mapping.
fn terminal_frame(response_id: &str, message: &AssistantMessage) -> Value {
    let usage = usage_to_json(&message.usage);
    match message.stop_reason {
        StopReason::Stop | StopReason::ToolUse => json!({
            "type": "response.completed",
            "response": { "id": response_id, "status": "completed", "usage": usage },
        }),
        StopReason::Length => json!({
            "type": "response.incomplete",
            "response": {
                "id": response_id,
                "status": "incomplete",
                "incomplete_details": { "reason": "max_output_tokens" },
                "usage": usage,
            },
        }),
        StopReason::Error => {
            let (code, error_message) =
                error_frame_parts(message.error_message.as_deref().expect(
                    "a wire-realistic Error message always has error_message",
                ));
            json!({
                "type": "error",
                "code": code,
                "message": error_message,
            })
        }
        StopReason::Aborted => {
            unreachable!(
                "wire_assistant_message never draws StopReason::Aborted"
            )
        }
    }
}

/// The exact inverse of `stream.rs`'s `parse_usage`: `input_tokens` folds
/// `cache_read`/`cache_write` back in, since `StreamProcessor` subtracts
/// them back out.
fn usage_to_json(usage: &Usage) -> Value {
    let mut response_usage = json!({
        "input_tokens": usage.input + usage.cache_read + usage.cache_write,
        "output_tokens": usage.output,
        "total_tokens": usage.total_tokens,
        "input_tokens_details": {
            "cached_tokens": usage.cache_read,
            "cache_write_tokens": usage.cache_write,
        },
    });
    if let Some(reasoning) = usage.reasoning {
        response_usage["output_tokens_details"] =
            json!({ "reasoning_tokens": reasoning });
    }
    response_usage
}

/// A frame of a type `StreamProcessor` does not recognize, drawn from a
/// small realistic sample (`docs/reference/testing.md`'s
/// `codex.rate_limits` example, and the `response.in_progress`/`response.queued`
/// events `openai-websocket.md` and `openai-responses-shared.ts` mention
/// but never act on).
fn draw_unknown_frame(tc: &TestCase) -> Value {
    tc.draw(gs::sampled_from(vec![
        json!({ "type": "codex.rate_limits" }),
        json!({ "type": "response.in_progress" }),
        json!({ "type": "response.queued" }),
        json!({ "type": "some.totally.unknown.frame", "foo": "bar" }),
    ]))
}

/// Inserts [`draw_unknown_frame`]s at random points in `frames`, for
/// testing that `StreamProcessor` ignores them wherever they land. Never
/// inserts after the last frame, so a caller relying on the last frame
/// being the terminal frame (e.g. to compute a strict prefix) still can.
pub fn interleave_unknown_frames(
    tc: &TestCase,
    frames: Vec<Value>,
) -> Vec<Value> {
    let mut result = Vec::with_capacity(frames.len() * 2);
    let last = frames.len().saturating_sub(1);
    for (i, frame) in frames.into_iter().enumerate() {
        if i > 0 && tc.draw(gs::booleans()) {
            result.push(draw_unknown_frame(tc));
        }
        result.push(frame);
        if i < last && tc.draw(gs::booleans()) {
            result.push(draw_unknown_frame(tc));
        }
    }
    result
}
