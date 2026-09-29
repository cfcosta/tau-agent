//! Streaming events for one assistant response, and the accumulator that
//! rebuilds the final message from them.
//!
//! This mirrors pi's `AssistantMessageEvent` (`packages/ai/src/types.ts`),
//! with one deliberate difference: pi attaches a shared, mutable `partial`
//! message to every event. Here each event owns only its delta, and
//! [`Accumulator`] rebuilds the message.
//!
//! A valid stream is `Start`, then any number of blocks, then exactly one
//! terminal event (`Done` or `Error`). A block is `*Start`, its `*Delta`s,
//! then `*End`, and blocks never overlap. Block indices count up from 0 in
//! the order blocks start.

use crate::{
    message::{
        API,
        AssistantBlock,
        AssistantMessage,
        PROVIDER,
        StopReason,
        TextContent,
        ThinkingContent,
        Timestamp,
        ToolCall,
        Usage,
    },
    retry::Class,
};

#[derive(Debug, Clone, PartialEq)]
pub enum AssistantEvent {
    Start {
        model: String,
        response_id: Option<String>,
        timestamp: Timestamp,
    },
    TextStart {
        index: usize,
    },
    TextDelta {
        index: usize,
        delta: String,
    },
    /// Carries the authoritative block, including its signature.
    TextEnd {
        index: usize,
        content: TextContent,
    },
    ThinkingStart {
        index: usize,
    },
    ThinkingDelta {
        index: usize,
        delta: String,
    },
    /// Carries the authoritative block, including the encrypted reasoning.
    ThinkingEnd {
        index: usize,
        content: ThinkingContent,
    },
    ToolCallStart {
        index: usize,
        id: String,
        name: String,
    },
    /// A fragment of the tool call's JSON arguments.
    ToolCallDelta {
        index: usize,
        delta: String,
    },
    /// Carries the authoritative tool call with its parsed arguments.
    ToolCallEnd {
        index: usize,
        tool_call: ToolCall,
    },
    Done {
        reason: DoneReason,
        usage: Usage,
        response_id: Option<String>,
    },
    Error {
        reason: ErrorReason,
        message: String,
        usage: Usage,
        /// Whether the turn may be retried, decided where the failure's
        /// code and timing are known (see [`crate::retry::classify`]).
        class: Class,
    },
}

/// How a successful response ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DoneReason {
    Stop,
    Length,
    ToolUse,
}

/// How a failed response ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorReason {
    Error,
    Aborted,
}

impl From<DoneReason> for StopReason {
    fn from(reason: DoneReason) -> Self {
        match reason {
            DoneReason::Stop => Self::Stop,
            DoneReason::Length => Self::Length,
            DoneReason::ToolUse => Self::ToolUse,
        }
    }
}

impl From<ErrorReason> for StopReason {
    fn from(reason: ErrorReason) -> Self {
        match reason {
            ErrorReason::Error => Self::Error,
            ErrorReason::Aborted => Self::Aborted,
        }
    }
}

/// An event that breaks the stream grammar.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("event {position}: {reason}")]
pub struct GrammarError {
    /// Position of the offending event in the stream, from 0.
    pub position: usize,
    pub reason: &'static str,
}

/// Rebuilds an [`AssistantMessage`] from a stream of events, checking the
/// stream grammar as it goes.
#[derive(Debug, Default)]
pub struct Accumulator {
    position: usize,
    message: Option<AssistantMessage>,
    open: Option<OpenBlock>,
    /// Raw JSON of the open tool call's arguments, as streamed so far.
    arguments: String,
    finished: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenBlock {
    Text(usize),
    Thinking(usize),
    ToolCall(usize),
}

impl Accumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies one event.
    pub fn push(&mut self, event: AssistantEvent) -> Result<(), GrammarError> {
        let result = self.apply(event);
        self.position += 1;
        result
    }

    /// The message so far. Blocks that are still open show the text
    /// streamed so far; an open tool call shows empty arguments.
    pub fn partial(&self) -> Option<&AssistantMessage> {
        self.message.as_ref()
    }

    /// The raw JSON arguments streamed so far for the open tool call.
    pub fn partial_arguments(&self) -> Option<&str> {
        matches!(self.open, Some(OpenBlock::ToolCall(_)))
            .then_some(self.arguments.as_str())
    }

    /// Whether a terminal event has been applied.
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// The final message, once a terminal event has been applied.
    pub fn finish(self) -> Result<AssistantMessage, GrammarError> {
        match self.message {
            Some(message) if self.finished => Ok(message),
            _ => Err(GrammarError {
                position: self.position,
                reason: "stream ended without a terminal event",
            }),
        }
    }

    fn apply(&mut self, event: AssistantEvent) -> Result<(), GrammarError> {
        let position = self.position;
        let fail = |reason| Err(GrammarError { position, reason });
        if self.finished {
            return fail("event after the terminal event");
        }
        let Some(message) = self.message.as_mut() else {
            return match event {
                AssistantEvent::Start {
                    model,
                    response_id,
                    timestamp,
                } => {
                    self.message = Some(AssistantMessage {
                        content: Vec::new(),
                        api: API.to_owned(),
                        provider: PROVIDER.to_owned(),
                        model,
                        response_id,
                        usage: Usage::default(),
                        stop_reason: StopReason::Stop,
                        error_message: None,
                        timestamp,
                    });
                    Ok(())
                }
                _ => fail("stream must begin with Start"),
            };
        };
        let next = message.content.len();

        match event {
            AssistantEvent::Start { .. } => fail("second Start"),

            AssistantEvent::TextStart { index } => {
                if self.open.is_some() || index != next {
                    return fail("TextStart out of sequence");
                }
                message.content.push(AssistantBlock::Text(TextContent {
                    text: String::new(),
                    text_signature: None,
                }));
                self.open = Some(OpenBlock::Text(index));
                Ok(())
            }
            AssistantEvent::TextDelta { index, delta } => {
                if self.open != Some(OpenBlock::Text(index)) {
                    return fail("TextDelta for a block that is not open");
                }
                if let Some(AssistantBlock::Text(block)) =
                    message.content.last_mut()
                {
                    block.text.push_str(&delta);
                }
                Ok(())
            }
            AssistantEvent::TextEnd { index, content } => {
                if self.open != Some(OpenBlock::Text(index)) {
                    return fail("TextEnd for a block that is not open");
                }
                message.content[index] = AssistantBlock::Text(content);
                self.open = None;
                Ok(())
            }

            AssistantEvent::ThinkingStart { index } => {
                if self.open.is_some() || index != next {
                    return fail("ThinkingStart out of sequence");
                }
                message.content.push(AssistantBlock::Thinking(
                    ThinkingContent {
                        thinking: String::new(),
                        thinking_signature: None,
                        redacted: None,
                    },
                ));
                self.open = Some(OpenBlock::Thinking(index));
                Ok(())
            }
            AssistantEvent::ThinkingDelta { index, delta } => {
                if self.open != Some(OpenBlock::Thinking(index)) {
                    return fail("ThinkingDelta for a block that is not open");
                }
                if let Some(AssistantBlock::Thinking(block)) =
                    message.content.last_mut()
                {
                    block.thinking.push_str(&delta);
                }
                Ok(())
            }
            AssistantEvent::ThinkingEnd { index, content } => {
                if self.open != Some(OpenBlock::Thinking(index)) {
                    return fail("ThinkingEnd for a block that is not open");
                }
                message.content[index] = AssistantBlock::Thinking(content);
                self.open = None;
                Ok(())
            }

            AssistantEvent::ToolCallStart { index, id, name } => {
                if self.open.is_some() || index != next {
                    return fail("ToolCallStart out of sequence");
                }
                message.content.push(AssistantBlock::ToolCall(ToolCall {
                    id,
                    name,
                    arguments: Default::default(),
                }));
                self.arguments.clear();
                self.open = Some(OpenBlock::ToolCall(index));
                Ok(())
            }
            AssistantEvent::ToolCallDelta { index, delta } => {
                if self.open != Some(OpenBlock::ToolCall(index)) {
                    return fail("ToolCallDelta for a block that is not open");
                }
                self.arguments.push_str(&delta);
                Ok(())
            }
            AssistantEvent::ToolCallEnd { index, tool_call } => {
                if self.open != Some(OpenBlock::ToolCall(index)) {
                    return fail("ToolCallEnd for a block that is not open");
                }
                message.content[index] = AssistantBlock::ToolCall(tool_call);
                self.arguments.clear();
                self.open = None;
                Ok(())
            }

            AssistantEvent::Done {
                reason,
                usage,
                response_id,
            } => {
                if self.open.is_some() {
                    return fail("Done while a block is open");
                }
                message.stop_reason = reason.into();
                message.usage = usage;
                if response_id.is_some() {
                    message.response_id = response_id;
                }
                self.finished = true;
                Ok(())
            }
            AssistantEvent::Error {
                reason,
                message: error,
                usage,
                ..
            } => {
                // An error may cut a block off mid-stream; the partial block
                // stays in the message, as it does in pi.
                message.stop_reason = reason.into();
                message.error_message = Some(error);
                message.usage = usage;
                self.open = None;
                self.finished = true;
                Ok(())
            }
        }
    }
}
