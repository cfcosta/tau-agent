//! Transcript types.
//!
//! The serde shape mirrors pi's JSON (`packages/ai/src/types.ts`):
//! messages are tagged by `role`, content blocks by `type`, and field
//! names are camelCase. Only the OpenAI Responses subset is kept.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

/// The `api` value of every assistant message tau-agent produces.
pub const API: &str = "openai-responses";

/// The `provider` value of every assistant message tau-agent produces.
pub const PROVIDER: &str = "openai";

/// Milliseconds since the Unix epoch.
pub type Timestamp = u64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "camelCase")]
pub enum Message {
    User(UserMessage),
    Assistant(AssistantMessage),
    ToolResult(ToolResultMessage),
}

impl Message {
    /// The value of the `role` tag.
    pub fn role(&self) -> &'static str {
        match self {
            Self::User(_) => "user",
            Self::Assistant(_) => "assistant",
            Self::ToolResult(_) => "toolResult",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserMessage {
    pub content: UserContent,
    pub timestamp: Timestamp,
}

/// A user message's content: a plain string or a list of blocks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UserContent {
    Text(String),
    Blocks(Vec<InputBlock>),
}

impl UserContent {
    /// The content's text, one block a line; images leave nothing.
    pub fn text(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Blocks(blocks) => text_of(blocks),
        }
    }
}

/// A block a user message or a tool result can hold.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum InputBlock {
    Text(TextContent),
    Image(ImageContent),
}

impl InputBlock {
    /// The block's text, if it is text.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(&text.text),
            Self::Image(_) => None,
        }
    }
}

/// The text of `blocks`, one block a line; images leave nothing.
pub fn text_of(blocks: &[InputBlock]) -> String {
    blocks
        .iter()
        .filter_map(InputBlock::as_text)
        .collect::<Vec<_>>()
        .join("\n")
}

/// A block an assistant message can hold.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AssistantBlock {
    Text(TextContent),
    Thinking(ThinkingContent),
    ToolCall(ToolCall),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextContent {
    pub text: String,
    /// Responses message metadata, replayed on full resends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_signature: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingContent {
    pub thinking: String,
    /// The serialized reasoning item, including `encrypted_content`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redacted: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageContent {
    /// Base64-encoded image bytes.
    pub data: String,
    pub mime_type: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// `call_id|item_id`, as pi writes it for the Responses API.
    pub id: String,
    pub name: String,
    pub arguments: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantMessage {
    pub content: Vec<AssistantBlock>,
    pub api: String,
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,
    pub usage: Usage,
    pub stop_reason: StopReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    pub timestamp: Timestamp,
}

impl AssistantMessage {
    /// What the model said, one text block a line.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                AssistantBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The tool calls the model made, in order.
    pub fn tool_calls(&self) -> impl Iterator<Item = &ToolCall> {
        self.content.iter().filter_map(|block| match block {
            AssistantBlock::ToolCall(call) => Some(call),
            _ => None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultMessage {
    pub tool_call_id: String,
    pub tool_name: String,
    pub content: Vec<InputBlock>,
    /// Tool-specific details. `Some(Value::Null)` is an explicit `null`,
    /// which pi keeps apart from an absent field.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub details: Option<Value>,
    pub is_error: bool,
    pub timestamp: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StopReason {
    Stop,
    Length,
    ToolUse,
    Error,
    Aborted,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    /// Reasoning tokens, a subset of `output`, when the server reports them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<u64>,
    pub total_tokens: u64,
    pub cost: UsageCost,
}

/// Adds another usage, field by field, cost included. `reasoning`, a
/// subset of `output` some responses report, is left alone.
impl std::ops::AddAssign<&Usage> for Usage {
    fn add_assign(&mut self, other: &Usage) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
        self.total_tokens += other.total_tokens;
        self.cost.input += other.cost.input;
        self.cost.output += other.cost.output;
        self.cost.cache_read += other.cost.cache_read;
        self.cost.cache_write += other.cost.cache_write;
        self.cost.total += other.cost.total;
    }
}

/// Cost in US dollars, split the way pi splits it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageCost {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub total: f64,
}

/// Deserializes a field that is present, even when it is `null`, as
/// `Some`. Absent fields fall back to `None` through `#[serde(default)]`.
fn present<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

#[cfg(test)]
mod usage_tests {
    use super::*;

    /// Usage adds up field by field, cost included.
    #[test]
    fn usage_adds_field_by_field() {
        let part = |n: u64| Usage {
            input: n,
            output: n + 1,
            cache_read: n + 2,
            cache_write: n + 3,
            reasoning: None,
            total_tokens: n + 4,
            cost: UsageCost {
                input: n as f64,
                output: n as f64 + 0.5,
                cache_read: n as f64 + 0.25,
                cache_write: n as f64 + 0.125,
                total: n as f64 + 1.0,
            },
        };
        let mut total = part(1);
        total += &part(10);
        assert_eq!(
            total,
            Usage {
                input: 11,
                output: 13,
                cache_read: 15,
                cache_write: 17,
                reasoning: None,
                total_tokens: 19,
                cost: UsageCost {
                    input: 11.0,
                    output: 12.0,
                    cache_read: 11.5,
                    cache_write: 11.25,
                    total: 13.0,
                },
            }
        );
    }
}
