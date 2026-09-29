//! Rendering messages as event streams, for round-trip properties.

use hegel::{TestCase, generators as gs};
use tau_ai::{
    event::{AssistantEvent, DoneReason, ErrorReason},
    message::{AssistantBlock, AssistantMessage, StopReason},
};

use crate::generators;

/// Splits `text` into chunks at char boundaries drawn by the test case.
/// Chunks may be empty (a server may send an empty delta); concatenating
/// them gives `text` back. Shrinks toward one chunk.
pub fn draw_split(tc: &TestCase, text: &str) -> Vec<String> {
    let cuts: Vec<usize> =
        tc.draw(gs::subsequences(generators::inner_boundaries(text)));
    let mut chunks = generators::split_at_cuts(text, &cuts);
    if tc.draw(gs::weighted_booleans(0.1)) {
        let at = tc.draw(gs::integers::<usize>().max_value(chunks.len()));
        chunks.insert(at, String::new());
    }
    chunks
}

/// Renders `message` as the event stream a server would produce for it,
/// with every block closed and deltas split at drawn boundaries.
///
/// The stop reason picks the terminal event; the response id is placed on
/// `Start` or on `Done`, as drawn.
pub fn draw_stream(
    tc: &TestCase,
    message: &AssistantMessage,
) -> Vec<AssistantEvent> {
    let id_at_start = message.response_id.is_some()
        && (!matches!(
            message.stop_reason,
            StopReason::Stop | StopReason::Length | StopReason::ToolUse
        ) || tc.draw(gs::booleans()));
    let mut events = vec![AssistantEvent::Start {
        model: message.model.clone(),
        response_id: if id_at_start {
            message.response_id.clone()
        } else {
            None
        },
        timestamp: message.timestamp,
    }];
    for (index, block) in message.content.iter().enumerate() {
        match block {
            AssistantBlock::Text(content) => {
                events.push(AssistantEvent::TextStart { index });
                for delta in draw_split(tc, &content.text) {
                    events.push(AssistantEvent::TextDelta { index, delta });
                }
                events.push(AssistantEvent::TextEnd {
                    index,
                    content: content.clone(),
                });
            }
            AssistantBlock::Thinking(content) => {
                events.push(AssistantEvent::ThinkingStart { index });
                for delta in draw_split(tc, &content.thinking) {
                    events.push(AssistantEvent::ThinkingDelta { index, delta });
                }
                events.push(AssistantEvent::ThinkingEnd {
                    index,
                    content: content.clone(),
                });
            }
            AssistantBlock::ToolCall(call) => {
                events.push(AssistantEvent::ToolCallStart {
                    index,
                    id: call.id.clone(),
                    name: call.name.clone(),
                });
                let json = serde_json::to_string(&call.arguments)
                    .expect("a JSON map always serializes");
                for delta in draw_split(tc, &json) {
                    events.push(AssistantEvent::ToolCallDelta { index, delta });
                }
                events.push(AssistantEvent::ToolCallEnd {
                    index,
                    tool_call: call.clone(),
                });
            }
        }
    }
    let usage = message.usage.clone();
    events.push(match message.stop_reason {
        StopReason::Error | StopReason::Aborted => AssistantEvent::Error {
            reason: if message.stop_reason == StopReason::Error {
                ErrorReason::Error
            } else {
                ErrorReason::Aborted
            },
            message: message.error_message.clone().unwrap_or_default(),
            class: tau_ai::retry::Class::Fatal,
            usage,
        },
        reason => AssistantEvent::Done {
            reason: match reason {
                StopReason::Length => DoneReason::Length,
                StopReason::ToolUse => DoneReason::ToolUse,
                _ => DoneReason::Stop,
            },
            usage,
            response_id: if id_at_start {
                None
            } else {
                message.response_id.clone()
            },
        },
    });
    events
}
