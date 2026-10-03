//! The `response.create` body (`tau_ai::responses::request`).

use hegel::{
    TestCase,
    generators as gs,
    generators::{Generator as _, PrintableGenerator},
};
use serde_json::{Value, json};
use tau_ai::responses::request::{
    Lineage,
    PROMPT_CACHE_KEY_MAX_CHARS,
    ReasoningEffort,
    Settings,
    ToolDefinition,
};
use tau_testing::generators;

/// The body as one JSON object.
fn body(
    settings: &Settings,
    input: Vec<Value>,
) -> serde_json::Map<String, Value> {
    tau_ai::responses::request::body(settings, input).to_map()
}

fn settings() -> impl PrintableGenerator<Settings> {
    // Settings is tau's own type, so its drawn values print through Debug.
    settings_unprinted().print_as_debug()
}

#[hegel::composite]
fn settings_unprinted(tc: &TestCase) -> Settings {
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
        ])).print_as_debug()),
        text_format: tc.draw(gs::optional(gs::just(json!({"type": "json_schema", "name": "T", "schema": {}, "strict": true})))),
        max_output_tokens: tc.draw(gs::optional(gs::integers::<u32>().min_value(1).max_value(8192))),
        service_tier: tc.draw(gs::optional(gs::sampled_from(vec!["flex".to_owned(), "priority".to_owned()]))),
        lineage: tc.draw(gs::optional(generators::text(100))).map(|path| Lineage {
            path,
            parent: None,
        }),
    }
}

fn input(tc: &TestCase) -> Vec<Value> {
    tc.draw(gs::vecs(generators::lane::item()).max_size(4))
}

/// Two bodies for the same settings differ only in `input`, which is
/// what lets the delta rule continue a run.
#[hegel::test(test_cases = 300)]
fn bodies_differ_only_in_input(tc: TestCase) {
    let settings = tc.draw(settings());
    let mut a = body(&settings, input(&tc));
    let mut b = body(&settings, input(&tc));
    a.remove("input");
    b.remove("input");
    assert_eq!(a, b);
}

/// The WebSocket rules hold for every body: fixed type, `store: false`,
/// no forbidden fields, and the input as given.
#[hegel::test(test_cases = 300)]
fn bodies_follow_websocket_rules(tc: TestCase) {
    let settings = tc.draw(settings());
    let items = input(&tc);
    let body = body(&settings, items.clone());
    assert_eq!(body["type"], "response.create");
    assert_eq!(body["store"], false);
    for forbidden in
        ["stream", "background", "previous_response_id", "stream_id"]
    {
        assert!(!body.contains_key(forbidden), "{forbidden}");
    }
    assert_eq!(body["input"], Value::Array(items));
    assert_eq!(body["model"], json!(settings.model));
    assert_eq!(
        body.get("instructions"),
        settings.instructions.as_ref().map(|i| json!(i)).as_ref()
    );
    // Every tool, in order, as a function; no `tools` at all without any.
    let tools: Vec<Value> = settings
        .tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.parameters,
                "strict": tool.strict,
            })
        })
        .collect();
    assert_eq!(
        body.get("tools"),
        (!tools.is_empty()).then(|| Value::Array(tools)).as_ref()
    );
}

/// Reasoning models always ask for encrypted reasoning, so a full resend
/// can replay it; other models never do. The effort is sent only when
/// set.
#[hegel::test(test_cases = 300)]
fn encrypted_reasoning_follows_the_model(tc: TestCase) {
    let settings = tc.draw(settings());
    let body = body(&settings, vec![]);
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

/// Property inventory: generic request fields preserve the optional output
/// ceiling and clamp the cache key. Settings is the oracle; its generator
/// builds valid limits by construction, and `hegel.toml` supplies CI counts.
#[hegel::test(test_cases = 300)]
fn wire_limits_are_clamped(tc: TestCase) {
    let settings = tc.draw(settings());
    let body = body(&settings, vec![]);
    assert_eq!(
        body.get("max_output_tokens"),
        settings
            .max_output_tokens
            .map(|limit| json!(limit))
            .as_ref()
    );
    if let Some(key) = settings.lineage.as_ref().map(|lineage| &lineage.path) {
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

#[test]
fn generic_request_fields_include_only_configured_output_ceiling() {
    let mut settings = Settings {
        model: "gpt-5.5".into(),
        ..Settings::default()
    };
    assert!(!body(&settings, vec![]).contains_key("max_output_tokens"));
    settings.max_output_tokens = Some(2048);
    assert_eq!(body(&settings, vec![])["max_output_tokens"], json!(2048));
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
        ..Settings::default()
    };
    let body = body(
        &settings,
        vec![
            json!({"role": "user", "content": [{"type": "input_text", "text": "hi"}]}),
        ],
    );
    assert_eq!(
        Value::Object(body),
        json!({
            "type": "response.create",
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
            "input": [{"role": "user", "content": [{"type": "input_text", "text": "hi"}]}]
        })
    );
}
