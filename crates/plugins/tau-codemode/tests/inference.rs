//! Property inventory: accepting JSON schemas preserve exact trees (round trip);
//! typed schemas reject values of another type (independent validator oracle).
//! Generator plan: Hegel JSON trees (at most three children per branch), an
//! accepting `true` schema, and typed scalars. Oversize answers and integer
//! literals outside Luau's exact range fail; other trees round trip. Shrinking
//! removes children and reduces integer magnitudes.
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

// This oracle examines serde_json's preserved integer scalars, independent of
// the production text scanner. Floating numbers and numeric strings are not
// integer literals and must not be rejected by this rule.
fn contains_unsafe_integer(value: &Value) -> bool {
    const EXACT: i64 = 1_i64 << 53;
    match value {
        Value::Number(number) => number.as_i64().map_or_else(
            || number.as_u64().is_some_and(|n| n > EXACT as u64),
            |n| !(-EXACT..=EXACT).contains(&n),
        ),
        Value::Array(items) => items.iter().any(contains_unsafe_integer),
        Value::Object(fields) => fields.values().any(contains_unsafe_integer),
        _ => false,
    }
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
fn resource_keywords_are_checked_only_at_schema_locations() {
    let literals = request(json!({
        "type": "object",
        "properties": {
            "$ref": {"const": {"$ref": "https://example.com/data"}},
            "$id": {"enum": [{"$id": "https://example.com/data"}],
                    "default": {"$dynamicRef": "https://example.com/data"},
                    "examples": [{"$anchor": "data"}]}
        },
        "additionalProperties": false
    }));
    assert!(literals.text_format().is_some());
    assert!(literals
        .decode_answer(r#"{"$ref":{"$ref":"https://example.com/data"},"$id":{"$id":"https://example.com/data"}}"#)
        .is_ok());

    for schema in [
        json!({"properties": {"x": {"$ref": "https://example.com/schema"}}}),
        json!({"patternProperties": {"x": {"$id": "remote"}}}),
        json!({"$defs": {"x": {"$anchor": "anchor"}}}),
        json!({"definitions": {"x": {"$dynamicRef": "https://example.com/schema"}}}),
        json!({"dependentSchemas": {"x": {"$ref": "https://example.com/schema"}}}),
        json!({"dependencies": {"x": {"$id": "remote"}}}),
        json!({"allOf": [{"$ref": "https://example.com/schema"}]}),
        json!({"items": {"$ref": "https://example.com/schema"}}),
    ] {
        assert!(
            InferRequest::parse(
                json!({"task": "x", "context": null, "schema": schema})
            )
            .is_err()
        );
    }

    let mut deep_data = json!(null);
    for _ in 0..70 {
        deep_data = json!({"$ref": deep_data});
    }
    assert!(InferRequest::parse(json!({"task": "x", "context": null, "schema": {"const": deep_data}})).is_err());
}

#[test]
fn simple_local_refs_keep_strict_format() {
    let request = request(json!({
        "$defs": {"leaf": {"type": "integer"}},
        "type": "object",
        "properties": {
            "left": {"$ref": "#/$defs/leaf"},
            "right": {"$ref": "#/$defs/leaf"}
        },
        "required": ["left", "right"],
        "additionalProperties": false
    }));
    assert!(request.text_format().is_some());
    assert!(request.decode_answer(r#"{"left":1,"right":2}"#).is_ok());
    assert!(request.decode_answer(r#"{"left":"1","right":2}"#).is_err());
}

// Inventory: a doubled-reference DAG of depth 30 has over 2^30 leaves but
// under 64 KiB of source. The independent oracle is its construction, not the
// production formatter. CI uses the workspace hegel.toml profile for the
// existing generated properties; this bounded adversarial case is fixed.
#[test]
fn doubled_reference_dag_falls_back_without_expansion() {
    let mut definitions = serde_json::Map::new();
    definitions.insert("d30".into(), json!({"type": "integer"}));
    for level in (0..30).rev() {
        let next = format!("#/$defs/d{}", level + 1);
        definitions.insert(
            format!("d{level}"),
            json!({"type": "object", "properties": {
                "left": {"$ref": next}, "right": {"$ref": next}
            }, "additionalProperties": false}),
        );
    }
    let schema = json!({
        "$defs": definitions,
        "type": "object",
        "properties": {"value": {"$ref": "#/$defs/d0"}},
        "required": ["value"],
        "additionalProperties": false
    });
    assert!(serde_json::to_vec(&schema).unwrap().len() < 64 * 1024);
    let request = request(schema.clone());
    assert!(request.text_format().is_none());
    assert!(request.input_text().contains(&schema.to_string()));
    assert!(request.decode_answer(r#"{"value":"wrong"}"#).is_err());
}

#[test]
fn cyclic_local_refs_fall_back() {
    let request = request(json!({
        "$defs": {"loop": {"type": "object", "properties": {
            "next": {"$ref": "#/$defs/loop"}
        }}},
        "type": "object",
        "properties": {"value": {"$ref": "#/$defs/loop"}}
    }));
    assert!(request.text_format().is_none());
    assert!(request.input_text().contains("Return only JSON"));
}

#[test]
fn expanded_schema_with_oversized_strict_form_falls_back() {
    let mut properties = serde_json::Map::new();
    for index in 0..500 {
        // Each name is 60 bytes. Strict mode repeats every name in `required`
        // and wraps the optional property in `anyOf`; its description is
        // repeated once per local reference. These independent terms alone
        // put the serialized strict form above 256 KiB.
        let name = format!("p{index:04}{}", "x".repeat(55));
        properties.insert(name, json!({"$ref": "#/$defs/leaf"}));
    }
    let schema = json!({
        "$defs": {"leaf": {"type": "string", "description": "x".repeat(350)}},
        "type": "object",
        "properties": properties,
        "additionalProperties": false
    });
    assert!(serde_json::to_vec(&schema).unwrap().len() < 64 * 1024);
    // 350 description bytes, two 60-byte copies of each name, and at
    // least 55 bytes of JSON syntax and strict nullable wrapping per entry.
    let description_bytes = schema["$defs"]["leaf"]["description"]
        .as_str()
        .unwrap()
        .len();
    let lower_bound: usize = schema["properties"]
        .as_object()
        .unwrap()
        .keys()
        .map(|name| description_bytes + 2 * name.len() + 55)
        .sum();
    assert!(lower_bound > 256 * 1024);
    let request = request(schema.clone());
    assert!(request.text_format().is_none());
    assert!(request.input_text().contains(&schema.to_string()));
    assert!(request.decode_answer(r#"{"p0000":"x"}"#).is_err());
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
    if answer.len() > 64 * 1024 || contains_unsafe_integer(&value) {
        assert!(result.is_err(), "unsafe or oversized answer: {answer}");
    } else {
        assert_eq!(result.unwrap(), value);
    }
}

#[test]
fn integer_boundary_does_not_reject_float_or_numeric_string() {
    let request = request(json!(true));
    for answer in [
        "9007199254740992",
        "-9007199254740992",
        "9.007199254740993e15",
        "\"9007199254740993\"",
    ] {
        assert!(request.decode_answer(answer).is_ok(), "{answer}");
    }
    for answer in [
        "9007199254740993",
        "-9007199254740993",
        "18446744073709551616",
    ] {
        assert!(request.decode_answer(answer).is_err(), "{answer}");
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
