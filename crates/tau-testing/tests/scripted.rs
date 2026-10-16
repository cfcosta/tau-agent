//! `ScriptedModel` behaviour, per `docs/reference/testing.md`'s
//! "ScriptedModel" section.

use std::time::Duration;

use futures_util::StreamExt;
use hegel::{
    TestCase,
    generators::{self as gs, Generator as _, PrintableGenerator},
};
use serde_json::{Map, Value};
use tau_ai::{
    event::{Accumulator, AssistantEvent, ErrorReason, GrammarError},
    llm::{EventStream, Llm},
    message::{
        AssistantBlock,
        AssistantMessage,
        ImageContent,
        InputBlock,
        Message,
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

#[derive(Clone, Debug, hegel::PrettyPrintable)]
enum GenBlock {
    Text(String),
    Thinking(String),
    ToolCall(String, Map<String, Value>),
}

fn gen_block() -> impl PrintableGenerator<GenBlock> {
    hegel::one_of!(
        tau_testing::generators::text(20).map(GenBlock::Text),
        tau_testing::generators::text(20).map(GenBlock::Thinking),
        hegel::tuples!(
            tau_testing::generators::id("tool_"),
            tau_testing::generators::json_object(1),
        )
        .map(|(name, arguments)| GenBlock::ToolCall(name, arguments)),
    )
}

#[derive(Clone, Debug, hegel::PrettyPrintable)]
struct GenTurn {
    blocks: Vec<GenBlock>,
    // Our own library type: printed through Debug.
    #[pretty(debug)]
    stop: StopReason,
    usage: (u64, u64),
}

#[hegel::composite]
fn gen_turn(tc: &TestCase) -> GenTurn {
    let blocks = tc.draw(gs::vecs(gen_block()).max_size(4));
    let has_tool_call = blocks
        .iter()
        .any(|block| matches!(block, GenBlock::ToolCall(..)));
    let stop = if has_tool_call {
        StopReason::ToolUse
    } else {
        tc.draw(
            gs::sampled_from(vec![StopReason::Stop, StopReason::Length])
                .print_as_debug(),
        )
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
/// satisfies the `Accumulator` grammar (`crates/tau-ai/src/event.rs`),
/// streams each text or thought of two or more characters as several
/// deltas, and the accumulated message holds exactly the scripted
/// content, usage and stop reason. A fresh session has no cached prefix, so `cache_read` is
/// always 0 and `usage.input`/`usage.output` equal the scripted override
/// exactly (see the module docs' "Cache simulation").
#[hegel::test(test_cases = 200)]
fn scripted_turn_matches_grammar_and_content(tc: TestCase) {
    let turn = tc.draw(gen_turn());
    let blocks = turn.blocks.clone();
    let stop = turn.stop;
    let usage = turn.usage;
    let model = script_model_with(turn);

    let events: Vec<AssistantEvent> = tau_testing::block_on(async {
        let mut session = model.open(settings("gpt-test")).await.unwrap();
        session.respond(&[], 0).collect().await
    });
    let mut acc = Accumulator::new();
    for event in events.iter().cloned() {
        acc.push(event)
            .expect("a scripted turn's stream always satisfies the grammar");
    }
    let message = acc
        .finish()
        .expect("a scripted turn's stream always satisfies the grammar");
    // A text or a thought two characters long or more streams as more
    // than one delta, none of them empty.
    for (index, block) in blocks.iter().enumerate() {
        let (GenBlock::Text(text) | GenBlock::Thinking(text)) = block else {
            continue;
        };
        let deltas: Vec<&String> = events
            .iter()
            .filter_map(|event| match event {
                AssistantEvent::TextDelta { index: at, delta }
                | AssistantEvent::ThinkingDelta { index: at, delta }
                    if *at == index =>
                {
                    Some(delta)
                }
                _ => None,
            })
            .collect();
        let long = text.chars().count() >= 2;
        assert_eq!(deltas.len() > 1, long, "{text:?}: {deltas:?}");
        assert!(!long || deltas.iter().all(|delta| !delta.is_empty()));
    }

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
        let mut session_b =
            model.clone().open(settings("gpt-test")).await.unwrap();

        let first = accumulate(session_a.respond(&[], 0)).await.unwrap();
        let second = accumulate(session_b.respond(&[], 0)).await.unwrap();

        assert_eq!(text_of(&first), vec!["first"]);
        assert_eq!(text_of(&second), vec!["second"]);
        model.assert_exhausted();
    });
}

/// A scripted cost is reported as the turn's output and total cost, on
/// error turns too; a turn without one costs nothing.
#[test]
fn scripted_cost_is_reported_in_usage() {
    tau_testing::block_on(async {
        let model = ScriptedModel::new()
            .turn(|t| t.text("paid").cost(0.25))
            .turn(|t| t.error("server_error", "boom").cost(0.5))
            .turn(|t| t.text("free"));
        let mut session = model.open(settings("gpt-test")).await.unwrap();
        let mut costs = Vec::new();
        for _ in 0..3 {
            let message = accumulate(session.respond(&[], 0)).await.unwrap();
            let cost = message.usage.cost;
            assert_eq!(cost.output, cost.total);
            assert_eq!(cost.input + cost.cache_read + cost.cache_write, 0.0);
            costs.push(cost.total);
        }
        assert_eq!(costs, vec![0.25, 0.5, 0.0]);
    });
}

/// Each scripted failure, and a script run out, is a stream of exactly
/// `Start` then the documented terminal `Error`, with no content in
/// between: its message, and the retry class the loop acts on. An error
/// is classified by its code, a failure before start is retryable, and a
/// drop mid-stream or an exhausted script is not.
#[test]
fn scripted_failures_carry_their_message_and_retry_class() {
    use tau_ai::retry::Class;
    use tau_testing::scripted::EXHAUSTED_MESSAGE;
    tau_testing::block_on(async {
        let model = ScriptedModel::new()
            .turn(|t| t.error("rate_limit_exceeded", "slow down"))
            .turn(|t| t.error("insufficient_quota", "pay up"))
            .turn(|t| t.error("context_length_exceeded", "too long"))
            .turn(|t| t.fails_before_start())
            .turn(|t| t.dropped());
        let mut session = model.open(settings("gpt-test")).await.unwrap();
        for (expected_message, expected_class) in [
            ("rate_limit_exceeded: slow down", Class::Retryable),
            ("insufficient_quota: pay up", Class::Fatal),
            ("context_length_exceeded: too long", Class::ContextOverflow),
            (FAILS_BEFORE_START_MESSAGE, Class::Retryable),
            (DROPPED_MESSAGE, Class::Fatal),
            (EXHAUSTED_MESSAGE, Class::Fatal),
        ] {
            let events: Vec<AssistantEvent> =
                session.respond(&[], 0).collect().await;
            match &events[..] {
                [
                    AssistantEvent::Start { .. },
                    AssistantEvent::Error {
                        reason,
                        message,
                        class,
                        ..
                    },
                ] => {
                    assert_eq!(
                        (*reason, message.as_str(), *class),
                        (ErrorReason::Error, expected_message, expected_class)
                    );
                }
                other => panic!("{expected_message}: {other:?}"),
            }
        }
    });
}

/// A warm-up records the session's settings, draws no scripted turn and
/// costs nothing.
#[test]
fn warm_ups_are_recorded_and_free() {
    tau_testing::block_on(async {
        let model = ScriptedModel::new().turn(|t| t.text("hi"));
        let mut session = model.open(settings("gpt-warm")).await.unwrap();
        let usage = session.warm_up(0).await.unwrap();
        assert_eq!(usage, Usage::default());
        assert_eq!(model.warm_ups(), vec![settings("gpt-warm")]);
        assert_eq!(model.remaining(), 1);
        assert!(model.requests().is_empty());
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
#[should_panic(expected = "scripted turn(s) were never used")]
fn assert_exhausted_panics_when_turns_are_left() {
    let model = ScriptedModel::new().turn(|t| t.text("hi"));
    model.assert_exhausted();
}

// =============================================================================
// Errors
// =============================================================================

// =============================================================================
// Cache simulation
// =============================================================================

/// `ceil(chars / 4)`: pi's `estimateTokens`, in Unicode scalar values.
fn tokens_of(chars: usize) -> u64 {
    (chars as u64).div_ceil(4)
}

/// pi's `serializeContext` for a transcript of user texts:
/// `"user:{text}"` per message, joined by a blank line.
fn serialized_users(texts: &[String]) -> String {
    texts
        .iter()
        .map(|text| format!("user:{text}"))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The cache simulation over drawn user transcripts, against pi's faux
/// formula: a session's first request reads nothing from the cache and
/// writes its whole prompt; the next one reads the characters it shares
/// with the first as a prefix (all of the first prompt when it extends
/// that transcript), writes the rest, and pays `input` for the prompt
/// less what it read. Every count is `ceil(chars / 4)`.
#[hegel::test(test_cases = 200)]
fn cache_simulation_follows_pi_faux_formula(tc: TestCase) {
    let text = || tau_testing::generators::text(12);
    let first: Vec<String> = tc.draw(gs::vecs(text()).min_size(1).max_size(3));
    let extends = tc.draw(gs::booleans());
    let second: Vec<String> = if extends {
        let more: Vec<String> = tc.draw(gs::vecs(text()).max_size(3));
        first.iter().cloned().chain(more).collect()
    } else {
        tc.draw(gs::vecs(text()).min_size(1).max_size(3))
    };
    let (prompt1, prompt2) =
        (serialized_users(&first), serialized_users(&second));
    let shared = prompt1
        .chars()
        .zip(prompt2.chars())
        .take_while(|(a, b)| a == b)
        .count();
    let (length1, length2) = (prompt1.chars().count(), prompt2.chars().count());
    if extends {
        tc.event("the second request extends the first");
        assert_eq!(shared, length1);
    }
    let messages = |texts: &[String]| {
        texts.iter().map(|t| user_message(t)).collect::<Vec<_>>()
    };
    tau_testing::block_on(async {
        let model = ScriptedModel::new()
            .turn(|t| t.text("ok"))
            .turn(|t| t.text("ok"));
        let mut session = model.open(settings("gpt-test")).await.unwrap();
        let one = accumulate(session.respond(&messages(&first), 0))
            .await
            .unwrap();
        assert_eq!(
            (one.usage.cache_read, one.usage.cache_write, one.usage.input),
            (0, tokens_of(length1), tokens_of(length1)),
            "first request"
        );
        let two = accumulate(session.respond(&messages(&second), 0))
            .await
            .unwrap();
        assert_eq!(two.usage.cache_read, tokens_of(shared), "cache read");
        assert_eq!(
            two.usage.cache_write,
            tokens_of(length2 - shared),
            "cache write"
        );
        assert_eq!(
            two.usage.cache_read + two.usage.input,
            tokens_of(length2),
            "the prompt, read or paid"
        );
        // "ok" is two characters.
        assert_eq!((one.usage.output, two.usage.output), (1, 1));
    });
}

/// The cache estimate covers every message kind the transcript can hold,
/// not just plain user text: an image block's placeholder text
/// (`input_block_text`), a tool result's own text (`tool_result_text`)
/// and an assistant's text all count toward the serialized prompt. A
/// request that shares no prefix with the session's last one reads
/// nothing from the cache, though the session has history (pi's faux
/// provider, `withUsageEstimate`).
#[test]
fn cache_estimate_covers_image_blocks_and_tool_results() {
    tau_testing::block_on(async {
        let model = ScriptedModel::new()
            .turn(|t| t.text("ok"))
            .turn(|t| t.text("ok"))
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
        let mut session_b =
            model.clone().open(settings("gpt-test")).await.unwrap();
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

        // "user:hi" first; then "assistant:zzz" (13 chars), which shares
        // no prefix with it: nothing read, ceil(13 / 4) = 4 written.
        let mut session_c = model.open(settings("gpt-test")).await.unwrap();
        accumulate(session_c.respond(&[user_message("hi")], 0))
            .await
            .unwrap();
        let message =
            accumulate(session_c.respond(&[assistant_text_message("zzz")], 0))
                .await
                .unwrap();
        assert_eq!(message.usage.cache_read, 0);
        assert_eq!(message.usage.cache_write, 4);
        assert_eq!(message.usage.input, 4);
        model.assert_exhausted();
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
