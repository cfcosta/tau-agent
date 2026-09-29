//! Transcript → OpenAI Responses `input` array (`tau_ai::responses::input`).
//!
//! Wire shapes are pinned from pi's `convertResponsesMessages`
//! (`packages/ai/src/api/openai-responses-shared.ts`, commit `2b0a123`).
//! The invariants and the reasoning-pairing rule are documented in
//! `crates/tau-ai/src/responses/input.rs` and in
//! `docs/reference/testing.md`'s `tau-ai` property inventory.

use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use hegel::{TestCase, generators as gs};
use serde_json::{Map, Value, json};
use tau_ai::{
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
        ThinkingContent,
        ToolCall,
        ToolResultMessage,
        Usage,
        UserContent,
        UserMessage,
    },
    responses::input::{
        InputCache,
        response_items,
        split_tool_call_id,
        to_input,
    },
};
use tau_testing::generators;

// =============================================================================
// Golden shapes, pinned from pi
// =============================================================================

/// pi always wraps a plain string user message in one `input_text`
/// block; it never sends `content` as a bare string.
/// (`openai-responses-shared.ts:230-234`)
#[test]
fn user_text_item_shape() {
    let messages = vec![Message::User(UserMessage {
        content: UserContent::Text("hi".into()),
        timestamp: 0,
    })];
    assert_eq!(
        to_input(&messages),
        vec![json!({
            "role": "user",
            "content": [{"type": "input_text", "text": "hi"}],
        })]
    );
}

/// A block list maps text to `input_text` and an image to `input_image`
/// with a `data:` URL. (`openai-responses-shared.ts:236-248`)
#[test]
fn user_blocks_item_shape() {
    let messages = vec![Message::User(UserMessage {
        content: UserContent::Blocks(vec![
            InputBlock::Text(TextContent {
                text: "look".into(),
                text_signature: None,
            }),
            InputBlock::Image(ImageContent {
                data: "AAAA".into(),
                mime_type: "image/png".into(),
            }),
        ]),
        timestamp: 0,
    })];
    assert_eq!(
        to_input(&messages),
        vec![json!({
            "role": "user",
            "content": [
                {"type": "input_text", "text": "look"},
                {"type": "input_image", "detail": "auto", "image_url": "data:image/png;base64,AAAA"},
            ],
        })]
    );
}

/// A user message with no content blocks is dropped entirely.
/// (`openai-responses-shared.ts:249`)
#[test]
fn user_message_with_no_content_is_skipped() {
    let messages = vec![Message::User(UserMessage {
        content: UserContent::Blocks(vec![]),
        timestamp: 0,
    })];
    assert!(to_input(&messages).is_empty());
}

/// A plain (legacy) `textSignature` is used verbatim as the message id.
/// (`openai-responses-shared.ts:59-77`, `:282-289`)
#[test]
fn assistant_text_item_uses_legacy_signature_as_id() {
    let messages =
        one_assistant_message(vec![AssistantBlock::Text(TextContent {
            text: "hi".into(),
            text_signature: Some("msg_abc".into()),
        })]);
    assert_eq!(
        to_input(&messages),
        vec![json!({
            "type": "message",
            "role": "assistant",
            "status": "completed",
            "id": "msg_abc",
            "content": [{"type": "output_text", "text": "hi", "annotations": []}],
        })]
    );
}

/// A `TextSignatureV1` JSON signature carries a structured id and,
/// optionally, a phase. (`openai-responses-shared.ts:53-77`)
#[test]
fn assistant_text_item_uses_v1_signature_with_phase() {
    let signature =
        json!({"v": 1, "id": "msg_v1", "phase": "final_answer"}).to_string();
    let messages =
        one_assistant_message(vec![AssistantBlock::Text(TextContent {
            text: "done".into(),
            text_signature: Some(signature),
        })]);
    assert_eq!(
        to_input(&messages),
        vec![json!({
            "type": "message",
            "role": "assistant",
            "status": "completed",
            "id": "msg_v1",
            "phase": "final_answer",
            "content": [{"type": "output_text", "text": "done", "annotations": []}],
        })]
    );
}

/// `"commentary"` is the other phase pi keeps.
/// (`openai-responses-shared.ts:67`)
#[test]
fn assistant_text_item_keeps_commentary_phase() {
    let signature =
        json!({"v": 1, "id": "msg_c", "phase": "commentary"}).to_string();
    let messages =
        one_assistant_message(vec![AssistantBlock::Text(TextContent {
            text: "hi".into(),
            text_signature: Some(signature),
        })]);
    assert_eq!(to_input(&messages)[0]["phase"], "commentary");
}

/// A phase that is neither `"commentary"` nor `"final_answer"` is
/// dropped, not passed through. (`openai-responses-shared.ts:67-69`)
#[test]
fn assistant_text_item_drops_unknown_phase() {
    let signature =
        json!({"v": 1, "id": "msg_d", "phase": "draft"}).to_string();
    let messages =
        one_assistant_message(vec![AssistantBlock::Text(TextContent {
            text: "hi".into(),
            text_signature: Some(signature),
        })]);
    assert!(to_input(&messages)[0].get("phase").is_none());
}

/// A garbled `{`-prefixed signature that is not valid `TextSignatureV1`
/// JSON falls back to being used, whole, as the legacy id — it is not an
/// error. (`openai-responses-shared.ts:63-76`, the `catch` / fall-through)
#[test]
fn assistant_text_item_falls_back_to_raw_signature_on_bad_json() {
    let messages =
        one_assistant_message(vec![AssistantBlock::Text(TextContent {
            text: "hi".into(),
            text_signature: Some("{not json".into()),
        })]);
    let items = to_input(&messages);
    assert_eq!(items[0]["id"], "{not json");
}

/// With no signature at all, the id falls back to `msg_pi_{hash}`, a
/// hash of the message's `response_id`, the text block's position, and
/// the text itself — deviation from pi (`openai-responses-shared.ts:272-278`),
/// which counts emitted messages instead. See the module docs
/// ("Deviations from pi") for why ours must not depend on a transcript
/// position. The expected hashes are pinned from a from-scratch
/// reimplementation of `shortHash` in Python, not from running pi.
#[test]
fn assistant_text_item_fallback_id_with_no_signature() {
    let messages = one_assistant_message(vec![
        AssistantBlock::Text(TextContent {
            text: "a".into(),
            text_signature: None,
        }),
        AssistantBlock::Text(TextContent {
            text: "b".into(),
            text_signature: None,
        }),
    ]);
    let items = to_input(&messages);
    assert_eq!(items[0]["id"], "msg_pi_1pjp42klgo7y3");
    assert_eq!(items[1]["id"], "msg_pi_1o59vyycw4679");
}

/// The fallback id also depends on the message's `response_id`, which is
/// available at completion time (unlike a transcript position) and keeps
/// two different assistant messages with identical, signature-less text
/// from colliding.
#[test]
fn assistant_text_item_fallback_id_uses_response_id() {
    let mut assistant = assistant_with(
        vec![AssistantBlock::Text(TextContent {
            text: "hi".into(),
            text_signature: None,
        })],
        StopReason::Stop,
    );
    assistant.response_id = Some("resp_1".into());
    let items = to_input(&[Message::Assistant(assistant)]);
    assert_eq!(items[0]["id"], "msg_pi_ye7ywj10a8cx6");
}

/// A signature id of exactly 64 characters is kept as-is: OpenAI's limit
/// is inclusive. (`openai-responses-shared.ts:275-281`)
#[test]
fn assistant_text_item_id_at_64_chars_is_not_shortened() {
    let id = "y".repeat(64);
    let signature = json!({"v": 1, "id": id}).to_string();
    let messages =
        one_assistant_message(vec![AssistantBlock::Text(TextContent {
            text: "hi".into(),
            text_signature: Some(signature),
        })]);
    assert_eq!(to_input(&messages)[0]["id"], id);
}

/// A signature id past 64 characters is replaced by `msg_{shortHash}`.
/// The expected hash is pinned from a from-scratch reimplementation of
/// pi's `shortHash` (`packages/ai/src/utils/hash.ts`) in Python, not
/// from running pi itself. (`openai-responses-shared.ts:275-281`)
#[test]
fn assistant_text_item_id_past_64_chars_is_shortened() {
    let id = "y".repeat(65);
    let signature = json!({"v": 1, "id": id}).to_string();
    let messages =
        one_assistant_message(vec![AssistantBlock::Text(TextContent {
            text: "hi".into(),
            text_signature: Some(signature),
        })]);
    assert_eq!(to_input(&messages)[0]["id"], "msg_1htpiug1roojrq");
}

/// A `v` other than `1` is not `TextSignatureV1`: the id falls back to
/// the whole raw signature, not the JSON's `id` field.
/// (`openai-responses-shared.ts:66-76`)
#[test]
fn assistant_text_item_rejects_non_v1_signature() {
    let signature = json!({"v": 2, "id": "should_not_be_used"}).to_string();
    let messages =
        one_assistant_message(vec![AssistantBlock::Text(TextContent {
            text: "hi".into(),
            text_signature: Some(signature.clone()),
        })]);
    assert_eq!(to_input(&messages)[0]["id"], signature);
}

/// The reasoning item pi stored in `thinkingSignature` is replayed
/// unmodified. (`openai-responses-shared.ts:264-268`)
#[test]
fn assistant_reasoning_item_is_replayed_verbatim() {
    let reasoning_item = json!({"id": "rs_1", "type": "reasoning", "summary": [], "encrypted_content": "abc"});
    let messages = one_assistant_message(vec![
        AssistantBlock::Thinking(ThinkingContent {
            thinking: "...".into(),
            thinking_signature: Some(reasoning_item.to_string()),
            redacted: None,
        }),
        AssistantBlock::Text(TextContent {
            text: "answer".into(),
            text_signature: Some("msg_1".into()),
        }),
    ]);
    let items = to_input(&messages);
    assert_eq!(items[0], reasoning_item);
}

/// A thinking block with no signature carries nothing to replay, so it
/// is dropped rather than turned into text. (`openai-responses-shared.ts:264-268`,
/// no `else` branch)
#[test]
fn assistant_reasoning_without_signature_is_dropped() {
    let messages = one_assistant_message(vec![
        AssistantBlock::Thinking(ThinkingContent {
            thinking: "...".into(),
            thinking_signature: None,
            redacted: None,
        }),
        AssistantBlock::Text(TextContent {
            text: "answer".into(),
            text_signature: Some("msg_1".into()),
        }),
    ]);
    let items = to_input(&messages);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["type"], "message");
}

/// Deviation from pi: an unparsable `thinkingSignature` is dropped, not
/// a panic. pi's bare `JSON.parse` here has no `try`/`catch` and would
/// throw (`openai-responses-shared.ts:266`).
#[test]
fn assistant_reasoning_with_unparsable_signature_is_dropped_not_panicking() {
    let messages = one_assistant_message(vec![AssistantBlock::Thinking(
        ThinkingContent {
            thinking: "...".into(),
            thinking_signature: Some("{not json".into()),
            redacted: None,
        },
    )]);
    assert_eq!(to_input(&messages), Vec::<Value>::new());
}

/// A tool call's id splits into `call_id` (before `|`) and `id` (after).
/// (`openai-responses-shared.ts:290-328`)
#[test]
fn assistant_tool_call_item_shape() {
    let mut arguments = Map::new();
    arguments.insert("q".into(), json!("tokio"));
    let call = ToolCall {
        id: "call_1|fc_1".into(),
        name: "search".into(),
        arguments,
    };
    let messages = vec![
        Message::Assistant(assistant_with(
            vec![AssistantBlock::ToolCall(call)],
            StopReason::ToolUse,
        )),
        Message::ToolResult(ToolResultMessage {
            tool_call_id: "call_1|fc_1".into(),
            tool_name: "search".into(),
            content: vec![InputBlock::Text(TextContent {
                text: "found".into(),
                text_signature: None,
            })],
            details: None,
            is_error: false,
            timestamp: 1,
        }),
    ];
    let items = to_input(&messages);
    assert_eq!(
        items[0],
        json!({
            "type": "function_call",
            "call_id": "call_1",
            "id": "fc_1",
            "name": "search",
            "arguments": "{\"q\":\"tokio\"}",
        })
    );
}

/// A text-only tool result becomes a plain string output.
/// (`openai-responses-shared.ts:81-108`, `hasText` branch)
#[test]
fn tool_result_text_only_is_a_plain_string_output() {
    let items =
        to_input(&with_call_and_result(vec![InputBlock::Text(TextContent {
            text: "42".into(),
            text_signature: None,
        })]));
    let output = find_output(&items);
    assert_eq!(output["output"], json!("42"));
}

/// A tool result with an image becomes a content array of
/// `input_text`/`input_image` blocks. (`openai-responses-shared.ts:92-107`)
#[test]
fn tool_result_with_image_is_a_content_array() {
    let items = to_input(&with_call_and_result(vec![
        InputBlock::Text(TextContent {
            text: "see:".into(),
            text_signature: None,
        }),
        InputBlock::Image(ImageContent {
            data: "AAAA".into(),
            mime_type: "image/png".into(),
        }),
    ]));
    let output = find_output(&items);
    assert_eq!(
        output["output"],
        json!([
            {"type": "input_text", "text": "see:"},
            {"type": "input_image", "detail": "auto", "image_url": "data:image/png;base64,AAAA"},
        ])
    );
}

/// A tool result with no content at all becomes the `"(no tool
/// output)"` placeholder. (`openai-responses-shared.ts:93`)
#[test]
fn tool_result_with_no_content_is_a_placeholder() {
    let items = to_input(&with_call_and_result(vec![]));
    let output = find_output(&items);
    assert_eq!(output["output"], json!("(no tool output)"));
}

// =============================================================================
// Known cases, from pi at 2b0a123
// =============================================================================

/// pi: `packages/ai/test/openai-responses-reasoning-replay-e2e.test.ts`,
/// "skips reasoning-only history after an aborted turn". A turn that
/// aborted holding only reasoning must not replay an orphaned reasoning
/// item; tau-agent drops the whole turn (rule 3 in the module docs), and
/// later turns are unaffected.
#[test]
fn aborted_turn_holding_only_reasoning_is_dropped() {
    let messages = vec![
        Message::User(UserMessage {
            content: UserContent::Text(
                "Use the double_number tool to double 21.".into(),
            ),
            timestamp: 0,
        }),
        Message::Assistant(AssistantMessage {
            content: vec![AssistantBlock::Thinking(ThinkingContent {
                thinking: "Let me think.".into(),
                thinking_signature: Some(
                    json!({"id": "rs_1", "type": "reasoning", "summary": []})
                        .to_string(),
                ),
                redacted: None,
            })],
            api: API.to_owned(),
            provider: PROVIDER.to_owned(),
            model: "gpt-5-mini".into(),
            response_id: None,
            usage: Usage::default(),
            stop_reason: StopReason::Aborted,
            error_message: Some("aborted".into()),
            timestamp: 1,
        }),
        Message::User(UserMessage {
            content: UserContent::Text(
                "Say hello to confirm you can continue.".into(),
            ),
            timestamp: 2,
        }),
    ];
    let items = to_input(&messages);
    assert_eq!(
        items.len(),
        2,
        "only the two user items should survive: {items:?}"
    );
    assert!(items.iter().all(|item| item["type"] != "reasoning"));
}

/// pi: `packages/ai/test/tool-call-without-result.test.ts`, "should
/// filter out tool calls without corresponding tool results". A tool
/// call the user walked away from (no result ever recorded) is dropped,
/// and the conversation continues.
#[test]
fn tool_call_with_no_result_is_dropped() {
    let call = ToolCall {
        id: "call_1|fc_1".into(),
        name: "calculate".into(),
        arguments: Map::new(),
    };
    let messages = vec![
        Message::User(UserMessage {
            content: UserContent::Text("Please calculate 25 * 18.".into()),
            timestamp: 0,
        }),
        Message::Assistant(assistant_with(
            vec![AssistantBlock::ToolCall(call)],
            StopReason::ToolUse,
        )),
        Message::User(UserMessage {
            content: UserContent::Text(
                "Never mind, just tell me what is 2+2?".into(),
            ),
            timestamp: 2,
        }),
    ];
    let items = to_input(&messages);
    assert_eq!(
        items.len(),
        2,
        "the orphaned call should be dropped: {items:?}"
    );
    assert!(items.iter().all(|item| item["type"] != "function_call"));
}

/// pi: `packages/ai/test/tool-call-id-normalization.test.ts` — ids from
/// the OpenAI Responses API are `{call_id}|{id}`, where `{id}` can be
/// 400+ characters with `+`, `/`, `=` (the id from issue #1022).
#[test]
fn split_tool_call_id_known_cases() {
    let long = "call_pAYbIr76hXIjncD9UE4eGfnS|t5nnb2qYMFWGSsr13fhCd1CaCu3t3qONEPuOudu4HSVEtA8YJSL6FAZUxvoOoD792VIJWl91g87EdqsCWp9krVsdBysQoDaf9lMCLb8BS4EYi4gQd5kBQBYLlgD71PYwvf+TbMD9J9/5OMD42oxSRj8H+vRf78/l2Xla33LWz4nOgsddBlbvabICRs8GHt5C9PK5keFtzyi3lsyVKNlfduK3iphsZqs4MLv4zyGJnvZo/+QzShyk5xnMSQX/f98+aEoNflEApCdEOXipipgeiNWnpFSHbcwmMkZoJhURNu+JEz3xCh1mrXeYoN5o+trLL3IXJacSsLYXDrYTipZZbJFRPAucgbnjYBC+/ZzJOfkwCs+Gkw7EoZR7ZQgJ8ma+9586n4tT4cI8DEhBSZsWMjrCt8dxKg==";
    let (call_id, item_id) = split_tool_call_id(long);
    assert_eq!(call_id, "call_pAYbIr76hXIjncD9UE4eGfnS");
    assert_eq!(
        item_id,
        Some(&long["call_pAYbIr76hXIjncD9UE4eGfnS|".len()..])
    );

    // An id with no `|` at all (pi: `openai-responses-shared.ts:167`).
    assert_eq!(split_tool_call_id("bare_id"), ("bare_id", None));
}

/// Deviation from pi: `split_tool_call_id` splits at the *first* `|`
/// and keeps everything after it as `item_id`. pi's `id.split("|")`
/// destructuring (`openai-responses-shared.ts:292`) keeps only the
/// first two segments, silently dropping anything past a second `|`.
#[test]
fn split_tool_call_id_keeps_extra_pipes_unlike_pi() {
    assert_eq!(split_tool_call_id("a|b|c"), ("a", Some("b|c")));
}

// =============================================================================
// Properties, over generated transcripts
// =============================================================================

/// Invariants 1 and 2: every surviving `function_call` has exactly one
/// `function_call_output` claiming it later in the input, and every
/// output claims an earlier call — over damaged transcripts, where
/// results can be missing, extra, or answer a dropped (aborted/errored)
/// turn's calls (pi's orphan bug, `docs/reference/pi-audit.md`).
/// Converting the same transcript twice gives the same result
/// (determinism), checked here since it is cheap.
#[hegel::test(test_cases = 300)]
fn function_calls_and_outputs_pair_one_to_one(tc: TestCase) {
    let messages =
        shared_call_ids(&tc, tc.draw(generators::damaged_transcript()));
    let items = to_input(&messages);
    assert_eq!(
        to_input(&messages),
        items,
        "conversion is not deterministic"
    );

    let mut pending: HashMap<String, i64> = HashMap::new();
    for item in &items {
        // A user item has no "type" field at all (pi's shorthand); treat
        // anything but the two tool item types as "other".
        match item.get("type").and_then(Value::as_str).unwrap_or("") {
            "function_call" => {
                let call_id = item["call_id"].as_str().unwrap().to_owned();
                *pending.entry(call_id).or_insert(0) += 1;
            }
            "function_call_output" => {
                let call_id = item["call_id"].as_str().unwrap();
                let count = pending
                    .get_mut(call_id)
                    .filter(|c| **c > 0)
                    .unwrap_or_else(|| {
                        panic!(
                            "output with no matching call earlier: {call_id}"
                        )
                    });
                *count -= 1;
            }
            _ => {}
        }
    }
    let unmatched: Vec<_> = pending
        .into_iter()
        .filter(|&(_, count)| count != 0)
        .collect();
    assert!(
        unmatched.is_empty(),
        "function_call(s) with no output: {unmatched:?}"
    );
}

/// The reasoning pairing rule: every kept reasoning item is followed,
/// possibly after more reasoning items of its own message, by an
/// assistant `message` or `function_call` item of that same message.
/// Each assistant item is tagged with its message's position first (a
/// reasoning, text and call item id of `rs_`/`msg_`/`fc_{message}_{block}`),
/// so the check can tell which message an item came from.
///
/// Consecutive reasoning items are allowed: the rule trims only the
/// reasoning left trailing by a dropped call, as pi never drops
/// reasoning between two kept items.
#[hegel::test(test_cases = 300)]
fn no_reasoning_item_is_left_dangling(tc: TestCase) {
    let mut messages =
        shared_call_ids(&tc, tc.draw(generators::damaged_transcript()));
    tag_assistant_items(&mut messages);
    let items = to_input(&messages);
    for (i, item) in items.iter().enumerate() {
        if item["type"] != "reasoning" {
            continue;
        }
        tc.event("a reasoning item was kept");
        let origin = item_origin(item).expect("a tagged reasoning item");
        let mut next = i + 1;
        while next < items.len() && items[next]["type"] == "reasoning" {
            assert_eq!(
                item_origin(&items[next]),
                Some(origin),
                "reasoning at {i} is followed by another message's \
                 reasoning at {next}"
            );
            next += 1;
        }
        let Some(follower) = items.get(next) else {
            panic!("reasoning item at {i} has nothing after it: {item}");
        };
        let is_output = (follower["type"] == "message"
            && follower["role"] == "assistant")
            || follower["type"] == "function_call";
        assert!(
            is_output,
            "reasoning at {i} is followed by {follower}, not an output item"
        );
        assert_eq!(
            item_origin(follower),
            Some(origin),
            "reasoning at {i} is followed by another message's item"
        );
    }
}

/// Undamaged transcripts (every tool call answered, no aborted/errored
/// turn — see `generators::transcript`) drop nothing: the number of
/// surviving `function_call` items equals the number of tool calls in
/// the transcript, and every text appears, in the order a simple walk
/// of the transcript finds it.
#[hegel::test(test_cases = 300)]
fn undamaged_transcript_drops_nothing(tc: TestCase) {
    let messages = shared_call_ids(&tc, tc.draw(generators::transcript()));
    let items = to_input(&messages);

    let expected_calls: usize = messages
        .iter()
        .filter_map(|m| match m {
            Message::Assistant(a) => Some(
                a.content
                    .iter()
                    .filter(|b| matches!(b, AssistantBlock::ToolCall(_)))
                    .count(),
            ),
            _ => None,
        })
        .sum();
    let actual_calls = items
        .iter()
        .filter(|item| item["type"] == "function_call")
        .count();
    assert_eq!(actual_calls, expected_calls);

    assert_eq!(extract_texts(&items), walk_texts(&messages));
}

/// Ids: a surviving `function_call`'s `call_id` and `id`, joined back
/// with `|`, equal the original tool call id.
#[hegel::test(test_cases = 300)]
fn function_call_id_round_trips(tc: TestCase) {
    let call = tc.draw(generators::tool_call());
    let messages = vec![
        Message::Assistant(assistant_with(
            vec![AssistantBlock::ToolCall(call.clone())],
            StopReason::ToolUse,
        )),
        Message::ToolResult(
            tc.draw(generators::tool_result_for_call(call.clone())),
        ),
    ];
    let items = to_input(&messages);
    let function_call = items
        .iter()
        .find(|item| item["type"] == "function_call")
        .expect("the call has a matching result, so it survives");
    let call_id = function_call["call_id"].as_str().unwrap();
    let item_id = function_call["id"]
        .as_str()
        .expect("tau_testing's tool_call() ids always have an item_id half");
    assert_eq!(format!("{call_id}|{item_id}"), call.id);
}

/// `split_tool_call_id` loses nothing, whatever the id holds: the
/// `call_id` has no `|`, the `item_id` is there exactly when the id has
/// a `|`, and joining the halves back with `|` gives the id.
#[hegel::test]
fn split_tool_call_id_rejoins_to_the_id(tc: TestCase) {
    let id: String = tc.draw(gs::text().alphabet("ab|").max_size(8));
    let (call_id, item_id) = split_tool_call_id(&id);
    assert!(!call_id.contains('|'), "{call_id:?}");
    assert_eq!(item_id.is_some(), id.contains('|'));
    let rejoined = match item_id {
        Some(item_id) => format!("{call_id}|{item_id}"),
        None => call_id.to_owned(),
    };
    assert_eq!(rejoined, id);
}

/// The delta rule needs `response_items` to give the same items
/// `to_input` would give for that message once every one of its tool
/// calls has a result (see `response_items`'s own docs). For an
/// undamaged transcript that is always the case, so `to_input` of the
/// whole transcript must equal, walked message by message, `response_items`
/// of each assistant message spliced in at its position — checked here by
/// comparing `response_items(assistant)` against the slice of `to_input`'s
/// own output at the position a simple item-count walk predicts.
#[hegel::test(test_cases = 300)]
fn response_items_matches_to_input_for_undamaged_transcripts(tc: TestCase) {
    let messages = shared_call_ids(&tc, tc.draw(generators::transcript()));
    let actual = to_input(&messages);

    let mut cursor = 0;
    for message in &messages {
        match message {
            Message::User(user) => {
                let has_content = match &user.content {
                    UserContent::Text(_) => true,
                    UserContent::Blocks(blocks) => !blocks.is_empty(),
                };
                cursor += usize::from(has_content);
            }
            Message::Assistant(assistant) => {
                let items = response_items(assistant);
                assert!(
                    cursor + items.len() <= actual.len(),
                    "response_items is longer than what to_input kept"
                );
                assert_eq!(
                    &actual[cursor..cursor + items.len()],
                    items.as_slice(),
                    "response_items must match to_input's own conversion \
                     of an assistant message once nothing is dropped"
                );
                cursor += items.len();
            }
            Message::ToolResult(_) => cursor += 1,
        }
    }
    assert_eq!(cursor, actual.len(), "items left over, or missing");
}

/// The delta rule's append-only assumption: converting a prefix that
/// ends right after a completed turn (every tool call introduced so far
/// has its result within the prefix) gives a prefix of converting the
/// whole transcript. This does *not* hold for a prefix cut mid-batch,
/// between a tool call and its result: `to_input` would then drop that
/// call from the prefix (no result there yet) but keep it in the full
/// conversion, so only the prefixes `safe_prefix_lengths` names are
/// checked.
#[hegel::test(test_cases = 300)]
fn undamaged_prefix_after_a_completed_turn_is_a_prefix_of_the_full_input(
    tc: TestCase,
) {
    let messages = shared_call_ids(&tc, tc.draw(generators::transcript()));
    let full = to_input(&messages);
    for len in safe_prefix_lengths(&messages) {
        let prefix = to_input(&messages[..len]);
        assert!(
            full.starts_with(&prefix),
            "prefix of length {len} is not a prefix of the full input"
        );
    }
}

/// The cache converts exactly as `to_input` does, whatever it saw
/// before: a transcript that grew, one cut short, or a different one
/// altogether (a compaction, a fork).
#[hegel::test(test_cases = 300)]
fn the_cache_converts_like_to_input(tc: TestCase) {
    let base = shared_call_ids(&tc, tc.draw(generators::damaged_transcript()));
    let steps = tc.draw(gs::integers::<usize>().min_value(1).max_value(5));
    let mut cache = InputCache::new();
    for step in 0..steps {
        let messages = if tc.draw(gs::booleans()) {
            let len = tc.draw(gs::integers::<usize>().max_value(base.len()));
            base[..len].to_vec()
        } else {
            shared_call_ids(&tc, tc.draw(generators::damaged_transcript()))
        };
        let cached: Vec<Value> = cache
            .input(&messages)
            .iter()
            .map(|item| (**item).clone())
            .collect();
        assert_eq!(cached, to_input(&messages), "step {step}");
    }
}

/// Once a turn completes, the next turn's input starts with the very
/// items of the previous one, not copies: the lane matches them by
/// pointer.
#[hegel::test(test_cases = 300)]
fn the_cache_shares_the_items_of_earlier_turns(tc: TestCase) {
    let messages = shared_call_ids(&tc, tc.draw(generators::transcript()));
    let mut cache = InputCache::new();
    let mut previous: Vec<Arc<Value>> = Vec::new();
    for len in safe_prefix_lengths(&messages) {
        let input = cache.input(&messages[..len]);
        assert!(input.len() >= previous.len(), "length {len}");
        for (index, (before, now)) in previous.iter().zip(&input).enumerate() {
            assert!(Arc::ptr_eq(before, now), "length {len}, item {index}");
        }
        previous = input;
    }
}

// =============================================================================
// Helpers
// =============================================================================

fn assistant_with(
    content: Vec<AssistantBlock>,
    stop_reason: StopReason,
) -> AssistantMessage {
    AssistantMessage {
        content,
        api: API.to_owned(),
        provider: PROVIDER.to_owned(),
        model: "gpt-5.5".into(),
        response_id: None,
        usage: Usage::default(),
        stop_reason,
        error_message: None,
        timestamp: 0,
    }
}

fn one_assistant_message(content: Vec<AssistantBlock>) -> Vec<Message> {
    let has_tool_call = content
        .iter()
        .any(|b| matches!(b, AssistantBlock::ToolCall(_)));
    let stop_reason = if has_tool_call {
        StopReason::ToolUse
    } else {
        StopReason::Stop
    };
    vec![Message::Assistant(assistant_with(content, stop_reason))]
}

fn with_call_and_result(content: Vec<InputBlock>) -> Vec<Message> {
    let call = ToolCall {
        id: "call_1|fc_1".into(),
        name: "search".into(),
        arguments: Map::new(),
    };
    vec![
        Message::Assistant(assistant_with(
            vec![AssistantBlock::ToolCall(call)],
            StopReason::ToolUse,
        )),
        Message::ToolResult(ToolResultMessage {
            tool_call_id: "call_1|fc_1".into(),
            tool_name: "search".into(),
            content,
            details: None,
            is_error: false,
            timestamp: 1,
        }),
    ]
}

fn find_output(items: &[Value]) -> &Value {
    items
        .iter()
        .find(|item| item["type"] == "function_call_output")
        .expect("a function_call_output item")
}

/// Every `text` field of every `input_text`/`output_text` block, walking
/// the converted items in order. The oracle for
/// `undamaged_transcript_drops_nothing`.
fn extract_texts(items: &[Value]) -> Vec<String> {
    let mut texts = Vec::new();
    for item in items {
        if let Some(content) = item.get("content").and_then(Value::as_array) {
            for block in content {
                if matches!(
                    block["type"].as_str(),
                    Some("input_text") | Some("output_text")
                ) {
                    texts.push(block["text"].as_str().unwrap().to_owned());
                }
            }
        }
    }
    texts
}

/// A simple walk of the transcript collecting every text a user or
/// assistant message carries, in order. Independent of `to_input`: the
/// oracle for `undamaged_transcript_drops_nothing`.
fn walk_texts(messages: &[Message]) -> Vec<String> {
    let mut texts = Vec::new();
    for message in messages {
        match message {
            Message::User(user) => match &user.content {
                UserContent::Text(text) => texts.push(text.clone()),
                UserContent::Blocks(blocks) => {
                    for block in blocks {
                        if let InputBlock::Text(text) = block {
                            texts.push(text.text.clone());
                        }
                    }
                }
            },
            Message::Assistant(assistant) => {
                for block in &assistant.content {
                    if let AssistantBlock::Text(text) = block {
                        texts.push(text.text.clone());
                    }
                }
            }
            Message::ToolResult(_) => {}
        }
    }
    texts
}

/// The prefix lengths of `messages` that end right after a completed
/// turn: every tool call introduced anywhere in the prefix already has
/// its result inside that same prefix. A simple walk counting "calls
/// still owed" per `call_id` (a count, not a flag, so two calls that
/// happen to share an id each still need their own result), independent
/// of `to_input`'s own pairing — the oracle for
/// `undamaged_prefix_after_a_completed_turn_is_a_prefix_of_the_full_input`.
/// An errored/aborted assistant message owes nothing (rule 3 drops its
/// calls too), matching what `to_input` would keep for it.
fn safe_prefix_lengths(messages: &[Message]) -> Vec<usize> {
    let mut owed: HashMap<&str, i64> = HashMap::new();
    let mut lengths = Vec::new();
    for (i, message) in messages.iter().enumerate() {
        match message {
            Message::Assistant(assistant)
                if !matches!(
                    assistant.stop_reason,
                    StopReason::Error | StopReason::Aborted
                ) =>
            {
                for block in &assistant.content {
                    if let AssistantBlock::ToolCall(call) = block {
                        let (call_id, _) = split_tool_call_id(&call.id);
                        *owed.entry(call_id).or_insert(0) += 1;
                    }
                }
            }
            Message::ToolResult(result) => {
                let (call_id, _) = split_tool_call_id(&result.tool_call_id);
                if let Some(count) = owed.get_mut(call_id) {
                    *count -= 1;
                }
            }
            _ => {}
        }
        if owed.values().all(|&count| count == 0) {
            lengths.push(i + 1);
        }
    }
    lengths
}

/// The call ids transcripts draw from: few enough that calls in
/// different turns share one, as they can in a long run, which is where
/// pairing by `call_id` (and pi's orphan bug) gets interesting.
const CALL_IDS: [&str; 3] = ["call_a", "call_b", "call_c"];

/// `messages` with each tool call's `call_id` redrawn from [`CALL_IDS`].
/// A result keeps answering the call it answered (the first call with
/// its old id not yet answered); a result that answered none may now
/// claim one.
fn shared_call_ids(tc: &TestCase, mut messages: Vec<Message>) -> Vec<Message> {
    let mut renamed: HashMap<String, VecDeque<String>> = HashMap::new();
    let pool = || gs::sampled_from(CALL_IDS.to_vec());
    for message in &mut messages {
        match message {
            Message::Assistant(assistant) => {
                for block in &mut assistant.content {
                    if let AssistantBlock::ToolCall(call) = block {
                        let call_id = tc.draw(pool());
                        let id = match split_tool_call_id(&call.id).1 {
                            Some(item_id) => format!("{call_id}|{item_id}"),
                            None => call_id.to_owned(),
                        };
                        renamed
                            .entry(call.id.clone())
                            .or_default()
                            .push_back(id.clone());
                        call.id = id;
                    }
                }
            }
            Message::ToolResult(result) => {
                if let Some(id) = renamed
                    .get_mut(&result.tool_call_id)
                    .and_then(VecDeque::pop_front)
                {
                    result.tool_call_id = id;
                } else if tc.draw(gs::booleans()) {
                    result.tool_call_id = tc.draw(pool()).to_owned();
                }
            }
            Message::User(_) => {}
        }
    }
    messages
}

/// Gives every assistant item an id naming its message and block:
/// `rs_{message}_{block}` for reasoning (when its signature parses),
/// `msg_...` for text and `fc_...` as a tool call's item id. Changes no
/// pairing: `call_id`s stay as they are.
fn tag_assistant_items(messages: &mut [Message]) {
    for (m, message) in messages.iter_mut().enumerate() {
        let Message::Assistant(assistant) = message else {
            continue;
        };
        for (b, block) in assistant.content.iter_mut().enumerate() {
            match block {
                AssistantBlock::Thinking(thinking) => {
                    let parses =
                        thinking.thinking_signature.as_deref().is_some_and(
                            |s| serde_json::from_str::<Value>(s).is_ok(),
                        );
                    if parses {
                        thinking.thinking_signature = Some(
                            json!({
                                "type": "reasoning",
                                "id": format!("rs_{m}_{b}"),
                                "summary": [],
                            })
                            .to_string(),
                        );
                    }
                }
                AssistantBlock::Text(text) => {
                    text.text_signature = Some(format!("msg_{m}_{b}"));
                }
                AssistantBlock::ToolCall(call) => {
                    let call_id = split_tool_call_id(&call.id).0.to_owned();
                    call.id = format!("{call_id}|fc_{m}_{b}");
                }
            }
        }
    }
}

/// The message an item tagged by [`tag_assistant_items`] came from.
fn item_origin(item: &Value) -> Option<usize> {
    let id = item.get("id")?.as_str()?;
    let rest = ["rs_", "msg_", "fc_"]
        .iter()
        .find_map(|prefix| id.strip_prefix(prefix))?;
    rest.split('_').next()?.parse().ok()
}
