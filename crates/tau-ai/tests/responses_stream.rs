//! `response.*` stream processing (`tau_ai::responses::stream`).
//!
//! Frame shapes and known cases are pinned from pi's
//! `processResponsesStream` (`packages/ai/src/api/openai-responses-shared.ts`)
//! and its tests, commit `2b0a123`. See `crates/tau-ai/src/responses/stream.rs`
//! for the full frame-to-event mapping this module checks, and
//! `docs/reference/testing.md`'s `tau-ai` property inventory and "Known
//! cases" table.

use hegel::{TestCase, generators as gs};
use serde_json::json;
use tau_ai::{
    event::{Accumulator, AssistantEvent, DoneReason, ErrorReason},
    message::{StopReason, Usage},
    responses::stream::StreamProcessor,
    retry::Class,
};
use tau_testing::openai::{
    draw_response_frames,
    interleave_unknown_frames,
    wire_assistant_message,
};

/// Runs `frames` through a fresh processor, checking every event against
/// the [`Accumulator`] grammar as it goes (the "processor output always
/// satisfies the grammar" property), and returns the accumulator.
fn run(
    model: &str,
    timestamp: u64,
    frames: &[serde_json::Value],
) -> Accumulator {
    let mut processor = StreamProcessor::new(model.to_owned(), timestamp);
    let mut acc = Accumulator::new();
    for frame in frames {
        for event in processor.push(frame) {
            acc.push(event)
                .expect("processor output must satisfy the grammar");
        }
    }
    acc
}

// =============================================================================
// Round trip and metamorphic properties
// =============================================================================

/// Event processor: rendering a wire-realistic response as `response.*`
/// frames (with arbitrary delta splits, and unknown frames sometimes
/// interleaved) and processing it gives back the response, usage
/// included and cost zero. `docs/reference/testing.md`'s "Event
/// processor" row.
#[hegel::test(test_cases = 500)]
fn round_trip_wire_realistic_message(tc: TestCase) {
    let message = tc.draw(wire_assistant_message());
    let frames = draw_response_frames(&tc, &message);
    let frames = if tc.draw(gs::booleans()) {
        interleave_unknown_frames(&tc, frames)
    } else {
        frames
    };
    let acc = run(&message.model, message.timestamp, &frames);
    assert!(acc.is_finished());
    let result = acc.finish().expect("a full stream always terminates");
    assert_eq!(result, message);
}

/// Unknown server event types anywhere in a stream are ignored and do
/// not change the result. `docs/reference/testing.md`'s "tau-ai"
/// property inventory.
#[hegel::test(test_cases = 500)]
fn unknown_frames_anywhere_do_not_change_the_result(tc: TestCase) {
    let message = tc.draw(wire_assistant_message());
    let clean = draw_response_frames(&tc, &message);
    let with_unknowns = interleave_unknown_frames(&tc, clean.clone());

    let clean_result = run(&message.model, message.timestamp, &clean)
        .finish()
        .expect("the clean stream terminates");
    let noisy_result = run(&message.model, message.timestamp, &with_unknowns)
        .finish()
        .expect("inserting unknown frames must not break termination");
    assert_eq!(clean_result, noisy_result);
}

/// Every strict prefix of a valid frame stream, closed instead of
/// completed, ends in `Error`, never `Done`, and every event along the
/// way (including the ones `close()` produces) satisfies the
/// [`Accumulator`] grammar. `docs/reference/testing.md`: "Every strict
/// prefix of a valid event stream ends in error, never in done."
#[hegel::test(test_cases = 500)]
fn strict_prefix_closed_always_ends_in_error(tc: TestCase) {
    let message = tc.draw(wire_assistant_message());
    let frames = draw_response_frames(&tc, &message);
    // A strict prefix never includes the last (terminal) frame.
    let cut = tc.draw(gs::integers::<usize>().max_value(frames.len() - 1));

    let mut processor =
        StreamProcessor::new(message.model.clone(), message.timestamp);
    let mut acc = Accumulator::new();
    for frame in &frames[..cut] {
        for event in processor.push(frame) {
            acc.push(event)
                .expect("prefix events must satisfy the grammar");
        }
    }
    assert!(!processor.is_finished());
    for event in processor.close() {
        acc.push(event)
            .expect("close() events must satisfy the grammar");
    }
    assert!(processor.is_finished());
    let result = acc.finish().expect("close() always terminates the stream");
    assert_eq!(result.stop_reason, StopReason::Error);
}

/// Frames pushed after the terminal event are ignored: they produce no
/// events, and `close()` afterward is a no-op too.
#[hegel::test(test_cases = 500)]
fn frames_after_terminal_are_ignored(tc: TestCase) {
    let message = tc.draw(wire_assistant_message());
    let frames = draw_response_frames(&tc, &message);
    let mut processor =
        StreamProcessor::new(message.model.clone(), message.timestamp);
    for frame in &frames {
        processor.push(frame);
    }
    assert!(processor.is_finished());

    let replay = tc.draw(gs::sampled_from(frames));
    assert!(processor.push(&replay).is_empty());
    assert!(processor.close().is_empty());
}

// =============================================================================
// Known cases from pi (docs/reference/testing.md, "Known cases", Events row)
// =============================================================================

/// A stream that ends with no terminal event: `close()` produces the
/// error tau-agent's own WebSocket reader raises in this situation.
/// pi: `openai-responses-terminal-event.test.ts` ("rejects streams that
/// end before a terminal response event" /
/// "emits an error final result when the wrapper stream ends before a
/// terminal response event").
#[test]
fn stream_without_terminal_event_closes_as_error() {
    let frames = vec![
        json!({ "type": "response.created", "response": { "id": "resp_early_eof" } }),
        json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": { "type": "reasoning", "id": "rs_early_eof", "summary": [] },
        }),
        json!({
            "type": "response.reasoning_text.delta",
            "output_index": 0,
            "delta": "partial reasoning before the stream ends",
        }),
    ];
    let mut processor = StreamProcessor::new("gpt-5-mini".to_owned(), 0);
    let mut acc = Accumulator::new();
    for frame in &frames {
        for event in processor.push(frame) {
            acc.push(event).unwrap();
        }
    }
    assert!(!processor.is_finished());
    let closing = processor.close();
    assert_eq!(closing.len(), 1);
    assert_eq!(
        closing[0],
        AssistantEvent::Error {
            reason: ErrorReason::Error,
            message: "WebSocket stream closed before response.completed".into(),
            usage: Usage::default(),
            // Output had begun: resending would repeat it.
            class: Class::Fatal,
        }
    );
    for event in closing {
        acc.push(event).unwrap();
    }
    let result = acc.finish().unwrap();
    assert_eq!(result.stop_reason, StopReason::Error);
    assert_eq!(
        result.error_message.as_deref(),
        Some("WebSocket stream closed before response.completed")
    );
}

/// `response.incomplete` with `max_output_tokens` turns a provisional
/// stop into `length`. pi: `openai-responses-terminal-event.test.ts`
/// ("finalizes incomplete terminal events as length stops").
#[test]
fn incomplete_max_output_tokens_maps_to_length() {
    let frames = vec![json!({
        "type": "response.incomplete",
        "response": {
            "id": "resp_incomplete",
            "status": "incomplete",
            "incomplete_details": { "reason": "max_output_tokens" },
            "usage": {
                "input_tokens": 30,
                "output_tokens": 12,
                "total_tokens": 42,
                "input_tokens_details": { "cached_tokens": 5 },
            },
        },
    })];
    let mut processor = StreamProcessor::new("gpt-5-mini".to_owned(), 0);
    let mut acc = Accumulator::new();
    for frame in &frames {
        for event in processor.push(frame) {
            acc.push(event).unwrap();
        }
    }
    let result = acc.finish().unwrap();
    assert_eq!(result.stop_reason, StopReason::Length);
    assert_eq!(result.response_id.as_deref(), Some("resp_incomplete"));
    assert_eq!(
        result.usage,
        Usage {
            input: 25,
            output: 12,
            cache_read: 5,
            cache_write: 0,
            reasoning: None,
            total_tokens: 42,
            cost: Default::default(),
        }
    );
}

/// `response.incomplete` with `content_filter` is a non-retryable error,
/// not a length stop. pi: `openai-responses-terminal-event.test.ts`
/// ("finalizes content-filtered incomplete responses as non-retryable
/// errors").
#[test]
fn incomplete_content_filter_maps_to_error() {
    let frames = vec![json!({
        "type": "response.incomplete",
        "response": {
            "id": "resp_incomplete",
            "status": "incomplete",
            "incomplete_details": { "reason": "content_filter" },
        },
    })];
    let mut processor = StreamProcessor::new("gpt-5-mini".to_owned(), 0);
    let mut acc = Accumulator::new();
    for frame in &frames {
        for event in processor.push(frame) {
            acc.push(event).unwrap();
        }
    }
    assert_eq!(processor.error_code(), None);
    let result = acc.finish().unwrap();
    assert_eq!(result.stop_reason, StopReason::Error);
    assert_eq!(
        result.error_message.as_deref(),
        Some("Response incomplete: content_filter")
    );
}

/// The internal partial-JSON scratch buffer never reaches the emitted or
/// persisted tool call: only the fully parsed arguments do. pi:
/// `openai-responses-partial-json-cleanup.test.ts` ("removes partialJson
/// from persisted tool-call blocks at output_item.done").
#[test]
fn tool_call_scratch_buffer_never_reaches_the_emitted_call() {
    let frames = vec![
        json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {
                "type": "function_call",
                "id": "fc_test",
                "call_id": "call_test",
                "name": "edit",
                "arguments": "",
            },
        }),
        json!({
            "type": "response.function_call_arguments.delta",
            "output_index": 0,
            "delta": "{\"path\":\"README.md\"",
        }),
        json!({
            "type": "response.function_call_arguments.delta",
            "output_index": 0,
            "delta": ",\"content\":\"updated\"}",
        }),
        json!({
            "type": "response.function_call_arguments.done",
            "output_index": 0,
            "arguments": "{\"path\":\"README.md\",\"content\":\"updated\"}",
        }),
        json!({
            "type": "response.output_item.done",
            "output_index": 0,
            "item": {
                "type": "function_call",
                "id": "fc_test",
                "call_id": "call_test",
                "name": "edit",
                "arguments": "{\"path\":\"README.md\",\"content\":\"updated\"}",
            },
        }),
        json!({
            "type": "response.completed",
            "response": { "id": "resp_test", "status": "completed" },
        }),
    ];
    let mut processor = StreamProcessor::new("gpt-5-mini".to_owned(), 0);
    let mut acc = Accumulator::new();
    for frame in &frames {
        for event in processor.push(frame) {
            acc.push(event).unwrap();
        }
    }
    let result = acc.finish().unwrap();
    assert_eq!(result.content.len(), 1);
    let tau_ai::message::AssistantBlock::ToolCall(call) = &result.content[0]
    else {
        panic!("expected a tool call block");
    };
    assert_eq!(call.id, "call_test|fc_test");
    assert_eq!(call.name, "edit");
    assert_eq!(
        call.arguments,
        serde_json::json!({ "path": "README.md", "content": "updated" })
            .as_object()
            .unwrap()
            .clone()
    );
    assert_eq!(result.stop_reason, StopReason::ToolUse);
}

/// An `error` frame with `previous_response_not_found` exposes that code
/// through `error_code()`, separately from the human-readable message —
/// the lane needs the raw code to drive the recovery ladder.
/// `docs/reference/openai-websocket.md`'s "Recovery ladder"; pi's
/// equivalent path: `openai-codex-stream.test.ts:2216`.
#[test]
fn previous_response_not_found_exposes_its_code() {
    let frames = vec![json!({
        "type": "error",
        "code": "previous_response_not_found",
        "message": "Previous response with id 'resp_abc' not found.",
        "param": "previous_response_id",
    })];
    let mut processor = StreamProcessor::new("gpt-5-mini".to_owned(), 0);
    let mut acc = Accumulator::new();
    for frame in &frames {
        for event in processor.push(frame) {
            acc.push(event).unwrap();
        }
    }
    assert_eq!(processor.error_code(), Some("previous_response_not_found"));
    let result = acc.finish().unwrap();
    assert_eq!(result.stop_reason, StopReason::Error);
    assert_eq!(
        result.error_message.as_deref(),
        Some(
            "Error Code previous_response_not_found: Previous response with id 'resp_abc' not found."
        )
    );
}

/// The server nests an `error` frame's details under `error`, as it
/// rejects an effort the model does not take.
#[test]
fn a_nested_error_frame_keeps_its_code_and_message() {
    let frame = json!({
        "type": "error",
        "status": 400,
        "error": {
            "type": "invalid_request_error",
            "code": "unsupported_value",
            "message": "Unsupported value: 'minimal' is not supported with the 'gpt-6-sol' model.",
            "param": "reasoning.effort",
        },
    });
    let mut processor = StreamProcessor::new("gpt-6-sol".to_owned(), 0);
    let mut acc = Accumulator::new();
    for event in processor.push(&frame) {
        acc.push(event).unwrap();
    }
    assert_eq!(processor.error_code(), Some("unsupported_value"));
    assert_eq!(
        acc.finish().unwrap().error_message.as_deref(),
        Some(
            "Error Code unsupported_value: Unsupported value: 'minimal' is not supported with the 'gpt-6-sol' model."
        )
    );
}

/// A `response.failed` frame carries the provider's error, and its code
/// is exposed too. pi: `openai-responses-terminal-event.test.ts`
/// ("rejects failed terminal events with the provider error").
#[test]
fn response_failed_exposes_code_and_message() {
    let frames = vec![json!({
        "type": "response.failed",
        "response": {
            "id": "resp_failed",
            "status": "failed",
            "error": { "code": "server_error", "message": "boom" },
        },
    })];
    let mut processor = StreamProcessor::new("gpt-5-mini".to_owned(), 0);
    let mut acc = Accumulator::new();
    for frame in &frames {
        for event in processor.push(frame) {
            acc.push(event).unwrap();
        }
    }
    assert_eq!(processor.error_code(), Some("server_error"));
    let result = acc.finish().unwrap();
    assert_eq!(result.stop_reason, StopReason::Error);
    assert_eq!(result.error_message.as_deref(), Some("server_error: boom"));
}

// =============================================================================
// Golden example
// =============================================================================

/// A realistic text response, combining pi's message-item shape
/// (`openai-responses-shared.ts`'s `output_item.added`/`.done` for a
/// `message` item) with the usage numbers from
/// `openai-responses-terminal-event.test.ts`'s `createCompletedEvents`
/// (`input_tokens: 20, output_tokens: 7, cached_tokens: 2,
/// cache_write_tokens: 3, total_tokens: 27`), checked against the exact
/// expected event sequence.
#[test]
fn golden_text_response() {
    let frames = vec![
        json!({ "type": "response.created", "response": { "id": "resp_golden" } }),
        json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {
                "type": "message",
                "id": "msg_golden",
                "role": "assistant",
                "status": "in_progress",
                "content": [],
            },
        }),
        json!({ "type": "response.output_text.delta", "output_index": 0, "delta": "Hello" }),
        json!({ "type": "response.output_text.delta", "output_index": 0, "delta": " world" }),
        json!({
            "type": "response.output_item.done",
            "output_index": 0,
            "item": {
                "type": "message",
                "id": "msg_golden",
                "role": "assistant",
                "status": "completed",
                "content": [{ "type": "output_text", "text": "Hello world", "annotations": [] }],
            },
        }),
        json!({
            "type": "response.completed",
            "response": {
                "id": "resp_golden",
                "status": "completed",
                "usage": {
                    "input_tokens": 20,
                    "output_tokens": 7,
                    "total_tokens": 27,
                    "input_tokens_details": { "cached_tokens": 2, "cache_write_tokens": 3 },
                },
            },
        }),
    ];

    let mut processor = StreamProcessor::new("gpt-5-mini".to_owned(), 1234);
    let mut events = Vec::new();
    for frame in &frames {
        events.extend(processor.push(frame));
    }

    let expected_usage = Usage {
        input: 15,
        output: 7,
        cache_read: 2,
        cache_write: 3,
        reasoning: None,
        total_tokens: 27,
        cost: Default::default(),
    };
    assert_eq!(
        events,
        vec![
            AssistantEvent::Start {
                model: "gpt-5-mini".into(),
                response_id: Some("resp_golden".into()),
                timestamp: 1234,
            },
            AssistantEvent::TextStart { index: 0 },
            AssistantEvent::TextDelta {
                index: 0,
                delta: "Hello".into()
            },
            AssistantEvent::TextDelta {
                index: 0,
                delta: " world".into()
            },
            AssistantEvent::TextEnd {
                index: 0,
                content: tau_ai::message::TextContent {
                    text: "Hello world".into(),
                    text_signature: Some(
                        tau_ai::responses::stream::encode_text_signature_v1(
                            "msg_golden",
                            None
                        )
                    ),
                },
            },
            AssistantEvent::Done {
                reason: DoneReason::Stop,
                usage: expected_usage.clone(),
                response_id: Some("resp_golden".into()),
            },
        ]
    );

    let mut acc = Accumulator::new();
    for event in events {
        acc.push(event).unwrap();
    }
    let result = acc.finish().unwrap();
    assert_eq!(result.stop_reason, StopReason::Stop);
    assert_eq!(result.usage, expected_usage);
    assert_eq!(result.response_id.as_deref(), Some("resp_golden"));
    assert_eq!(processor.response_id(), Some("resp_golden"));
}

// =============================================================================
// Exact-event regressions
//
// The round-trip property only checks the *final* accumulated message, so
// it cannot see a spurious or missing intermediate delta event: the
// authoritative block at `*End` overwrites whatever the deltas showed
// along the way. These tests assert on `StreamProcessor::push`'s raw
// output instead, to pin down the streaming behaviour itself.
// =============================================================================

/// Tool-call argument streaming: an item that already carries partial
/// `arguments` at `added` gets an immediate delta for them; each
/// `function_call_arguments.delta` is forwarded; a `.done` only produces
/// a delta for the *unsent suffix* when its full string actually extends
/// what streamed, and produces nothing when it does not (pi:
/// `openai-responses-shared.ts`'s `function_call_arguments.done` handler,
/// `if (event.arguments.startsWith(previousPartialJson))`).
#[test]
fn tool_call_argument_deltas_are_exact() {
    let mut processor = StreamProcessor::new("gpt-5-mini".to_owned(), 0);

    let added = processor.push(&json!({
        "type": "response.output_item.added",
        "output_index": 0,
        "item": {
            "type": "function_call",
            "id": "fc_a",
            "call_id": "call_a",
            "name": "search",
            "arguments": "partial",
        },
    }));
    assert_eq!(
        added,
        vec![
            AssistantEvent::Start {
                model: "gpt-5-mini".into(),
                response_id: None,
                timestamp: 0,
            },
            AssistantEvent::ToolCallStart {
                index: 0,
                id: "call_a|fc_a".into(),
                name: "search".into(),
            },
            AssistantEvent::ToolCallDelta {
                index: 0,
                delta: "partial".into()
            },
        ]
    );

    let delta = processor.push(&json!({
        "type": "response.function_call_arguments.delta",
        "output_index": 0,
        "delta": "-more",
    }));
    assert_eq!(
        delta,
        vec![AssistantEvent::ToolCallDelta {
            index: 0,
            delta: "-more".into()
        }]
    );

    // The full string extends "partial-more" by "-x": only that suffix is
    // sent.
    let done_extends = processor.push(&json!({
        "type": "response.function_call_arguments.done",
        "output_index": 0,
        "arguments": "partial-more-x",
    }));
    assert_eq!(
        done_extends,
        vec![AssistantEvent::ToolCallDelta {
            index: 0,
            delta: "-x".into()
        }]
    );

    // A second `.done` with the exact same string extends nothing: no
    // delta event, even though the frame type is handled.
    let done_repeats = processor.push(&json!({
        "type": "response.function_call_arguments.done",
        "output_index": 0,
        "arguments": "partial-more-x",
    }));
    assert_eq!(done_repeats, Vec::<AssistantEvent>::new());
}

/// A tool call whose `added` item carries no initial `arguments` gets no
/// immediate delta (only the empty case is reachable from real server
/// data, but the code must still not synthesize one).
#[test]
fn tool_call_with_no_initial_arguments_gets_no_immediate_delta() {
    let mut processor = StreamProcessor::new("gpt-5-mini".to_owned(), 0);
    let added = processor.push(&json!({
        "type": "response.output_item.added",
        "output_index": 0,
        "item": {
            "type": "function_call",
            "id": "fc_a",
            "call_id": "call_a",
            "name": "search",
            "arguments": "",
        },
    }));
    assert_eq!(
        added,
        vec![
            AssistantEvent::Start {
                model: "gpt-5-mini".into(),
                response_id: None,
                timestamp: 0,
            },
            AssistantEvent::ToolCallStart {
                index: 0,
                id: "call_a|fc_a".into(),
                name: "search".into(),
            },
        ]
    );
}

/// `response.reasoning_summary_part.done` inserts a `"\n\n"` separator
/// into the streamed thinking text, both as a delta event and in the
/// final block (whose `summary` is empty here, so it falls back to the
/// accumulated text). pi: `openai-responses-shared.ts`'s
/// `response.reasoning_summary_part.done` handler.
#[test]
fn reasoning_summary_part_done_inserts_a_separator() {
    let mut processor = StreamProcessor::new("gpt-5-mini".to_owned(), 0);
    let mut events = Vec::new();
    events.extend(processor.push(&json!({
        "type": "response.output_item.added",
        "output_index": 0,
        "item": { "type": "reasoning", "id": "rs_x", "summary": [] },
    })));
    events.extend(processor.push(&json!({
        "type": "response.reasoning_summary_text.delta",
        "output_index": 0,
        "delta": "hello",
    })));
    let part_done = processor.push(&json!({
        "type": "response.reasoning_summary_part.done",
        "output_index": 0,
    }));
    assert_eq!(
        part_done,
        vec![AssistantEvent::ThinkingDelta {
            index: 0,
            delta: "\n\n".into()
        }]
    );
    events.extend(part_done);
    events.extend(processor.push(&json!({
        "type": "response.reasoning_summary_text.delta",
        "output_index": 0,
        "delta": "world",
    })));
    events.extend(processor.push(&json!({
        "type": "response.output_item.done",
        "output_index": 0,
        "item": { "type": "reasoning", "id": "rs_x", "summary": [] },
    })));

    let mut acc = Accumulator::new();
    for event in events {
        acc.push(event).unwrap();
    }
    let partial = acc.partial().unwrap();
    let tau_ai::message::AssistantBlock::Thinking(thinking) =
        &partial.content[0]
    else {
        panic!("expected a thinking block");
    };
    assert_eq!(thinking.thinking, "hello\n\nworld");
}

/// A reasoning item's `summary` array, when present, is what the final
/// block's text comes from — not the streamed delta text — joined with
/// `"\n\n"` between parts. pi: `item.summary?.map((s) => s.text).join("\n\n")`
/// (`openai-responses-shared.ts`'s `output_item.done` reasoning handler).
#[test]
fn reasoning_summary_array_wins_over_streamed_text() {
    let frames = vec![
        json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": { "type": "reasoning", "id": "rs_y", "summary": [] },
        }),
        json!({
            "type": "response.reasoning_summary_text.delta",
            "output_index": 0,
            "delta": "ignored, superseded by the summary array below",
        }),
        json!({
            "type": "response.output_item.done",
            "output_index": 0,
            "item": {
                "type": "reasoning",
                "id": "rs_y",
                "summary": [
                    { "type": "summary_text", "text": "Part A" },
                    { "type": "summary_text", "text": "Part B" },
                ],
            },
        }),
    ];
    let mut processor = StreamProcessor::new("gpt-5-mini".to_owned(), 0);
    let mut acc = Accumulator::new();
    for frame in &frames {
        for event in processor.push(frame) {
            acc.push(event).unwrap();
        }
    }
    let partial = acc.partial().unwrap();
    let tau_ai::message::AssistantBlock::Thinking(thinking) =
        &partial.content[0]
    else {
        panic!("expected a thinking block");
    };
    assert_eq!(thinking.thinking, "Part A\n\nPart B");
}

/// The `output_index` a frame carries is read from the frame itself, not
/// assumed: two blocks whose real `output_index` values are neither 0
/// nor 1, opened without the first closing, resolve to independent
/// slots and independent content indices.
#[test]
fn output_index_identifies_independent_slots() {
    let mut processor = StreamProcessor::new("gpt-5-mini".to_owned(), 0);

    let first = processor.push(&json!({
        "type": "response.output_item.added",
        "output_index": 7,
        "item": {
            "type": "message",
            "id": "msg_a",
            "role": "assistant",
            "status": "in_progress",
            "content": [],
        },
    }));
    assert!(matches!(
        first.last(),
        Some(AssistantEvent::TextStart { index: 0 })
    ));

    let second = processor.push(&json!({
        "type": "response.output_item.added",
        "output_index": 3,
        "item": { "type": "reasoning", "id": "rs_b", "summary": [] },
    }));
    assert_eq!(second, vec![AssistantEvent::ThinkingStart { index: 1 }]);

    let text_delta = processor.push(&json!({
        "type": "response.output_text.delta",
        "output_index": 7,
        "delta": "hi",
    }));
    assert_eq!(
        text_delta,
        vec![AssistantEvent::TextDelta {
            index: 0,
            delta: "hi".into()
        }]
    );

    let thinking_delta = processor.push(&json!({
        "type": "response.reasoning_text.delta",
        "output_index": 3,
        "delta": "hmm",
    }));
    assert_eq!(
        thinking_delta,
        vec![AssistantEvent::ThinkingDelta {
            index: 1,
            delta: "hmm".into()
        }]
    );
}

/// `response.completed`/`response.incomplete` with no `status` field
/// defensively maps to `Stop` (pi: `if (!status) return { stopReason:
/// "stop" }`), the same as an explicit `"completed"` — but a `status`
/// that is neither `"completed"` nor `"incomplete"` is a protocol error,
/// not silently a success.
#[test]
fn terminal_status_absent_is_stop_but_unexpected_status_is_error() {
    let absent_status = StreamProcessor::new("gpt-5-mini".to_owned(), 0).push(
        &json!({ "type": "response.completed", "response": { "id": "r1" } }),
    );
    assert!(matches!(
        absent_status.last(),
        Some(AssistantEvent::Done {
            reason: DoneReason::Stop,
            ..
        })
    ));

    let unexpected_status = StreamProcessor::new("gpt-5-mini".to_owned(), 0)
        .push(&json!({
            "type": "response.completed",
            "response": { "id": "r2", "status": "some_future_status" },
        }));
    match unexpected_status.last() {
        Some(AssistantEvent::Error { message, .. }) => {
            assert_eq!(
                message,
                "Unexpected response status: some_future_status"
            );
        }
        other => panic!("expected an Error event, got {other:?}"),
    }
}

/// The class a failure's `Error` event carries decides whether the loop
/// retries the turn: a socket that closes before any output is retried,
/// and an API failure is classified by its code (and, for an `error`
/// frame, its status) as `retry::classify` does, never by its text.
#[test]
fn failures_carry_their_retry_class() {
    let class_of = |frames: Vec<serde_json::Value>, close: bool| {
        let mut processor = StreamProcessor::new("gpt-5-mini".to_owned(), 0);
        let mut events: Vec<AssistantEvent> =
            frames.iter().flat_map(|f| processor.push(f)).collect();
        if close {
            events.extend(processor.close());
        }
        match events.last() {
            Some(AssistantEvent::Error { class, .. }) => *class,
            other => panic!("expected an error, got {other:?}"),
        }
    };
    let created =
        json!({ "type": "response.created", "response": { "id": "r" } });
    let failed = |code: &str, kind: &str| {
        json!({
            "type": "response.failed",
            "response": { "error": { "code": code, "type": kind, "message": "m" } },
        })
    };
    assert_eq!(class_of(vec![created.clone()], true), Class::Retryable);
    assert_eq!(class_of(vec![], true), Class::Retryable);
    assert_eq!(
        class_of(
            vec![created.clone(), failed("server_error", "server_error")],
            false
        ),
        Class::Retryable
    );
    assert_eq!(
        class_of(
            vec![failed("insufficient_quota", "insufficient_quota")],
            false
        ),
        Class::Fatal
    );
    assert_eq!(
        class_of(
            vec![failed("context_length_exceeded", "invalid_request_error")],
            false
        ),
        Class::ContextOverflow
    );
    assert_eq!(
        class_of(vec![failed("mystery", "api_error")], false),
        Class::Retryable,
        "an unknown code falls through to the type"
    );
    assert_eq!(
        class_of(
            vec![
                json!({"type": "error", "code": "mystery", "message": "m", "status": 429})
            ],
            false
        ),
        Class::Retryable,
        "an unknown code falls through to the status"
    );
    assert_eq!(
        class_of(
            vec![json!({"type": "error", "code": "mystery", "message": "m"})],
            false
        ),
        Class::Fatal
    );
    assert_eq!(
        class_of(
            vec![
                json!({"type": "response.completed", "response": {"status": "cancelled"}})
            ],
            false
        ),
        Class::Fatal
    );
}
