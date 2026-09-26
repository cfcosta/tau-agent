//! Context compaction: the I/O-free core.
//!
//! Ported from pi's `packages/coding-agent/src/core/compaction`
//! (`compaction.ts`, `utils.ts`; `branch-summarization.ts` is a
//! different feature and is not ported here). See
//! `docs/reference/compaction.md` for the rules this module implements.
//!
//! Everything here is a pure function or a plain data type: no store, no
//! LLM request, no async. A caller (the agent loop) decides when to run
//! these, sends the summary request itself, and persists the result.
//! Compaction is off by default; a run only reaches this module when it
//! is configured with a [`Compaction`].
//!
//! ## Deviations from pi
//!
//! - **Token anchor.** The anchor is pi's `calculateContextTokens`:
//!   `usage.totalTokens`, or when that is zero, `input + output +
//!   cache_read + cache_write`. Cached tokens are part of the context,
//!   so leaving them out would undercount it.
//! - **Character counting.** pi's `estimateTokens` counts JS string
//!   `.length` (UTF-16 code units). This module counts Unicode scalar
//!   values (`str::chars().count()`), which differs only for characters
//!   outside the Basic Multilingual Plane (pi counts those as 2, this
//!   counts them as 1). Both are heuristics for a `chars / 4` estimate,
//!   so the difference does not change which side of a threshold a
//!   realistic transcript falls on.
//! - **Cut point over a flat transcript.** pi's `findCutPoint` walks a
//!   list of session *entries*, some of which (system-prompt changes,
//!   thinking-level changes, ...) carry no context-visible message at
//!   all, so it has a second pass that walks backward from the chosen
//!   cut to reabsorb any such metadata-only entries immediately before
//!   it. tau-agent's transcript is a flat `&[Message]` with no
//!   metadata-only entries, so every element is context-visible and
//!   that pass is unnecessary here.
//! - **Rejected-summary labels.** pi's `getSummarizationFailure` takes a
//!   `label` argument and produces "Turn prefix summarization failed:
//!   ..." for the split-turn prefix request. [`check_summary`] always
//!   uses the plain "Summarization failed: ..." wording the doc pins,
//!   for both requests.
//! - **File-operation sets.** pi tracks read/written/edited paths in
//!   `Set<string>` and sorts them into a list when needed.
//!   [`FileOperations`] uses a `BTreeSet`, which keeps the same
//!   deduplicated, sorted result without a separate sort step.

use std::{collections::BTreeSet, fmt};

use serde_json::Value;
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

/// A tool result is serialized with at most this many characters before
/// being cut off (`docs/reference/compaction.md`, "Input").
const TOOL_RESULT_MAX_CHARS: usize = 2000;

// ============================================================================
// Settings
// ============================================================================

/// Compaction thresholds (`docs/reference/compaction.md`).
///
/// Compaction is off by default: a run only compacts when it is built
/// with `Agent::compaction(Compaction::default())` (or a customized
/// value). This type carries no "enabled" flag; the caller decides
/// whether compaction runs at all by whether it holds one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Compaction {
    /// Tokens reserved for the model's response and the next request's
    /// growth. Compaction triggers once the estimate passes
    /// `context_window - reserve_tokens`. Defaults to 16,384.
    pub reserve_tokens: u64,
    /// The minimum number of trailing tokens compaction tries to keep
    /// verbatim, uncompacted. Defaults to 20,000.
    pub keep_recent_tokens: u64,
}

impl Default for Compaction {
    fn default() -> Self {
        Self {
            reserve_tokens: 16_384,
            keep_recent_tokens: 20_000,
        }
    }
}

impl Compaction {
    pub fn reserve_tokens(mut self, reserve_tokens: u64) -> Self {
        self.reserve_tokens = reserve_tokens;
        self
    }

    pub fn keep_recent_tokens(mut self, keep_recent_tokens: u64) -> Self {
        self.keep_recent_tokens = keep_recent_tokens;
        self
    }
}

// ============================================================================
// Token estimation
// ============================================================================

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

/// Whether compaction should run (`docs/reference/compaction.md`, "When
/// it triggers"): the estimate has passed `context_window -
/// reserve_tokens`.
pub fn should_compact(
    tokens: u64,
    context_window: u64,
    compaction: &Compaction,
) -> bool {
    tokens > context_window.saturating_sub(compaction.reserve_tokens)
}

// ============================================================================
// Cut point
// ============================================================================

/// A tool result must never be a cut point: it must always follow its
/// call. Every other message is a valid boundary.
fn is_cut_point_message(message: &Message) -> bool {
    !matches!(message, Message::ToolResult(_))
}

/// Only a user message starts a new turn. (pi additionally treats a few
/// other entry kinds — `bashExecution`, `custom`, `branchSummary`,
/// `compactionSummary` — as turn starts; tau-agent's `Message` has no
/// equivalents, so `User` is the only case.)
fn is_turn_start_message(message: &Message) -> bool {
    matches!(message, Message::User(_))
}

/// The result of [`find_cut_point`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CutPoint {
    /// Index of the first message to keep verbatim.
    pub first_kept_index: usize,
    /// The index of the user message that starts the turn being split,
    /// when [`Self::is_split_turn`] is `true`.
    pub turn_start_index: Option<usize>,
    /// Whether `first_kept_index` falls inside a turn rather than at
    /// its start, so the turn's prefix needs its own summary
    /// (`docs/reference/compaction.md`, "Cut point", step 3).
    pub is_split_turn: bool,
}

/// The user message that starts the turn containing `index` (pi's
/// `findTurnStartIndex`): the nearest user message at or before `index`.
fn find_turn_start_index(messages: &[Message], index: usize) -> Option<usize> {
    (0..=index)
        .rev()
        .find(|&i| is_turn_start_message(&messages[i]))
}

/// Finds where to cut `messages` so that at least `keep_recent_tokens`
/// worth of trailing content stays verbatim
/// (`docs/reference/compaction.md`, "Cut point"): walk backward from the
/// newest message accumulating estimated tokens, then snap to the
/// nearest valid boundary — never between a tool call and its result,
/// never on a tool result itself (pi's `findCutPoint`).
///
/// If no valid boundary exists at or after the point where the budget
/// was reached (typically an oversized trailing tool result with
/// nothing after it), this falls back to the latest valid boundary
/// overall, which keeps *more* than `keep_recent_tokens` rather than
/// splitting a call from its result (pi issue #9740).
pub fn find_cut_point(
    messages: &[Message],
    keep_recent_tokens: u64,
) -> CutPoint {
    let cut_points: Vec<usize> = (0..messages.len())
        .filter(|&i| is_cut_point_message(&messages[i]))
        .collect();

    let Some(&first_cut_point) = cut_points.first() else {
        return CutPoint {
            first_kept_index: 0,
            turn_start_index: None,
            is_split_turn: false,
        };
    };

    let mut accumulated = 0u64;
    let mut cut_index = first_cut_point;
    for i in (0..messages.len()).rev() {
        let tokens = estimate_message_tokens(&messages[i]);
        if tokens == 0 {
            continue;
        }
        accumulated += tokens;
        if accumulated >= keep_recent_tokens {
            cut_index = cut_points
                .iter()
                .copied()
                .find(|&candidate| candidate >= i)
                .unwrap_or(
                    *cut_points.last().expect("cut_points is non-empty"),
                );
            break;
        }
    }

    let starts_turn = is_turn_start_message(&messages[cut_index]);
    let turn_start_index = if starts_turn {
        None
    } else {
        find_turn_start_index(messages, cut_index)
    };
    CutPoint {
        first_kept_index: cut_index,
        turn_start_index,
        is_split_turn: !starts_turn && turn_start_index.is_some(),
    }
}

// ============================================================================
// Serialization for the summary request
// ============================================================================

/// Truncates `text` to `max_chars` characters, appending pi's truncation
/// marker (`docs/reference/compaction.md`, "Input").
fn truncate_for_summary(text: &str, max_chars: usize) -> String {
    let total_chars = text.chars().count();
    if total_chars <= max_chars {
        return text.to_owned();
    }
    let truncated_chars = total_chars - max_chars;
    let prefix: String = text.chars().take(max_chars).collect();
    format!("{prefix}\n\n[... {truncated_chars} more characters truncated]")
}

/// Text-only content of `blocks`, concatenated with no separator (pi's
/// `contentText(content, "")`; images contribute nothing here).
fn input_blocks_text(blocks: &[InputBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            InputBlock::Text(content) => Some(content.text.as_str()),
            InputBlock::Image(_) => None,
        })
        .collect()
}

fn user_content_text(content: &UserContent) -> String {
    match content {
        UserContent::Text(text) => text.clone(),
        UserContent::Blocks(blocks) => input_blocks_text(blocks),
    }
}

/// Serializes `messages` into the flat, non-conversational text that
/// goes inside `<conversation>` tags (`docs/reference/compaction.md`,
/// "Input"; pi's `serializeConversation`). Tool results are truncated to
/// their first 2,000 characters.
pub fn serialize_conversation(messages: &[Message]) -> String {
    let mut parts = Vec::new();

    for message in messages {
        match message {
            Message::User(message) => {
                let text = user_content_text(&message.content);
                if !text.is_empty() {
                    parts.push(format!("[User]: {text}"));
                }
            }
            Message::Assistant(message) => {
                let thinking: Vec<&str> = message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        AssistantBlock::Thinking(content) => {
                            Some(content.thinking.as_str())
                        }
                        _ => None,
                    })
                    .collect();
                if !thinking.is_empty() {
                    parts.push(format!(
                        "[Assistant thinking]: {}",
                        thinking.join("\n")
                    ));
                }

                let has_text = message
                    .content
                    .iter()
                    .any(|block| matches!(block, AssistantBlock::Text(_)));
                if has_text {
                    let text: Vec<&str> = message
                        .content
                        .iter()
                        .filter_map(|block| match block {
                            AssistantBlock::Text(content) => {
                                Some(content.text.as_str())
                            }
                            _ => None,
                        })
                        .collect();
                    parts.push(format!("[Assistant]: {}", text.join("\n")));
                }

                let tool_calls: Vec<String> = message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        AssistantBlock::ToolCall(call) => {
                            let arguments = call
                                .arguments
                                .iter()
                                .map(|(key, value)| {
                                    let value = serde_json::to_string(value)
                                        .unwrap_or_else(|_| "null".to_owned());
                                    format!("{key}={value}")
                                })
                                .collect::<Vec<_>>()
                                .join(", ");
                            Some(format!("{}({arguments})", call.name))
                        }
                        _ => None,
                    })
                    .collect();
                if !tool_calls.is_empty() {
                    parts.push(format!(
                        "[Assistant tool calls]: {}",
                        tool_calls.join("; ")
                    ));
                }
            }
            Message::ToolResult(message) => {
                let text = input_blocks_text(&message.content);
                if !text.is_empty() {
                    parts.push(format!(
                        "[Tool result]: {}",
                        truncate_for_summary(&text, TOOL_RESULT_MAX_CHARS)
                    ));
                }
            }
        }
    }

    parts.join("\n\n")
}

// ============================================================================
// File-operation tracking
// ============================================================================

/// Files a tool call touched, tracked across compactions
/// (`docs/reference/compaction.md`, "File lists"; pi's
/// `FileOperations`/`extractFileOpsFromMessage`/`computeFileLists`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileOperations {
    pub read: BTreeSet<String>,
    pub written: BTreeSet<String>,
    pub edited: BTreeSet<String>,
}

impl FileOperations {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the `path` argument of any `read`/`write`/`edit` tool
    /// call in `message`. Every other message is ignored.
    pub fn extract_from_message(&mut self, message: &Message) {
        let Message::Assistant(assistant) = message else {
            return;
        };
        for block in &assistant.content {
            let AssistantBlock::ToolCall(call) = block else {
                continue;
            };
            let Some(path) = call.arguments.get("path").and_then(Value::as_str)
            else {
                continue;
            };
            match call.name.as_str() {
                "read" => {
                    self.read.insert(path.to_owned());
                }
                "write" => {
                    self.written.insert(path.to_owned());
                }
                "edit" => {
                    self.edited.insert(path.to_owned());
                }
                _ => {}
            }
        }
    }

    /// [`Self::extract_from_message`] over every message in `messages`.
    pub fn extract_from_messages(&mut self, messages: &[Message]) {
        for message in messages {
            self.extract_from_message(message);
        }
    }

    /// The final read-only and modified file lists (pi's
    /// `computeFileLists`): files that were only read (never written or
    /// edited), and every written or edited file, each sorted.
    pub fn file_lists(&self) -> (Vec<String>, Vec<String>) {
        let modified: BTreeSet<&String> =
            self.edited.iter().chain(self.written.iter()).collect();
        let read_only = self
            .read
            .iter()
            .filter(|path| !modified.contains(path))
            .cloned()
            .collect();
        let modified_files = modified.into_iter().cloned().collect();
        (read_only, modified_files)
    }
}

/// Renders file lists as the XML-ish tags appended to a summary (pi's
/// `formatFileOperations`). Returns `""` when both lists are empty.
pub fn format_file_operations(
    read_files: &[String],
    modified_files: &[String],
) -> String {
    let mut sections = Vec::new();
    if !read_files.is_empty() {
        sections.push(format!(
            "<read-files>\n{}\n</read-files>",
            read_files.join("\n")
        ));
    }
    if !modified_files.is_empty() {
        sections.push(format!(
            "<modified-files>\n{}\n</modified-files>",
            modified_files.join("\n")
        ));
    }
    if sections.is_empty() {
        return String::new();
    }
    format!("\n\n{}", sections.join("\n\n"))
}

// ============================================================================
// Prompts (verbatim from pi)
// ============================================================================

/// The system prompt for a summarization request (pi's
/// `SUMMARIZATION_SYSTEM_PROMPT`).
pub const SUMMARIZATION_SYSTEM_PROMPT: &str = "You are a context summarization assistant. Your task is to read a conversation between a user and an AI assistant, then produce a structured summary following the exact format specified.

Do NOT continue the conversation. Do NOT respond to any questions in the conversation. ONLY output the structured summary.";

/// The initial summarization prompt, used when there is no prior summary
/// (pi's `SUMMARIZATION_PROMPT`).
pub const SUMMARIZATION_PROMPT: &str = "The messages above are a conversation to summarize. Create a structured context checkpoint summary that another LLM will use to continue the work.

Use this EXACT format:

## Goal
[What is the user trying to accomplish? Can be multiple items if the session covers different tasks.]

## Constraints & Preferences
- [Any constraints, preferences, or requirements mentioned by user]
- [Or \"(none)\" if none were mentioned]

## Progress
### Done
- [x] [Completed tasks/changes]

### In Progress
- [ ] [Current work]

### Blocked
- [Issues preventing progress, if any]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Next Steps
1. [Ordered list of what should happen next]

## Critical Context
- [Any data, examples, or references needed to continue]
- [Or \"(none)\" if not applicable]

Keep each section concise. Preserve exact file paths, function names, and error messages.";

/// The summarization prompt used when a prior summary exists (pi's
/// `UPDATE_SUMMARIZATION_PROMPT`, which is
/// `UPDATE_SUMMARIZATION_INSTRUCTIONS` prefixed by a short header).
pub const UPDATE_SUMMARIZATION_PROMPT: &str = "The messages above are NEW conversation messages to incorporate into the existing summary provided in <previous-summary> tags.

Update the existing structured summary with new information. RULES:
- PRESERVE all existing information from the previous summary
- ADD new progress, decisions, and context from the new messages
- UPDATE the Progress section: move items from \"In Progress\" to \"Done\" when completed
- UPDATE \"Next Steps\" based on what was accomplished
- PRESERVE exact file paths, function names, and error messages
- If something is no longer relevant, you may remove it

Use this EXACT format:

## Goal
[Preserve existing goals, add new ones if the task expanded]

## Constraints & Preferences
- [Preserve existing, add new ones discovered]

## Progress
### Done
- [x] [Include previously done items AND newly completed items]

### In Progress
- [ ] [Current work - update based on progress]

### Blocked
- [Current blockers - remove if resolved]

## Key Decisions
- **[Decision]**: [Brief rationale] (preserve all previous, add new)

## Next Steps
1. [Update based on current state]

## Critical Context
- [Preserve important context, add new if needed]

Keep each section concise. Preserve exact file paths, function names, and error messages.";

/// The prompt for a split-turn prefix summary
/// (`docs/reference/compaction.md`, "Cut point", step 3; pi's
/// `TURN_PREFIX_SUMMARIZATION_PROMPT`).
pub const TURN_PREFIX_SUMMARIZATION_PROMPT: &str = "The messages above are earlier context from an ongoing conversation. Later messages are stored separately and do not need to be reconstructed.

Create a concise checkpoint of the user's request and the progress shown above. This checkpoint will be placed before the later messages so the conversation can continue with the necessary context.

## Original Request
[What did the user ask for?]

## Progress So Far
- [Key decisions and work completed in these messages]

## Context Needed to Continue
- [Information from these messages needed to understand the later work]

Only summarize information explicitly present above. Do not infer or recreate later messages.";

// ============================================================================
// Summary request
// ============================================================================

/// Builds the summary request's user-message text
/// (`docs/reference/compaction.md`, "Input" and "Prior summary"): the
/// messages to summarize, serialized and wrapped in `<conversation>`;
/// the prior summary, if any, wrapped in `<previous-summary>`; then the
/// initial or "update" prompt, with an optional instructions suffix
/// (pi's `generateSummaryWithUsage`).
pub fn build_summary_request(
    messages: &[Message],
    previous_summary: Option<&str>,
    custom_instructions: Option<&str>,
) -> String {
    let mut prompt = if previous_summary.is_some() {
        UPDATE_SUMMARIZATION_PROMPT.to_owned()
    } else {
        SUMMARIZATION_PROMPT.to_owned()
    };
    if let Some(instructions) = custom_instructions {
        prompt = format!("{prompt}\n\nAdditional focus: {instructions}");
    }

    let conversation = serialize_conversation(messages);
    let mut request =
        format!("<conversation>\n{conversation}\n</conversation>\n\n");
    if let Some(summary) = previous_summary {
        request.push_str(&format!(
            "<previous-summary>\n{summary}\n</previous-summary>\n\n"
        ));
    }
    request.push_str(&prompt);
    request
}

/// Builds a split-turn prefix summary request (pi's
/// `generateTurnPrefixSummary`): the turn's messages so far, serialized,
/// followed by [`TURN_PREFIX_SUMMARIZATION_PROMPT`].
pub fn build_turn_prefix_summary_request(messages: &[Message]) -> String {
    let conversation = serialize_conversation(messages);
    format!(
        "# Conversation\n{conversation}\n\n# Instructions\n{TURN_PREFIX_SUMMARIZATION_PROMPT}"
    )
}

/// Merges a history summary with a split-turn prefix summary into the
/// single summary text a compaction record holds (pi's `compact`).
pub fn merge_split_turn_summary(history: &str, turn_prefix: &str) -> String {
    format!(
        "{history}\n\n---\n\n**Turn Context (split turn):**\n\n{turn_prefix}"
    )
}

/// The largest `max_output_tokens` a summary request may ask for
/// (`docs/reference/compaction.md`, "Output limit"): the smaller of
/// `0.8 * reserve_tokens` and the model's maximum output tokens. A
/// `model_max_output_tokens` of `0` means the model has no documented
/// cap (pi's `model.maxTokens > 0 ? model.maxTokens : Infinity`).
pub fn summary_max_output_tokens(
    reserve_tokens: u64,
    model_max_output_tokens: u64,
) -> u64 {
    capped_budget(0.8, reserve_tokens, model_max_output_tokens)
}

/// The largest `max_output_tokens` a split-turn prefix summary may ask
/// for: pi budgets it at half the reserve, not 0.8 of it.
pub fn turn_prefix_max_output_tokens(
    reserve_tokens: u64,
    model_max_output_tokens: u64,
) -> u64 {
    capped_budget(0.5, reserve_tokens, model_max_output_tokens)
}

fn capped_budget(share: f64, reserve_tokens: u64, model_max: u64) -> u64 {
    let budget = (share * reserve_tokens as f64).floor() as u64;
    if model_max == 0 {
        budget
    } else {
        budget.min(model_max)
    }
}

// ============================================================================
// Rejected summaries
// ============================================================================

/// Why a summarization response could not become a checkpoint
/// (`docs/reference/compaction.md`, "Rejected summaries").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionError(String);

impl fmt::Display for CompactionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CompactionError {}

/// Validates a summarization response and extracts its text
/// (`docs/reference/compaction.md`, "Rejected summaries"; pi's
/// `getSummarizationFailure` plus the tool-call check in
/// `generateSummaryWithUsage`). Compaction must write nothing when this
/// returns `Err`.
pub fn check_summary(
    response: &AssistantMessage,
) -> Result<String, CompactionError> {
    match response.stop_reason {
        StopReason::Error => {
            let message =
                response.error_message.as_deref().unwrap_or("Unknown error");
            return Err(CompactionError(format!(
                "Summarization failed: {message}"
            )));
        }
        StopReason::Length => {
            return Err(CompactionError(
                "Summarization failed: generation hit the token cap and \
                 the summary is incomplete"
                    .to_owned(),
            ));
        }
        StopReason::Stop | StopReason::ToolUse | StopReason::Aborted => {}
    }

    if response
        .content
        .iter()
        .any(|block| matches!(block, AssistantBlock::ToolCall(_)))
    {
        return Err(CompactionError(
            "Summarization attempted to call a tool".to_owned(),
        ));
    }

    let text: Vec<&str> = response
        .content
        .iter()
        .filter_map(|block| match block {
            AssistantBlock::Text(content) => Some(content.text.as_str()),
            _ => None,
        })
        .collect();
    Ok(text.join("\n"))
}
