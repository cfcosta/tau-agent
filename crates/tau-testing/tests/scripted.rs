//! `ScriptedModel` behaviour, per `docs/reference/testing.md`'s
//! "ScriptedModel" section.

use std::time::Duration;

use futures_util::StreamExt;
use hegel::{TestCase, generators as gs};
use serde_json::{Map, Value};
use tau_ai::{
    event::{Accumulator, AssistantEvent, ErrorReason, GrammarError},
    llm::{EventStream, Llm},
    message::{
        API,
        AssistantBlock,
        AssistantMessage,
        ImageContent,
        InputBlock,
        Message,
        PROVIDER,
        StopReason,
        TextContent,
        ToolResultMessage,
        Usage,
        UserContent,
        UserMessage,
    },
    responses::request::Settings,
};
use tau_testing::scripted::{
    DROPPED_MESSAGE,
    FAILS_BEFORE_START_MESSAGE,
    ScriptedModel,
};

fn settings(model: &str) -> Settings {
    Settings {
        model: model.to_owned(),
        ..Default::default()
    }
}

fn user_message(text: &str) -> Message {
    Message::User(UserMessage {
        content: UserContent::Text(text.to_owned()),
        timestamp: 0,
    })
}

fn assistant_text_message(text: &str) -> Message {
    Message::Assistant(AssistantMessage {
        content: vec![AssistantBlock::Text(TextContent {
            text: text.to_owned(),
            text_signature: None,
        })],
        api: API.to_owned(),
        provider: PROVIDER.to_owned(),
        model: "gpt-test".to_owned(),
        response_id: None,
        usage: Usage::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 0,
    })
}

fn text_of(message: &AssistantMessage) -> Vec<&str> {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            AssistantBlock::Text(content) => Some(content.text.as_str()),
            _ => None,
        })
        .collect()
}

/// Drains `stream`, checking the grammar in `crates/tau-ai/src/event.rs` as
/// it goes, and returns the accumulated message.
async fn accumulate(
    mut stream: EventStream,
) -> Result<AssistantMessage, GrammarError> {
    let mut acc = Accumulator::new();
    while let Some(event) = stream.next().await {
        acc.push(event)?;
    }
    acc.finish()
}

// =============================================================================
// Generators for scripted turns
// =============================================================================

#[derive(Clone, Debug)]
enum GenBlock {
    Text(String),
    Thinking(String),
    ToolCall(String, Map<String, Value>),
}

#[hegel::composite]
fn gen_block(tc: TestCase) -> GenBlock {
    match tc.draw(gs::integers::<u8>().max_value(2)) {
        0 => GenBlock::Text(tc.draw(tau_testing::generators::text(20))),
        1 => GenBlock::Thinking(tc.draw(tau_testing::generators::text(20))),
        _ => GenBlock::ToolCall(
            tc.draw(tau_testing::generators::id("tool_")),
            tc.draw(tau_testing::generators::json_object(1)),
        ),
    }
}

#[derive(Clone, Debug)]
struct GenTurn {
    blocks: Vec<GenBlock>,
    stop: StopReason,
    usage: (u64, u64),
}

#[hegel::composite]
fn gen_turn(tc: TestCase) -> GenTurn {
    let blocks = tc.draw(gs::vecs(gen_block()).max_size(4));
    let has_tool_call = blocks
        .iter()
        .any(|block| matches!(block, GenBlock::ToolCall(..)));
    let stop = if has_tool_call {
        StopReason::ToolUse
    } else {
        tc.draw(gs::sampled_from(vec![StopReason::Stop, StopReason::Length]))
    };
    let usage = (
        tc.draw(gs::integers::<u64>().max_value(100_000)),
        tc.draw(gs::integers::<u64>().max_value(100_000)),
    );
    GenTurn {
        blocks,
        stop,
        usage,
    }
}

fn script_model_with(turn: GenTurn) -> ScriptedModel {
    ScriptedModel::new().turn(move |mut t| {
        for block in turn.blocks {
            t = match block {
                GenBlock::Text(text) => t.text(text),
                GenBlock::Thinking(text) => t.thinking(text),
                GenBlock::ToolCall(name, arguments) => {
                    t.tool_call(name, Value::Object(arguments))
                }
            };
        }
        t.stop(turn.stop).usage(turn.usage.0, turn.usage.1)
    })
}

// =============================================================================
// Grammar and content
// =============================================================================

/// For any scripted turn, opened on a fresh session, its response stream
/// satisfies the `Accumulator` grammar (`crates/tau-ai/src/event.rs`), and
/// the accumulated message holds exactly the scripted content, usage and
/// stop reason. A fresh session has no cached prefix, so `cache_read` is
/// always 0 and `usage.input`/`usage.output` equal the scripted override
/// exactly (see the module docs' "Cache simulation").
#[hegel::test(test_cases = 200)]
fn scripted_turn_matches_grammar_and_content(tc: TestCase) {
    let turn = tc.draw(gen_turn());
    let blocks = turn.blocks.clone();
    let stop = turn.stop;
    let usage = turn.usage;
    let model = script_model_with(turn);

    let message = tau_testing::block_on(async {
        let mut session = model.open(settings("gpt-test")).await.unwrap();
        let stream = session.respond(&[], 0);
        accumulate(stream).await
    })
    .expect("a scripted turn's stream always satisfies the grammar");

    assert_eq!(message.stop_reason, stop);
    assert_eq!(message.usage.input, usage.0);
    assert_eq!(message.usage.output, usage.1);
    assert_eq!(message.usage.cache_read, 0);
    assert_eq!(
        message.usage.total_tokens,
        message.usage.input
            + message.usage.output
            + message.usage.cache_read
            + message.usage.cache_write
    );
    assert_eq!(message.content.len(), blocks.len());
    for (actual, expected) in message.content.iter().zip(blocks.iter()) {
        match (actual, expected) {
            (AssistantBlock::Text(a), GenBlock::Text(e)) => {
                assert_eq!(&a.text, e)
            }
            (AssistantBlock::Thinking(a), GenBlock::Thinking(e)) => {
                assert_eq!(&a.thinking, e)
            }
            (
                AssistantBlock::ToolCall(a),
                GenBlock::ToolCall(name, arguments),
            ) => {
                assert_eq!(&a.name, name);
                assert_eq!(&a.arguments, arguments);
            }
            (actual, expected) => {
                panic!(
                    "content block kind mismatch: {actual:?} vs {expected:?}"
                )
            }
        }
    }
}

// =============================================================================
// Recording
// =============================================================================

/// Every request is recorded, in the order `respond` was called, with the
/// exact settings and transcript it was given.
#[hegel::test(test_cases = 100)]
fn requests_are_recorded_in_order_with_exact_transcripts(tc: TestCase) {
    let transcripts: Vec<Vec<Message>> = tc.draw(
        gs::vecs(tau_testing::generators::transcript())
            .min_size(1)
            .max_size(4),
    );
    let mut model = ScriptedModel::new();
    for _ in &transcripts {
        model = model.turn(|t| t.text("ok"));
    }
    let request_settings = settings("gpt-test");

    tau_testing::block_on(async {
        let mut session = model.open(request_settings.clone()).await.unwrap();
        for transcript in &transcripts {
            let _ = accumulate(session.respond(transcript, 0)).await;
        }
    });

    let requests = model.requests();
    assert_eq!(requests.len(), transcripts.len());
    for (request, transcript) in requests.iter().zip(transcripts.iter()) {
        assert_eq!(request.settings, request_settings);
        assert_eq!(&request.transcript, transcript);
    }
}

/// Sessions opened from the same `ScriptedModel` (or a clone of it) draw
/// from one shared script queue, in the order `respond` was called.
#[test]
fn sessions_from_the_same_model_share_one_script_queue() {
    tau_testing::block_on(async {
        let model = ScriptedModel::new()
            .turn(|t| t.text("first"))
            .turn(|t| t.text("second"));
        let mut session_a = model.open(settings("gpt-test")).await.unwrap();
        let mut session_b = model.open(settings("gpt-test")).await.unwrap();

        let first = accumulate(session_a.respond(&[], 0)).await.unwrap();
        let second = accumulate(session_b.respond(&[], 0)).await.unwrap();

        assert_eq!(text_of(&first), vec!["first"]);
        assert_eq!(text_of(&second), vec!["second"]);
        model.assert_exhausted();
    });
}

// =============================================================================
// Exhaustion
// =============================================================================

/// Running out of scripted turns never hangs: the stream still opens with
/// `Start` and ends with a clear `Error`.
#[test]
fn exhausted_script_yields_a_clear_error_and_never_hangs() {
    tau_testing::block_on(async {
        let model = ScriptedModel::new();
        let mut session = model.open(settings("gpt-test")).await.unwrap();
        let stream = session.respond(&[], 0);

        let message =
            tokio::time::timeout(Duration::from_secs(5), accumulate(stream))
                .await
                .expect("an exhausted script must not hang")
                .expect("the exhaustion stream still satisfies the grammar");

        assert_eq!(message.stop_reason, StopReason::Error);
        assert!(
            message
                .error_message
                .as_deref()
                .unwrap_or_default()
                .contains("exhausted"),
            "expected the error message to say the script is exhausted, got {:?}",
            message.error_message
        );
    });
}

#[test]
fn remaining_and_assert_exhausted_track_the_script() {
    let model = ScriptedModel::new().turn(|t| t.text("hi"));
    assert_eq!(model.remaining(), 1);
}

#[test]
#[should_panic(expected = "scripted turn(s) were never used")]
fn assert_exhausted_panics_when_turns_are_left() {
    let model = ScriptedModel::new().turn(|t| t.text("hi"));
    model.assert_exhausted();
}

// =============================================================================
// Errors
// =============================================================================

/// Each error kind starts with `Start` and ends with the documented
/// terminal `Error` event, with no content in between.
#[test]
fn error_turns_start_then_terminate_with_the_documented_error() {
    tau_testing::block_on(async {
        let model = ScriptedModel::new()
            .turn(|t| t.error("rate_limit_exceeded", "slow down"));
        let mut session = model.open(settings("gpt-test")).await.unwrap();
        let mut stream = session.respond(&[], 0);
        assert!(matches!(
            stream.next().await,
            Some(AssistantEvent::Start { .. })
        ));
        match stream.next().await {
            Some(AssistantEvent::Error {
                reason, message, ..
            }) => {
                assert_eq!(reason, ErrorReason::Error);
                assert_eq!(message, "rate_limit_exceeded: slow down");
            }
            other => panic!("expected a terminal Error, got {other:?}"),
        }
        assert!(stream.next().await.is_none());
    });

    tau_testing::block_on(async {
        let model = ScriptedModel::new().turn(|t| t.dropped());
        let mut session = model.open(settings("gpt-test")).await.unwrap();
        let mut stream = session.respond(&[], 0);
        assert!(matches!(
            stream.next().await,
            Some(AssistantEvent::Start { .. })
        ));
        match stream.next().await {
            Some(AssistantEvent::Error { message, .. }) => {
                assert_eq!(message, DROPPED_MESSAGE);
            }
            other => panic!("expected a terminal Error, got {other:?}"),
        }
        assert!(stream.next().await.is_none());
    });

    tau_testing::block_on(async {
        let model = ScriptedModel::new().turn(|t| t.fails_before_start());
        let mut session = model.open(settings("gpt-test")).await.unwrap();
        let mut stream = session.respond(&[], 0);
        assert!(matches!(
            stream.next().await,
            Some(AssistantEvent::Start { .. })
        ));
        match stream.next().await {
            Some(AssistantEvent::Error { message, .. }) => {
                assert_eq!(message, FAILS_BEFORE_START_MESSAGE);
            }
            other => panic!("expected a terminal Error, got {other:?}"),
        }
        assert!(stream.next().await.is_none());
    });
}

// =============================================================================
// Cache simulation
// =============================================================================

/// Ported from pi's faux provider (`packages/ai/src/providers/faux.ts`,
/// `withUsageEstimate`): a session's first request has nothing to share, so
/// `cache_read` is 0 and `cache_write` pays for the whole prompt. A later
/// request that extends the same session's previous transcript reports
/// `cache_read` for the shared prefix and `cache_write` for the rest, with
/// `input` reduced by `cache_read`. A request that shares no prefix with the
/// session's last transcript (even though the session has history) reports
/// `cache_read: 0` again.
#[test]
fn cache_simulation_matches_pi_faux_formula() {
    tau_testing::block_on(async {
        let model = ScriptedModel::new()
            .turn(|t| t.text("ok"))
            .turn(|t| t.text("ok"))
            .turn(|t| t.text("ok"));
        let mut session = model.open(settings("gpt-test")).await.unwrap();

        // Request 1: transcript serializes to "user:hi" (7 chars). Every
        // turn's own content is "ok" (2 chars), so output is ceil(2/4) = 1.
        let message1 = accumulate(session.respond(&[user_message("hi")], 0))
            .await
            .unwrap();
        assert_eq!(message1.usage.cache_read, 0);
        assert_eq!(message1.usage.cache_write, 2); // ceil(7 / 4)
        assert_eq!(message1.usage.input, 2); // ceil(7 / 4) - cache_read(0)
        assert_eq!(message1.usage.output, 1); // ceil(2 / 4)

        // Request 2: transcript serializes to "user:hi\n\nuser:yo" (16
        // chars), which starts with all 7 chars of request 1's prompt.
        let message2 = accumulate(
            session.respond(&[user_message("hi"), user_message("yo")], 0),
        )
        .await
        .unwrap();
        assert_eq!(message2.usage.cache_read, 2); // ceil(7 / 4)
        assert_eq!(message2.usage.cache_write, 3); // ceil((16 - 7) / 4)
        assert_eq!(message2.usage.input, 2); // ceil(16 / 4) - cache_read(2)
        assert_eq!(message2.usage.output, 1);

        // Request 3: transcript serializes to "assistant:zzz" (13 chars),
        // sharing no prefix at all with request 2's prompt.
        let message3 =
            accumulate(session.respond(&[assistant_text_message("zzz")], 0))
                .await
                .unwrap();
        assert_eq!(message3.usage.cache_read, 0);
        assert_eq!(message3.usage.cache_write, 4); // ceil(13 / 4)
        assert_eq!(message3.usage.input, 4); // ceil(13 / 4) - cache_read(0)
        assert_eq!(message3.usage.output, 1);
    });
}

/// The cache estimate covers every message kind the transcript can hold,
/// not just plain user text: an image block's placeholder text
/// (`input_block_text`) and a tool result's own text
/// (`tool_result_text`) both count toward the serialized prompt.
#[test]
fn cache_estimate_covers_image_blocks_and_tool_results() {
    tau_testing::block_on(async {
        let model = ScriptedModel::new()
            .turn(|t| t.text("ok"))
            .turn(|t| t.text("ok"));

        // "user:[image:image/png:4]" = 5 + 19 = 24 chars -> ceil(24 / 4) = 6
        let mut session_a =
            model.clone().open(settings("gpt-test")).await.unwrap();
        let image_transcript = vec![Message::User(UserMessage {
            content: UserContent::Blocks(vec![InputBlock::Image(
                ImageContent {
                    data: "AAAA".to_owned(),
                    mime_type: "image/png".to_owned(),
                },
            )]),
            timestamp: 0,
        })];
        let message = accumulate(session_a.respond(&image_transcript, 0))
            .await
            .unwrap();
        assert_eq!(message.usage.cache_read, 0);
        assert_eq!(message.usage.cache_write, 6);
        assert_eq!(message.usage.input, 6);

        // "toolResult:grep\nresult text" = 11 + 16 = 27 chars -> ceil(27 / 4) = 7
        let mut session_b = model.open(settings("gpt-test")).await.unwrap();
        let tool_result_transcript =
            vec![Message::ToolResult(ToolResultMessage {
                tool_call_id: "call_1|fc_1".to_owned(),
                tool_name: "grep".to_owned(),
                content: vec![InputBlock::Text(TextContent {
                    text: "result text".to_owned(),
                    text_signature: None,
                })],
                details: None,
                is_error: false,
                timestamp: 0,
            })];
        let message = accumulate(session_b.respond(&tool_result_transcript, 0))
            .await
            .unwrap();
        assert_eq!(message.usage.cache_read, 0);
        assert_eq!(message.usage.cache_write, 7);
        assert_eq!(message.usage.input, 7);
    });
}

// =============================================================================
// Delay
// =============================================================================

/// A scripted delay sleeps on tokio's clock, so it costs no real time under
/// `tau_testing::block_on`'s paused runtime.
#[test]
fn delay_advances_only_the_paused_clock() {
    tau_testing::block_on(async {
        let model = ScriptedModel::new()
            .turn(|t| t.text("slow").delay(Duration::from_secs(30)));
        let mut session = model.open(settings("gpt-test")).await.unwrap();

        let start = tokio::time::Instant::now();
        let message = accumulate(session.respond(&[], 0)).await.unwrap();

        assert_eq!(text_of(&message), vec!["slow"]);
        assert!(tokio::time::Instant::now() - start >= Duration::from_secs(30));
    });
}

// =============================================================================
// Dropping the stream
// =============================================================================

/// Dropping a response stream before its terminal event does not corrupt
/// the session or the model: the next request is answered normally, from
/// the next scripted turn.
#[test]
fn dropping_the_stream_mid_way_is_safe() {
    tau_testing::block_on(async {
        let model = ScriptedModel::new()
            .turn(|t| t.text("a turn with more than one delta of text in it"))
            .turn(|t| t.text("second"));
        let mut session = model.open(settings("gpt-test")).await.unwrap();

        {
            let mut stream = session.respond(&[], 0);
            let _ = stream.next().await;
            // `stream` is dropped here, before its terminal event.
        }

        let message = accumulate(session.respond(&[], 0)).await.unwrap();
        assert_eq!(text_of(&message), vec!["second"]);
        model.assert_exhausted();
    });
}

// =============================================================================
// Response ids and settings
// =============================================================================

/// Each request gets its own, non-empty response id.
#[test]
fn response_ids_are_distinct_across_requests() {
    tau_testing::block_on(async {
        let model = ScriptedModel::new()
            .turn(|t| t.text("a"))
            .turn(|t| t.text("b"));
        let mut session = model.open(settings("gpt-test")).await.unwrap();

        let mut first = session.respond(&[], 0);
        let id1 = match first.next().await {
            Some(AssistantEvent::Start { response_id, .. }) => response_id,
            other => panic!("expected Start, got {other:?}"),
        };
        while first.next().await.is_some() {}

        let mut second = session.respond(&[], 0);
        let id2 = match second.next().await {
            Some(AssistantEvent::Start { response_id, .. }) => response_id,
            other => panic!("expected Start, got {other:?}"),
        };

        assert!(id1.as_deref().is_some_and(|id| !id.is_empty()));
        assert!(id2.as_deref().is_some_and(|id| !id.is_empty()));
        assert_ne!(id1, id2);
    });
}

/// A session reports exactly the settings it was opened with.
#[test]
fn session_reports_the_settings_it_was_opened_with() {
    tau_testing::block_on(async {
        let model = ScriptedModel::new().turn(|t| t.text("hi"));
        let opened_with = settings("gpt-test-settings");
        let session = model.open(opened_with.clone()).await.unwrap();
        assert_eq!(session.settings(), &opened_with);
    });
}

// =============================================================================
// Deterministic streaming
// =============================================================================

/// A turn's text streams as more than one delta when it is long enough to
/// split, and the deltas concatenate back to exactly the scripted text.
#[test]
fn text_deltas_concatenate_to_the_scripted_text() {
    tau_testing::block_on(async {
        let text = "the quick brown fox jumps";
        let model = ScriptedModel::new().turn(|t| t.text(text));
        let mut session = model.open(settings("gpt-test")).await.unwrap();
        let mut stream = session.respond(&[], 0);

        let mut deltas = Vec::new();
        while let Some(event) = stream.next().await {
            if let AssistantEvent::TextDelta { delta, .. } = event {
                deltas.push(delta);
            }
        }

        assert!(
            deltas.len() > 1,
            "a text this long should stream more than one delta, got {deltas:?}"
        );
        assert_eq!(deltas.concat(), text);
    });
}

// =============================================================================
// turn_with
// =============================================================================

/// `turn_with` computes its response from the incoming transcript, like
/// pi's `FauxResponseFactory`.
#[test]
fn turn_with_computes_from_the_incoming_transcript() {
    tau_testing::block_on(async {
        let model = ScriptedModel::new().turn_with(|transcript| {
            let count = transcript.len();
            AssistantMessage {
                content: vec![AssistantBlock::Text(TextContent {
                    text: format!("saw {count} messages"),
                    text_signature: None,
                })],
                api: API.to_owned(),
                provider: PROVIDER.to_owned(),
                model: "gpt-test".to_owned(),
                response_id: None,
                usage: Usage::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 0,
            }
        });
        let mut session = model.open(settings("gpt-test")).await.unwrap();

        let message = accumulate(
            session.respond(&[user_message("hi"), user_message("yo")], 0),
        )
        .await
        .unwrap();

        assert_eq!(text_of(&message), vec!["saw 2 messages"]);
    });
}
