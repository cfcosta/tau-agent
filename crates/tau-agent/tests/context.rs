//! The loop's token estimate and overflow check (`tau_agent::context`;
//! `docs/reference/compaction.md`, "Token estimate"), which every
//! compaction compacts by.
//!
//! Oracle: `docs/reference/compaction.md`, and pi's
//! `packages/coding-agent/src/core/compaction/{compaction,utils}.ts`.
//! See `docs/reference/testing.md`'s `tau-agent` property inventory for
//! the rows this file covers.

use hegel::{TestCase, generators as gs};
use serde_json::{Map, json};
use tau_agent::context::{
    estimate_context_tokens,
    estimate_message_tokens,
    is_context_overflow,
};
use tau_ai::message::{
    AssistantBlock,
    AssistantMessage,
    ImageContent,
    InputBlock,
    Message,
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
        model: "gpt-5.5".to_owned(),
        response_id: None,
        usage,
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 0,
    })
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

#[derive(Debug, Clone, Copy, hegel::PrettyPrintable)]
enum TokenEstimateMessageKind {
    UserText,
    UserBlocks,
    AssistantText,
    AssistantThinking,
    AssistantToolCall,
    /// Text, thinking and a tool call in one message: their characters
    /// are added before the message is rounded once.
    AssistantMixed,
    ToolResultText,
    ToolResultImages,
}

#[derive(Debug)]
struct MessageWithCharacterCount {
    message: Message,
    character_count: usize,
}

#[derive(Debug)]
enum TinyJsonArgument {
    Null,
    Boolean(bool),
    Integer(u16),
    Unicode(String),
}

impl TinyJsonArgument {
    fn as_json_value(&self) -> serde_json::Value {
        match self {
            Self::Null => serde_json::Value::Null,
            Self::Boolean(value) => json!(value),
            Self::Integer(value) => json!(value),
            Self::Unicode(value) => json!(value),
        }
    }

    /// Counts compact JSON characters from the scalar's semantics. Unicode
    /// strings are drawn only from non-escaped scalar values.
    fn compact_json_character_count(&self) -> usize {
        match self {
            Self::Null => 4,
            Self::Boolean(false) => 5,
            Self::Boolean(true) => 4,
            Self::Integer(value) => value.to_string().chars().count(),
            Self::Unicode(value) => value.chars().count() + 2,
        }
    }
}

const IMAGE_ESTIMATE_CHARACTERS: usize = 4_800;

/// Draws bounded text from ASCII and unescaped Unicode scalar values.
#[hegel::test_helper]
fn draw_text_scalars(
    tc: &TestCase,
    min_scalars: usize,
    max_scalars: usize,
) -> String {
    let scalars: Vec<char> = tc.draw(
        gs::vecs(gs::sampled_from(vec!['a', 'é', '日', '🦀']))
            .min_size(min_scalars)
            .max_size(max_scalars),
    );
    scalars.into_iter().collect()
}

/// Draws one tiny JSON scalar directly, so the character oracle can count
/// its canonical compact representation without asking serde to render it.
#[hegel::test_helper]
fn draw_tiny_json_argument(tc: &TestCase) -> TinyJsonArgument {
    match tc.draw(gs::integers::<u8>().max_value(3)) {
        0 => TinyJsonArgument::Null,
        1 => TinyJsonArgument::Boolean(tc.draw(gs::booleans())),
        2 => TinyJsonArgument::Integer(
            tc.draw(gs::integers::<u16>().max_value(99)),
        ),
        _ => TinyJsonArgument::Unicode(draw_text_scalars(tc, 1, 3)),
    }
}

fn image_input_block() -> InputBlock {
    InputBlock::Image(ImageContent {
        data: String::new(),
        mime_type: "image/png".to_owned(),
    })
}

/// Builds an assistant tool-call message and counts its semantic parts:
/// tool-name scalars plus the compact JSON object syntax and scalar values.
fn counted_tool_call(
    name: String,
    fields: Vec<(String, TinyJsonArgument)>,
) -> MessageWithCharacterCount {
    let json_character_count = 2
        + fields.len().saturating_sub(1)
        + fields
            .iter()
            .map(|(key, value)| {
                key.chars().count() + 3 + value.compact_json_character_count()
            })
            .sum::<usize>();
    let arguments = Map::from_iter(
        fields
            .iter()
            .map(|(key, value)| (key.clone(), value.as_json_value())),
    );
    MessageWithCharacterCount {
        character_count: name.chars().count() + json_character_count,
        message: Message::Assistant(assistant_response(
            StopReason::ToolUse,
            None,
            vec![AssistantBlock::ToolCall(ToolCall {
                id: "call|fc".to_owned(),
                name,
                arguments,
            })],
        )),
    }
}

/// Draws one valid message and its independent semantic character count.
#[hegel::test_helper]
fn draw_counted_message(
    tc: &TestCase,
    kind: TokenEstimateMessageKind,
) -> MessageWithCharacterCount {
    match kind {
        TokenEstimateMessageKind::UserText => {
            // The fixed five-character prefix guarantees this generated
            // message distinguishes Unicode scalars from UTF-8 bytes and
            // rounds up independently of all other messages.
            let suffix: char =
                tc.draw(gs::sampled_from(vec!['a', 'é', '日', '🦀']));
            let text = format!("ééééx{suffix}");
            MessageWithCharacterCount {
                character_count: text.chars().count(),
                message: user_text(&text),
            }
        }
        TokenEstimateMessageKind::UserBlocks => {
            let text = draw_text_scalars(tc, 0, 8);
            let character_count =
                text.chars().count() + IMAGE_ESTIMATE_CHARACTERS;
            MessageWithCharacterCount {
                character_count,
                message: Message::User(UserMessage {
                    content: UserContent::Blocks(vec![
                        InputBlock::Text(TextContent {
                            text,
                            text_signature: None,
                        }),
                        image_input_block(),
                    ]),
                    timestamp: 0,
                }),
            }
        }
        TokenEstimateMessageKind::AssistantText => {
            let text = draw_text_scalars(tc, 0, 8);
            MessageWithCharacterCount {
                character_count: text.chars().count(),
                message: assistant_text(&text),
            }
        }
        TokenEstimateMessageKind::AssistantThinking => {
            let thinking = draw_text_scalars(tc, 0, 8);
            MessageWithCharacterCount {
                character_count: thinking.chars().count(),
                message: Message::Assistant(assistant_response(
                    StopReason::Stop,
                    None,
                    vec![AssistantBlock::Thinking(ThinkingContent {
                        thinking,
                        thinking_signature: None,
                        redacted: None,
                    })],
                )),
            }
        }
        TokenEstimateMessageKind::AssistantToolCall => {
            let name = draw_text_scalars(tc, 1, 6);
            let fields = vec![
                ("n".to_owned(), TinyJsonArgument::Null),
                ("b".to_owned(), TinyJsonArgument::Boolean(false)),
                ("i".to_owned(), TinyJsonArgument::Integer(12)),
                ("s".to_owned(), TinyJsonArgument::Unicode("é".to_owned())),
                ("v".to_owned(), draw_tiny_json_argument(tc)),
            ];
            counted_tool_call(name, fields)
        }
        TokenEstimateMessageKind::AssistantMixed => {
            let text = draw_text_scalars(tc, 0, 8);
            let thinking = draw_text_scalars(tc, 0, 8);
            let call = counted_tool_call(
                draw_text_scalars(tc, 1, 6),
                vec![("v".to_owned(), draw_tiny_json_argument(tc))],
            );
            let Message::Assistant(mut assistant) = call.message else {
                unreachable!("counted_tool_call builds an assistant message")
            };
            let character_count = text.chars().count()
                + thinking.chars().count()
                + call.character_count;
            assistant.content.splice(
                0..0,
                [
                    text_block(&text),
                    AssistantBlock::Thinking(ThinkingContent {
                        thinking,
                        thinking_signature: None,
                        redacted: None,
                    }),
                ],
            );
            MessageWithCharacterCount {
                character_count,
                message: Message::Assistant(assistant),
            }
        }
        TokenEstimateMessageKind::ToolResultText => {
            let text = draw_text_scalars(tc, 0, 8);
            MessageWithCharacterCount {
                character_count: text.chars().count(),
                message: Message::ToolResult(ToolResultMessage {
                    tool_call_id: "call|fc".to_owned(),
                    tool_name: "read".to_owned(),
                    content: vec![InputBlock::Text(TextContent {
                        text,
                        text_signature: None,
                    })],
                    details: None,
                    is_error: false,
                    timestamp: 0,
                }),
            }
        }
        TokenEstimateMessageKind::ToolResultImages => {
            let text = draw_text_scalars(tc, 0, 8);
            let character_count =
                text.chars().count() + 2 * IMAGE_ESTIMATE_CHARACTERS;
            MessageWithCharacterCount {
                character_count,
                message: Message::ToolResult(ToolResultMessage {
                    tool_call_id: "call|fc".to_owned(),
                    tool_name: "read".to_owned(),
                    content: vec![
                        InputBlock::Text(TextContent {
                            text,
                            text_signature: None,
                        }),
                        image_input_block(),
                        image_input_block(),
                    ],
                    details: None,
                    is_error: false,
                    timestamp: 0,
                }),
            }
        }
    }
}

/// With no reported usage, context tokens equal the sum of independently
/// rounded semantic character counts (`docs/reference/testing.md`;
/// `docs/reference/compaction.md`, "Token estimate").
///
/// Property inventory: the oracle carries scalar counts from original text,
/// 4,800 characters per image, and manually counted compact scalar JSON for
/// tool calls. Assistant constructors set zero usage. Every case contains all
/// eight message forms, one of them an assistant message whose text,
/// thinking and tool call are added before it is rounded, plus up to eight
/// generated forms; text is bounded to eight scalars, names to six, and JSON
/// integers to 0..=99. Shrinking can simplify generated fields and remove
/// extra messages while retaining the required forms.
#[hegel::test(test_cases = 500)]
fn context_estimate_rounds_semantic_characters_per_unreported_message(
    tc: TestCase,
) {
    let kinds = vec![
        TokenEstimateMessageKind::UserText,
        TokenEstimateMessageKind::UserBlocks,
        TokenEstimateMessageKind::AssistantText,
        TokenEstimateMessageKind::AssistantThinking,
        TokenEstimateMessageKind::AssistantToolCall,
        TokenEstimateMessageKind::AssistantMixed,
        TokenEstimateMessageKind::ToolResultText,
        TokenEstimateMessageKind::ToolResultImages,
    ];
    let additional_kinds: Vec<TokenEstimateMessageKind> = tc.draw(
        gs::vecs(gs::sampled_from(vec![
            TokenEstimateMessageKind::UserText,
            TokenEstimateMessageKind::UserBlocks,
            TokenEstimateMessageKind::AssistantText,
            TokenEstimateMessageKind::AssistantThinking,
            TokenEstimateMessageKind::AssistantToolCall,
            TokenEstimateMessageKind::AssistantMixed,
            TokenEstimateMessageKind::ToolResultText,
            TokenEstimateMessageKind::ToolResultImages,
        ]))
        .max_size(8),
    );
    let counted_messages: Vec<_> = kinds
        .into_iter()
        .chain(additional_kinds)
        .map(|kind| draw_counted_message(&tc, kind))
        .collect();

    let expected: u64 = counted_messages
        .iter()
        .map(|entry| entry.character_count.div_ceil(4) as u64)
        .sum();
    let messages: Vec<Message> = counted_messages
        .into_iter()
        .map(|entry| entry.message)
        .collect();
    assert_eq!(estimate_context_tokens(&messages), expected);
}

/// A response's usage anchors the estimate when any one of its five
/// counts is nonzero, and then stands for the whole context before it:
/// the reported total when there is one, else the sum of the four
/// counters, cached tokens included (pi's `calculateContextTokens`;
/// `docs/reference/compaction.md`, "Token estimate"). Usage that is all
/// zero is no report, and the estimate falls back to characters: pi
/// issue #8328, pinned as the explicit case of all five counts zero.
/// Each count is drawn as zero half the time, so every combination of
/// reported fields comes up.
#[hegel::test(test_cases = 500)]
#[hegel::explicit_test_case(
    input = 0_u64,
    output = 0_u64,
    cache_read = 0_u64,
    cache_write = 0_u64,
    total_tokens = 0_u64
)]
fn a_reported_usage_anchors_the_estimate_at_its_context(tc: TestCase) {
    let count = || {
        hegel::one_of!(gs::just(0u64), gs::integers::<u64>().max_value(1 << 40))
    };
    let input = tc.draw(count());
    let output = tc.draw(count());
    let cache_read = tc.draw(count());
    let cache_write = tc.draw(count());
    let total_tokens = tc.draw(count());
    let usage = Usage {
        input,
        output,
        cache_read,
        cache_write,
        reasoning: None,
        total_tokens,
        cost: UsageCost::default(),
    };
    let counters =
        usage.input + usage.output + usage.cache_read + usage.cache_write;
    // 400 characters, then "ok": 100 and 1 token by characters.
    let expected = if usage.total_tokens > 0 {
        usage.total_tokens
    } else if counters > 0 {
        counters
    } else {
        101
    };
    let messages =
        vec![user_text(&"z".repeat(400)), assistant_with_usage(usage)];
    assert_eq!(estimate_context_tokens(&messages), expected);
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
