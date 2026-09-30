//! `response.*` server frames → [`AssistantEvent`]s.
//!
//! Ports pi's `processResponsesStream`
//! (`packages/ai/src/api/openai-responses-shared.ts:434-770`) for the
//! subset tau-agent speaks: OpenAI direct, WebSocket only, API keys only,
//! `store: false`. No custom tools, no grammar-constrained sampling
//! (`custom_tool_call*` items and events are ignored, like any other
//! unsupported item type). Frames outside the protocol are not
//! modelled: any frame this module does not recognize (such as
//! `codex.response.metadata`, which the plan route sends) is ignored
//! rather than rejected.
//!
//! [`StreamProcessor`] is pure: no I/O, no clock. The caller feeds it
//! parsed frames from one lane's socket traffic with
//! [`StreamProcessor::push`], in order, and gets back the
//! [`AssistantEvent`]s to publish. The stream grammar in [`crate::event`]
//! is upheld: the first event is always [`AssistantEvent::Start`], and
//! there is exactly one terminal event ([`AssistantEvent::Done`] or
//! [`AssistantEvent::Error`]), after which every frame is ignored.
//! A frame it does not know (such as `codex.response.metadata`, which
//! can come before `response.created`) is skipped and does not start the stream,
//! so `Start` carries the response id.
//!
//! ## Frame → event mapping
//!
//! | Frame                                                                     | Event(s)                                     |
//! | -------------------------------------------------------------------------- | --------------------------------------------- |
//! | `response.created` (first frame processed)                                 | `Start`, with `response_id` if already known |
//! | `response.output_item.added` (`reasoning`/`message`/`function_call`)       | `*Start`                                     |
//! | `response.reasoning_summary_text.delta`, `response.reasoning_text.delta`   | `ThinkingDelta`                              |
//! | `response.reasoning_summary_part.done`                                     | `ThinkingDelta` with `"\n\n"`                |
//! | `response.output_text.delta`, `response.refusal.delta`                     | `TextDelta`                                  |
//! | `response.function_call_arguments.delta`                                   | `ToolCallDelta`                              |
//! | `response.function_call_arguments.done`                                    | `ToolCallDelta` with the unsent suffix, if the full string extends what was streamed |
//! | `response.output_item.done`                                                | `*End`, with the authoritative block         |
//! | `response.completed`, `response.incomplete`                                | `Done` or `Error` (see below)                |
//! | `response.failed`                                                          | `Error`                                      |
//! | `error`                                                                    | `Error`                                      |
//! | anything else                                                              | ignored                                      |
//!
//! An item type this module does not support (e.g. `custom_tool_call`)
//! never gets a slot, so its later delta/done frames are silently
//! ignored too, the same way pi's `getSlot`/`getOrCreateSlot` skip an
//! absent slot.
//!
//! ## Stop reason mapping
//!
//! Ported from pi's `mapStopReason` (`openai-responses-shared.ts:766`),
//! restricted to the statuses `response.completed`/`response.incomplete`
//! can actually carry in this transport (the `failed`/`cancelled` arms of
//! pi's `mapStopReason` are unreachable here: `response.failed` is a
//! separate frame type, handled directly, and there is no
//! `response.cancelled` frame):
//!
//! | `status`                  | `incomplete_details.reason` | Outcome                                                                                             |
//! | -------------------------- | ----------------------------- | ----------------------------------------------------------------------------------------------------- |
//! | `completed`                 | —                              | `Done(Stop)`                                                                                         |
//! | `incomplete`                | `max_output_tokens`            | `Done(Length)`                                                                                       |
//! | `incomplete`                | anything else, or absent       | `Error("Response incomplete: {reason}")` / `Error("Response incomplete without a provider reason")` |
//! | absent                       | —                              | `Done(Stop)` (pi: `if (!status) return { stopReason: "stop" }`)                                     |
//! | any other status string      | —                              | `Error("Unexpected response status: {status}")` — not a status these two frames can actually carry  |
//!
//! `Done(Stop)` is upgraded to `Done(ToolUse)` whenever any
//! `function_call` item was ever seen in this response, matching pi's
//! `output.content.some(toolCall) && stopReason === "stop"` check — this
//! upgrade never applies to `Length` or `Error`.
//!
//! `response.failed` becomes `Error` with `"{code}: {message}"` (pi:
//! `${error.code || "unknown"}: ${error.message || "no message"}`), or,
//! lacking an `error` object, `"incomplete: {reason}"` from
//! `incomplete_details`, or `"Unknown error (no error details in
//! response)"`. A top-level `error` frame becomes `Error` with `"Error
//! Code {code}: {message}"` (pi throws exactly this string). Either way,
//! [`StreamProcessor::error_code`] exposes the raw `code`, so the lane
//! can recognize `previous_response_not_found` and
//! `websocket_connection_limit_reached` without parsing the message
//! text.
//!
//! [`StreamProcessor::close`] models the raw WebSocket frame reader
//! ending without a terminal frame (pi's `parseWebSocket`,
//! `openai-codex-responses.ts:1414`): it is a no-op once a terminal event
//! was already produced, and otherwise emits one `Error` with pi's exact
//! message, `"WebSocket stream closed before response.completed"`.
//!
//! ## Usage mapping
//!
//! Ported verbatim from `finalizeResponse`
//! (`openai-responses-shared.ts:596-612`): `usage.input_tokens` counts
//! cached and cache-write tokens too, so both are subtracted to get
//! [`Usage::input`] (new tokens only); [`Usage::cache_read`] and
//! [`Usage::cache_write`] default to 0 when
//! `usage.input_tokens_details` is absent; [`Usage::reasoning`] is `Some`
//! only when `usage.output_tokens_details.reasoning_tokens` is actually
//! present in the frame (`None` means the server did not report it, not
//! that it was 0); [`Usage::total_tokens`] is `usage.total_tokens`,
//! defaulting to 0. [`Usage::cost`] is always [`UsageCost::default`] —
//! pricing is `tau_ai::cost`'s job, not this module's. A response with
//! no `usage` object at all leaves usage at its default (all zero),
//! matching "usage on abort" in `docs/reference/openai-websocket.md`.
//!
//! ## Tool call arguments
//!
//! The raw JSON text is accumulated verbatim as it streams (matching
//! pi's `partialJson` scratch field), but that scratch text is never
//! exposed as the authoritative arguments: at `response.output_item.done`
//! the *complete* text — the `done` item's own `arguments` field if
//! present, else everything accumulated — is parsed fresh with
//! [`crate::partial_json::PartialJson`], which applies the same escape
//! and control-character leniency as pi's `repairJson`. If even that
//! fails to produce an object, the arguments are `{}`, matching pi's
//! `parseStreamingJson`'s ultimate fallback
//! (`packages/ai/src/utils/json-parse.ts:119`). This guarantees the
//! known case from `openai-responses-partial-json-cleanup.test.ts`: the
//! scratch buffer itself is never what gets persisted or emitted.
//!
//! ## Deviations from pi
//!
//! - No Azure `encrypted_content` backfill (`backfillReasoningSignatures`,
//!   `openai-responses-shared.ts:592`): that back-fills a gap specific to
//!   Azure OpenAI's Responses implementation, out of scope for the direct
//!   OpenAI API this module speaks.
//! - No message "phase" stop-reason tracking
//!   (`applyMessagePhaseStopReason`): pi uses it only to show a
//!   provisional `stopReason` on the *partial* message while streaming,
//!   always overwritten by the terminal event before the final message
//!   is observed; tau-ai's [`AssistantEvent`] has no equivalent
//!   mid-stream field to update. The `phase` value itself is still
//!   preserved, round-tripped into `text_signature` via
//!   [`encode_text_signature_v1`], since a resend must replay it.
//! - `redacted` on [`ThinkingContent`] is always `None`: it exists for
//!   Anthropic/Bedrock's redacted-thinking blocks, which this API family
//!   never produces.
//! - Blocks are assumed not to overlap on the wire (one open
//!   `output_index` slot at a time), matching how tau-agent's own
//!   requests are built and how OpenAI emits items in practice;
//!   [`crate::event::Accumulator`] enforces this as a hard grammar rule,
//!   unlike pi's independently-indexed slot map, which would tolerate
//!   genuine interleaving.
//! - A `function_call` item with no `id` falls back to a bare `call_id`
//!   (no `|`), rather than pi's unconditional `${call_id}|${id}`; this
//!   only matters for malformed server data, since OpenAI always sends
//!   both.

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::{
    event::{AssistantEvent, DoneReason, ErrorReason},
    message::{
        TextContent,
        ThinkingContent,
        Timestamp,
        ToolCall,
        Usage,
        UsageCost,
    },
    partial_json::PartialJson,
    retry::{Class, Failure, classify},
};

/// Turns one lane's `response.*` server frames into [`AssistantEvent`]s.
///
/// See the [module documentation](self) for the full frame-to-event
/// mapping.
#[derive(Debug)]
pub struct StreamProcessor {
    model: String,
    timestamp: Timestamp,
    started: bool,
    finished: bool,
    has_tool_call: bool,
    next_index: usize,
    response_id: Option<String>,
    error_code: Option<String>,
    slots: HashMap<u64, Slot>,
}

/// The state of one still-open `output_index`, keyed by that index (not
/// by our own content index, which is only assigned once the slot
/// opens — see "Deviations from pi" for why at most one of these is ever
/// open at a time in practice).
#[derive(Debug)]
enum Slot {
    Thinking {
        index: usize,
        /// Text streamed via delta events, used only as a fallback when
        /// the closing item carries neither a summary nor content array.
        accumulated: String,
    },
    Text {
        index: usize,
    },
    ToolCall {
        index: usize,
        id: String,
        name: String,
        /// Raw JSON text streamed so far, verbatim.
        raw_args: String,
    },
}

/// How a `response.completed`/`response.incomplete` frame's `status`
/// resolves, before the tool-use upgrade.
enum TerminalOutcome {
    Done(DoneReason),
    Error(String),
}

/// The frames [`StreamProcessor::push`] acts on; any other is skipped.
const KNOWN_FRAMES: &[&str] = &[
    "response.created",
    "response.output_item.added",
    "response.reasoning_summary_text.delta",
    "response.reasoning_text.delta",
    "response.reasoning_summary_part.done",
    "response.output_text.delta",
    "response.refusal.delta",
    "response.function_call_arguments.delta",
    "response.function_call_arguments.done",
    "response.output_item.done",
    "response.completed",
    "response.incomplete",
    "response.failed",
    "error",
];

impl StreamProcessor {
    /// Creates a processor for one response. `model` and `timestamp` seed
    /// the `Start` event; tau-ai has no clock of its own, so the caller
    /// supplies the timestamp.
    pub fn new(model: String, timestamp: Timestamp) -> Self {
        Self {
            model,
            timestamp,
            started: false,
            finished: false,
            has_tool_call: false,
            next_index: 0,
            response_id: None,
            error_code: None,
            slots: HashMap::new(),
        }
    }

    /// Feeds one parsed server frame. Returns the events it produces, in
    /// order. A no-op once a terminal event has already been produced.
    pub fn push(&mut self, frame: &Value) -> Vec<AssistantEvent> {
        let mut events = Vec::new();
        if self.finished {
            return events;
        }
        let Some(frame_type) = frame.get("type").and_then(Value::as_str) else {
            return events;
        };
        // A frame tau-ai does not know (`codex.response.metadata`, which can
        // come before `response.created`) is skipped, and does not start
        // the stream: `Start` waits for a frame that carries the
        // response, so it has the response id.
        if !KNOWN_FRAMES.contains(&frame_type) {
            return events;
        }
        if frame_type == "response.created"
            && let Some(id) = response_field_str(frame, "id")
        {
            self.response_id = Some(id.to_owned());
        }
        self.ensure_start(&mut events);
        match frame_type {
            "response.output_item.added" => {
                self.handle_output_item_added(frame, &mut events)
            }
            "response.reasoning_summary_text.delta"
            | "response.reasoning_text.delta" => {
                self.handle_thinking_delta(frame, &mut events)
            }
            "response.reasoning_summary_part.done" => {
                self.handle_thinking_part_done(frame, &mut events)
            }
            "response.output_text.delta" | "response.refusal.delta" => {
                self.handle_text_delta(frame, &mut events)
            }
            "response.function_call_arguments.delta" => {
                self.handle_tool_call_delta(frame, &mut events)
            }
            "response.function_call_arguments.done" => {
                self.handle_tool_call_args_done(frame, &mut events)
            }
            "response.output_item.done" => {
                self.handle_output_item_done(frame, &mut events)
            }
            "response.completed" | "response.incomplete" => {
                self.handle_terminal(frame, &mut events)
            }
            "response.failed" => self.handle_failed(frame, &mut events),
            "error" => self.handle_error(frame, &mut events),
            _ => {}
        }
        events
    }

    /// The socket closed, or the caller gave up, before a terminal event
    /// was seen. A no-op if one already was; otherwise ends the stream
    /// with the `Error` pi's raw frame reader raises in the same
    /// situation (`openai-codex-responses.ts:1414`).
    pub fn close(&mut self) -> Vec<AssistantEvent> {
        let mut events = Vec::new();
        if self.finished {
            return events;
        }
        self.ensure_start(&mut events);
        self.finished = true;
        // Nothing but `Start` went out: the turn can be sent again.
        let class = classify(&Failure::Transport {
            before_first_event: self.next_index == 0,
        });
        events.push(AssistantEvent::Error {
            reason: ErrorReason::Error,
            message: "WebSocket stream closed before response.completed"
                .to_owned(),
            usage: Usage::default(),
            class,
        });
        events
    }

    /// Whether a terminal event has been produced.
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// The response id, once known. Reliable after a successful
    /// `response.completed` (for the next turn's continuation); it may
    /// also be set earlier, from `response.created`.
    pub fn response_id(&self) -> Option<&str> {
        self.response_id.as_deref()
    }

    /// The raw `code` of the frame that produced the terminal `Error`
    /// event, when there was one (an `error` frame, or a
    /// `response.failed` with an `error` object). The lane uses this to
    /// recognize `previous_response_not_found` and
    /// `websocket_connection_limit_reached` without parsing
    /// [`AssistantEvent::Error`]'s message.
    pub fn error_code(&self) -> Option<&str> {
        self.error_code.as_deref()
    }

    fn ensure_start(&mut self, events: &mut Vec<AssistantEvent>) {
        if self.started {
            return;
        }
        self.started = true;
        events.push(AssistantEvent::Start {
            model: self.model.clone(),
            response_id: self.response_id.clone(),
            timestamp: self.timestamp,
        });
    }

    fn alloc_index(&mut self) -> usize {
        let index = self.next_index;
        self.next_index += 1;
        index
    }

    /// Opens a slot for `output_index` from an `added`/`done` item, if it
    /// does not already have one and its type is one this module
    /// supports. A no-op otherwise (matching pi's `getOrCreateSlot`
    /// skipping unsupported item types).
    fn create_slot(
        &mut self,
        output_index: u64,
        item: &Value,
        events: &mut Vec<AssistantEvent>,
    ) {
        if self.slots.contains_key(&output_index) {
            return;
        }
        let Some(item_type) = item.get("type").and_then(Value::as_str) else {
            return;
        };
        match item_type {
            "reasoning" => {
                let index = self.alloc_index();
                events.push(AssistantEvent::ThinkingStart { index });
                self.slots.insert(
                    output_index,
                    Slot::Thinking {
                        index,
                        accumulated: String::new(),
                    },
                );
            }
            "message" => {
                let index = self.alloc_index();
                events.push(AssistantEvent::TextStart { index });
                self.slots.insert(output_index, Slot::Text { index });
            }
            "function_call" => {
                let index = self.alloc_index();
                let call_id =
                    item.get("call_id").and_then(Value::as_str).unwrap_or("");
                let item_id = item.get("id").and_then(Value::as_str);
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                let id = match item_id {
                    Some(item_id) => format!("{call_id}|{item_id}"),
                    None => call_id.to_owned(),
                };
                events.push(AssistantEvent::ToolCallStart {
                    index,
                    id: id.clone(),
                    name: name.clone(),
                });
                self.has_tool_call = true;
                let initial_args = item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                if !initial_args.is_empty() {
                    events.push(AssistantEvent::ToolCallDelta {
                        index,
                        delta: initial_args.clone(),
                    });
                }
                self.slots.insert(
                    output_index,
                    Slot::ToolCall {
                        index,
                        id,
                        name,
                        raw_args: initial_args,
                    },
                );
            }
            _ => {}
        }
    }

    fn handle_output_item_added(
        &mut self,
        frame: &Value,
        events: &mut Vec<AssistantEvent>,
    ) {
        let (Some(output_index), Some(item)) =
            (output_index(frame), frame.get("item"))
        else {
            return;
        };
        self.create_slot(output_index, item, events);
    }

    fn handle_thinking_delta(
        &mut self,
        frame: &Value,
        events: &mut Vec<AssistantEvent>,
    ) {
        let (Some(output_index), Some(delta)) = (
            output_index(frame),
            frame.get("delta").and_then(Value::as_str),
        ) else {
            return;
        };
        if let Some(Slot::Thinking { index, accumulated }) =
            self.slots.get_mut(&output_index)
        {
            accumulated.push_str(delta);
            events.push(AssistantEvent::ThinkingDelta {
                index: *index,
                delta: delta.to_owned(),
            });
        }
    }

    fn handle_thinking_part_done(
        &mut self,
        frame: &Value,
        events: &mut Vec<AssistantEvent>,
    ) {
        let Some(output_index) = output_index(frame) else {
            return;
        };
        if let Some(Slot::Thinking { index, accumulated }) =
            self.slots.get_mut(&output_index)
        {
            accumulated.push_str("\n\n");
            events.push(AssistantEvent::ThinkingDelta {
                index: *index,
                delta: "\n\n".to_owned(),
            });
        }
    }

    fn handle_text_delta(
        &mut self,
        frame: &Value,
        events: &mut Vec<AssistantEvent>,
    ) {
        let (Some(output_index), Some(delta)) = (
            output_index(frame),
            frame.get("delta").and_then(Value::as_str),
        ) else {
            return;
        };
        if let Some(Slot::Text { index }) = self.slots.get(&output_index) {
            events.push(AssistantEvent::TextDelta {
                index: *index,
                delta: delta.to_owned(),
            });
        }
    }

    fn handle_tool_call_delta(
        &mut self,
        frame: &Value,
        events: &mut Vec<AssistantEvent>,
    ) {
        let (Some(output_index), Some(delta)) = (
            output_index(frame),
            frame.get("delta").and_then(Value::as_str),
        ) else {
            return;
        };
        if let Some(Slot::ToolCall {
            index, raw_args, ..
        }) = self.slots.get_mut(&output_index)
        {
            raw_args.push_str(delta);
            events.push(AssistantEvent::ToolCallDelta {
                index: *index,
                delta: delta.to_owned(),
            });
        }
    }

    /// `response.function_call_arguments.done` carries the complete
    /// arguments string. Like pi, this only ever emits the *unsent
    /// suffix* as a delta (when the full string extends what was already
    /// streamed) — never a correction — so a live view built purely from
    /// deltas only ever grows. The authoritative value is computed
    /// separately, at `response.output_item.done`.
    fn handle_tool_call_args_done(
        &mut self,
        frame: &Value,
        events: &mut Vec<AssistantEvent>,
    ) {
        let (Some(output_index), Some(full)) = (
            output_index(frame),
            frame.get("arguments").and_then(Value::as_str),
        ) else {
            return;
        };
        if let Some(Slot::ToolCall {
            index, raw_args, ..
        }) = self.slots.get_mut(&output_index)
        {
            if let Some(suffix) = full.strip_prefix(raw_args.as_str())
                && !suffix.is_empty()
            {
                events.push(AssistantEvent::ToolCallDelta {
                    index: *index,
                    delta: suffix.to_owned(),
                });
            }
            *raw_args = full.to_owned();
        }
    }

    fn handle_output_item_done(
        &mut self,
        frame: &Value,
        events: &mut Vec<AssistantEvent>,
    ) {
        let (Some(output_index), Some(item)) =
            (output_index(frame), frame.get("item"))
        else {
            return;
        };
        self.create_slot(output_index, item, events);
        let Some(slot) = self.slots.remove(&output_index) else {
            return;
        };
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
        match (item_type, slot) {
            ("reasoning", Slot::Thinking { index, accumulated }) => {
                let summary_text = join_texts(item.get("summary"));
                let content_text = join_texts(item.get("content"));
                let thinking = if !summary_text.is_empty() {
                    summary_text
                } else if !content_text.is_empty() {
                    content_text
                } else {
                    accumulated
                };
                events.push(AssistantEvent::ThinkingEnd {
                    index,
                    content: ThinkingContent {
                        thinking,
                        thinking_signature: serde_json::to_string(item).ok(),
                        redacted: None,
                    },
                });
            }
            ("message", Slot::Text { index }) => {
                let text = join_message_content(item.get("content"));
                let item_id =
                    item.get("id").and_then(Value::as_str).unwrap_or("");
                let phase = item.get("phase").and_then(Value::as_str);
                events.push(AssistantEvent::TextEnd {
                    index,
                    content: TextContent {
                        text,
                        text_signature: Some(encode_text_signature_v1(
                            item_id, phase,
                        )),
                    },
                });
            }
            (
                "function_call",
                Slot::ToolCall {
                    index,
                    id,
                    name,
                    raw_args,
                },
            ) => {
                let final_text = item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or(raw_args);
                events.push(AssistantEvent::ToolCallEnd {
                    index,
                    tool_call: ToolCall {
                        id,
                        name,
                        arguments: parse_final_arguments(&final_text),
                    },
                });
            }
            // Item type does not match the slot's kind: nothing sensible
            // to close. Should not happen with real server data.
            _ => {}
        }
    }

    fn handle_terminal(
        &mut self,
        frame: &Value,
        events: &mut Vec<AssistantEvent>,
    ) {
        let response = frame.get("response");
        if let Some(id) = response_field_str(frame, "id") {
            self.response_id = Some(id.to_owned());
        }
        let usage = response
            .and_then(|r| r.get("usage"))
            .map(parse_usage)
            .unwrap_or_default();
        let status = response
            .and_then(|r| r.get("status"))
            .and_then(Value::as_str);
        let incomplete_reason = response
            .and_then(|r| r.get("incomplete_details"))
            .and_then(|d| d.get("reason"))
            .and_then(Value::as_str);
        let outcome = match status {
            Some("completed") => TerminalOutcome::Done(DoneReason::Stop),
            Some("incomplete") => map_incomplete(incomplete_reason),
            // Absent status defensively maps to `stop`, matching pi's
            // `if (!status) return { stopReason: "stop" }`
            // (`openai-responses-shared.ts:767`). Any other status string
            // is not one `response.completed`/`response.incomplete` can
            // actually carry (pi's own switch is exhaustive over the enum
            // and would fail to compile if a new one appeared), so it is
            // treated as a protocol error rather than silently a success.
            None => TerminalOutcome::Done(DoneReason::Stop),
            Some(other) => TerminalOutcome::Error(format!(
                "Unexpected response status: {other}"
            )),
        };
        self.finished = true;
        match outcome {
            TerminalOutcome::Done(reason) => {
                let reason = if reason == DoneReason::Stop && self.has_tool_call
                {
                    DoneReason::ToolUse
                } else {
                    reason
                };
                events.push(AssistantEvent::Done {
                    reason,
                    usage,
                    response_id: self.response_id.clone(),
                });
            }
            TerminalOutcome::Error(message) => {
                events.push(AssistantEvent::Error {
                    reason: ErrorReason::Error,
                    message,
                    usage,
                    class: Class::Fatal,
                });
            }
        }
    }

    fn handle_failed(
        &mut self,
        frame: &Value,
        events: &mut Vec<AssistantEvent>,
    ) {
        let response = frame.get("response");
        let error = response.and_then(|r| r.get("error"));
        self.error_code = error
            .and_then(|e| e.get("code"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let message = if let Some(error) = error {
            let code = error
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let msg = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("no message");
            format!("{code}: {msg}")
        } else if let Some(reason) = response
            .and_then(|r| r.get("incomplete_details"))
            .and_then(|d| d.get("reason"))
            .and_then(Value::as_str)
        {
            format!("incomplete: {reason}")
        } else {
            "Unknown error (no error details in response)".to_owned()
        };
        self.finished = true;
        let class = classify(&Failure::Api {
            code: self.error_code.as_deref(),
            kind: error.and_then(|e| e.get("type")).and_then(Value::as_str),
            status: None,
        });
        events.push(AssistantEvent::Error {
            reason: ErrorReason::Error,
            message,
            usage: Usage::default(),
            class,
        });
    }

    fn handle_error(
        &mut self,
        frame: &Value,
        events: &mut Vec<AssistantEvent>,
    ) {
        // The server nests the details in `error`; pi's frames carry
        // them at the top.
        let details = frame
            .get("error")
            .filter(|error| error.is_object())
            .unwrap_or(frame);
        let field = |name| details.get(name).and_then(Value::as_str);
        let code = field("code");
        self.error_code = code.map(str::to_owned);
        let message = field("message").unwrap_or("");
        let code_text = code.unwrap_or("unknown");
        self.finished = true;
        let class = classify(&Failure::Api {
            code,
            kind: field("type"),
            status: frame
                .get("status")
                .and_then(Value::as_u64)
                .and_then(|status| u16::try_from(status).ok()),
        });
        events.push(AssistantEvent::Error {
            reason: ErrorReason::Error,
            message: format!("Error Code {code_text}: {message}"),
            usage: Usage::default(),
            class,
        });
    }
}

fn output_index(frame: &Value) -> Option<u64> {
    frame.get("output_index").and_then(Value::as_u64)
}

fn response_field_str<'a>(frame: &'a Value, field: &str) -> Option<&'a str> {
    frame
        .get("response")
        .and_then(|r| r.get(field))
        .and_then(Value::as_str)
}

/// `incomplete_details.reason` → the terminal outcome, per pi's
/// `mapStopReason`'s `"incomplete"` arm.
fn map_incomplete(reason: Option<&str>) -> TerminalOutcome {
    match reason {
        Some("max_output_tokens") => TerminalOutcome::Done(DoneReason::Length),
        Some(reason) => {
            TerminalOutcome::Error(format!("Response incomplete: {reason}"))
        }
        None => TerminalOutcome::Error(
            "Response incomplete without a provider reason".to_owned(),
        ),
    }
}

/// Joins the `.text` field of every element of a summary/content array
/// (pi: `item.summary?.map((s) => s.text).join("\n\n")`), or `""` when
/// `value` is absent or not an array.
fn join_texts(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n\n")
        })
        .unwrap_or_default()
}

/// Joins a message item's `content` array: `output_text` elements
/// contribute their `text`, everything else (a `refusal` element)
/// contributes its `refusal` field, concatenated with no separator (pi:
/// `item.content?.map((c) => (c.type === "output_text" ? c.text :
/// c.refusal)).join("")`).
fn join_message_content(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    let field = if item.get("type").and_then(Value::as_str)
                        == Some("output_text")
                    {
                        "text"
                    } else {
                        "refusal"
                    };
                    item.get(field).and_then(Value::as_str).unwrap_or("")
                })
                .collect::<String>()
        })
        .unwrap_or_default()
}

/// Encodes pi's `TextSignatureV1` (`encodeTextSignatureV1`,
/// `openai-responses-shared.ts:52`): the message item's id, and its
/// `phase` when the item has one. This is what a resend replays as the
/// item's `id` (see `responses::input::parse_text_signature`).
pub fn encode_text_signature_v1(item_id: &str, phase: Option<&str>) -> String {
    let mut payload = Map::new();
    payload.insert("v".to_owned(), Value::from(1));
    payload.insert("id".to_owned(), Value::from(item_id));
    if let Some(phase) = phase {
        payload.insert("phase".to_owned(), Value::from(phase));
    }
    serde_json::to_string(&Value::Object(payload))
        .expect("a JSON object always serializes")
}

/// Parses the complete tool-call arguments text leniently, falling back
/// to `{}` exactly when pi's `parseStreamingJson` would: see the "Tool
/// call arguments" section of the [module documentation](self).
fn parse_final_arguments(text: &str) -> Map<String, Value> {
    let mut parser = PartialJson::new();
    parser.push(text);
    match parser.finish() {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    }
}

/// `finalizeResponse`'s usage mapping
/// (`openai-responses-shared.ts:596-612`); see the "Usage mapping"
/// section of the [module documentation](self).
fn parse_usage(usage: &Value) -> Usage {
    let get_u64 =
        |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    let details = usage.get("input_tokens_details");
    let cache_read = details
        .and_then(|d| d.get("cached_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_write = details
        .and_then(|d| d.get("cache_write_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let input_tokens = get_u64("input_tokens");
    let reasoning = usage
        .get("output_tokens_details")
        .and_then(|d| d.get("reasoning_tokens"))
        .and_then(Value::as_u64);
    Usage {
        input: input_tokens
            .saturating_sub(cache_read)
            .saturating_sub(cache_write),
        output: get_u64("output_tokens"),
        cache_read,
        cache_write,
        reasoning,
        total_tokens: get_u64("total_tokens"),
        cost: UsageCost::default(),
    }
}
