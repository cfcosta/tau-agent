//! Context compaction (`tau_agent::compaction`), the I/O-free core.
//!
//! Oracle for the rules: `docs/reference/compaction.md`, and pi's
//! `packages/coding-agent/src/core/compaction/{compaction,utils}.ts`.
//! See `docs/reference/testing.md`'s `tau-agent` property inventory for
//! the rows this file covers.

use std::collections::VecDeque;

use hegel::{TestCase, generators as gs};
use serde_json::{Map, json};
use tau_agent::compaction::{
    Compaction,
    CutPoint,
    FileOperations,
    Plan,
    SUMMARIZATION_PROMPT,
    TURN_PREFIX_SUMMARIZATION_PROMPT,
    UPDATE_SUMMARIZATION_PROMPT,
    build_summary_request,
    build_turn_prefix_summary_request,
    check_summary,
    estimate_context_tokens,
    estimate_message_tokens,
    find_cut_point,
    format_file_operations,
    is_context_overflow,
    merge_split_turn_summary,
    plan,
    serialize_conversation,
    should_compact,
    summary_max_output_tokens,
    turn_prefix_max_output_tokens,
};
use tau_ai::message::{
    API,
    AssistantBlock,
    AssistantMessage,
    ImageContent,
    InputBlock,
    Message,
    PROVIDER,
    StopReason,
    TextContent,
    ThinkingContent,
    ToolCall,
    ToolResultMessage,
    Usage,
    UsageCost,
    UserContent,
    UserMessage,
};
use tau_testing::generators;

// =============================================================================
// Helpers
// =============================================================================

fn text_block(text: &str) -> AssistantBlock {
    AssistantBlock::Text(TextContent {
        text: text.to_owned(),
        text_signature: None,
    })
}

fn assistant_response(
    stop_reason: StopReason,
    error_message: Option<&str>,
    content: Vec<AssistantBlock>,
) -> AssistantMessage {
    AssistantMessage {
        content,
        api: API.to_owned(),
        provider: PROVIDER.to_owned(),
        model: "gpt-5.5".to_owned(),
        response_id: None,
        usage: Usage::default(),
        stop_reason,
        error_message: error_message.map(str::to_owned),
        timestamp: 0,
    }
}

fn user_text(text: &str) -> Message {
    Message::User(UserMessage {
        content: UserContent::Text(text.to_owned()),
        timestamp: 0,
    })
}

fn assistant_text(text: &str) -> Message {
    Message::Assistant(assistant_response(
        StopReason::Stop,
        None,
        vec![text_block(text)],
    ))
}

fn assistant_with_usage(usage: Usage) -> Message {
    Message::Assistant(AssistantMessage {
        content: vec![text_block("ok")],
        api: API.to_owned(),
        provider: PROVIDER.to_owned(),
        model: "gpt-5.5".to_owned(),
        response_id: None,
        usage,
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 0,
    })
}

/// Zeroes out every assistant message's usage, so `messages` reports no
/// usage anywhere (`docs/reference/compaction.md`, "Token estimate").
fn zero_out_usage(mut messages: Vec<Message>) -> Vec<Message> {
    for message in &mut messages {
        if let Message::Assistant(assistant) = message {
            assistant.usage = Usage::default();
        }
    }
    messages
}

/// Rewrites every assistant message's usage to grow with the transcript:
/// each one reports at least as many tokens as
/// [`estimate_context_tokens`] would estimate for everything up to and
/// including it. This mirrors how a real API's `usage.input` behaves
/// (it is the size of everything sent so far, which only grows for a
/// transcript that never shrinks), and is what makes
/// [`token_estimate_never_decreases_when_a_message_is_appended`] provable
/// rather than incidentally true: [`estimate_context_tokens`] only ever
/// *replaces* its running total with a fresh assistant message's own
/// reported usage, so that replacement must not read as a decrease.
fn with_growing_usage(mut messages: Vec<Message>) -> Vec<Message> {
    let mut estimate_so_far = 0u64;
    for message in &mut messages {
        let tokens = estimate_message_tokens(message);
        if let Message::Assistant(assistant) = message {
            let input = estimate_so_far + tokens + 1;
            assistant.usage = Usage {
                input,
                output: 0,
                cache_read: 0,
                cache_write: 0,
                reasoning: None,
                total_tokens: input,
                cost: UsageCost::default(),
            };
            estimate_so_far = input;
        } else {
            estimate_so_far += tokens;
        }
    }
    messages
}

// =============================================================================
// Token estimation (testing.md: "never decreases", "no reported usage")
// =============================================================================

/// The token estimate never decreases when a message is appended
/// (`docs/reference/testing.md`, tau-agent property inventory).
#[hegel::test(test_cases = 300)]
fn token_estimate_never_decreases_when_a_message_is_appended(tc: TestCase) {
    let messages = with_growing_usage(tc.draw(generators::transcript()));
    let mut previous = 0u64;
    for n in 1..=messages.len() {
        let estimate = estimate_context_tokens(&messages[..n]);
        assert!(
            estimate >= previous,
            "estimate dropped from {previous} to {estimate} after message {n}"
        );
        previous = estimate;
    }
}

/// With no reported usage anywhere, the estimate is `chars / 4` over
/// every message (`docs/reference/testing.md`; `docs/reference/compaction.md`,
/// "Token estimate").
#[hegel::test(test_cases = 500)]
fn no_reported_usage_estimate_is_chars_over_four_everywhere(tc: TestCase) {
    let messages = zero_out_usage(tc.draw(generators::transcript()));
    let expected: u64 = messages.iter().map(estimate_message_tokens).sum();
    assert_eq!(estimate_context_tokens(&messages), expected);
}

/// pi issue #8328: an assistant message whose usage is present but
/// all-zero must not be mistaken for "no usage reported" being false —
/// it still falls back to the chars/4 estimate, not to zero.
#[test]
fn pi_known_case_zero_usage_assistant_falls_back_to_chars_over_four() {
    let messages =
        vec![user_text(&"x".repeat(400)), assistant_text("response")];
    let expected: u64 = messages.iter().map(estimate_message_tokens).sum();
    assert_eq!(estimate_context_tokens(&messages), expected);
    assert!(expected > 0);
}

/// `should_compact` is exactly `tokens > context_window - reserve_tokens`,
/// saturating so a reserve larger than the window never underflows.
#[hegel::test(test_cases = 300)]
fn should_compact_matches_the_threshold_formula(tc: TestCase) {
    let tokens = tc.draw(gs::integers::<u64>().max_value(1_000_000));
    let context_window = tc.draw(gs::integers::<u64>().max_value(1_000_000));
    let reserve_tokens = tc.draw(gs::integers::<u64>().max_value(1_000_000));
    let compaction = tau_agent::compaction::Compaction::default()
        .reserve_tokens(reserve_tokens);
    let expected = tokens > context_window.saturating_sub(reserve_tokens);
    assert_eq!(
        should_compact(tokens, context_window, &compaction),
        expected
    );
}

// =============================================================================
// Cut point (testing.md: "never splits a tool call", "kept suffix")
// =============================================================================

/// Compaction's cut point never falls between a tool call and its
/// result, and never lands on a tool result itself
/// (`docs/reference/testing.md`; `docs/reference/compaction.md`, "Cut
/// point", step 2).
#[hegel::test(test_cases = 500)]
fn cut_point_never_splits_a_tool_call_from_its_result(tc: TestCase) {
    let messages = tc.draw(generators::transcript());
    let keep_recent_tokens = tc.draw(gs::integers::<u64>().max_value(50_000));
    let cut = find_cut_point(&messages, keep_recent_tokens);

    assert!(
        !matches!(messages[cut.first_kept_index], Message::ToolResult(_)),
        "cut point {} landed on a tool result",
        cut.first_kept_index
    );

    // Every tool call and its result must land on the same side of the
    // cut: both kept, or both dropped. Matched in generation order
    // (FIFO) rather than by id: `tau_testing::generators::tool_call`
    // draws its id independently per call, so two unrelated calls in
    // the same transcript can legitimately share an id.
    let mut pending_calls: VecDeque<bool> = VecDeque::new();
    for (i, message) in messages.iter().enumerate() {
        match message {
            Message::Assistant(assistant) => {
                for block in &assistant.content {
                    if matches!(block, AssistantBlock::ToolCall(_)) {
                        pending_calls.push_back(i >= cut.first_kept_index);
                    }
                }
            }
            Message::ToolResult(_) => {
                if let Some(call_kept) = pending_calls.pop_front() {
                    assert_eq!(
                        call_kept,
                        i >= cut.first_kept_index,
                        "a tool call and its result at index {i} are on \
                         different sides of the cut"
                    );
                }
            }
            Message::User(_) => {}
        }
    }
}

/// The unsnapped trigger point `find_cut_point`'s own backward walk
/// computes before it snaps to a valid boundary: the largest suffix
/// start index whose estimated tokens already reach
/// `keep_recent_tokens`, ignoring whether that index is itself a valid
/// cut point. Used only to state
/// [`kept_suffix_holds_at_least_keep_recent_tokens_unless_pulled_forward`]
/// precisely.
fn naive_trigger_index(messages: &[Message], keep_recent_tokens: u64) -> usize {
    let mut accumulated = 0u64;
    for i in (0..messages.len()).rev() {
        let tokens = estimate_message_tokens(&messages[i]);
        if tokens == 0 {
            continue;
        }
        accumulated += tokens;
        if accumulated >= keep_recent_tokens {
            return i;
        }
    }
    0
}

/// The kept suffix holds at least `keep_recent_tokens`, unless the whole
/// transcript holds fewer (`docs/reference/testing.md`;
/// `docs/reference/compaction.md`, "Cut point", step 1) — **whenever the
/// cut is not pulled forward past the point where the budget was
/// reached**. pi's own snapping rule (`compaction.ts`'s comment: "Prefer
/// the closest valid cut point at or after this entry") can choose a
/// boundary *later* than that point when nothing valid sits at or before
/// it, e.g. an oversized tool result followed immediately by a small
/// final assistant turn with no further tool call. That later boundary
/// then keeps less than `keep_recent_tokens`, on purpose: the excluded
/// content is exactly what the turn-prefix/history summary is meant to
/// absorb, not data quietly lost. See
/// [`regression_cut_can_be_pulled_forward_below_the_budget`] for that
/// exception pinned as a concrete case (found by this property during
/// the port, not a numbered pi issue).
#[hegel::test(test_cases = 500)]
fn kept_suffix_holds_at_least_keep_recent_tokens_unless_pulled_forward(
    tc: TestCase,
) {
    let messages = tc.draw(generators::transcript());
    let keep_recent_tokens = tc.draw(gs::integers::<u64>().max_value(50_000));
    let cut = find_cut_point(&messages, keep_recent_tokens);
    let trigger = naive_trigger_index(&messages, keep_recent_tokens);
    let total_tokens: u64 = messages.iter().map(estimate_message_tokens).sum();

    if cut.first_kept_index <= trigger {
        let kept_tokens: u64 = messages[cut.first_kept_index..]
            .iter()
            .map(estimate_message_tokens)
            .sum();
        assert!(
            kept_tokens >= keep_recent_tokens.min(total_tokens),
            "kept {kept_tokens} tokens from index {}, wanted at least {}",
            cut.first_kept_index,
            keep_recent_tokens.min(total_tokens)
        );
    } else {
        // The cut was pulled forward past the trigger point: every
        // message strictly between them must be an invalid boundary (a
        // tool result), since otherwise `find_cut_point` would have
        // chosen that nearer boundary instead.
        for (i, message) in messages
            .iter()
            .enumerate()
            .take(cut.first_kept_index)
            .skip(trigger)
        {
            assert!(
                matches!(message, Message::ToolResult(_)),
                "message {i} between the trigger ({trigger}) and the cut \
                 ({}) is a valid boundary that should have been chosen \
                 instead",
                cut.first_kept_index
            );
        }
    }
}

/// Found while writing
/// [`kept_suffix_holds_at_least_keep_recent_tokens_unless_pulled_forward`]:
/// an oversized tool result that sits *before* the transcript's last
/// message (not just trailing, as in pi issue #9740) can still force the
/// cut forward past it, past the small final assistant turn that follows
/// it, ending up with a kept suffix far below `keep_recent_tokens`. This
/// is the mirror image of #9740: there, nothing valid exists *after* the
/// oversized content, so the cut falls back *earlier* (keeping more);
/// here, a valid boundary exists right after it, so the cut moves
/// *later* (keeping less). Both are the same rule
/// (`docs/reference/compaction.md`, "Cut point", step 2: snap to the
/// nearest valid boundary at or after the trigger, or the latest one
/// overall) — this case just lands on the other side of it.
#[test]
fn regression_cut_can_be_pulled_forward_below_the_budget() {
    let messages = vec![
        user_text("please read this file"),
        Message::Assistant(assistant_response(
            StopReason::ToolUse,
            None,
            vec![AssistantBlock::ToolCall(ToolCall {
                id: "call-1|fc-1".to_owned(),
                name: "read".to_owned(),
                arguments: Map::from_iter([(
                    "path".to_owned(),
                    json!("big.txt"),
                )]),
            })],
        )),
        Message::ToolResult(ToolResultMessage {
            tool_call_id: "call-1|fc-1".to_owned(),
            tool_name: "read".to_owned(),
            content: vec![InputBlock::Text(TextContent {
                text: "x".repeat(8000),
                text_signature: None,
            })],
            details: None,
            is_error: false,
            timestamp: 0,
        }),
        assistant_text("Done."),
    ];

    let cut = find_cut_point(&messages, 1000);
    // The oversized tool result (index 2) is not itself a valid
    // boundary, and the next one (index 3, the final "Done.") is; the
    // cut is pulled all the way forward to it.
    assert_eq!(
        cut,
        CutPoint {
            first_kept_index: 3,
            turn_start_index: Some(0),
            is_split_turn: true,
        }
    );
    let kept_tokens: u64 = messages[cut.first_kept_index..]
        .iter()
        .map(estimate_message_tokens)
        .sum();
    assert!(
        kept_tokens < 1000,
        "kept {kept_tokens} tokens, expected < 1000"
    );
}

/// A split-turn cut always names the user message that starts the turn
/// being split, and that message is at or before the cut
/// (`docs/reference/compaction.md`, "Cut point", step 3).
#[hegel::test(test_cases = 500)]
fn split_turn_names_a_turn_start_at_or_before_the_cut(tc: TestCase) {
    let messages = tc.draw(generators::transcript());
    let keep_recent_tokens = tc.draw(gs::integers::<u64>().max_value(50_000));
    let cut = find_cut_point(&messages, keep_recent_tokens);

    if cut.is_split_turn {
        let turn_start = cut
            .turn_start_index
            .expect("a split turn always names its start");
        assert!(turn_start <= cut.first_kept_index);
        assert!(matches!(messages[turn_start], Message::User(_)));
    } else {
        assert!(cut.turn_start_index.is_none());
    }
}

/// If the whole transcript's estimated tokens fit under
/// `keep_recent_tokens`, nothing is cut (`docs/reference/compaction.md`,
/// "Cut point", step 1; pi's "should keep everything if all messages fit
/// within budget").
#[hegel::test(test_cases = 300)]
fn everything_is_kept_when_the_whole_transcript_fits(tc: TestCase) {
    let messages = tc.draw(generators::transcript());
    let total: u64 = messages.iter().map(estimate_message_tokens).sum();
    let cut = find_cut_point(&messages, total + 1);
    assert_eq!(cut.first_kept_index, 0);
    assert!(!cut.is_split_turn);
}

/// pi issue #9740: an oversized trailing tool result must not force the
/// cut to land on (or split around) the result itself. The cut falls
/// back to the latest valid boundary before it — here, the tool call
/// that produced it — even though that keeps far more than
/// `keep_recent_tokens`.
#[test]
fn pi_known_case_9740_oversized_trailing_tool_result() {
    let messages = vec![
        user_text("old history"),
        assistant_text("old answer"),
        user_text("read the large file"),
        Message::Assistant(assistant_response(
            StopReason::ToolUse,
            None,
            vec![AssistantBlock::ToolCall(ToolCall {
                id: "call-1|fc-1".to_owned(),
                name: "read".to_owned(),
                arguments: Map::from_iter([(
                    "path".to_owned(),
                    json!("big.txt"),
                )]),
            })],
        )),
        Message::ToolResult(ToolResultMessage {
            tool_call_id: "call-1|fc-1".to_owned(),
            tool_name: "read".to_owned(),
            content: vec![InputBlock::Text(TextContent {
                text: "x".repeat(8000),
                text_signature: None,
            })],
            details: None,
            is_error: false,
            timestamp: 0,
        }),
    ];

    let cut = find_cut_point(&messages, 1000);
    assert_eq!(
        cut,
        CutPoint {
            first_kept_index: 3,
            turn_start_index: Some(2),
            is_split_turn: true,
        }
    );

    // What a caller would summarize vs. keep as a turn prefix, matching
    // pi's own assertions on this case (`compaction.test.ts`, the
    // "#9740" regression).
    let turn_start = cut.turn_start_index.unwrap();
    let messages_to_summarize = &messages[..turn_start];
    let turn_prefix_messages = &messages[turn_start..cut.first_kept_index];
    assert_eq!(messages_to_summarize, &messages[0..2]);
    assert_eq!(turn_prefix_messages, &messages[2..3]);
}

// =============================================================================
// Serialization (testing.md: input to the summary request)
// =============================================================================

/// A tool result longer than 2,000 characters is cut to its first 2,000
/// characters, followed by the truncation marker
/// (`docs/reference/compaction.md`, "Input").
#[test]
fn tool_result_is_truncated_to_2000_characters() {
    let long_result = "y".repeat(2500);
    let messages = vec![Message::ToolResult(ToolResultMessage {
        tool_call_id: "call-1|fc-1".to_owned(),
        tool_name: "read".to_owned(),
        content: vec![InputBlock::Text(TextContent {
            text: long_result.clone(),
            text_signature: None,
        })],
        details: None,
        is_error: false,
        timestamp: 0,
    })];
    let serialized = serialize_conversation(&messages);
    let expected = format!(
        "[Tool result]: {}\n\n[... 500 more characters truncated]",
        &long_result[..2000]
    );
    assert_eq!(serialized, expected);
}

/// A tool result within 2,000 characters is serialized verbatim, with no
/// truncation marker.
#[hegel::test(test_cases = 100)]
fn short_tool_result_is_not_truncated(tc: TestCase) {
    let text = tc.draw(generators::text(200));
    let messages = vec![Message::ToolResult(ToolResultMessage {
        tool_call_id: "call-1|fc-1".to_owned(),
        tool_name: "read".to_owned(),
        content: vec![InputBlock::Text(TextContent {
            text: text.clone(),
            text_signature: None,
        })],
        details: None,
        is_error: false,
        timestamp: 0,
    })];
    let serialized = serialize_conversation(&messages);
    if text.is_empty() {
        assert_eq!(serialized, "");
    } else {
        assert_eq!(serialized, format!("[Tool result]: {text}"));
    }
}

/// A tool call is rendered as `name(key=value, ...)`, with each argument
/// value JSON-encoded, matching pi's `serializeConversation`.
#[test]
fn tool_call_is_serialized_with_json_encoded_arguments() {
    let messages = vec![Message::Assistant(assistant_response(
        StopReason::ToolUse,
        None,
        vec![AssistantBlock::ToolCall(ToolCall {
            id: "call-1|fc-1".to_owned(),
            name: "read".to_owned(),
            arguments: Map::from_iter([
                ("path".to_owned(), json!("big.txt")),
                ("limit".to_owned(), json!(10)),
            ]),
        })],
    ))];
    let serialized = serialize_conversation(&messages);
    assert_eq!(
        serialized,
        r#"[Assistant tool calls]: read(path="big.txt", limit=10)"#
    );
}

// =============================================================================
// File operations (testing.md: file lists carried across compactions)
// =============================================================================

/// A file that is only ever read stays in the read list; a file that is
/// written or edited moves to the modified list even if it was also
/// read, and both lists come out sorted (pi's `computeFileLists`).
#[test]
fn file_lists_separate_read_only_from_modified_and_sort() {
    let mut ops = FileOperations::new();
    let read_call = |name: &str, path: &str| {
        Message::Assistant(assistant_response(
            StopReason::ToolUse,
            None,
            vec![AssistantBlock::ToolCall(ToolCall {
                id: "call|fc".to_owned(),
                name: name.to_owned(),
                arguments: Map::from_iter([("path".to_owned(), json!(path))]),
            })],
        ))
    };
    ops.extract_from_messages(&[
        read_call("read", "z.txt"),
        read_call("read", "a.txt"),
        read_call("write", "a.txt"),
        read_call("edit", "m.txt"),
        read_call("bash", "ignored.txt"),
    ]);

    let (read_files, modified_files) = ops.file_lists();
    assert_eq!(read_files, vec!["z.txt".to_owned()]);
    assert_eq!(modified_files, vec!["a.txt".to_owned(), "m.txt".to_owned()]);
}

/// `format_file_operations` wraps each non-empty list in its own tag,
/// and returns an empty string when both lists are empty
/// (`docs/reference/compaction.md`, "File lists").
#[test]
fn format_file_operations_wraps_each_list_and_is_empty_when_both_are() {
    assert_eq!(format_file_operations(&[], &[]), "");
    assert_eq!(
        format_file_operations(&["a.txt".to_owned()], &[]),
        "\n\n<read-files>\na.txt\n</read-files>"
    );
    assert_eq!(
        format_file_operations(&[], &["b.txt".to_owned()]),
        "\n\n<modified-files>\nb.txt\n</modified-files>"
    );
    assert_eq!(
        format_file_operations(&["a.txt".to_owned()], &["b.txt".to_owned()]),
        "\n\n<read-files>\na.txt\n</read-files>\n\n<modified-files>\nb.txt\n</modified-files>"
    );
}

// =============================================================================
// Summary request (testing.md: output limit, prompt variant)
// =============================================================================

/// The summary request's output limit never exceeds the model's maximum
/// output (`docs/reference/testing.md`; `docs/reference/compaction.md`,
/// "Output limit"). `0` means the model has no documented cap.
#[hegel::test(test_cases = 500)]
fn summary_output_limit_never_exceeds_model_max(tc: TestCase) {
    let reserve_tokens = tc.draw(gs::integers::<u64>().max_value(1_000_000));
    let model_max_output_tokens =
        tc.draw(gs::integers::<u64>().max_value(1_000_000));
    let limit =
        summary_max_output_tokens(reserve_tokens, model_max_output_tokens);
    if model_max_output_tokens > 0 {
        assert!(limit <= model_max_output_tokens);
    }
    assert!(limit <= (0.8 * reserve_tokens as f64).floor() as u64);
}

/// With no prior summary, the request uses the initial prompt and has no
/// `<previous-summary>` section (`docs/reference/compaction.md`, "Prior
/// summary"; pi's `generateSummaryWithUsage`).
#[test]
fn summary_request_uses_the_initial_prompt_with_no_previous_summary() {
    let messages = vec![user_text("hello")];
    let request = build_summary_request(&messages, None, None);
    assert!(
        request
            .starts_with("<conversation>\n[User]: hello\n</conversation>\n\n")
    );
    assert!(request.ends_with(SUMMARIZATION_PROMPT));
    assert!(!request.contains("<previous-summary>"));
}

/// With a prior summary, the request uses the "update" prompt and
/// includes the prior summary in `<previous-summary>` tags.
#[test]
fn summary_request_uses_the_update_prompt_with_a_previous_summary() {
    let messages = vec![user_text("hello")];
    let request =
        build_summary_request(&messages, Some("prior summary text"), None);
    assert!(request.contains(
        "<previous-summary>\nprior summary text\n</previous-summary>\n\n"
    ));
    assert!(request.ends_with(UPDATE_SUMMARIZATION_PROMPT));
    assert!(!request.contains(&format!("\n\n{SUMMARIZATION_PROMPT}")));
}

/// Custom instructions are appended after the chosen prompt.
#[test]
fn summary_request_appends_custom_instructions() {
    let messages = vec![user_text("hello")];
    let request =
        build_summary_request(&messages, None, Some("focus on the bug"));
    assert!(request.ends_with(&format!(
        "{SUMMARIZATION_PROMPT}\n\nAdditional focus: focus on the bug"
    )));
}

// =============================================================================
// Rejected summaries (testing.md: "fails compaction and writes nothing")
// =============================================================================

/// A summary response that stops with `error` is rejected with pi's
/// exact wording (`docs/reference/compaction.md`, "Rejected summaries").
#[test]
fn error_stop_reason_is_rejected_with_its_message() {
    let response = assistant_response(StopReason::Error, Some("boom"), vec![]);
    let error = check_summary(&response).unwrap_err();
    assert_eq!(error.to_string(), "Summarization failed: boom");
}

/// An `error` stop with no message falls back to "Unknown error", like
/// pi's `getSummarizationFailure`.
#[test]
fn error_stop_reason_without_a_message_uses_a_placeholder() {
    let response = assistant_response(StopReason::Error, None, vec![]);
    let error = check_summary(&response).unwrap_err();
    assert_eq!(error.to_string(), "Summarization failed: Unknown error");
}

/// pi issue #7048: a length-truncated summary must never become a
/// checkpoint, and the error says exactly why.
#[test]
fn pi_known_case_7048_length_stop_is_rejected_as_incomplete() {
    let response = assistant_response(
        StopReason::Length,
        None,
        vec![text_block("partial summar")],
    );
    let error = check_summary(&response).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Summarization failed: generation hit the token cap and the \
         summary is incomplete"
    );
}

/// A response that calls a tool instead of summarizing is rejected.
#[test]
fn a_tool_call_in_the_response_is_rejected() {
    let response = assistant_response(
        StopReason::Stop,
        None,
        vec![AssistantBlock::ToolCall(ToolCall {
            id: "call-1|fc-1".to_owned(),
            name: "read".to_owned(),
            arguments: Map::new(),
        })],
    );
    let error = check_summary(&response).unwrap_err();
    assert_eq!(error.to_string(), "Summarization attempted to call a tool");
}

/// A successful response's text blocks are joined and returned.
#[test]
fn a_successful_response_returns_its_joined_text() {
    let response = assistant_response(
        StopReason::Stop,
        None,
        vec![text_block("## Goal"), text_block("line two")],
    );
    let text = check_summary(&response).expect("a stop response is accepted");
    assert_eq!(text, "## Goal\nline two");
}

/// `check_summary` rejects an `error`/`length` stop or a tool call, and
/// only those three cases, over generated responses
/// (`docs/reference/testing.md`: "A summary that stops with `length` or
/// `error`, or that calls a tool, fails compaction and writes nothing").
#[hegel::test(test_cases = 300)]
fn check_summary_rejects_exactly_error_length_and_tool_calls(tc: TestCase) {
    let response = tc.draw(generators::assistant_message());
    let result = check_summary(&response);
    let has_tool_call = response
        .content
        .iter()
        .any(|block| matches!(block, AssistantBlock::ToolCall(_)));
    match response.stop_reason {
        StopReason::Error => {
            assert!(result.is_err());
        }
        StopReason::Length => {
            assert!(result.is_err());
        }
        _ if has_tool_call => {
            assert!(result.is_err());
        }
        _ => {
            assert!(result.is_ok());
        }
    }
}

// =============================================================================
// Mutation-covering tests
//
// Each of these pins a rule the property and example tests above happen
// not to reach: `cargo mutants` found the gap in each case (see the
// module's mutants run for the full list).
// =============================================================================

/// Each `Compaction` builder sets exactly its own field, leaving the
/// other untouched.
#[test]
fn compaction_builders_set_their_own_field() {
    let compaction = Compaction::default()
        .reserve_tokens(1)
        .keep_recent_tokens(2);
    assert_eq!(compaction.reserve_tokens, 1);
    assert_eq!(compaction.keep_recent_tokens, 2);
}

/// `estimate_message_tokens` counts every character it should, and
/// nothing else: a user message's text and image blocks, and an
/// assistant message's text, thinking and tool-call (name plus
/// JSON-encoded arguments, *added* together, not multiplied).
#[test]
fn estimate_message_tokens_counts_characters_precisely() {
    // 8 characters -> ceil(8 / 4) = 2.
    assert_eq!(estimate_message_tokens(&user_text("abcdefgh")), 2);

    // 2 text characters + 4,800 for the image -> ceil(4802 / 4) = 1201.
    let user_with_image = Message::User(UserMessage {
        content: UserContent::Blocks(vec![
            InputBlock::Text(TextContent {
                text: "ab".to_owned(),
                text_signature: None,
            }),
            InputBlock::Image(ImageContent {
                data: String::new(),
                mime_type: "image/png".to_owned(),
            }),
        ]),
        timestamp: 0,
    });
    assert_eq!(estimate_message_tokens(&user_with_image), 1201);

    // text (4 chars) + thinking (6 chars) + tool call (name "toolname" =
    // 8 chars, arguments `{"a":"bb"}` = 10 chars, summed to 18, not
    // multiplied to 80) = 28 total -> ceil(28 / 4) = 7.
    let assistant = Message::Assistant(assistant_response(
        StopReason::ToolUse,
        None,
        vec![
            text_block("abcd"),
            AssistantBlock::Thinking(ThinkingContent {
                thinking: "efghij".to_owned(),
                thinking_signature: None,
                redacted: None,
            }),
            AssistantBlock::ToolCall(ToolCall {
                id: "call|fc".to_owned(),
                name: "toolname".to_owned(),
                arguments: Map::from_iter([("a".to_owned(), json!("bb"))]),
            }),
        ],
    ));
    assert_eq!(estimate_message_tokens(&assistant), 7);
}

/// An assistant message's thinking and text blocks are both carried into
/// the serialized conversation, on their own lines.
#[test]
fn assistant_thinking_and_text_are_both_serialized() {
    let messages = vec![Message::Assistant(assistant_response(
        StopReason::Stop,
        None,
        vec![
            AssistantBlock::Thinking(ThinkingContent {
                thinking: "pondering".to_owned(),
                thinking_signature: None,
                redacted: None,
            }),
            text_block("final answer"),
        ],
    ))];
    assert_eq!(
        serialize_conversation(&messages),
        "[Assistant thinking]: pondering\n\n[Assistant]: final answer"
    );
}

/// Usage counts as "reported" from any single one of its five fields
/// being nonzero, not only from a specific one — and a message that
/// counts as the anchor contributes the context it reports, discarding
/// everything estimated before it.
#[test]
fn usage_is_reported_from_any_single_nonzero_field() {
    let build = |usage: Usage| {
        vec![user_text(&"z".repeat(400)), assistant_with_usage(usage)]
    };
    let zero = || Usage {
        input: 0,
        output: 0,
        cache_read: 0,
        cache_write: 0,
        reasoning: None,
        total_tokens: 0,
        cost: UsageCost::default(),
    };

    for usage in [
        Usage {
            total_tokens: 1,
            ..zero()
        },
        Usage { input: 1, ..zero() },
        Usage {
            output: 1,
            ..zero()
        },
        Usage {
            cache_read: 1,
            ..zero()
        },
        Usage {
            cache_write: 1,
            ..zero()
        },
    ] {
        let expected = 1;
        let messages = build(usage.clone());
        assert_eq!(
            estimate_context_tokens(&messages),
            expected,
            "usage {usage:?} should count as reported and anchor the \
             estimate, discarding the preceding user message"
        );
    }
}

/// The anchor is the reported total when there is one, and otherwise
/// the sum of the four counters (pi's `calculateContextTokens`): cached
/// tokens are part of the context.
#[test]
fn the_anchor_is_the_reported_context() {
    let with = |input, output, cache_read, cache_write, total_tokens| {
        let messages = vec![
            user_text(&"z".repeat(400)),
            assistant_with_usage(Usage {
                input,
                output,
                cache_read,
                cache_write,
                reasoning: None,
                total_tokens,
                cost: UsageCost::default(),
            }),
        ];
        estimate_context_tokens(&messages)
    };
    assert_eq!(with(5, 3, 7, 11, 0), 26);
    assert_eq!(with(5, 3, 7, 11, 100), 100);
}

/// When no user message precedes the cut at all, the cut is not a split
/// turn, even though the cut message itself is not a turn start: the
/// missing turn start (`turn_start_index: None`) overrides it.
#[test]
fn no_turn_start_before_the_cut_means_not_a_split_turn() {
    let messages = vec![Message::Assistant(assistant_response(
        StopReason::Stop,
        None,
        vec![text_block("no user before me")],
    ))];
    let cut = find_cut_point(&messages, 1);
    assert_eq!(cut.first_kept_index, 0);
    assert_eq!(cut.turn_start_index, None);
    assert!(!cut.is_split_turn);
}

/// `build_turn_prefix_summary_request` wraps the serialized conversation
/// and the turn-prefix prompt in the exact shape pi's
/// `generateTurnPrefixSummary` builds.
#[test]
fn turn_prefix_summary_request_wraps_conversation_and_prompt() {
    let messages = vec![user_text("hello")];
    let request = build_turn_prefix_summary_request(&messages);
    assert_eq!(
        request,
        format!(
            "# Conversation\n[User]: hello\n\n# Instructions\n{TURN_PREFIX_SUMMARIZATION_PROMPT}"
        )
    );
}

/// `merge_split_turn_summary` joins the history and turn-prefix
/// summaries with pi's exact separator.
#[test]
fn merge_split_turn_summary_joins_with_the_expected_separator() {
    assert_eq!(
        merge_split_turn_summary("history text", "prefix text"),
        "history text\n\n---\n\n**Turn Context (split turn):**\n\nprefix text"
    );
}

/// `summary_max_output_tokens` is exactly `min(floor(0.8 * reserve),
/// model_max)`, with `0` treated as "no cap" — not a constant.
#[test]
fn summary_max_output_tokens_exact_values() {
    assert_eq!(summary_max_output_tokens(100, 1000), 80);
    assert_eq!(summary_max_output_tokens(100, 0), 80);
    assert_eq!(summary_max_output_tokens(100, 50), 50);
    assert_eq!(summary_max_output_tokens(0, 1000), 0);
    assert_eq!(turn_prefix_max_output_tokens(100, 1000), 50);
    assert_eq!(turn_prefix_max_output_tokens(100, 0), 50);
    assert_eq!(turn_prefix_max_output_tokens(101, 40), 40);
}

/// `plan` is `find_cut_point` over the messages after an earlier
/// summary, shifted back into place: a split turn's start is summarized
/// with the prefix prompt and everything before it with the main one;
/// otherwise everything before the cut is history. A cut that keeps
/// every unsummarized message plans nothing.
#[hegel::test(test_cases = 300)]
fn plan_is_the_cut_point_after_the_summary(tc: TestCase) {
    let mut messages = tc.draw(generators::transcript());
    let summarized = usize::from(tc.draw(gs::booleans()));
    if summarized == 1 {
        messages.insert(0, user_text("summary"));
    }
    let keep = tc.draw(gs::integers::<u64>().max_value(2_000));
    let cut = find_cut_point(&messages[summarized..], keep);
    let kept_from = summarized + cut.first_kept_index;
    let expected = (cut.first_kept_index > 0).then(|| {
        if cut.is_split_turn {
            let start = summarized + cut.turn_start_index.unwrap();
            Plan {
                history: summarized..start,
                turn_prefix: Some(start..kept_from),
                kept_from,
            }
        } else {
            Plan {
                history: summarized..kept_from,
                turn_prefix: None,
                kept_from,
            }
        }
    });
    assert_eq!(plan(&messages, summarized, keep), expected);
}

/// An overflow is a failed response that says so, in any of OpenAI's
/// wordings; other failures and successful responses are not.
#[test]
fn context_overflow_is_recognized_by_its_wording() {
    let failed = |message: &str| {
        assistant_response(StopReason::Error, Some(message), vec![])
    };
    for message in [
        "context_length_exceeded: too long",
        "Your input exceeds the context window of this model",
        "This model's Maximum Context Length is 1000 tokens",
    ] {
        assert!(is_context_overflow(&failed(message)), "{message}");
    }
    assert!(!is_context_overflow(&failed("server_error: boom")));
    assert!(!is_context_overflow(&assistant_response(
        StopReason::Error,
        None,
        vec![]
    )));
    assert!(!is_context_overflow(&assistant_response(
        StopReason::Stop,
        Some("context_length_exceeded"),
        vec![]
    )));
}
