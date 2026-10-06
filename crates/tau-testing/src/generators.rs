//! Shared Hegel generators.
//!
//! Every generator builds valid values directly and stays small, so a
//! shrunk counterexample is short enough to read.

use std::{collections::HashSet, time::Duration};

use hegel::{
    TestCase,
    generators::{self as gs, Generator, PrintableGenerator},
};
use serde_json::{Map, Number, Value};
use tau_ai::{
    message::{
        AssistantBlock,
        AssistantMessage,
        ImageContent,
        InputBlock,
        Message,
        StopReason,
        TextContent,
        ThinkingContent,
        ToolCall,
        ToolResultMessage,
        Usage,
        UsageCost,
        UserContent,
        UserMessage,
    },
    responses::stream::encode_text_signature_v1,
};

/// Characters the code handles specially: line endings, a BOM, the
/// Unicode line separators, NNBSP, smart quotes, a combining mark and
/// multi-byte characters.
pub const SPECIAL_CHARS: &str = "ab \n\r\t\u{feff}\u{2028}\u{2029}\u{202f}\u{2018}\u{2019}\u{201c}\u{201d}\u{2014}e\u{301}é日🦀";

/// Short text: either arbitrary Unicode or text built from
/// [`SPECIAL_CHARS`].
#[hegel::composite]
pub fn text(tc: &TestCase, max_size: usize) -> String {
    tc.draw(hegel::one_of!(
        gs::text().max_size(max_size),
        gs::text().alphabet(SPECIAL_CHARS).max_size(max_size),
    ))
}

/// A short identifier, like the ids OpenAI assigns.
#[hegel::composite]
pub fn id(tc: &TestCase, prefix: &'static str) -> String {
    let suffix: String = tc.draw(
        gs::text()
            .alphabet("abcdefghijklmnopqrstuvwxyz0123456789")
            .min_size(1)
            .max_size(8),
    );
    format!("{prefix}{suffix}")
}

/// A JSON number that survives a text round trip.
#[hegel::composite]
pub fn json_number(tc: &TestCase) -> Number {
    if tc.draw(gs::booleans()) {
        Number::from(tc.draw(gs::integers::<i64>()))
    } else {
        let f: f64 =
            tc.draw(gs::floats::<f64>().allow_nan(false).allow_infinity(false));
        Number::from_f64(f).expect("finite floats are valid JSON numbers")
    }
}

/// Any JSON value, nested at most `depth` levels: arrays and objects of
/// at most four entries, whose strings and keys lean on
/// [`SPECIAL_CHARS`].
pub(crate) fn json_value(depth: u32) -> impl PrintableGenerator<Value> {
    gs::recursive(
        hegel::one_of!(
            gs::just(Value::Null),
            gs::booleans().map(Value::Bool),
            json_number().map(Value::Number),
            text(16).map(Value::String),
        ),
        |values| {
            hegel::one_of!(
                gs::vecs(values.clone()).max_size(4).map(Value::Array),
                gs::vecs(hegel::tuples!(text(8), values)).max_size(4).map(
                    |entries| Value::Object(entries.into_iter().collect())
                ),
            )
        },
    )
    .max_depth(depth as usize)
}

/// A JSON object, nested at most `depth` levels below its fields.
pub fn json_object(depth: u32) -> impl PrintableGenerator<Map<String, Value>> {
    gs::vecs(hegel::tuples!(text(8), json_value(depth)))
        .max_size(4)
        .map(|entries| entries.into_iter().collect::<Map<String, Value>>())
}

pub fn usage() -> impl PrintableGenerator<Usage> {
    // Usage is tau's own type, so its drawn values print through Debug.
    usage_unprinted().print_as_debug()
}

#[hegel::composite]
fn usage_unprinted(tc: &TestCase) -> Usage {
    let tokens = || gs::integers::<u64>().max_value(1 << 40);
    let dollars = || {
        gs::floats::<f64>()
            .min_value(0.0)
            .max_value(1e6)
            .allow_nan(false)
            .allow_infinity(false)
    };
    Usage {
        input: tc.draw(tokens()),
        output: tc.draw(tokens()),
        cache_read: tc.draw(tokens()),
        cache_write: tc.draw(tokens()),
        reasoning: tc.draw(gs::optional(tokens())),
        total_tokens: tc.draw(tokens()),
        cost: UsageCost {
            input: tc.draw(dollars()),
            output: tc.draw(dollars()),
            cache_read: tc.draw(dollars()),
            cache_write: tc.draw(dollars()),
            total: tc.draw(dollars()),
        },
    }
}

pub fn stop_reason() -> impl PrintableGenerator<StopReason> {
    // StopReason is tau's own type, so its drawn values print through Debug.
    stop_reason_unprinted().print_as_debug()
}

#[hegel::composite]
fn stop_reason_unprinted(tc: &TestCase) -> StopReason {
    tc.draw(
        gs::sampled_from(vec![
            StopReason::Stop,
            StopReason::Length,
            StopReason::ToolUse,
            StopReason::Error,
            StopReason::Aborted,
        ])
        .print_as_debug(),
    )
}

pub fn text_content() -> impl PrintableGenerator<TextContent> {
    // TextContent is tau's own type, so its drawn values print through Debug.
    text_content_unprinted().print_as_debug()
}

#[hegel::composite]
fn text_content_unprinted(tc: &TestCase) -> TextContent {
    let id = tc.draw(gs::optional(id("msg_")));
    TextContent {
        text: tc.draw(text(40)),
        text_signature: id.map(|id| encode_text_signature_v1(&id, None)),
    }
}

pub(crate) fn thinking_content() -> impl PrintableGenerator<ThinkingContent> {
    // ThinkingContent is tau's own type, so its drawn values print through Debug.
    thinking_content_unprinted().print_as_debug()
}

#[hegel::composite]
fn thinking_content_unprinted(tc: &TestCase) -> ThinkingContent {
    ThinkingContent {
        thinking: tc.draw(text(40)),
        thinking_signature: tc.draw(gs::optional(id("rs_"))),
        redacted: tc.draw(gs::optional(gs::booleans())),
    }
}

pub fn image_content() -> impl PrintableGenerator<ImageContent> {
    // ImageContent is tau's own type, so its drawn values print through Debug.
    image_content_unprinted().print_as_debug()
}

#[hegel::composite]
fn image_content_unprinted(tc: &TestCase) -> ImageContent {
    ImageContent {
        data: tc.draw(
            gs::text()
                .alphabet("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/=")
                .max_size(24),
        ),
        mime_type: tc.draw(gs::sampled_from(vec![
            "image/png".to_owned(),
            "image/jpeg".to_owned(),
        ])),
    }
}

/// A tool call whose id has pi's `call_id|item_id` form.
pub fn tool_call() -> impl PrintableGenerator<ToolCall> {
    // ToolCall is tau's own type, so its drawn values print through Debug.
    tool_call_unprinted().print_as_debug()
}

#[hegel::composite]
fn tool_call_unprinted(tc: &TestCase) -> ToolCall {
    let call_id = tc.draw(id("call_"));
    let item_id = tc.draw(id("fc_"));
    ToolCall {
        id: format!("{call_id}|{item_id}"),
        name: tc.draw(id("tool_")),
        arguments: tc.draw(json_object(2)),
    }
}

pub(crate) fn input_block() -> impl PrintableGenerator<InputBlock> {
    // InputBlock is tau's own type, so its drawn values print through Debug.
    input_block_unprinted().print_as_debug()
}

fn input_block_unprinted() -> impl Generator<InputBlock> {
    hegel::one_of!(
        text_content().map(InputBlock::Text),
        image_content().map(InputBlock::Image),
    )
}

pub(crate) fn assistant_block() -> impl PrintableGenerator<AssistantBlock> {
    // AssistantBlock is tau's own type, so its drawn values print through Debug.
    assistant_block_unprinted().print_as_debug()
}

fn assistant_block_unprinted() -> impl Generator<AssistantBlock> {
    hegel::one_of!(
        text_content().map(AssistantBlock::Text),
        thinking_content().map(AssistantBlock::Thinking),
        tool_call().map(AssistantBlock::ToolCall),
    )
}

pub fn user_message() -> impl PrintableGenerator<UserMessage> {
    // UserMessage is tau's own type, so its drawn values print through Debug.
    user_message_unprinted().print_as_debug()
}

#[hegel::composite]
fn user_message_unprinted(tc: &TestCase) -> UserMessage {
    let content = if tc.draw(gs::booleans()) {
        UserContent::Text(tc.draw(text(40)))
    } else {
        UserContent::Blocks(tc.draw(gs::vecs(input_block()).max_size(3)))
    };
    UserMessage {
        content,
        timestamp: tc.draw(timestamp()),
    }
}

pub fn assistant_message() -> impl PrintableGenerator<AssistantMessage> {
    // AssistantMessage is tau's own type, so its drawn values print through Debug.
    assistant_message_unprinted().print_as_debug()
}

#[hegel::composite]
fn assistant_message_unprinted(tc: &TestCase) -> AssistantMessage {
    let stop_reason = tc.draw(stop_reason());
    let failed = matches!(stop_reason, StopReason::Error | StopReason::Aborted);
    let mut content: Vec<AssistantBlock> =
        tc.draw(gs::vecs(assistant_block()).max_size(4));
    CallIds::default().make_unique(&mut content);
    AssistantMessage {
        content,
        model: tc.draw(gs::sampled_from(vec![
            "gpt-5.5".to_owned(),
            "gpt-5.5-mini".to_owned(),
        ])),
        response_id: tc.draw(gs::optional(id("resp_"))),
        usage: tc.draw(usage()),
        stop_reason,
        error_message: if failed {
            Some(tc.draw(text(40)))
        } else {
            None
        },
        timestamp: tc.draw(timestamp()),
    }
}

pub(crate) fn tool_result_message() -> impl PrintableGenerator<ToolResultMessage>
{
    // ToolResultMessage is tau's own type, so its drawn values print through Debug.
    tool_result_message_unprinted().print_as_debug()
}

#[hegel::composite]
fn tool_result_message_unprinted(tc: &TestCase) -> ToolResultMessage {
    ToolResultMessage {
        tool_call_id: tc.draw(id("call_")),
        tool_name: tc.draw(id("tool_")),
        content: tc.draw(gs::vecs(input_block()).max_size(3)),
        details: tc.draw(gs::optional(json_value(2))),
        is_error: tc.draw(gs::booleans()),
        timestamp: tc.draw(timestamp()),
    }
}

pub fn message() -> impl PrintableGenerator<Message> {
    // Message is tau's own type, so its drawn values print through Debug.
    message_unprinted().print_as_debug()
}

fn message_unprinted() -> impl Generator<Message> {
    hegel::one_of!(
        user_message().map(Message::User),
        assistant_message().map(Message::Assistant),
        tool_result_message().map(Message::ToolResult),
    )
}

/// Milliseconds since the Unix epoch, within the next few centuries.
pub fn timestamp() -> impl PrintableGenerator<u64> {
    gs::integers::<u64>().max_value(10_000_000_000_000)
}

/// A [`tau_ai::retry::RetryPolicy`] with small, valid bounds: `base` in
/// `1ms..=10s`, `max_delay >= base` (also within a few seconds of it, so
/// shrunk failures stay short), and `max_attempts` in `0..=10`.
pub fn retry_policy() -> impl PrintableGenerator<tau_ai::retry::RetryPolicy> {
    // RetryPolicy is tau's own type, so its drawn values print through Debug.
    retry_policy_unprinted().print_as_debug()
}

#[hegel::composite]
fn retry_policy_unprinted(tc: &TestCase) -> tau_ai::retry::RetryPolicy {
    let base_ms = tc.draw(gs::integers::<u64>().min_value(1).max_value(10_000));
    let extra_ms =
        tc.draw(gs::integers::<u64>().min_value(0).max_value(10_000));
    tau_ai::retry::RetryPolicy {
        max_attempts: tc.draw(gs::integers::<u32>().max_value(10)),
        base: Duration::from_millis(base_ms),
        max_delay: Duration::from_millis(base_ms + extra_ms),
    }
}

/// A [`Usage`] whose token counts are each drawn independently up to
/// `max_tokens`, with `cost` left at its default (callers that exercise
/// `tau_ai::cost` compute it themselves). Useful for properties that
/// need to keep a request's total input tokens
/// (`input + cache_read + cache_write`) under a known bound, such as a
/// model's pricing-tier threshold.
pub fn usage_with_max_tokens(
    max_tokens: u64,
) -> impl PrintableGenerator<Usage> {
    // Usage is tau's own type, so its drawn values print through Debug.
    usage_with_max_tokens_unprinted(max_tokens).print_as_debug()
}

#[hegel::composite]
fn usage_with_max_tokens_unprinted(tc: &TestCase, max_tokens: u64) -> Usage {
    let tokens = || gs::integers::<u64>().max_value(max_tokens);
    Usage {
        input: tc.draw(tokens()),
        output: tc.draw(tokens()),
        cache_read: tc.draw(tokens()),
        cache_write: tc.draw(tokens()),
        reasoning: tc.draw(gs::optional(tokens())),
        total_tokens: tc.draw(tokens()),
        cost: UsageCost::default(),
    }
}

/// Splits `text` into a random sequence of chunks at character
/// boundaries, for feeding an incremental parser one arbitrary piece at
/// a time. Concatenating the result always reproduces `text` exactly:
/// the chunking can be a single chunk (what it shrinks toward), one
/// chunk per character, or anything in between. No chunk is empty,
/// unless `text` is.
#[hegel::composite]
pub fn char_chunks(tc: &TestCase, text: String) -> Vec<String> {
    let cuts: Vec<usize> = tc.draw(gs::subsequences(inner_boundaries(&text)));
    split_at_cuts(&text, &cuts)
}

/// The char boundaries of `text` strictly inside it.
pub(crate) fn inner_boundaries(text: &str) -> Vec<usize> {
    text.char_indices().map(|(i, _)| i).skip(1).collect()
}

/// `text` cut at `cuts` (increasing char boundaries inside it).
pub(crate) fn split_at_cuts(text: &str, cuts: &[usize]) -> Vec<String> {
    let mut chunks = Vec::with_capacity(cuts.len() + 1);
    let mut start = 0;
    for &cut in cuts {
        chunks.push(text[start..cut].to_owned());
        start = cut;
    }
    chunks.push(text[start..].to_owned());
    chunks
}

/// Tool call and item ids already used, so a message or transcript can
/// give each call its own.
#[derive(Default)]
pub(crate) struct CallIds {
    calls: HashSet<String>,
    items: HashSet<String>,
}

impl CallIds {
    /// Renames the tool calls in `blocks` whose `call_id` or `item_id`
    /// half was already used, by appending a number to that half. Drawn
    /// ids are short and shrink toward each other, so without this a
    /// message or transcript would often hold two calls with one id.
    pub(crate) fn make_unique(&mut self, blocks: &mut [AssistantBlock]) {
        for block in blocks {
            let AssistantBlock::ToolCall(call) = block else {
                continue;
            };
            call.id = match call.id.split_once('|') {
                Some((call_id, item_id)) => format!(
                    "{}|{}",
                    fresh(&mut self.calls, call_id),
                    fresh(&mut self.items, item_id)
                ),
                None => fresh(&mut self.calls, &call.id),
            };
        }
    }
}

/// `id`, or `id` with the smallest number appended that `used` does not
/// hold yet; either way, now in `used`.
fn fresh(used: &mut HashSet<String>, id: &str) -> String {
    let unique = if used.contains(id) {
        (2..)
            .map(|n| format!("{id}{n}"))
            .find(|candidate| !used.contains(candidate))
            .expect("some number is free")
    } else {
        id.to_owned()
    };
    used.insert(unique.clone());
    unique
}

pub mod lane;

// =============================================================================
// Transcripts for `tau_ai::responses::input` (crates/tau-ai/tests/responses_input.rs)
// =============================================================================

/// A JSON string shaped like an OpenAI reasoning item: `{"id", "type":
/// "reasoning", "summary": [], "encrypted_content"?}`. This is what a
/// [`ThinkingContent::thinking_signature`] holds for real (pi stores the
/// serialized reasoning item there), so a thinking block built from
/// [`thinking_content_for_transcript`] always replays.
#[hegel::composite]
pub(crate) fn reasoning_item_json(tc: &TestCase) -> String {
    let value = serde_json::json!({
        "id": tc.draw(id("rs_")),
        "type": "reasoning",
        "summary": [],
        "encrypted_content": tc.draw(gs::optional(text(20))),
    });
    serde_json::to_string(&value).expect("a JSON object always serializes")
}

/// A [`ThinkingContent`] whose signature is usually a real, parseable
/// reasoning item ([`reasoning_item_json`]), but sometimes absent or
/// unparsable text, so transcripts drawn from it exercise all three
/// paths in `responses::input::build_reasoning_item`.
pub(crate) fn thinking_content_for_transcript()
-> impl PrintableGenerator<ThinkingContent> {
    // ThinkingContent is tau's own type, so its drawn values print through Debug.
    thinking_content_for_transcript_unprinted().print_as_debug()
}

#[hegel::composite]
fn thinking_content_for_transcript_unprinted(tc: &TestCase) -> ThinkingContent {
    let signature = tc.draw(hegel::one_of!(
        gs::just(None),
        text(10).map(Some),
        reasoning_item_json().map(Some),
    ));
    ThinkingContent {
        thinking: tc.draw(text(40)),
        thinking_signature: signature,
        redacted: tc.draw(gs::optional(gs::booleans())),
    }
}

/// Like [`assistant_block`], but its thinking blocks come from
/// [`thinking_content_for_transcript`], so most of them hold a real
/// reasoning item instead of the bare id [`thinking_content`] draws.
pub(crate) fn assistant_block_for_transcript()
-> impl PrintableGenerator<AssistantBlock> {
    // AssistantBlock is tau's own type, so its drawn values print through Debug.
    assistant_block_for_transcript_unprinted().print_as_debug()
}

fn assistant_block_for_transcript_unprinted() -> impl Generator<AssistantBlock>
{
    hegel::one_of!(
        text_content().map(AssistantBlock::Text),
        thinking_content_for_transcript().map(AssistantBlock::Thinking),
        tool_call().map(AssistantBlock::ToolCall),
    )
}

/// One assistant turn of a [`transcript`]: `stop_reason` is always
/// `ToolUse` when the turn holds a tool call, and `Stop` or `Length`
/// otherwise, so a `transcript()` is realistic and never needs an
/// `Error`/`Aborted` turn to be well-formed.
pub(crate) fn assistant_step() -> impl PrintableGenerator<AssistantMessage> {
    // AssistantMessage is tau's own type, so its drawn values print through Debug.
    assistant_step_unprinted().print_as_debug()
}

#[hegel::composite]
fn assistant_step_unprinted(tc: &TestCase) -> AssistantMessage {
    let mut content: Vec<AssistantBlock> = tc.draw(
        gs::vecs(assistant_block_for_transcript())
            .min_size(1)
            .max_size(4),
    );
    CallIds::default().make_unique(&mut content);
    let has_tool_call = content
        .iter()
        .any(|b| matches!(b, AssistantBlock::ToolCall(_)));
    let stop_reason = if has_tool_call {
        StopReason::ToolUse
    } else {
        tc.draw(
            gs::sampled_from(vec![StopReason::Stop, StopReason::Length])
                .print_as_debug(),
        )
    };
    AssistantMessage {
        content,
        model: tc.draw(gs::sampled_from(vec![
            "gpt-5.5".to_owned(),
            "gpt-5.5-mini".to_owned(),
        ])),
        response_id: tc.draw(gs::optional(id("resp_"))),
        usage: tc.draw(usage()),
        stop_reason,
        error_message: None,
        timestamp: tc.draw(timestamp()),
    }
}

/// A [`ToolResultMessage`] that answers `call`: same id and tool name,
/// otherwise drawn like `tool_result_message`.
pub fn tool_result_for_call(
    call: ToolCall,
) -> impl PrintableGenerator<ToolResultMessage> {
    // ToolResultMessage is tau's own type, so its drawn values print through Debug.
    tool_result_for_call_unprinted(call).print_as_debug()
}

#[hegel::composite]
fn tool_result_for_call_unprinted(
    tc: &TestCase,
    call: ToolCall,
) -> ToolResultMessage {
    ToolResultMessage {
        tool_call_id: call.id.clone(),
        tool_name: call.name.clone(),
        content: tc.draw(gs::vecs(input_block()).max_size(3)),
        details: tc.draw(gs::optional(json_value(2))),
        is_error: tc.draw(gs::booleans()),
        timestamp: tc.draw(timestamp()),
    }
}

/// A realistic, valid-by-construction transcript: one or more `user ->
/// assistant (-> tool results -> assistant)*` turns. Every tool call has
/// its own id and exactly one matching result, and no assistant message
/// ever errors or aborts, so converting it with
/// `tau_ai::responses::input::to_input` drops nothing.
pub fn transcript() -> impl PrintableGenerator<Vec<Message>> {
    // Vec<Message> is tau's own type, so its drawn values print through Debug.
    transcript_unprinted().print_as_debug()
}

#[hegel::composite]
fn transcript_unprinted(tc: &TestCase) -> Vec<Message> {
    let turns = tc.draw(gs::integers::<usize>().min_value(1).max_value(3));
    let mut messages = Vec::new();
    let mut ids = CallIds::default();
    for _ in 0..turns {
        messages.push(Message::User(tc.draw(user_message())));
        let max_rounds =
            tc.draw(gs::integers::<usize>().min_value(1).max_value(3));
        for round in 0..max_rounds {
            let mut step = tc.draw(assistant_step());
            ids.make_unique(&mut step.content);
            let calls: Vec<ToolCall> = step
                .content
                .iter()
                .filter_map(|b| match b {
                    AssistantBlock::ToolCall(call) => Some(call.clone()),
                    _ => None,
                })
                .collect();
            messages.push(Message::Assistant(step));
            for call in &calls {
                messages.push(Message::ToolResult(
                    tc.draw(tool_result_for_call(call.clone())),
                ));
            }
            if calls.is_empty() || round + 1 == max_rounds {
                break;
            }
        }
    }
    messages
}

/// A [`transcript`] with damage applied: some tool results dropped
/// (orphaning their calls), stray tool results inserted (matching no
/// call, or claiming a real call's id wherever they land), an existing
/// result moved out of place (before its call, or after the next user
/// message), a random assistant turn flipped to `Error`/`Aborted` (while
/// its already-recorded tool results stay put, pi's orphan bug from
/// `transform-messages.ts:201`), and sometimes a synthetic turn that
/// aborts holding only reasoning (pi's known case,
/// `openai-responses-reasoning-replay-e2e.test.ts`).
pub fn damaged_transcript() -> impl PrintableGenerator<Vec<Message>> {
    // Vec<Message> is tau's own type, so its drawn values print through Debug.
    damaged_transcript_unprinted().print_as_debug()
}

#[hegel::composite]
fn damaged_transcript_unprinted(tc: &TestCase) -> Vec<Message> {
    let messages = tc.draw(transcript());

    // Drop some tool results, leaving their calls orphaned.
    let mut kept_flags = Vec::with_capacity(messages.len());
    for message in &messages {
        let drop = matches!(message, Message::ToolResult(_))
            && tc.draw(gs::booleans());
        kept_flags.push(!drop);
    }
    let mut messages: Vec<Message> = messages
        .into_iter()
        .zip(kept_flags)
        .filter_map(|(m, keep)| keep.then_some(m))
        .collect();

    // Insert a few stray tool results: most answer no call, some claim
    // the id of a real call, which then has two results.
    let call_ids: Vec<String> = messages
        .iter()
        .flat_map(|m| match m {
            Message::Assistant(a) => a
                .content
                .iter()
                .filter_map(|b| match b {
                    AssistantBlock::ToolCall(call) => Some(call.id.clone()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        })
        .collect();
    let stray_count = tc.draw(gs::integers::<usize>().max_value(2));
    for _ in 0..stray_count {
        let mut stray = tc.draw(tool_result_message());
        if !call_ids.is_empty() && tc.draw(gs::weighted_booleans(0.3)) {
            stray.tool_call_id = tc.draw(gs::sampled_from(call_ids.clone()));
        }
        let at = tc.draw(gs::integers::<usize>().max_value(messages.len()));
        messages.insert(at, Message::ToolResult(stray));
    }

    // Move one result out of place: before the message holding its call,
    // or past the next user message.
    let results: Vec<usize> = (0..messages.len())
        .filter(|&i| matches!(messages[i], Message::ToolResult(_)))
        .collect();
    if !results.is_empty() && tc.draw(gs::weighted_booleans(0.3)) {
        let from = tc.draw(gs::sampled_from(results));
        let Message::ToolResult(result) = messages.remove(from) else {
            unreachable!("a result position")
        };
        let call_at = messages[..from].iter().rposition(|m| match m {
            Message::Assistant(a) => a.content.iter().any(|b| {
                matches!(b, AssistantBlock::ToolCall(c) if c.id == result.tool_call_id)
            }),
            _ => false,
        });
        let after_user = messages[from..]
            .iter()
            .position(|m| matches!(m, Message::User(_)))
            .map(|i| from + i + 1);
        let targets: Vec<usize> =
            call_at.into_iter().chain(after_user).collect();
        let to = if targets.is_empty() {
            from
        } else {
            tc.draw(gs::sampled_from(targets))
        };
        messages.insert(to, Message::ToolResult(result));
    }

    // Turn a random assistant message into an aborted/errored turn,
    // keeping whatever tool results already answered its calls.
    let assistant_positions: Vec<usize> = (0..messages.len())
        .filter(|&i| matches!(messages[i], Message::Assistant(_)))
        .collect();
    if !assistant_positions.is_empty() && tc.draw(gs::booleans()) {
        let idx = tc.draw(gs::sampled_from(assistant_positions));
        if let Message::Assistant(assistant) = &mut messages[idx] {
            assistant.stop_reason = tc.draw(
                gs::sampled_from(vec![StopReason::Error, StopReason::Aborted])
                    .print_as_debug(),
            );
            assistant.error_message = Some(tc.draw(text(20)));
        }
    }

    // Sprinkle in a turn that aborts holding only reasoning.
    if tc.draw(gs::booleans()) {
        let thinking = ThinkingContent {
            thinking: tc.draw(text(20)),
            thinking_signature: Some(tc.draw(reasoning_item_json())),
            redacted: None,
        };
        let aborted = AssistantMessage {
            content: vec![AssistantBlock::Thinking(thinking)],
            model: "gpt-5.5".to_owned(),
            response_id: tc.draw(gs::optional(id("resp_"))),
            usage: tc.draw(usage()),
            stop_reason: StopReason::Aborted,
            error_message: Some(tc.draw(text(20))),
            timestamp: tc.draw(timestamp()),
        };
        let at = tc.draw(gs::integers::<usize>().max_value(messages.len()));
        messages.insert(at, Message::Assistant(aborted));
    }

    messages
}

// =============================================================================
// Fuzzing `tau_ai::partial_json` (crates/tau-ai/tests/partial_json.rs)
// =============================================================================

/// Applies 1-3 small, independent edits to `text`: deleting a
/// character, duplicating one that is already there next to itself, or
/// inserting one drawn from a JSON-syntax-heavy alphabet (plus
/// [`SPECIAL_CHARS`]). Meant to turn mostly-valid JSON text into text
/// that is *almost* valid JSON, for differential fuzzing against a
/// tolerant reference parser.
#[hegel::composite]
pub fn mutate_text(tc: &TestCase, text: String) -> String {
    const EDIT_ALPHABET: &str = "{}[]:,\"\\ 0123456789.eE+-truefalsn/x";
    let mut chars: Vec<char> = text.chars().collect();
    let edits = tc.draw(gs::integers::<u32>().min_value(1).max_value(3));
    for _ in 0..edits {
        let len = chars.len();
        let action = if len == 0 {
            1
        } else {
            tc.draw(gs::integers::<u8>().max_value(2))
        };
        match action {
            0 => {
                let pos = tc.draw(gs::integers::<usize>().max_value(len - 1));
                chars.remove(pos);
            }
            2 => {
                let src = tc.draw(gs::integers::<usize>().max_value(len - 1));
                let pos = tc.draw(gs::integers::<usize>().max_value(len));
                chars.insert(pos, chars[src]);
            }
            _ => {
                let pos =
                    tc.draw(gs::integers::<usize>().max_value(chars.len()));
                let c: char = tc.draw(hegel::one_of!(
                    gs::characters()
                        .categories(&[])
                        .include_characters(EDIT_ALPHABET),
                    gs::characters()
                        .categories(&[])
                        .include_characters(SPECIAL_CHARS),
                ));
                chars.insert(pos, c);
            }
        }
    }
    chars.into_iter().collect()
}

// =============================================================================
// JSON schemas for `tau_agent::schema` (crates/tau-agent/tests/schema.rs)
// =============================================================================

/// Whether `schema` is one of the nullable shapes [`draw_schema_case`]
/// produces (`{"type": "null"}` or an `anyOf` with such a branch). Used
/// only to decide, at generation time, whether an optional property may
/// be omitted from a generated value: omitting one that is already
/// nullable would make the round trip through
/// `tau_agent::schema::strip_nulls_for_optional` ambiguous (pi keeps a
/// legitimately-nullable field's `null` rather than stripping it), so
/// such properties are always given a value instead.
fn schema_case_is_nullable(schema: &Value) -> bool {
    if matches!(schema.get("type"), Some(Value::String(s)) if s == "null") {
        return true;
    }
    match schema.get("anyOf").and_then(Value::as_array) {
        Some(variants) => variants.iter().any(schema_case_is_nullable),
        None => false,
    }
}

/// Draws a value matching `schema`, which must be one of the shapes
/// [`draw_schema_case`] produces. Used to fill array items and to give
/// several properties independent values under one schema.
fn draw_value_for_schema_case(tc: &TestCase, schema: &Value) -> Value {
    match schema.get("type").and_then(Value::as_str) {
        Some("string") => match schema.get("enum").and_then(Value::as_array) {
            Some(values) => tc.draw(gs::sampled_from(values.clone())),
            None => Value::String(tc.draw(text(10))),
        },
        Some("number") => Value::Number(tc.draw(json_number())),
        Some("boolean") => Value::Bool(tc.draw(gs::booleans())),
        Some("null") => Value::Null,
        Some("array") => {
            let items_schema = schema.get("items").expect(
                "draw_schema_case always sets items on an array schema",
            );
            let len = tc.draw(gs::integers::<usize>().max_value(3));
            Value::Array(
                (0..len)
                    .map(|_| draw_value_for_schema_case(tc, items_schema))
                    .collect(),
            )
        }
        Some("object") => {
            let properties = schema
                .get("properties")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let required: std::collections::HashSet<&str> = schema
                .get("required")
                .and_then(Value::as_array)
                .map(|values| values.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let mut object = Map::new();
            for (key, property_schema) in &properties {
                let must_include = required.contains(key.as_str())
                    || schema_case_is_nullable(property_schema)
                    || tc.draw(gs::booleans());
                if must_include {
                    object.insert(
                        key.clone(),
                        draw_value_for_schema_case(tc, property_schema),
                    );
                }
            }
            Value::Object(object)
        }
        None => {
            // The only schema shape with no "type" that draw_schema_case
            // produces is a nullable `anyOf`.
            let variants = schema
                .get("anyOf")
                .and_then(Value::as_array)
                .expect("a typeless draw_schema_case shape is an anyOf");
            let idx =
                tc.draw(gs::integers::<usize>().max_value(variants.len() - 1));
            draw_value_for_schema_case(tc, &variants[idx])
        }
        Some(other) => {
            unreachable!("draw_schema_case never produces type {other:?}")
        }
    }
}

// The recursion lives in a plain function for the same reason as
// `draw_json_value`: a composite that draws itself would have an opaque
// type that contains itself.
fn draw_schema_case(tc: &TestCase, depth: u32) -> (Value, Value) {
    let kinds: u32 = if depth == 0 { 5 } else { 7 };
    match tc.draw(gs::integers::<u32>().max_value(kinds - 1)) {
        0 => {
            let value = tc.draw(text(10));
            (serde_json::json!({"type": "string"}), Value::String(value))
        }
        1 => {
            let value = tc.draw(json_number());
            (serde_json::json!({"type": "number"}), Value::Number(value))
        }
        2 => {
            let value = tc.draw(gs::booleans());
            (serde_json::json!({"type": "boolean"}), Value::Bool(value))
        }
        3 => {
            // A closed string enum.
            let variants: Vec<String> = (0..tc
                .draw(gs::integers::<usize>().min_value(1).max_value(3)))
                .map(|i| format!("v{i}"))
                .collect();
            let chosen = tc.draw(gs::sampled_from(variants.clone()));
            (
                serde_json::json!({"type": "string", "enum": variants}),
                Value::String(chosen),
            )
        }
        4 => {
            // A nullable scalar, expressed the way pi's `schemaAllowsNull`
            // recognizes: `anyOf` with a `{"type": "null"}` branch.
            let (inner_schema, inner_value) =
                match tc.draw(gs::integers::<u8>().max_value(2)) {
                    0 => (
                        serde_json::json!({"type": "string"}),
                        Value::String(tc.draw(text(8))),
                    ),
                    1 => (
                        serde_json::json!({"type": "number"}),
                        Value::Number(tc.draw(json_number())),
                    ),
                    _ => (
                        serde_json::json!({"type": "boolean"}),
                        Value::Bool(tc.draw(gs::booleans())),
                    ),
                };
            let use_null = tc.draw(gs::booleans());
            (
                serde_json::json!({"anyOf": [inner_schema, {"type": "null"}]}),
                if use_null { Value::Null } else { inner_value },
            )
        }
        5 => {
            let (item_schema, _) = draw_schema_case(tc, depth - 1);
            let len = tc.draw(gs::integers::<usize>().max_value(3));
            let values: Vec<Value> = (0..len)
                .map(|_| draw_value_for_schema_case(tc, &item_schema))
                .collect();
            (
                serde_json::json!({"type": "array", "items": item_schema}),
                Value::Array(values),
            )
        }
        _ => draw_object_schema_case(tc, depth),
    }
}

/// The object-schema case of [`draw_schema_case`], factored out so the
/// public generators can draw a root schema that is always an object:
/// `to_strict` rejects any other root type outright, so a top-level
/// [`draw_schema_case`] call (which can pick a scalar, an array, or an
/// object) is not by itself a valid *root* schema generator.
fn draw_object_schema_case(tc: &TestCase, depth: u32) -> (Value, Value) {
    let property_count = tc.draw(gs::integers::<usize>().max_value(3));
    let mut properties = Map::new();
    let mut required = Vec::new();
    let mut value = Map::new();
    for i in 0..property_count {
        let key = format!("p{i}");
        let (property_schema, property_value) =
            draw_schema_case(tc, depth.saturating_sub(1));
        // An optional property that `to_strict` cannot already see
        // as nullable gets wrapped in a synthetic `anyOf: [prop,
        // null]`. Re-running `to_strict` on that wrapper rejects it
        // if `prop` is itself an object or array (pi's own
        // `isStructuredSchema` gate on `anyOf` variants), so a
        // structured property must stay required for `to_strict` to
        // be idempotent; pi's algorithm has the same limitation.
        let is_structured = matches!(
            property_schema.get("type").and_then(Value::as_str),
            Some("object") | Some("array")
        );
        let is_required = is_structured || tc.draw(gs::booleans());
        let nullable = schema_case_is_nullable(&property_schema);
        if is_required {
            required.push(Value::String(key.clone()));
            value.insert(key.clone(), property_value);
        } else if nullable || tc.draw(gs::booleans()) {
            // Nullable-and-optional properties are always given a
            // value (see `schema_case_is_nullable`); other optional
            // properties are sometimes omitted.
            value.insert(key.clone(), property_value);
        }
        properties.insert(key, property_schema);
    }
    let additional_properties_false = tc.draw(gs::booleans());
    let mut schema = Map::new();
    schema.insert("type".to_owned(), Value::String("object".to_owned()));
    schema.insert("properties".to_owned(), Value::Object(properties));
    schema.insert("required".to_owned(), Value::Array(required));
    if additional_properties_false {
        schema.insert("additionalProperties".to_owned(), Value::Bool(false));
    }
    (Value::Object(schema), Value::Object(value))
}

/// A JSON schema that `tau_agent::schema::to_strict` accepts, by
/// construction: nested objects with required and optional properties,
/// arrays, a closed string enum, and nullable fields and `anyOf` unions
/// of the shapes pi's strict rewrite supports. `depth` bounds how deep
/// nested objects and arrays may go. The root is always an object, since
/// `to_strict` requires that.
///
/// pi's strict rewrite never supports `$ref`/`$defs`/`definitions` (they
/// are in `UNSUPPORTED_STRICT_SCHEMA_KEYS`), so this generator never
/// produces them; see [`unsupported_schema_with_reason`] for schemas built from
/// exactly those keywords.
#[hegel::composite]
pub fn strict_schema(tc: &TestCase, depth: u32) -> Value {
    draw_object_schema_case(tc, depth).0
}

/// A [`strict_schema`] paired with a value that validates against it.
/// Optional properties without their own nullable shape are sometimes
/// omitted from the value (built by construction, not filtered after
/// the fact); a nullable-and-optional property is always given a value,
/// so the pairing stays useful for the round trip through
/// `tau_agent::schema::strip_nulls_for_optional` (see
/// `schema_case_is_nullable`).
#[hegel::composite]
pub fn strict_schema_with_value(tc: &TestCase, depth: u32) -> (Value, Value) {
    draw_object_schema_case(tc, depth)
}

/// Schema keywords pi's strict rewrite does not support
/// (`constrained-sampling.ts` `UNSUPPORTED_STRICT_SCHEMA_KEYS`), copied
/// from pi apart from tau-agent's own list, so a key tau-agent forgets
/// fails its schema tests.
const UNSUPPORTED_KEYS: &[&str] = &[
    "$ref",
    "$defs",
    "definitions",
    "allOf",
    "oneOf",
    "patternProperties",
    "dependentSchemas",
    "dependencies",
    "unevaluatedProperties",
    "propertyNames",
    "contains",
    "prefixItems",
    "not",
    "if",
    "then",
    "else",
];

/// A JSON schema that `tau_agent::schema::to_strict` must reject, and the
/// reason it must give: a [`strict_schema`] with exactly one bad node put
/// at a drawn position (the root, a property, array items or an `anyOf`
/// variant), so detection is exercised at depth. The bad node is one of
/// pi's documented failure cases (`constrained-sampling.ts` /
/// `constrained-sampling.test.ts`) minus object unions, which tau-agent
/// accepts, plus a recursive `$ref`: an unsupported keyword (`$defs` and
/// `definitions` only below the root, since root ones are inlined), a
/// tuple `items`, a schema-valued or `true` `additionalProperties`, a
/// recursive `$ref`, an empty `anyOf`, `properties` that are not a map or
/// sit without `type: "object"`, `required` naming an unknown property or
/// holding a non-string, a boolean (`true`/`false`) schema node, or a
/// non-object root.
#[hegel::composite]
pub fn unsupported_schema_with_reason(tc: &TestCase) -> (Value, String) {
    let mut schema = draw_object_schema_case(tc, 2).0;
    let mut slots = Vec::new();
    subschema_pointers(&schema, String::new(), &mut slots);
    let at: String = tc.draw(gs::sampled_from(slots));
    let at_root = at.is_empty();
    let mut kinds = vec![
        "keyword",
        "tuple items",
        "open additionalProperties",
        "empty anyOf",
        "properties without object",
        "properties not a map",
        "unknown required",
        "non-string required",
    ];
    if at_root {
        kinds.push("non-object root");
    } else {
        kinds.extend(["recursive $ref", "boolean"]);
    }
    let (bad, reason) = match tc.draw(gs::sampled_from(kinds)) {
        "keyword" => {
            let keys: Vec<&str> = UNSUPPORTED_KEYS
                .iter()
                .copied()
                .filter(|key| !at_root || !matches!(*key, "$defs" | "definitions"))
                .collect();
            let key = tc.draw(gs::sampled_from(keys));
            let mut node = Map::new();
            node.insert("type".to_owned(), Value::String("object".to_owned()));
            node.insert(key.to_owned(), serde_json::json!({}));
            (Value::Object(node), format!("{key} schemas are unsupported"))
        }
        "tuple items" => (
            serde_json::json!({"type": "array", "items": [{"type": "string"}, {"type": "number"}]}),
            "tuple schemas are unsupported".to_owned(),
        ),
        "open additionalProperties" => (
            serde_json::json!({
                "type": "object",
                "additionalProperties": tc.draw(gs::sampled_from(vec![
                    Value::Bool(true),
                    serde_json::json!({"type": "string"}),
                ])),
            }),
            "schema-valued or true additionalProperties is unsupported"
                .to_owned(),
        ),
        "empty anyOf" => (
            serde_json::json!({"anyOf": []}),
            "anyOf must contain at least one schema".to_owned(),
        ),
        "properties without object" => (
            serde_json::json!({"type": "string", "properties": {"a": {"type": "string"}}}),
            "properties require type object".to_owned(),
        ),
        "properties not a map" => (
            serde_json::json!({"type": "object", "properties": []}),
            "object properties must be a schema map".to_owned(),
        ),
        "unknown required" => (
            serde_json::json!({"type": "object", "required": ["missing"], "properties": {}}),
            "required contains an unknown property".to_owned(),
        ),
        "non-string required" => (
            serde_json::json!({"type": "object", "required": [1], "properties": {}}),
            "object required must be a string array".to_owned(),
        ),
        "non-object root" => (
            tc.draw(gs::sampled_from(vec![
                serde_json::json!({"type": "string"}),
                serde_json::json!({"type": "array", "items": {"type": "string"}}),
                Value::Bool(true),
                Value::Bool(false),
            ])),
            "root schema must have type object".to_owned(),
        ),
        "recursive $ref" => {
            schema.as_object_mut().expect("the root is an object").insert(
                "$defs".to_owned(),
                serde_json::json!({"Node": {
                    "type": "object",
                    "properties": {"next": {"$ref": "#/$defs/Node"}},
                }}),
            );
            (
                serde_json::json!({"$ref": "#/$defs/Node"}),
                "recursive $ref schemas are unsupported".to_owned(),
            )
        }
        _ => (
            Value::Bool(tc.draw(gs::booleans())),
            "boolean schemas are unsupported".to_owned(),
        ),
    };
    *schema
        .pointer_mut(&at)
        .expect("a pointer collected from the schema") = bad;
    (schema, reason)
}

/// JSON pointers to `schema` and every schema node below it: property
/// schemas, `items` and `anyOf` variants. Property names are drawn by
/// [`draw_object_schema_case`] as `p<n>`, so they need no escaping.
fn subschema_pointers(schema: &Value, at: String, out: &mut Vec<String>) {
    if let Some(properties) =
        schema.get("properties").and_then(Value::as_object)
    {
        for (key, property) in properties {
            subschema_pointers(property, format!("{at}/properties/{key}"), out);
        }
    }
    if let Some(items) = schema.get("items") {
        subschema_pointers(items, format!("{at}/items"), out);
    }
    if let Some(variants) = schema.get("anyOf").and_then(Value::as_array) {
        for (i, variant) in variants.iter().enumerate() {
            subschema_pointers(variant, format!("{at}/anyOf/{i}"), out);
        }
    }
    out.push(at);
}

// =============================================================================
// Tool-argument schemas for `tau_agent::validation`
// =============================================================================

// The recursion lives in a plain function for the same reason as
// `draw_json_value`: a composite that draws itself would have an opaque
// type that contains itself.
fn draw_arg_schema(tc: &TestCase, depth: u32) -> Value {
    let kinds: u32 = if depth == 0 { 5 } else { 8 };
    match tc.draw(gs::integers::<u32>().max_value(kinds - 1)) {
        0 => serde_json::json!({"type": "string"}),
        1 => serde_json::json!({"type": "number"}),
        2 => serde_json::json!({"type": "integer"}),
        3 => serde_json::json!({"type": "boolean"}),
        4 => serde_json::json!({"type": "null"}),
        5 => {
            let items = draw_arg_schema(tc, depth - 1);
            serde_json::json!({"type": "array", "items": items})
        }
        6 => draw_arg_object_schema(tc, depth - 1),
        _ => {
            // `anyOf` of two independently drawn schemas, e.g. a nullable
            // union when one arm happens to be `{"type": "null"}`.
            let a = draw_arg_schema(tc, depth - 1);
            let b = draw_arg_schema(tc, depth - 1);
            serde_json::json!({"anyOf": [a, b]})
        }
    }
}

fn draw_arg_object_schema(tc: &TestCase, depth: u32) -> Value {
    let count = tc.draw(gs::integers::<usize>().min_value(1).max_value(3));
    let mut properties = Map::new();
    let mut required = Vec::new();
    for i in 0..count {
        let name = format!("p{i}");
        let property_schema = draw_arg_schema(tc, depth);
        if tc.draw(gs::booleans()) {
            required.push(Value::String(name.clone()));
        }
        properties.insert(name, property_schema);
    }
    serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required,
    })
}

/// A JSON Schema built only from the shapes `tau_agent::validation`'s
/// coercion pass handles: `string`/`number`/`integer`/`boolean`/`null`
/// leaves, `array` (a single `items` schema, never a tuple), `object`
/// (required and optional properties, no `additionalProperties`) and
/// `anyOf`. `depth` bounds how deep arrays, objects and unions may
/// nest.
#[hegel::composite]
pub fn arg_schema(tc: &TestCase, depth: u32) -> Value {
    draw_arg_schema(tc, depth)
}

fn draw_arg_value(tc: &TestCase, schema: &Value) -> Value {
    if let Some(any_of) = schema.get("anyOf").and_then(Value::as_array) {
        let index =
            tc.draw(gs::integers::<usize>().max_value(any_of.len() - 1));
        return draw_arg_value(tc, &any_of[index]);
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("string") => Value::String(tc.draw(text(8))),
        Some("number") => Value::Number(tc.draw(json_number())),
        Some("integer") => Value::Number(Number::from(
            tc.draw(
                gs::integers::<i64>()
                    .min_value(-1_000_000)
                    .max_value(1_000_000),
            ),
        )),
        Some("boolean") => Value::Bool(tc.draw(gs::booleans())),
        Some("null") => Value::Null,
        Some("array") => {
            let items_schema = schema
                .get("items")
                .cloned()
                .unwrap_or(Value::Object(Map::new()));
            let len = tc.draw(gs::integers::<usize>().max_value(3));
            Value::Array(
                (0..len)
                    .map(|_| draw_arg_value(tc, &items_schema))
                    .collect(),
            )
        }
        Some("object") => {
            let properties = schema
                .get("properties")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let required: std::collections::HashSet<&str> = schema
                .get("required")
                .and_then(Value::as_array)
                .map(|values| values.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let mut object = Map::new();
            for (key, property_schema) in &properties {
                let include =
                    required.contains(key.as_str()) || tc.draw(gs::booleans());
                if include {
                    object.insert(
                        key.clone(),
                        draw_arg_value(tc, property_schema),
                    );
                }
            }
            Value::Object(object)
        }
        // draw_arg_schema never produces a schema with no recognized
        // `type` and no `anyOf` (handled above).
        _ => Value::Null,
    }
}

/// A value that validates against `schema` (one of [`arg_schema`]'s
/// shapes), built by construction: every required property gets a
/// value, an optional one sometimes does, and `anyOf` picks one branch
/// and satisfies it. Pairs with [`arg_schema`] for coercion properties
/// that need a value already valid under the schema they drew.
#[hegel::composite]
pub fn arg_value_for_schema(tc: &TestCase, schema: Value) -> Value {
    draw_arg_value(tc, &schema)
}
