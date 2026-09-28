//! The size of a run's context, as the loop sees it
//! (`docs/reference/compaction.md`, "Token estimate"), and whether a
//! failed response failed because its context was too long.
//!
//! The loop uses these for [`ContextView`](crate::plugin::ContextView)
//! and to offer an overflow to context plugins; plugins use them to
//! decide when to rewrite.

use tau_ai::message::{
    AssistantBlock,
    AssistantMessage,
    InputBlock,
    Message,
    StopReason,
    Usage,
    UserContent,
};

/// Characters an image contributes to the `chars / 4` estimate (pi's
/// `ESTIMATED_IMAGE_CHARS`).
const ESTIMATED_IMAGE_CHARS: u64 = 4800;

fn text_chars(text: &str) -> u64 {
    text.chars().count() as u64
}

fn input_block_chars(block: &InputBlock) -> u64 {
    match block {
        InputBlock::Text(content) => text_chars(&content.text),
        InputBlock::Image(_) => ESTIMATED_IMAGE_CHARS,
    }
}

fn user_content_chars(content: &UserContent) -> u64 {
    match content {
        UserContent::Text(text) => text_chars(text),
        UserContent::Blocks(blocks) => {
            blocks.iter().map(input_block_chars).sum()
        }
    }
}

fn assistant_block_chars(block: &AssistantBlock) -> u64 {
    match block {
        AssistantBlock::Text(content) => text_chars(&content.text),
        AssistantBlock::Thinking(content) => text_chars(&content.thinking),
        AssistantBlock::ToolCall(call) => {
            let arguments_chars = serde_json::to_string(&call.arguments)
                .map(|json| text_chars(&json))
                .unwrap_or(0);
            text_chars(&call.name) + arguments_chars
        }
    }
}

/// Estimated token count of one message, using pi's `chars / 4`
/// heuristic (`docs/reference/compaction.md`, "Token estimate"). This
/// deliberately overestimates: it is meant to trigger compaction a
/// little early, not to be an exact tokenizer.
pub fn estimate_message_tokens(message: &Message) -> u64 {
    let chars = match message {
        Message::User(message) => user_content_chars(&message.content),
        Message::Assistant(message) => {
            message.content.iter().map(assistant_block_chars).sum()
        }
        Message::ToolResult(message) => {
            message.content.iter().map(input_block_chars).sum()
        }
    };
    chars.div_ceil(4)
}

/// Whether `usage` was actually reported (as opposed to an all-zero
/// placeholder), by pi's `calculateContextTokens(usage) > 0`: any of the
/// reported-total or the four raw counters is nonzero.
fn usage_is_reported(usage: &Usage) -> bool {
    usage.total_tokens > 0
        || usage.input > 0
        || usage.output > 0
        || usage.cache_read > 0
        || usage.cache_write > 0
}

/// The context size a response reports (pi's `calculateContextTokens`):
/// the server's total, or the sum of the four counters without one.
fn context_tokens(usage: &Usage) -> u64 {
    if usage.total_tokens > 0 {
        usage.total_tokens
    } else {
        usage.input + usage.output + usage.cache_read + usage.cache_write
    }
}

/// The anchor token count and index of the last assistant message whose
/// usage counts (`docs/reference/compaction.md`, "Token estimate", step
/// 1): not aborted, not errored, and with some usage actually reported.
fn last_usage_anchor(messages: &[Message]) -> Option<(u64, usize)> {
    messages
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, message)| {
            let Message::Assistant(assistant) = message else {
                return None;
            };
            if matches!(
                assistant.stop_reason,
                StopReason::Error | StopReason::Aborted
            ) {
                return None;
            }
            if !usage_is_reported(&assistant.usage) {
                return None;
            }
            Some((context_tokens(&assistant.usage), index))
        })
}

/// Estimated total context tokens for `messages`
/// (`docs/reference/compaction.md`, "Token estimate"): the last
/// successful assistant message's usage, plus `chars / 4` for every
/// message after it, or `chars / 4` over everything when no message has
/// reported usage yet.
pub fn estimate_context_tokens(messages: &[Message]) -> u64 {
    match last_usage_anchor(messages) {
        Some((anchor_tokens, index)) => {
            let trailing: u64 = messages[index + 1..]
                .iter()
                .map(estimate_message_tokens)
                .sum();
            anchor_tokens + trailing
        }
        None => messages.iter().map(estimate_message_tokens).sum(),
    }
}

/// Whether a failed response failed because its input did not fit the
/// model's context window (OpenAI's wordings, from pi's `overflow.ts`).
pub fn is_context_overflow(message: &AssistantMessage) -> bool {
    if message.stop_reason != StopReason::Error {
        return false;
    }
    let Some(error) = &message.error_message else {
        return false;
    };
    let error = error.to_lowercase();
    [
        "context_length_exceeded",
        "exceeds the context window",
        "maximum context length",
    ]
    .iter()
    .any(|pattern| error.contains(pattern))
}
