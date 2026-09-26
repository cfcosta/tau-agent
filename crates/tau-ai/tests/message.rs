//! Message serialization: the serde shape mirrors pi's JSON.

use hegel::TestCase;
use serde_json::json;
use tau_ai::message::{AssistantBlock, Message, StopReason};
use tau_testing::generators;

/// Any message survives a JSON round trip unchanged.
#[hegel::test(test_cases = 500)]
fn message_json_round_trip(tc: TestCase) {
    let message = tc.draw(generators::message());
    let json = serde_json::to_string(&message).unwrap();
    tc.note(&json);
    let back: Message = serde_json::from_str(&json).unwrap();
    assert_eq!(back, message);
}

/// Every message carries its `role` tag and every content block its
/// `type` tag, and every key of the schema is camelCase.
#[hegel::test]
fn message_json_is_tagged_camel_case(tc: TestCase) {
    let message = tc.draw(generators::message());
    let value = serde_json::to_value(&message).unwrap();
    assert_eq!(value["role"], message.role());
    if let Some(blocks) = value["content"].as_array() {
        for block in blocks {
            assert!(block["type"].is_string(), "untagged block: {block}");
        }
    }
    assert_schema_keys_camel_case(&value);
}

/// Checks every object key, except inside `arguments` and `details`,
/// whose keys come from tools, not from our schema.
fn assert_schema_keys_camel_case(value: &serde_json::Value) {
    use serde_json::Value;
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                assert!(!key.contains('_'), "snake_case key {key:?}");
                if key != "arguments" && key != "details" {
                    assert_schema_keys_camel_case(child);
                }
            }
        }
        Value::Array(items) => {
            items.iter().for_each(assert_schema_keys_camel_case)
        }
        _ => {}
    }
}

/// An assistant message in pi's exact JSON shape parses, and serializes
/// back to the same JSON.
#[test]
fn pi_assistant_message_shape() {
    let pi = json!({
        "role": "assistant",
        "content": [
            {"type": "thinking", "thinking": "Looking.", "thinkingSignature": "{\"id\":\"rs_1\"}"},
            {"type": "text", "text": "Calling.", "textSignature": "msg_1"},
            {"type": "toolCall", "id": "call_1|fc_1", "name": "search", "arguments": {"q": "tokio"}}
        ],
        "api": "openai-responses",
        "provider": "openai",
        "model": "gpt-5.5",
        "responseId": "resp_1",
        "usage": {
            "input": 10, "output": 5, "cacheRead": 2, "cacheWrite": 0, "totalTokens": 17,
            "cost": {"input": 0.1, "output": 0.2, "cacheRead": 0.01, "cacheWrite": 0.0, "total": 0.31}
        },
        "stopReason": "toolUse",
        "timestamp": 1_790_000_000_000u64
    });
    let message: Message = serde_json::from_value(pi.clone()).unwrap();
    let Message::Assistant(assistant) = &message else {
        panic!("expected an assistant message");
    };
    assert_eq!(assistant.stop_reason, StopReason::ToolUse);
    assert!(matches!(assistant.content[2], AssistantBlock::ToolCall(_)));
    assert_eq!(serde_json::to_value(&message).unwrap(), pi);
}

/// User and tool-result messages in pi's exact JSON shape round-trip.
#[test]
fn pi_user_and_tool_result_shapes() {
    for pi in [
        json!({"role": "user", "content": "hello", "timestamp": 1}),
        json!({
            "role": "user",
            "content": [
                {"type": "text", "text": "look"},
                {"type": "image", "data": "iVBORw0KGgo=", "mimeType": "image/png"}
            ],
            "timestamp": 2
        }),
        json!({
            "role": "toolResult",
            "toolCallId": "call_1|fc_1",
            "toolName": "search",
            "content": [{"type": "text", "text": "found"}],
            "isError": false,
            "timestamp": 3
        }),
    ] {
        let message: Message = serde_json::from_value(pi.clone()).unwrap();
        assert_eq!(serde_json::to_value(&message).unwrap(), pi);
    }
}

/// Regression from `message_json_round_trip`: an explicit `null` in
/// `details` came back as an absent field.
#[test]
fn tool_result_null_details_round_trip() {
    let pi = json!({
        "role": "toolResult",
        "toolCallId": "call_0",
        "toolName": "tool_0",
        "content": [],
        "details": null,
        "isError": false,
        "timestamp": 0
    });
    let message: Message = serde_json::from_value(pi.clone()).unwrap();
    assert_eq!(serde_json::to_value(&message).unwrap(), pi);
}
