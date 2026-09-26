//! Shared Hegel generators.
//!
//! Every generator builds valid values directly and stays small, so a
//! shrunk counterexample is short enough to read.

use std::time::Duration;

use hegel::{
    TestCase,
    generators::{self as gs, Generator},
};
use serde_json::{Map, Number, Value};
use tau_ai::message::{
    API,
    AssistantBlock,
    AssistantMessage,
    ImageContent,
    InputBlock,
    Message,
    PROVIDER,
    StopReason,
    TextContent,
    ThinkingContent,
    ToolCall,
    ToolResultMessage,
    Usage,
    UsageCost,
    UserContent,
    UserMessage,
};

/// Characters the code handles specially: line endings, a BOM, the
/// Unicode line separators, NNBSP, smart quotes, a combining mark and
/// multi-byte characters.
pub const SPECIAL_CHARS: &str = "ab \n\r\t\u{feff}\u{2028}\u{2029}\u{202f}\u{2018}\u{2019}\u{201c}\u{201d}\u{2014}e\u{301}é日🦀";

/// Short text: either arbitrary Unicode or text built from
/// [`SPECIAL_CHARS`].
#[hegel::composite]
pub fn text(tc: TestCase, max_size: usize) -> String {
    tc.draw(hegel::one_of!(
        gs::text().max_size(max_size),
        gs::text().alphabet(SPECIAL_CHARS).max_size(max_size),
    ))
}

/// A short identifier, like the ids OpenAI assigns.
#[hegel::composite]
pub fn id(tc: TestCase, prefix: &'static str) -> String {
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
pub fn json_number(tc: TestCase) -> Number {
    if tc.draw(gs::booleans()) {
        Number::from(tc.draw(gs::integers::<i64>()))
    } else {
        let f: f64 =
            tc.draw(gs::floats::<f64>().allow_nan(false).allow_infinity(false));
        Number::from_f64(f).expect("finite floats are valid JSON numbers")
    }
}

/// Any JSON value, nested at most `depth` levels.
#[hegel::composite]
pub fn json_value(tc: TestCase, depth: u32) -> Value {
    draw_json_value(&tc, depth)
}

/// A JSON object, nested at most `depth` levels below its fields.
#[hegel::composite]
pub fn json_object(tc: TestCase, depth: u32) -> Map<String, Value> {
    draw_json_object(&tc, depth)
}

// The recursion lives in plain functions: a composite that draws itself
// would have an opaque type that contains itself.
fn draw_json_value(tc: &TestCase, depth: u32) -> Value {
    let kinds = if depth == 0 { 4 } else { 6 };
    match tc.draw(gs::integers::<u32>().max_value(kinds - 1)) {
        0 => Value::Null,
        1 => Value::Bool(tc.draw(gs::booleans())),
        2 => Value::Number(tc.draw(json_number())),
        3 => Value::String(tc.draw(text(16))),
        4 => {
            let len = tc.draw(gs::integers::<usize>().max_value(4));
            Value::Array(
                (0..len).map(|_| draw_json_value(tc, depth - 1)).collect(),
            )
        }
        _ => Value::Object(draw_json_object(tc, depth - 1)),
    }
}

fn draw_json_object(tc: &TestCase, depth: u32) -> Map<String, Value> {
    let len = tc.draw(gs::integers::<usize>().max_value(4));
    (0..len)
        .map(|_| (tc.draw(text(8)), draw_json_value(tc, depth)))
        .collect()
}

#[hegel::composite]
pub fn usage(tc: TestCase) -> Usage {
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

#[hegel::composite]
pub fn stop_reason(tc: TestCase) -> StopReason {
    tc.draw(gs::sampled_from(vec![
        StopReason::Stop,
        StopReason::Length,
        StopReason::ToolUse,
        StopReason::Error,
        StopReason::Aborted,
    ]))
}

#[hegel::composite]
pub fn text_content(tc: TestCase) -> TextContent {
    TextContent {
        text: tc.draw(text(40)),
        text_signature: tc.draw(gs::optional(id("msg_"))),
    }
}

#[hegel::composite]
pub fn thinking_content(tc: TestCase) -> ThinkingContent {
    ThinkingContent {
        thinking: tc.draw(text(40)),
        thinking_signature: tc.draw(gs::optional(id("rs_"))),
        redacted: tc.draw(gs::optional(gs::booleans())),
    }
}

#[hegel::composite]
pub fn image_content(tc: TestCase) -> ImageContent {
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
#[hegel::composite]
pub fn tool_call(tc: TestCase) -> ToolCall {
    let call_id = tc.draw(id("call_"));
    let item_id = tc.draw(id("fc_"));
    ToolCall {
        id: format!("{call_id}|{item_id}"),
        name: tc.draw(id("tool_")),
        arguments: tc.draw(json_object(2)),
    }
}

#[hegel::composite]
pub fn input_block(tc: TestCase) -> InputBlock {
    if tc.draw(gs::booleans()) {
        InputBlock::Text(tc.draw(text_content()))
    } else {
        InputBlock::Image(tc.draw(image_content()))
    }
}

#[hegel::composite]
pub fn assistant_block(tc: TestCase) -> AssistantBlock {
    match tc.draw(gs::integers::<u8>().max_value(2)) {
        0 => AssistantBlock::Text(tc.draw(text_content())),
        1 => AssistantBlock::Thinking(tc.draw(thinking_content())),
        _ => AssistantBlock::ToolCall(tc.draw(tool_call())),
    }
}

#[hegel::composite]
pub fn user_message(tc: TestCase) -> UserMessage {
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

#[hegel::composite]
pub fn assistant_message(tc: TestCase) -> AssistantMessage {
    let stop_reason = tc.draw(stop_reason());
    let failed = matches!(stop_reason, StopReason::Error | StopReason::Aborted);
    AssistantMessage {
        content: tc.draw(gs::vecs(assistant_block()).max_size(4)),
        api: API.to_owned(),
        provider: PROVIDER.to_owned(),
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

#[hegel::composite]
pub fn tool_result_message(tc: TestCase) -> ToolResultMessage {
    ToolResultMessage {
        tool_call_id: tc.draw(id("call_")),
        tool_name: tc.draw(id("tool_")),
        content: tc.draw(gs::vecs(input_block()).max_size(3)),
        details: tc.draw(gs::optional(json_value(2))),
        is_error: tc.draw(gs::booleans()),
        timestamp: tc.draw(timestamp()),
    }
}

#[hegel::composite]
pub fn message(tc: TestCase) -> Message {
    match tc.draw(gs::integers::<u8>().max_value(2)) {
        0 => Message::User(tc.draw(user_message())),
        1 => Message::Assistant(tc.draw(assistant_message())),
        _ => Message::ToolResult(tc.draw(tool_result_message())),
    }
}

/// Milliseconds since the Unix epoch, within the next few centuries.
pub fn timestamp() -> impl Generator<u64> {
    gs::integers::<u64>().max_value(10_000_000_000_000)
}

/// A [`tau_ai::retry::RetryPolicy`] with small, valid bounds: `base` in
/// `1ms..=10s`, `max_delay >= base` (also within a few seconds of it, so
/// shrunk failures stay short), and `max_attempts` in `0..=10`.
#[hegel::composite]
pub fn retry_policy(tc: TestCase) -> tau_ai::retry::RetryPolicy {
    let base_ms = tc.draw(gs::integers::<u64>().min_value(1).max_value(10_000));
    let extra_ms =
        tc.draw(gs::integers::<u64>().min_value(0).max_value(10_000));
    tau_ai::retry::RetryPolicy {
        max_attempts: tc.draw(gs::integers::<u32>().max_value(10)),
        base: Duration::from_millis(base_ms),
        max_delay: Duration::from_millis(base_ms + extra_ms),
    }
}

pub mod lane;

/// A [`Usage`] whose token counts are each drawn independently up to
/// `max_tokens`, with `cost` left at its default (callers that exercise
/// `tau_ai::cost` compute it themselves). Useful for properties that
/// need to keep a request's total input tokens
/// (`input + cache_read + cache_write`) under a known bound, such as a
/// model's pricing-tier threshold.
#[hegel::composite]
pub fn usage_with_max_tokens(tc: TestCase, max_tokens: u64) -> Usage {
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
