//! Property inventory: accepting JSON schemas preserve exact trees (round trip);
//! typed schemas reject values of another type (independent validator oracle).
//! Generator plan: Hegel JSON trees (at most three children per branch), an
//! accepting `true` schema, and typed scalars. Oversize answers must fail;
//! smaller answers must retain their exact tree. Shrinking removes children.
//! CI uses the workspace hegel.toml profile; no per-test count override.

use hegel::{TestCase, extras::serde_json as json_gs, generators as gs};
use serde_json::{Value, json};
use tau_codemode::inference::InferRequest;

fn request(schema: Value) -> InferRequest {
    InferRequest::parse(
        json!({"task": "answer", "context": null, "schema": schema}),
    )
    .unwrap()
}

#[test]
fn rejects_missing_empty_and_unknown_arguments() {
    for value in [
        json!({}),
        json!({"task": "answer"}),
        json!({"task": " \n ", "context": null}),
        json!({"task": "answer", "context": null, "extra": 1}),
        json!({"task": 1, "context": null}),
        json!([]),
    ] {
        assert!(InferRequest::parse(value).is_err());
    }
    assert!(
        InferRequest::parse(json!({"task": "answer", "context": null})).is_ok()
    );
    assert!(
        InferRequest::parse(
            json!({"task": "answer", "context": null, "schema": null})
        )
        .unwrap()
        .schema
        .is_none()
    );
}

#[test]
fn enforces_byte_limits() {
    assert!(
        InferRequest::parse(
            json!({"task": "x".repeat(16 * 1024 + 1), "context": null})
        )
        .is_err()
    );
    assert!(
        InferRequest::parse(
            json!({"task": "x", "context": "x".repeat(256 * 1024)})
        )
        .is_err()
    );
    assert!(InferRequest::parse(json!({"task": "x", "context": null, "schema": {"description": "x".repeat(64 * 1024)}})).is_err());
    assert!(
        request(json!(true))
            .decode_answer(&"x".repeat(64 * 1024 + 1))
            .is_err()
    );
    let mut deep = json!(true);
    for _ in 0..70 {
        deep = json!({"items": deep});
    }
    assert!(
        InferRequest::parse(
            json!({"task": "x", "context": null, "schema": deep})
        )
        .is_err()
    );
}

#[test]
fn validates_schema_before_use_and_blocks_external_resources() {
    for schema in [
        json!({"type": "unknown"}),
        json!({"$ref": "https://example.com/schema.json"}),
        json!({"$ref": "file:///tmp/schema.json"}),
        json!({"$dynamicRef": "https://example.com/schema.json"}),
        json!({"$id": "https://example.com/schema.json"}),
        json!({"$anchor": "node"}),
    ] {
        assert!(
            InferRequest::parse(
                json!({"task": "x", "context": null, "schema": schema})
            )
            .is_err()
        );
    }
    let local = request(
        json!({"$defs": {"number": {"type": "integer"}}, "$ref": "#/$defs/number"}),
    );
    assert_eq!(local.decode_answer("3").unwrap(), json!(3));
    assert!(local.decode_answer("\"3\"").is_err());
    assert!(request(json!(false)).decode_answer("null").is_err());
}

#[test]
fn handles_provider_format_and_fallback() {
    let strict = request(json!({
        "type": "object",
        "properties": {"name": {"type": "string"}},
        "additionalProperties": false
    }));
    assert!(strict.text_format().is_some());
    assert_eq!(strict.decode_answer("{\"name\":null}").unwrap(), json!({}));
    assert_eq!(
        strict.decode_answer("{\"name\":\"é🐈\"}").unwrap(),
        json!({"name":"é🐈"})
    );
    assert!(strict.decode_answer("{\"name\":5}").is_err());
    assert!(!strict.input_text().contains("schema"));

    let fallback =
        request(json!({"type": "array", "items": {"type": "integer"}}));
    assert!(fallback.text_format().is_none());
    assert!(fallback.input_text().contains("Return only JSON"));
    assert!(fallback.decode_answer("[1,2]").is_ok());
    assert!(fallback.decode_answer("[\"1\"]").is_err());

    let plain =
        InferRequest::parse(json!({"task": "say hi", "context": {"a":1}}))
            .unwrap();
    assert_eq!(plain.decode_answer("héllo 🐈").unwrap(), json!("héllo 🐈"));
    assert!(plain.input_text().contains("Context (JSON):"));
    assert!(!plain.input_text().contains("parent"));
    assert_eq!(
        InferRequest::parameters()["required"],
        json!(["task", "context"])
    );
}

#[hegel::test]
fn accepting_schema_returns_the_exact_generated_json_tree(tc: TestCase) {
    let value: Value = tc.draw(json_gs::values());
    let answer = serde_json::to_string(&value).unwrap();
    let result = request(json!(true)).decode_answer(&answer);
    if answer.len() > 64 * 1024 {
        assert!(result.is_err());
    } else {
        assert_eq!(result.unwrap(), value);
    }
}

#[hegel::test]
fn typed_schema_never_coerces_a_string_to_a_number(tc: TestCase) {
    let number: i32 = tc.draw(gs::integers());
    let answer = json!(number.to_string()).to_string();
    assert!(
        request(json!({"type": "integer"}))
            .decode_answer(&answer)
            .is_err()
    );
}
