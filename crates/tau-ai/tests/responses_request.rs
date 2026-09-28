//! The `response.create` body (`tau_ai::responses::request`).

use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_ai::responses::request::{
    MIN_OUTPUT_TOKENS,
    PROMPT_CACHE_KEY_MAX_CHARS,
    ReasoningEffort,
    Settings,
    StreamId,
    ToolDefinition,
};
use tau_testing::generators;

/// The body as one JSON object.
fn body(
    settings: &Settings,
    input: Vec<Value>,
    lane: Option<&StreamId>,
) -> serde_json::Map<String, Value> {
    tau_ai::responses::request::body(settings, input, lane).to_map()
}

#[hegel::composite]
fn settings(tc: TestCase) -> Settings {
    let tool_count = tc.draw(gs::integers::<usize>().max_value(3));
    Settings {
        model: tc.draw(gs::sampled_from(vec!["gpt-5.5".to_owned(), "gpt-5.4-mini".to_owned()])),
        instructions: tc.draw(gs::optional(generators::text(40))),
        tools: (0..tool_count)
            .map(|i| ToolDefinition {
                name: format!("tool_{i}"),
                description: tc.draw(generators::text(20)),
                parameters: json!({"type": "object", "properties": {}}),
                strict: tc.draw(gs::booleans()),
            })
            .collect(),
        reasoning_model: tc.draw(gs::booleans()),
        reasoning: tc.draw(gs::optional(gs::sampled_from(vec![
            ReasoningEffort::None,
            ReasoningEffort::Minimal,
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
            ReasoningEffort::Xhigh,
            ReasoningEffort::Max,
        ]))),
        max_output_tokens: tc.draw(gs::optional(gs::integers::<u64>().max_value(200_000))),
        text_format: tc.draw(gs::optional(gs::just(json!({"type": "json_schema", "name": "T", "schema": {}, "strict": true})))),
        service_tier: tc.draw(gs::optional(gs::sampled_from(vec!["flex".to_owned(), "priority".to_owned()]))),
        prompt_cache_key: tc.draw(gs::optional(generators::text(100))),
    }
}

#[hegel::composite]
fn stream_id(tc: TestCase) -> StreamId {
    let id: String = tc.draw(
        gs::text()
            .alphabet("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_.-")
            .min_size(1)
            .max_size(256),
    );
    StreamId::new(id).expect("drawn from the allowed alphabet")
}

fn input(tc: &TestCase) -> Vec<Value> {
    tc.draw(gs::vecs(generators::lane::item()).max_size(4))
}

/// Two bodies for the same settings differ only in `input`, which is
/// what lets the delta rule continue a run.
#[hegel::test(test_cases = 300)]
fn bodies_differ_only_in_input(tc: TestCase) {
    let settings = tc.draw(settings());
    let lane = tc.draw(gs::optional(stream_id()));
    let mut a = body(&settings, input(&tc), lane.as_ref());
    let mut b = body(&settings, input(&tc), lane.as_ref());
    a.remove("input");
    b.remove("input");
    assert_eq!(a, b);
}

/// The WebSocket rules hold for every body: fixed type, `store: false`,
/// no forbidden fields, and the input as given.
#[hegel::test(test_cases = 300)]
fn bodies_follow_websocket_rules(tc: TestCase) {
    let settings = tc.draw(settings());
    let lane = tc.draw(gs::optional(stream_id()));
    let items = input(&tc);
    let body = body(&settings, items.clone(), lane.as_ref());
    assert_eq!(body["type"], "response.create");
    assert_eq!(body["store"], false);
    for forbidden in ["stream", "background", "previous_response_id"] {
        assert!(!body.contains_key(forbidden), "{forbidden}");
    }
    assert_eq!(body["input"], Value::Array(items));
    assert_eq!(
        body.get("stream_id").and_then(Value::as_str),
        lane.as_ref().map(StreamId::as_str)
    );
    assert_eq!(body["model"], json!(settings.model));
    assert_eq!(
        body.get("instructions"),
        settings.instructions.as_ref().map(|i| json!(i)).as_ref()
    );
}

/// Reasoning models always ask for encrypted reasoning, so a full resend
/// can replay it; other models never do. The effort is sent only when
/// set.
#[hegel::test(test_cases = 300)]
fn encrypted_reasoning_follows_the_model(tc: TestCase) {
    let settings = tc.draw(settings());
    let body = body(&settings, vec![], None);
    assert_eq!(body.contains_key("include"), settings.reasoning_model);
    if settings.reasoning_model {
        assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
    }
    let expected = match (settings.reasoning_model, settings.reasoning) {
        (true, Some(effort)) => {
            Some(json!({"effort": effort.as_str(), "summary": "auto"}))
        }
        _ => None,
    };
    assert_eq!(body.get("reasoning").cloned(), expected);
}

/// Wire limits: `max_output_tokens` is raised to 16, and
/// `prompt_cache_key` is cut to its first 64 characters.
#[hegel::test(test_cases = 300)]
fn wire_limits_are_clamped(tc: TestCase) {
    let settings = tc.draw(settings());
    let body = body(&settings, vec![], None);
    assert_eq!(
        body.get("max_output_tokens").and_then(Value::as_u64),
        settings.max_output_tokens.map(|m| m.max(MIN_OUTPUT_TOKENS))
    );
    if let Some(key) = &settings.prompt_cache_key {
        let sent = body["prompt_cache_key"].as_str().unwrap();
        assert_eq!(
            sent.chars().count(),
            key.chars().count().min(PROMPT_CACHE_KEY_MAX_CHARS)
        );
        assert!(key.starts_with(sent));
    } else {
        assert!(!body.contains_key("prompt_cache_key"));
    }
    assert_eq!(
        body.get("service_tier").and_then(Value::as_str),
        settings.service_tier.as_deref()
    );
    assert_eq!(
        body.get("text"),
        settings
            .text_format
            .as_ref()
            .map(|f| json!({"format": f}))
            .as_ref()
    );
}

/// Stream ids outside `[A-Za-z0-9_.-]{1,256}` are rejected.
#[test]
fn stream_id_rules() {
    assert!(StreamId::new("run_01J.x-y").is_ok());
    assert!(StreamId::new("a".repeat(256)).is_ok());
    for bad in ["", "has space", "slash/", "ünïcode", &"a".repeat(257)] {
        let error = StreamId::new(bad).unwrap_err();
        assert!(error.to_string().contains("1-256 characters"), "{bad}");
    }
}

/// A full body in the shape the WebSocket guide shows.
#[test]
fn golden_body() {
    let settings = Settings {
        model: "gpt-5.5".into(),
        instructions: Some("Be brief.".into()),
        tools: vec![ToolDefinition {
            name: "search".into(),
            description: "Search the web.".into(),
            parameters: json!({"type": "object", "properties": {"q": {"type": "string"}}, "required": ["q"], "additionalProperties": false}),
            strict: true,
        }],
        reasoning_model: true,
        reasoning: Some(ReasoningEffort::High),
        max_output_tokens: Some(8),
        ..Settings::default()
    };
    let lane = StreamId::new("run_1").unwrap();
    let body = body(
        &settings,
        vec![
            json!({"role": "user", "content": [{"type": "input_text", "text": "hi"}]}),
        ],
        Some(&lane),
    );
    assert_eq!(
        Value::Object(body),
        json!({
            "type": "response.create",
            "stream_id": "run_1",
            "model": "gpt-5.5",
            "store": false,
            "instructions": "Be brief.",
            "tools": [{
                "type": "function",
                "name": "search",
                "description": "Search the web.",
                "parameters": {"type": "object", "properties": {"q": {"type": "string"}}, "required": ["q"], "additionalProperties": false},
                "strict": true
            }],
            "reasoning": {"effort": "high", "summary": "auto"},
            "include": ["reasoning.encrypted_content"],
            "max_output_tokens": 16,
            "input": [{"role": "user", "content": [{"type": "input_text", "text": "hi"}]}]
        })
    );
}
