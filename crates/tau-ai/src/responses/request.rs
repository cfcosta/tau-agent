//! The `response.create` body for one turn.
//!
//! Ports the OpenAI subset of pi's `buildParams`
//! (`packages/ai/src/api/openai-responses.ts:284`) to the WebSocket mode
//! described in `docs/reference/openai-websocket.md`:
//!
//! - `type` is `response.create`, and `stream` and `background` are never
//!   sent: the WebSocket mode forbids them.
//! - `store` is always `false`.
//! - A reasoning model always gets `include: ["reasoning.encrypted_content"]`,
//!   so a full resend can replay its reasoning. pi only asks for it when a
//!   reasoning effort is set; without it, a full resend after a lost
//!   continuation would drop the reasoning.
//! - `max_output_tokens` is at least 16, which OpenAI requires (pi #6265).
//! - `prompt_cache_key` is cut to 64 characters, OpenAI's limit.
//!
//! Every field except `input` depends only on [`Settings`], so two turns
//! of one run differ only in `input`. The delta rule depends on that.

use std::fmt;

use serde_json::{Map, Value, json};

use crate::ws::proto::continuation::Body;

/// OpenAI rejects `max_output_tokens` below this (pi #6265).
pub const MIN_OUTPUT_TOKENS: u64 = 16;

/// OpenAI's maximum `prompt_cache_key` length, in characters.
pub const PROMPT_CACHE_KEY_MAX_CHARS: usize = 64;

/// A function tool offered to the model.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    /// The JSON schema of the arguments, already in the form to send.
    pub parameters: Value,
    pub strict: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl ReasoningEffort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

/// Everything about a request except its input. Fixed for a run.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Settings {
    pub model: String,
    pub instructions: Option<String>,
    pub tools: Vec<ToolDefinition>,
    /// Whether the model reasons; decides `include`.
    pub reasoning_model: bool,
    /// The effort to ask for. `None` leaves it to the server.
    pub reasoning: Option<ReasoningEffort>,
    pub max_output_tokens: Option<u64>,
    /// The `text.format` of a typed run.
    pub text_format: Option<Value>,
    pub service_tier: Option<String>,
    pub prompt_cache_key: Option<String>,
}

/// A lane's `stream_id`: 1–256 characters from `[A-Za-z0-9_.-]`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StreamId(String);

/// A string that is not a valid [`StreamId`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidStreamId(pub String);

impl fmt::Display for InvalidStreamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid stream_id {:?}: need 1-256 characters from [A-Za-z0-9_.-]",
            self.0
        )
    }
}

impl std::error::Error for InvalidStreamId {}

impl StreamId {
    pub fn new(id: impl Into<String>) -> Result<Self, InvalidStreamId> {
        let id = id.into();
        let valid = (1..=256).contains(&id.len())
            && id.bytes().all(|b| {
                b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-')
            });
        if valid {
            Ok(Self(id))
        } else {
            Err(InvalidStreamId(id))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Builds the `response.create` body for `input`, on the lane
/// `stream_id` or on the default lane.
pub fn body(
    settings: &Settings,
    input: Vec<Value>,
    stream_id: Option<&StreamId>,
) -> Body {
    let mut body = Map::new();
    body.insert("type".into(), json!("response.create"));
    if let Some(stream_id) = stream_id {
        body.insert("stream_id".into(), json!(stream_id.as_str()));
    }
    body.insert("model".into(), json!(settings.model));
    body.insert("store".into(), json!(false));
    if let Some(instructions) = &settings.instructions {
        body.insert("instructions".into(), json!(instructions));
    }
    if !settings.tools.is_empty() {
        let tools: Vec<Value> = settings
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                    "strict": tool.strict,
                })
            })
            .collect();
        body.insert("tools".into(), Value::Array(tools));
    }
    if settings.reasoning_model {
        if let Some(effort) = settings.reasoning {
            body.insert(
                "reasoning".into(),
                json!({ "effort": effort.as_str(), "summary": "auto" }),
            );
        }
        body.insert("include".into(), json!(["reasoning.encrypted_content"]));
    }
    if let Some(max) = settings.max_output_tokens {
        body.insert(
            "max_output_tokens".into(),
            json!(max.max(MIN_OUTPUT_TOKENS)),
        );
    }
    if let Some(format) = &settings.text_format {
        body.insert("text".into(), json!({ "format": format }));
    }
    if let Some(tier) = &settings.service_tier {
        body.insert("service_tier".into(), json!(tier));
    }
    if let Some(key) = &settings.prompt_cache_key {
        let clamped: String =
            key.chars().take(PROMPT_CACHE_KEY_MAX_CHARS).collect();
        body.insert("prompt_cache_key".into(), json!(clamped));
    }
    body.insert("input".into(), Value::Array(input));
    body
}
