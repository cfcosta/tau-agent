//! `ScriptedModel`: a deterministic [`tau_ai::llm::Llm`], ported from pi's
//! faux provider (`packages/ai/src/providers/faux.ts`), OpenAI-subset only.
//!
//! ```
//! use serde_json::json;
//! use tau_testing::scripted::ScriptedModel;
//!
//! let llm = ScriptedModel::new()
//!     .turn(|t| t.thinking("...").tool_call("search", json!({"q": "tokio"})))
//!     .turn(|t| t.text("Found it.").usage(1200, 80));
//! ```
//!
//! - **Shared script queue.** `ScriptedModel` clones share their script and
//!   their recording (`Arc` inside). Every [`tau_ai::llm::Llm::open`] call
//!   returns a new session, but every session opened from the same model
//!   (or from a clone of it) draws its turns from the *same* queue, in the
//!   order `respond` is called. Two runs sharing one `ScriptedModel`
//!   therefore interleave their draws from one script; give each run its
//!   own `ScriptedModel` to keep their scripts independent.
//! - **Recording.** Every request (`settings` plus the transcript it was
//!   asked to answer) is recorded in [`ScriptedModel::requests`], in call
//!   order, whichever session made the call.
//! - **Exhaustion.** Once the queue is empty, `respond` still answers: the
//!   stream is `Start` then `Error` with a message that says the script is
//!   exhausted. It never hangs. [`ScriptedModel::assert_exhausted`] and
//!   [`ScriptedModel::remaining`] let a test check the script was fully
//!   used.
//! - **Errors.** [`TurnBuilder::error`] scripts a failed turn (for example
//!   `rate_limit_exceeded` or `context_length_exceeded`);
//!   [`TurnBuilder::dropped`] scripts a connection dropped mid-stream, whose
//!   terminal event's message is exactly `"connection lost"`;
//!   [`TurnBuilder::fails_before_start`] scripts a connection that never
//!   got going. All three still open with `Start`, so the stream keeps the
//!   grammar in `tau_ai::event`: `Start` then exactly one terminal event.
//! - **Cache simulation.** Like pi's faux provider, each session tracks the
//!   serialized text of the transcript it last answered. The next request
//!   on that session reports `cache_read` for the common prefix with that
//!   text and `cache_write` for the rest, and `input` is the turn's prompt
//!   tokens minus `cache_read`. A session's first request has no prefix to
//!   share, so it reports `cache_read: 0` and pays full `cache_write`. This
//!   is per session, not global: a fresh session (a fresh `open` call)
//!   starts with no cached prefix even if other sessions of the same
//!   `ScriptedModel` have already answered requests.
//! - **Streaming.** A turn's text, thinking and tool-call-argument content
//!   is split into a few deltas, deterministically (no randomness), so a
//!   test can rely on exact event sequences.
//! - **Delay.** [`TurnBuilder::delay`] makes the stream sleep once, with
//!   `tokio::time::sleep`, before its first event. Under
//!   [`crate::block_on`]'s paused clock this costs no real time, so a test
//!   can assert on cancellation and timing without waiting.

use std::{
    collections::VecDeque,
    sync::{
        Arc,
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use futures_util::{FutureExt, StreamExt, future, stream};
use serde_json::Value;
use tau_ai::{
    event::{AssistantEvent, DoneReason, ErrorReason},
    llm::{EventStream, Llm, LlmError, LlmSession},
    message::{
        AssistantBlock,
        AssistantMessage,
        InputBlock,
        Message,
        StopReason,
        TextContent,
        ThinkingContent,
        Timestamp,
        ToolCall,
        ToolResultMessage,
        Usage,
        UsageCost,
        UserContent,
    },
    responses::request::Settings,
};

/// The message on the terminal `Error` event of a [`TurnBuilder::dropped`]
/// turn.
pub const DROPPED_MESSAGE: &str = "connection lost";

/// The message on the terminal `Error` event of a
/// [`TurnBuilder::fails_before_start`] turn.
pub const FAILS_BEFORE_START_MESSAGE: &str =
    "connection failed before the response could start";

/// The message on the terminal `Error` event a request gets when the
/// script has no turn left for it.
pub const EXHAUSTED_MESSAGE: &str = "ScriptedModel: script exhausted; no scripted turn is left for this request";

/// One request `ScriptedModel` received: the settings a session was opened
/// with, and the transcript it was asked to answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub settings: Settings,
    pub transcript: Vec<Message>,
}

type DynFactory =
    Arc<dyn Fn(&[Message]) -> AssistantMessage + Send + Sync + 'static>;

enum Turn {
    Static(StaticTurn),
    Dynamic(DynFactory),
}

struct StaticTurn {
    blocks: Vec<AssistantBlock>,
    stop: StopReason,
    /// The terminal event's error message. `Some` only when `stop` is
    /// `Error` or `Aborted`.
    message: Option<String>,
    /// Overrides the turn's `(input, output)` token estimate. `cache_read`
    /// and `cache_write` are always computed from the transcript, and
    /// layered on top of this (see the module docs' "Cache simulation").
    usage: Option<(u64, u64)>,
    /// The turn's cost in USD, reported as `usage.cost`.
    cost: f64,
    delay: Option<Duration>,
}

#[derive(Default)]
struct Inner {
    turns: VecDeque<Turn>,
    requests: Vec<Request>,
}

/// A deterministic [`Llm`], scripted turn by turn. See the module docs.
#[derive(Clone, Default)]
pub struct ScriptedModel {
    inner: Arc<Mutex<Inner>>,
    tool_call_ids: Arc<AtomicU64>,
    response_ids: Arc<AtomicU64>,
}

impl ScriptedModel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends one scripted turn, built with a [`TurnBuilder`].
    pub fn turn(self, build: impl FnOnce(TurnBuilder) -> TurnBuilder) -> Self {
        let builder = TurnBuilder::new(self.tool_call_ids.clone());
        let turn = build(builder).build();
        self.lock().turns.push_back(Turn::Static(turn));
        self
    }

    /// Appends a turn computed from the incoming transcript, like pi's
    /// `FauxResponseFactory`. The returned message's `content`,
    /// `stop_reason` and `error_message` are used as scripted; its
    /// `usage.input`/`usage.output` are used as the token-estimate
    /// override (see [`TurnBuilder::usage`]) and `cache_read`/`cache_write`
    /// are still computed from the transcript.
    pub fn turn_with(
        self,
        factory: impl Fn(&[Message]) -> AssistantMessage + Send + Sync + 'static,
    ) -> Self {
        self.lock()
            .turns
            .push_back(Turn::Dynamic(Arc::new(factory)));
        self
    }

    /// Every request received so far, in order.
    pub fn requests(&self) -> Vec<Request> {
        self.lock().requests.clone()
    }

    /// How many scripted turns have not been drawn yet.
    pub fn remaining(&self) -> usize {
        self.lock().turns.len()
    }

    /// Panics unless every scripted turn was drawn.
    pub fn assert_exhausted(&self) {
        let remaining = self.remaining();
        assert_eq!(
            remaining, 0,
            "ScriptedModel: {remaining} scripted turn(s) were never used"
        );
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn record(&self, request: Request) {
        self.lock().requests.push(request);
    }

    fn pop_turn(&self) -> Option<Turn> {
        self.lock().turns.pop_front()
    }

    fn next_response_id(&self) -> String {
        format!("resp_{}", self.response_ids.fetch_add(1, Ordering::Relaxed))
    }
}

impl Llm for ScriptedModel {
    fn open(
        &self,
        settings: Settings,
    ) -> futures_util::future::BoxFuture<
        'static,
        Result<Box<dyn LlmSession>, LlmError>,
    > {
        let session = ScriptedSession {
            model: self.clone(),
            settings,
            previous_prompt: None,
        };
        future::ready(Ok(Box::new(session) as Box<dyn LlmSession>)).boxed()
    }
}

struct ScriptedSession {
    model: ScriptedModel,
    settings: Settings,
    /// The serialized transcript of the last request this session
    /// answered, for the cache simulation. `None` before the first
    /// request.
    previous_prompt: Option<String>,
}

/// What one request resolves to, before it is turned into events.
struct ResolvedTurn {
    blocks: Vec<AssistantBlock>,
    stop: StopReason,
    message: Option<String>,
    usage: Option<(u64, u64)>,
    cost: f64,
    delay: Option<Duration>,
}

impl ScriptedSession {
    fn resolve(&self, transcript: &[Message]) -> ResolvedTurn {
        match self.model.pop_turn() {
            None => ResolvedTurn {
                blocks: Vec::new(),
                stop: StopReason::Error,
                message: Some(EXHAUSTED_MESSAGE.to_owned()),
                usage: None,
                cost: 0.0,
                delay: None,
            },
            Some(Turn::Dynamic(factory)) => {
                let response = factory(transcript);
                ResolvedTurn {
                    blocks: response.content,
                    stop: response.stop_reason,
                    message: response.error_message,
                    usage: Some((response.usage.input, response.usage.output)),
                    cost: response.usage.cost.total,
                    delay: None,
                }
            }
            Some(Turn::Static(turn)) => ResolvedTurn {
                blocks: turn.blocks,
                stop: turn.stop,
                message: turn.message,
                usage: turn.usage,
                cost: turn.cost,
                delay: turn.delay,
            },
        }
    }

    /// Reports `cache_read`/`cache_write` for the part of `transcript_text`
    /// that repeats (or doesn't) the previous request's transcript on this
    /// session, ported from pi's `withUsageEstimate`
    /// (`packages/ai/src/providers/faux.ts`).
    fn make_usage(
        &mut self,
        transcript_text: &str,
        content_text: &str,
        usage_override: Option<(u64, u64)>,
    ) -> Usage {
        let prompt_tokens = usage_override
            .map(|(input, _)| input)
            .unwrap_or_else(|| estimate_tokens(transcript_text));
        let output_tokens = usage_override
            .map(|(_, output)| output)
            .unwrap_or_else(|| estimate_tokens(content_text));

        let (cache_read, cache_write, input) = match &self.previous_prompt {
            Some(previous) => {
                let (matched, remainder) =
                    common_prefix_split(previous, transcript_text);
                let cache_read = estimate_tokens(&matched);
                let cache_write = estimate_tokens(&remainder);
                let input = prompt_tokens.saturating_sub(cache_read);
                (cache_read, cache_write, input)
            }
            None => (0, prompt_tokens, prompt_tokens),
        };
        self.previous_prompt = Some(transcript_text.to_owned());

        Usage {
            input,
            output: output_tokens,
            cache_read,
            cache_write,
            reasoning: None,
            total_tokens: input + output_tokens + cache_read + cache_write,
            cost: UsageCost::default(),
        }
    }

    fn materialize(
        &mut self,
        resolved: ResolvedTurn,
        transcript_text: &str,
        timestamp: Timestamp,
        response_id: String,
    ) -> (Vec<AssistantEvent>, Option<Duration>) {
        let ResolvedTurn {
            blocks,
            stop,
            message,
            usage,
            cost,
            delay,
        } = resolved;
        let content_text = assistant_content_text(&blocks);
        let mut usage = self.make_usage(transcript_text, &content_text, usage);
        usage.cost = UsageCost {
            output: cost,
            total: cost,
            ..UsageCost::default()
        };

        let mut events = vec![AssistantEvent::Start {
            model: self.settings.model.clone(),
            response_id: Some(response_id),
            timestamp,
        }];
        for (index, block) in blocks.into_iter().enumerate() {
            match block {
                AssistantBlock::Text(TextContent { text, .. }) => {
                    events.push(AssistantEvent::TextStart { index });
                    for delta in chunk_text(&text) {
                        events.push(AssistantEvent::TextDelta { index, delta });
                    }
                    events.push(AssistantEvent::TextEnd {
                        index,
                        content: TextContent {
                            text,
                            text_signature: None,
                        },
                    });
                }
                AssistantBlock::Thinking(ThinkingContent {
                    thinking, ..
                }) => {
                    events.push(AssistantEvent::ThinkingStart { index });
                    for delta in chunk_text(&thinking) {
                        events.push(AssistantEvent::ThinkingDelta {
                            index,
                            delta,
                        });
                    }
                    events.push(AssistantEvent::ThinkingEnd {
                        index,
                        content: ThinkingContent {
                            thinking,
                            thinking_signature: None,
                            redacted: None,
                        },
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
                    for delta in chunk_text(&json) {
                        events.push(AssistantEvent::ToolCallDelta {
                            index,
                            delta,
                        });
                    }
                    events.push(AssistantEvent::ToolCallEnd {
                        index,
                        tool_call: call,
                    });
                }
            }
        }

        match stop {
            StopReason::Error | StopReason::Aborted => {
                let reason = if stop == StopReason::Error {
                    ErrorReason::Error
                } else {
                    ErrorReason::Aborted
                };
                events.push(AssistantEvent::Error {
                    reason,
                    message: message.unwrap_or_default(),
                    usage,
                });
            }
            other => {
                let reason = match other {
                    StopReason::Length => DoneReason::Length,
                    StopReason::ToolUse => DoneReason::ToolUse,
                    _ => DoneReason::Stop,
                };
                events.push(AssistantEvent::Done {
                    reason,
                    usage,
                    response_id: None,
                });
            }
        }
        (events, delay)
    }
}

impl LlmSession for ScriptedSession {
    fn settings(&self) -> &Settings {
        &self.settings
    }

    fn respond(
        &mut self,
        transcript: &[Message],
        timestamp: Timestamp,
    ) -> EventStream {
        self.model.record(Request {
            settings: self.settings.clone(),
            transcript: transcript.to_vec(),
        });
        let response_id = self.model.next_response_id();
        let transcript_text = serialize_transcript(transcript);
        let resolved = self.resolve(transcript);
        let (events, delay) = self.materialize(
            resolved,
            &transcript_text,
            timestamp,
            response_id,
        );
        to_stream(events, delay)
    }
}

/// Turns `events` into a stream, sleeping once for `delay` (if any) before
/// the first event. Dropping the stream at any point is safe: it holds no
/// lock and no other resource across an `.await`.
fn to_stream(
    events: Vec<AssistantEvent>,
    delay: Option<Duration>,
) -> EventStream {
    stream::unfold(
        (events.into_iter(), delay),
        |(mut events, mut delay)| async move {
            if let Some(delay) = delay.take() {
                tokio::time::sleep(delay).await;
            }
            events.next().map(|event| (event, (events, delay)))
        },
    )
    .boxed()
}

/// Splits `text` into up to 3 deltas, deterministically. Concatenating the
/// result always reproduces `text`.
fn chunk_text(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return vec![String::new()];
    }
    let chunk_size = chars.len().div_ceil(3);
    chars
        .chunks(chunk_size)
        .map(|chunk| chunk.iter().collect())
        .collect()
}

/// `ceil(chars / 4)`, pi's `estimateTokens` (`packages/ai/src/providers/faux.ts`),
/// counting Unicode scalar values rather than UTF-16 code units.
fn estimate_tokens(text: &str) -> u64 {
    (text.chars().count() as u64).div_ceil(4)
}

/// The longest common prefix of `previous` and `current`, and the rest of
/// `current` after it, split at char boundaries (pi's `commonPrefixLength`).
fn common_prefix_split(previous: &str, current: &str) -> (String, String) {
    let previous: Vec<char> = previous.chars().collect();
    let current: Vec<char> = current.chars().collect();
    let common = previous
        .iter()
        .zip(current.iter())
        .take_while(|(a, b)| a == b)
        .count();
    (
        previous[..common].iter().collect(),
        current[common..].iter().collect(),
    )
}

fn input_block_text(block: &InputBlock) -> String {
    match block {
        InputBlock::Text(content) => content.text.clone(),
        InputBlock::Image(image) => {
            format!("[image:{}:{}]", image.mime_type, image.data.len())
        }
    }
}

fn user_content_text(content: &UserContent) -> String {
    match content {
        UserContent::Text(text) => text.clone(),
        UserContent::Blocks(blocks) => blocks
            .iter()
            .map(input_block_text)
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn assistant_content_text(blocks: &[AssistantBlock]) -> String {
    blocks
        .iter()
        .map(|block| match block {
            AssistantBlock::Text(content) => content.text.clone(),
            AssistantBlock::Thinking(content) => content.thinking.clone(),
            AssistantBlock::ToolCall(call) => format!(
                "{}:{}",
                call.name,
                serde_json::to_string(&call.arguments).unwrap_or_default()
            ),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn tool_result_text(message: &ToolResultMessage) -> String {
    std::iter::once(message.tool_name.clone())
        .chain(message.content.iter().map(input_block_text))
        .collect::<Vec<_>>()
        .join("\n")
}

fn message_text(message: &Message) -> String {
    match message {
        Message::User(user) => user_content_text(&user.content),
        Message::Assistant(assistant) => {
            assistant_content_text(&assistant.content)
        }
        Message::ToolResult(result) => tool_result_text(result),
    }
}

/// Ported from pi's `serializeContext` (`packages/ai/src/providers/faux.ts`):
/// `"{role}:{text}"` per message, joined by a blank line.
fn serialize_transcript(transcript: &[Message]) -> String {
    transcript
        .iter()
        .map(|message| format!("{}:{}", message.role(), message_text(message)))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Builds one scripted turn. See [`ScriptedModel::turn`].
pub struct TurnBuilder {
    blocks: Vec<AssistantBlock>,
    stop: Option<StopReason>,
    usage: Option<(u64, u64)>,
    cost: f64,
    delay: Option<Duration>,
    special: Option<Special>,
    tool_call_ids: Arc<AtomicU64>,
}

enum Special {
    Error(String),
    Dropped,
    FailsBeforeStart,
}

impl TurnBuilder {
    fn new(tool_call_ids: Arc<AtomicU64>) -> Self {
        Self {
            blocks: Vec::new(),
            stop: None,
            usage: None,
            cost: 0.0,
            delay: None,
            special: None,
            tool_call_ids,
        }
    }

    /// Appends a text block.
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.blocks.push(AssistantBlock::Text(TextContent {
            text: text.into(),
            text_signature: None,
        }));
        self
    }

    /// Appends a thinking block.
    pub fn thinking(mut self, thinking: impl Into<String>) -> Self {
        self.blocks.push(AssistantBlock::Thinking(ThinkingContent {
            thinking: thinking.into(),
            thinking_signature: None,
            redacted: None,
        }));
        self
    }

    /// Appends a tool call. Its id is `call_<n>|fc_<n>`, unique across
    /// every turn built from the same [`ScriptedModel`]. `arguments` must
    /// be a JSON object.
    pub fn tool_call(
        mut self,
        name: impl Into<String>,
        arguments: Value,
    ) -> Self {
        let Value::Object(arguments) = arguments else {
            panic!(
                "ScriptedModel: tool_call arguments must be a JSON object, got {arguments}"
            );
        };
        let n = self.tool_call_ids.fetch_add(1, Ordering::Relaxed);
        self.blocks.push(AssistantBlock::ToolCall(ToolCall {
            id: format!("call_{n}|fc_{n}"),
            name: name.into(),
            arguments,
        }));
        self
    }

    /// Overrides the turn's `(input, output)` token counts. Without this,
    /// they are estimated as `chars / 4` of the serialized transcript and
    /// of the turn's own content, as pi's faux provider does.
    /// `cache_read`/`cache_write` are always computed separately from the
    /// transcript (see the module docs' "Cache simulation") and layered on
    /// top of whichever `input` this gives.
    pub fn usage(mut self, input: u64, output: u64) -> Self {
        self.usage = Some((input, output));
        self
    }

    /// Sets the turn's cost in USD, reported as `usage.cost.output` and
    /// `usage.cost.total`. Without this, a turn costs nothing.
    pub fn cost(mut self, usd: f64) -> Self {
        self.cost = usd;
        self
    }

    /// Overrides the stop reason. Without this, a turn with at least one
    /// tool call stops with `ToolUse`, and any other turn stops with
    /// `Stop`.
    pub fn stop(mut self, reason: StopReason) -> Self {
        self.stop = Some(reason);
        self
    }

    /// Sleeps for `delay` (via `tokio::time::sleep`) before the stream's
    /// first event.
    pub fn delay(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
        self
    }

    /// Scripts a failed response: `Start`, then `Error` with message
    /// `"{code}: {message}"`. Any content already added is discarded.
    pub fn error(
        mut self,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        self.special = Some(Special::Error(format!(
            "{}: {}",
            code.into(),
            message.into()
        )));
        self
    }

    /// Scripts a connection dropped mid-stream: `Start`, then `Error` with
    /// message [`DROPPED_MESSAGE`]. Any content already added is discarded.
    pub fn dropped(mut self) -> Self {
        self.special = Some(Special::Dropped);
        self
    }

    /// Scripts a connection that never got going: `Start`, then `Error`
    /// with message [`FAILS_BEFORE_START_MESSAGE`], with no content in
    /// between. This still opens with `Start`, so the stream keeps the
    /// grammar in `tau_ai::event`.
    pub fn fails_before_start(mut self) -> Self {
        self.special = Some(Special::FailsBeforeStart);
        self
    }

    fn build(self) -> StaticTurn {
        let usage = self.usage;
        let cost = self.cost;
        let delay = self.delay;
        match self.special {
            Some(Special::Error(message)) => StaticTurn {
                blocks: Vec::new(),
                stop: self.stop.unwrap_or(StopReason::Error),
                message: Some(message),
                usage,
                cost,
                delay,
            },
            Some(Special::Dropped) => StaticTurn {
                blocks: Vec::new(),
                stop: self.stop.unwrap_or(StopReason::Error),
                message: Some(DROPPED_MESSAGE.to_owned()),
                usage,
                cost,
                delay,
            },
            Some(Special::FailsBeforeStart) => StaticTurn {
                blocks: Vec::new(),
                stop: self.stop.unwrap_or(StopReason::Error),
                message: Some(FAILS_BEFORE_START_MESSAGE.to_owned()),
                usage,
                cost,
                delay,
            },
            None => {
                let has_tool_call = self
                    .blocks
                    .iter()
                    .any(|block| matches!(block, AssistantBlock::ToolCall(_)));
                let stop = self.stop.unwrap_or(if has_tool_call {
                    StopReason::ToolUse
                } else {
                    StopReason::Stop
                });
                StaticTurn {
                    blocks: self.blocks,
                    stop,
                    message: None,
                    usage,
                    cost,
                    delay,
                }
            }
        }
    }
}
