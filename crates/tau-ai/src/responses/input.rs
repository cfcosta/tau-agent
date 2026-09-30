//! Transcript → OpenAI Responses `input` array.
//!
//! Ports pi's `convertResponsesMessages`
//! (`packages/ai/src/api/openai-responses-shared.ts:145`) for the
//! OpenAI Responses subset tau-agent speaks: a ChatGPT plan, WebSocket
//! transport, `store: false`. Instructions are sent separately as the
//! request's top-level `instructions`, so there is no system/developer
//! item here (our [`Message`] has no such variant to begin with).
//!
//! ## Item mapping
//!
//! | Transcript                          | Responses item                                                             |
//! | ------------------------------------ | --------------------------------------------------------------------------- |
//! | [`UserMessage`]                      | one `{"role":"user","content":[...]}` item (pi never sends a bare string)   |
//! | [`AssistantBlock::Text`]              | one `{"type":"message","role":"assistant",...}` item, `id` from the text signature |
//! | [`AssistantBlock::Thinking`]          | the stored reasoning item, replayed verbatim from `thinkingSignature`       |
//! | [`AssistantBlock::ToolCall`]          | one `function_call` item, `call_id`/`id` from [`split_tool_call_id`]        |
//! | [`ToolResultMessage`]                 | one `function_call_output` item                                            |
//!
//! [`response_items`] builds the first three rows for a single assistant
//! message on its own, with no pairing filter — see its own docs.
//!
//! ## Invariants (ours, stricter than pi)
//!
//! 1. Every `function_call_output` comes after its `function_call` with
//!    the same `call_id`. An output with no matching call earlier in the
//!    input is dropped. Pi does not enforce this in the general case:
//!    `transformMessages` (`transform-messages.ts:201`) drops an
//!    errored/aborted assistant message's blocks (including its tool
//!    calls) but still emits any tool result already recorded for it,
//!    producing an orphaned `function_call_output` (see
//!    `docs/reference/pi-audit.md`). We fix this by construction: an
//!    output is only kept when it is popped off a pending call queue
//!    (below), so a call that never made it into the input can never
//!    hand out a matching output.
//! 2. Every surviving `function_call` has exactly one output later in
//!    the input; a call with no output is dropped. Pi instead
//!    synthesizes a `"No result provided"` tool result
//!    (`transform-messages.ts:167`) so the call is never orphaned; we
//!    drop the call instead of inventing a result, which is simpler and
//!    still satisfies OpenAI (an unpaired call just never appears).
//! 3. Assistant messages with `stop_reason` `Error` or `Aborted` are
//!    dropped entirely, matching pi's decision to skip incomplete turns
//!    (`transform-messages.ts:195-203`). Unlike pi, dropping the turn
//!    also removes any later tool result that would otherwise reference
//!    its (now absent) calls — see invariant 1.
//! 4. Conversion is pure and deterministic: no clock, no randomness, no
//!    hidden state.
//!
//! ## Reasoning pairing rule
//!
//! OpenAI rejects a reasoning item that is not immediately followed by
//! the output item it was generated with. Pi never has to think about
//! this directly: it only ever drops reasoning by dropping the whole
//! assistant message that holds it (rule 3), so a kept reasoning item is
//! always followed by whatever else came after it in that message.
//! Because we can also drop a *single* unresolved tool call out of an
//! otherwise-kept message (rule 2), a reasoning item can end up trailing
//! with nothing after it — e.g. `[thinking, toolCall(dropped)]`. Our
//! rule: within one assistant message's surviving items, a reasoning
//! item is dropped if it is not followed by another surviving item.
//! This is applied from the end of the message backwards (so
//! `[thinking, toolCall(dropped)]` drops the reasoning too, while
//! `[thinking, toolCall(dropped), toolCall(kept)]` keeps the reasoning,
//! now immediately before the surviving call). This also covers pi's own
//! known case of a turn aborted holding only reasoning
//! (`openai-responses-reasoning-replay-e2e.test.ts`): such a message has
//! `stop_reason: Aborted` and is dropped whole by rule 3 before this rule
//! even runs.
//!
//! ## Deviations from pi
//!
//! - No `Model` parameter: pi's `convertToolResultOutput` downgrades a
//!   tool image to `"(see attached image)"` when the target model does
//!   not accept images (`model.input.includes("image")`). `to_input`
//!   converts for a fixed API family (OpenAI Responses, our vision
//!   models only), so images are always kept.
//! - No cross-provider/cross-model normalization: pi's
//!   `normalizeToolCallId` rewrites ids and drops `id`/`thoughtSignature`
//!   when the assistant message came from a different provider or model
//!   (`openai-responses-shared.ts:165`, `transform-messages.ts:131`).
//!   tau-agent only ever talks to the OpenAI Responses API, so every
//!   tool call id already has the shape we wrote it in; the `item_id`
//!   half is always kept when present.
//! - [`split_tool_call_id`] splits at the *first* `|`, keeping the whole
//!   remainder as `item_id`. Pi's `id.split("|")` destructuring
//!   (`openai-responses-shared.ts:292`) only ever keeps the first two
//!   `split` segments and silently drops anything after a second `|`.
//!   Our ids never contain a second `|`, so this only changes behaviour
//!   for a malformed id, and our version round-trips losslessly, which
//!   pi's does not.
//! - An unparsable `thinkingSignature` is dropped instead of crashing.
//!   Pi's `JSON.parse(block.thinkingSignature)` (`openai-responses-shared.ts:266`)
//!   has no `try`/`catch` and throws. We never panic on transcript data,
//!   so an unparsable signature is treated the same as a missing one.
//! - `sanitizeSurrogates` (`openai-responses-shared.ts:36`) strips
//!   unpaired UTF-16 surrogates before sending text. Rust's `String` is
//!   guaranteed valid UTF-8 and cannot hold an unpaired surrogate, so
//!   there is nothing to sanitize.
//! - The fallback text-message id, used when a text block has no
//!   signature, is `msg_pi_{hash}` where `hash` is [`short_hash`] of the
//!   message's `response_id`, the text block's position in the message,
//!   and the text itself. pi instead counts emitted messages
//!   (`msg_pi_{msgIndex}`, `openai-responses-shared.ts:272`) — a position
//!   in the transcript it is converting. We cannot do that: `response_items`
//!   (below) converts one assistant message before it has any position at
//!   all (the transcript it will join has not been extended with it yet),
//!   so it must compute the *same* id [`to_input`] would compute for that
//!   message later, at whatever index it lands on. pi gets away with a
//!   position-based fallback only because OpenAI always sets a real
//!   signature on a text block in practice, so the fallback never actually
//!   fires on a round trip; we do not get to assume that, since
//!   `response_items` must agree with `to_input` even in that case.

use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use serde_json::{Value, json};

use crate::message::{
    AssistantBlock,
    AssistantMessage,
    InputBlock,
    Message,
    StopReason,
    TextContent,
    ThinkingContent,
    ToolCall,
    ToolResultMessage,
    UserContent,
    UserMessage,
};

/// Splits a tool call id into its `call_id` and `item_id` halves.
///
/// Ids we write ourselves always have the `call_id|item_id` shape (see
/// [`crate::message::ToolCall::id`]). An id with no `|` is treated as a
/// bare `call_id` with no `item_id`, matching how pi falls back when a
/// foreign id has none (`openai-responses-shared.ts:167`). See the
/// module docs for how this differs from pi's own `split("|")` when an
/// id holds more than one `|`.
pub fn split_tool_call_id(id: &str) -> (&str, Option<&str>) {
    match id.split_once('|') {
        Some((call_id, item_id)) => (call_id, Some(item_id)),
        None => (id, None),
    }
}

/// Converts a transcript into the Responses `input` array.
///
/// See the module docs for the item mapping, the invariants this
/// guarantees, and where it differs from pi.
pub fn to_input(messages: &[Message]) -> Vec<Value> {
    let groups: Vec<Vec<ConvertedItem>> =
        messages.iter().map(build_group).collect();
    let items = select(&groups);
    // The groups hold the other reference to each item: drop them so
    // the items unwrap without a copy.
    drop(groups);
    items.into_iter().map(Arc::unwrap_or_clone).collect()
}

/// Converts transcripts turn after turn, as [`to_input`] does, but keeps
/// each message's items and reuses them while the message stays the
/// same. A run's transcript only grows between turns, so each turn
/// converts only its new messages.
///
/// The items come out shared, and an unchanged message yields the very
/// same `Arc`s as before. That is what lets a lane match a request
/// against its baseline by pointer (see
/// [`continuation`](crate::ws::proto::continuation)).
#[derive(Debug, Default)]
pub struct InputCache {
    groups: Vec<(Message, Vec<ConvertedItem>)>,
}

impl InputCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// The `input` array for `messages`: equal to `to_input(messages)`.
    pub fn input(&mut self, messages: &[Message]) -> Vec<Arc<Value>> {
        let same = self
            .groups
            .iter()
            .zip(messages)
            .take_while(|((cached, _), message)| cached == *message)
            .count();
        self.groups.truncate(same);
        self.groups.extend(
            messages[same..]
                .iter()
                .map(|message| (message.clone(), build_group(message))),
        );
        let groups: Vec<&[ConvertedItem]> = self
            .groups
            .iter()
            .map(|(_, items)| items.as_slice())
            .collect();
        select(&groups)
    }
}

/// Picks the items [`to_input`] keeps from each message's converted
/// items.
fn select(groups: &[impl AsRef<[ConvertedItem]>]) -> Vec<Arc<Value>> {
    let groups: Vec<&[ConvertedItem]> =
        groups.iter().map(AsRef::as_ref).collect();
    let mut keep: Vec<Vec<bool>> =
        groups.iter().map(|g| vec![false; g.len()]).collect();
    let mut pending: HashMap<&str, VecDeque<(usize, usize)>> = HashMap::new();

    // Pair each function_call with the first later output of the same
    // call_id (invariants 1 and 2).
    for (gi, group) in groups.iter().enumerate() {
        for (ii, item) in group.iter().enumerate() {
            match item {
                ConvertedItem::Plain(_) | ConvertedItem::Reasoning(_) => {
                    keep[gi][ii] = true
                }
                ConvertedItem::FunctionCall { call_id, .. } => {
                    pending.entry(call_id).or_default().push_back((gi, ii));
                }
                ConvertedItem::ToolOutput { call_id, .. } => {
                    if let Some(queue) = pending.get_mut(call_id.as_str())
                        && let Some((cg, ci)) = queue.pop_front()
                    {
                        keep[cg][ci] = true;
                        keep[gi][ii] = true;
                    }
                }
            }
        }
    }

    // Trim a reasoning item left trailing by a dropped call (the
    // reasoning pairing rule; see the module docs).
    for (gi, group) in groups.iter().enumerate() {
        while let Some(idx) = (0..group.len()).rev().find(|&idx| keep[gi][idx])
        {
            if matches!(group[idx], ConvertedItem::Reasoning(_)) {
                keep[gi][idx] = false;
            } else {
                break;
            }
        }
    }

    let mut result = Vec::new();
    for (gi, group) in groups.iter().enumerate() {
        for (ii, item) in group.iter().enumerate() {
            if keep[gi][ii] {
                result.push(item.value().clone());
            }
        }
    }
    result
}

/// The items a completed assistant message contributes to the *next*
/// request's input, on the assumption that every one of its tool calls
/// gets a result: no pairing filter, and the same per-block conversion
/// [`to_input`] uses (including the same reasoning handling and the same
/// text-message id fallback — see the module docs for why that fallback
/// cannot depend on the message's position in a transcript).
///
/// This is the `output_items` the WebSocket delta rule needs
/// (`docs/reference/openai-websocket.md`, "The delta rule";
/// `ws::proto::continuation::Continuation::record`): after a response
/// completes, the lane's baseline is `request.input ++ response_items`,
/// and the *next* request can send only what comes after that baseline.
/// For that to work, `response_items(message)` must equal whatever
/// [`to_input`] would keep for `message` once every one of its tool
/// calls has a matching result later in the transcript — see
/// `response_items_matches_to_input_for_undamaged_transcripts` in
/// `tests/responses_input.rs`.
///
/// Pi computes this by converting a single-message context
/// (`openai-codex-responses.ts:1566-1580`) and filtering out
/// `function_call_output`/`custom_tool_call_output` — which, for a
/// single assistant message, never appear anyway. It still goes through
/// `transformMessages`, so an errored/aborted turn (rule 3) contributes
/// nothing there too.
pub fn response_items(message: &AssistantMessage) -> Vec<Value> {
    let mut items = build_assistant_group(message);
    trim_trailing_reasoning(&mut items);
    items
        .into_iter()
        .map(|item| Arc::unwrap_or_clone(item.into_value()))
        .collect()
}

/// One converted Responses item, tagged with what [`to_input`] needs to
/// decide whether to keep it.
#[derive(Debug)]
enum ConvertedItem {
    /// Always kept: a user item or an assistant text item.
    Plain(Arc<Value>),
    /// A reasoning item, kept unless left trailing (see the module docs).
    Reasoning(Arc<Value>),
    /// A `function_call` item, kept only if a later output claims it.
    FunctionCall { value: Arc<Value>, call_id: String },
    /// A `function_call_output` item, kept only if it claims an earlier
    /// call.
    ToolOutput { value: Arc<Value>, call_id: String },
}

impl ConvertedItem {
    fn into_value(self) -> Arc<Value> {
        match self {
            ConvertedItem::Plain(v)
            | ConvertedItem::Reasoning(v)
            | ConvertedItem::FunctionCall { value: v, .. }
            | ConvertedItem::ToolOutput { value: v, .. } => v,
        }
    }

    fn value(&self) -> &Arc<Value> {
        match self {
            ConvertedItem::Plain(v)
            | ConvertedItem::Reasoning(v)
            | ConvertedItem::FunctionCall { value: v, .. }
            | ConvertedItem::ToolOutput { value: v, .. } => v,
        }
    }
}

fn build_group(message: &Message) -> Vec<ConvertedItem> {
    match message {
        Message::User(user) => build_user_item(user)
            .into_iter()
            .map(|item| ConvertedItem::Plain(Arc::new(item)))
            .collect(),
        Message::ToolResult(result) => vec![build_tool_output(result)],
        Message::Assistant(assistant) => build_assistant_group(assistant),
    }
}

/// Drops trailing reasoning items with nothing after them (the reasoning
/// pairing rule; see the module docs). [`to_input`] applies the same
/// rule over its "keep" flags, which also account for a dropped tool
/// call; here every item is assumed to survive, so trimming reduces to
/// popping reasoning items off the end.
fn trim_trailing_reasoning(items: &mut Vec<ConvertedItem>) {
    while matches!(items.last(), Some(ConvertedItem::Reasoning(_))) {
        items.pop();
    }
}

/// Builds the single user item for a message, or `None` when it would
/// carry no content (pi: `openai-responses-shared.ts:249`).
fn build_user_item(user: &UserMessage) -> Option<Value> {
    match &user.content {
        // pi always wraps a plain string in one `input_text` block; it
        // never sends `content` as a bare string
        // (`openai-responses-shared.ts:230-234`).
        UserContent::Text(text) => Some(json!({
            "role": "user",
            "content": [{ "type": "input_text", "text": text }],
        })),
        UserContent::Blocks(blocks) => {
            let content: Vec<Value> =
                blocks.iter().map(build_input_block).collect();
            if content.is_empty() {
                None
            } else {
                Some(json!({ "role": "user", "content": content }))
            }
        }
    }
}

fn build_input_block(block: &InputBlock) -> Value {
    match block {
        InputBlock::Text(text) => {
            json!({ "type": "input_text", "text": text.text })
        }
        InputBlock::Image(image) => json!({
            "type": "input_image",
            "detail": "auto",
            "image_url": format!("data:{};base64,{}", image.mime_type, image.data),
        }),
    }
}

/// Builds the raw, unfiltered items for one assistant message (every
/// tool call kept, no reasoning trimmed), dropping the whole message
/// when its turn errored or was aborted (invariant 3). [`to_input`] pairs
/// and trims this further; [`response_items`] only trims trailing
/// reasoning.
fn build_assistant_group(assistant: &AssistantMessage) -> Vec<ConvertedItem> {
    if matches!(
        assistant.stop_reason,
        StopReason::Error | StopReason::Aborted
    ) {
        return Vec::new();
    }
    let mut items = Vec::new();
    let mut text_block_index = 0usize;
    for block in &assistant.content {
        match block {
            AssistantBlock::Text(text) => {
                items.push(ConvertedItem::Plain(Arc::new(build_text_item(
                    assistant.response_id.as_deref(),
                    text_block_index,
                    text,
                ))));
                text_block_index += 1;
            }
            AssistantBlock::Thinking(thinking) => {
                if let Some(value) = build_reasoning_item(thinking) {
                    items.push(ConvertedItem::Reasoning(Arc::new(value)));
                }
            }
            AssistantBlock::ToolCall(call) => {
                items.push(build_function_call(call));
            }
        }
    }
    items
}

fn build_text_item(
    response_id: Option<&str>,
    text_block_index: usize,
    text: &TextContent,
) -> Value {
    let (mut id, phase) = match &text.text_signature {
        Some(signature) => parse_text_signature(signature),
        None => (
            fallback_text_id(response_id, text_block_index, &text.text),
            None,
        ),
    };
    // OpenAI requires the id to be at most 64 characters
    // (`openai-responses-shared.ts:275-281`).
    if text.text_signature.is_some() && id.len() > 64 {
        id = format!("msg_{}", short_hash(&id));
    }
    let mut value = json!({
        "type": "message",
        "role": "assistant",
        "content": [{ "type": "output_text", "text": text.text, "annotations": [] }],
        "status": "completed",
        "id": id,
    });
    if let Some(phase) = phase {
        value["phase"] = json!(phase);
    }
    value
}

/// A deterministic id for a text block with no signature, derived only
/// from data intrinsic to the block itself (never from a transcript
/// position — see the module docs on why [`response_items`] requires
/// that).
fn fallback_text_id(
    response_id: Option<&str>,
    text_block_index: usize,
    text: &str,
) -> String {
    let key = format!(
        "{}\u{0}{text_block_index}\u{0}{text}",
        response_id.unwrap_or("")
    );
    format!("msg_pi_{}", short_hash(&key))
}

/// Parses a `textSignature`: pi's `TextSignatureV1` JSON (`{v: 1, id,
/// phase?}`) when it decodes to one, otherwise the whole string is the
/// (legacy, plain) id, unparsed (`openai-responses-shared.ts:59-77`).
fn parse_text_signature(signature: &str) -> (String, Option<String>) {
    if signature.starts_with('{')
        && let Ok(value) = serde_json::from_str::<Value>(signature)
        && value.get("v").and_then(Value::as_i64) == Some(1)
        && let Some(id) = value.get("id").and_then(Value::as_str)
    {
        let phase = value
            .get("phase")
            .and_then(Value::as_str)
            .filter(|phase| *phase == "commentary" || *phase == "final_answer")
            .map(str::to_owned);
        return (id.to_owned(), phase);
    }
    (signature.to_owned(), None)
}

/// Replays the stored reasoning item verbatim
/// (`openai-responses-shared.ts:264-268`). `None` when there is no
/// signature, or it does not parse as JSON (a deviation from pi: see the
/// module docs).
fn build_reasoning_item(thinking: &ThinkingContent) -> Option<Value> {
    let signature = thinking.thinking_signature.as_ref()?;
    serde_json::from_str::<Value>(signature).ok()
}

fn build_function_call(call: &ToolCall) -> ConvertedItem {
    let (call_id, item_id) = split_tool_call_id(&call.id);
    let arguments = serde_json::to_string(&call.arguments)
        .expect("a JSON object always serializes");
    let mut value = json!({
        "type": "function_call",
        "call_id": call_id,
        "name": call.name,
        "arguments": arguments,
    });
    if let Some(item_id) = item_id {
        value["id"] = json!(item_id);
    }
    ConvertedItem::FunctionCall {
        value: Arc::new(value),
        call_id: call_id.to_owned(),
    }
}

fn build_tool_output(result: &ToolResultMessage) -> ConvertedItem {
    let (call_id, _item_id) = split_tool_call_id(&result.tool_call_id);
    let output = convert_tool_result_output(&result.content);
    let value = json!({
        "type": "function_call_output",
        "call_id": call_id,
        "output": output,
    });
    ConvertedItem::ToolOutput {
        value: Arc::new(value),
        call_id: call_id.to_owned(),
    }
}

/// Ports `convertToolResultOutput` (`openai-responses-shared.ts:81-108`)
/// for a model that always accepts images (see the module docs).
fn convert_tool_result_output(content: &[InputBlock]) -> Value {
    let mut text_parts = Vec::new();
    let mut images = Vec::new();
    for block in content {
        match block {
            InputBlock::Text(text) => text_parts.push(text.text.as_str()),
            InputBlock::Image(image) => images.push(image),
        }
    }
    let text_result = text_parts.join("\n");
    let has_text = !text_result.is_empty();

    if images.is_empty() {
        return Value::String(if has_text {
            text_result
        } else {
            "(no tool output)".to_owned()
        });
    }

    let mut output = Vec::new();
    if has_text {
        output.push(json!({ "type": "input_text", "text": text_result }));
    }
    for image in images {
        output.push(json!({
            "type": "input_image",
            "detail": "auto",
            "image_url": format!("data:{};base64,{}", image.mime_type, image.data),
        }));
    }
    Value::Array(output)
}

/// Ports pi's `shortHash` (`packages/ai/src/utils/hash.ts`), used to
/// shorten a `textSignature` id past OpenAI's 64-character limit.
fn short_hash(s: &str) -> String {
    let mut h1: u32 = 0xdead_beef;
    let mut h2: u32 = 0x41c6_ce57;
    for unit in s.encode_utf16() {
        let ch = u32::from(unit);
        h1 = (h1 ^ ch).wrapping_mul(2_654_435_761);
        h2 = (h2 ^ ch).wrapping_mul(1_597_334_677);
    }
    let h1 = (h1 ^ (h1 >> 16)).wrapping_mul(2_246_822_507)
        ^ (h2 ^ (h2 >> 13)).wrapping_mul(3_266_489_909);
    let h2 = (h2 ^ (h2 >> 16)).wrapping_mul(2_246_822_507)
        ^ (h1 ^ (h1 >> 13)).wrapping_mul(3_266_489_909);
    format!("{}{}", to_base36(h2), to_base36(h1))
}

fn to_base36(mut n: u32) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".to_owned();
    }
    let mut buf = Vec::new();
    while n > 0 {
        buf.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    buf.reverse();
    String::from_utf8(buf).expect("base36 digits are ASCII")
}

// `short_hash` and `to_base36` are private helpers with no invariant of
// their own to state as a property; they port one fixed algorithm
// (pi's `shortHash`), so the useful check is that the port is bit-exact.
// These pin values computed from a from-scratch reimplementation of
// pi's algorithm (`packages/ai/src/utils/hash.ts`) in Python, not from
// running pi itself, but they still catch any drift in the bit
// arithmetic (`^`, `>>`, `wrapping_mul`, base-36 digit order).
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_base36_pins_known_values() {
        assert_eq!(to_base36(0), "0");
        assert_eq!(to_base36(1), "1");
        assert_eq!(to_base36(35), "z");
        assert_eq!(to_base36(36), "10");
        assert_eq!(to_base36(37), "11");
        assert_eq!(to_base36(u32::MAX), "1z141z3");
    }

    /// `to_base36` writes any `u32` in base 36, with lowercase digits
    /// and no leading zero: reading it back gives the number.
    #[hegel::test]
    fn to_base36_round_trips(tc: hegel::TestCase) {
        let n = tc.draw(hegel::generators::integers::<u32>());
        let text = to_base36(n);
        assert_eq!(u32::from_str_radix(&text, 36), Ok(n));
        assert_eq!(text, text.to_lowercase());
        assert!(n == 0 || !text.starts_with('0'), "{text}");
    }

    #[test]
    fn short_hash_pins_known_values() {
        assert_eq!(short_hash(""), "k4n83c7h0j2b");
        assert_eq!(short_hash("abc"), "y0biex7f9bbh");
        assert_eq!(short_hash(&"x".repeat(65)), "yl02lyv9wrwf");
    }
}
