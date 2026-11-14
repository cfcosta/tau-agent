//! Transcript types.
//!
//! The serde shape mirrors pi's JSON (`packages/ai/src/types.ts`):
//! messages are tagged by `role`, content blocks by `type`, and field
//! names are camelCase. Only the OpenAI Responses subset is kept.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

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

    /// A usage, its token counts bounded so sums fit, its costs whole
    /// multiples of 1/1024 so sums are exact.
    fn usage(tc: &hegel::TestCase) -> Usage {
        use hegel::generators::{self as gs, Generator as _};
        let tokens = || gs::integers::<u64>().max_value(1 << 40);
        let usd = || {
            gs::integers::<u32>()
                .max_value(1 << 20)
                .map(|n| f64::from(n) / 1024.0)
        };
        Usage {
            input: tc.draw(tokens()),
            output: tc.draw(tokens()),
            cache_read: tc.draw(tokens()),
            cache_write: tc.draw(tokens()),
            reasoning: tc.draw(gs::optional(tokens())),
            total_tokens: tc.draw(tokens()),
            cost: UsageCost {
                input: tc.draw(usd()),
                output: tc.draw(usd()),
                cache_read: tc.draw(usd()),
                cache_write: tc.draw(usd()),
                total: tc.draw(usd()),
            },
        }
    }

    /// Adding a usage adds each count and each cost, field by field;
    /// `reasoning` is the first usage's own, not a sum. Either order
    /// gives the same counts and costs.
    #[hegel::test(test_cases = 200)]
    fn usage_adds_field_by_field(tc: hegel::TestCase) {
        let (a, b) = (usage(&tc), usage(&tc));
        let mut sum = a.clone();
        sum += &b;
        assert_eq!(
            sum,
            Usage {
                input: a.input + b.input,
                output: a.output + b.output,
                cache_read: a.cache_read + b.cache_read,
                cache_write: a.cache_write + b.cache_write,
                reasoning: a.reasoning,
                total_tokens: a.total_tokens + b.total_tokens,
                cost: UsageCost {
                    input: a.cost.input + b.cost.input,
                    output: a.cost.output + b.cost.output,
                    cache_read: a.cost.cache_read + b.cost.cache_read,
                    cache_write: a.cost.cache_write + b.cost.cache_write,
                    total: a.cost.total + b.cost.total,
                },
            }
        );
        let mut other_way = b.clone();
        other_way += &a;
        other_way.reasoning = a.reasoning;
        assert_eq!(other_way, sum);
    }
}
