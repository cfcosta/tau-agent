//! Rendering messages as event streams, for round-trip properties.

use hegel::{TestCase, generators as gs};
use tau_ai::{
    event::{AssistantEvent, DoneReason, ErrorReason},
    message::{AssistantBlock, AssistantMessage, StopReason},
};

/// Splits `text` into chunks at char boundaries drawn by the test case.
/// Chunks may be empty; concatenating them gives `text` back.
pub fn draw_split(tc: &TestCase, text: &str) -> Vec<String> {
    let boundaries: Vec<usize> = text
        .char_indices()
        .map(|(i, _)| i)
        .skip(1)
        .chain(std::iter::once(text.len()))
        .collect();
    let mut chunks = Vec::new();
    let mut start = 0;
    for &end in &boundaries {
        if end == text.len() || tc.draw(gs::booleans()) {
            chunks.push(text[start..end].to_owned());
            start = end;
        }
    }
    if chunks.is_empty() {
        chunks.push(String::new());
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
