//! Property inventory:
//! - `accepts_generated_required_objects_and_rejects_a_missing_property` uses
//!   an object-shape predicate over 1..=4 required integer keys.
//! - `accepts_generated_integers_without_coercing_numeric_strings` uses an
//!   integer-token predicate for values in -1000..=1000.
//! - `accepts_booleans_without_coercing_strings` exhausts booleans
//!   and their string-form invalid partners.
//! - `accepts_bounded_numbers_and_rejects_outside_the_interval` uses literal
//!   inclusive bounds and both neighboring out-of-range values.
//! - `accepts_bounded_enum_numbers_and_rejects_a_neighbor` checks the numeric
//!   range and singleton enum independently for values in -999..=999.
//! - `accepts_string_arrays_and_rejects_a_numeric_member` checks 1..=4 string
//!   members, each at most 16 bytes, against a numeric-member mutation.
//! - `accepts_nullable_strings_and_rejects_numbers` and
//!   `accepts_nonnullable_strings_and_rejects_null` use independent type
//!   predicates for nullable and nonnullable schemas.
//! - `a_true_schema_decodes_bounded_generated_json_trees_exactly` remains a
//!   separate general JSON parse/tree-preservation property.
//!
//! Generator plan: build every accepted value from bounded primitives; object
//! keys are sampled as subsequences of four distinct canonical names. The
//! rejected partner is a single known mutation (missing key, wrong primitive,
//! enum neighbor, or wrong array member). Shrinking reduces integers, strings,
//! arrays, and key subsequences while rebuilding each valid/invalid pair.
//! Fixed tables cover the 64 KiB answer boundary, schema JSON depths 64/65,
//! and the codec's default recursion guard at 127/128 nested arrays. The answer
//! parser does not apply the schema's 64-level guard. Fixed exact-integer
//! examples retain the ±2^53 and float/string cases.
//! CI uses the workspace hegel.toml profile; no per-test count override.

use hegel::{TestCase, extras::serde_json as json_gs, generators as gs};
use serde_json::{Value, json};
use tau_codemode::inference::InferRequest;

const ASCII_TEXT: &str =
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

fn request(schema: Value) -> InferRequest {
    InferRequest::parse(
        json!({"task": "answer", "context": null, "schema": schema}),
    )
    .unwrap()
}

fn is_json_integer(value: &Value) -> bool {
    value.as_f64().is_some_and(|number| number.fract() == 0.0)
}

fn assert_schema_pair_matches_predicate(
    schema: Value,
    accepted: Value,
    rejected: Value,
    accepts: impl Fn(&Value) -> bool,
    rejection_reason: &str,
) {
    assert!(
        accepts(&accepted),
        "accepted example violates independent oracle"
    );
    assert!(
        !accepts(&rejected),
        "invalid partner does not violate independent oracle: {rejection_reason}"
    );

    let request = request(schema);
    let accepted_json = serde_json::to_string(&accepted).unwrap();
    assert_eq!(request.decode_answer(&accepted_json).unwrap(), accepted);

    let rejected_json = serde_json::to_string(&rejected).unwrap();
    assert!(
        request.decode_answer(&rejected_json).is_err(),
        "schema accepted invalid partner ({rejection_reason}): {rejected_json}"
    );
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
fn answer_text_limit_includes_65536_bytes() {
    let request = request(json!(true));
    for (answer_bytes, accepted) in [(65_536, true), (65_537, false)] {
        // The JSON string text includes both quote bytes. InferRequest accepts
        // exactly MAX_ANSWER_BYTES and rejects only a larger answer.
        let text = "x".repeat(answer_bytes - 2);
        let answer = serde_json::to_string(&text).unwrap();
        assert_eq!(answer.len(), answer_bytes);
        if accepted {
            assert_eq!(request.decode_answer(&answer).unwrap(), json!(text));
        } else {
            assert!(request.decode_answer(&answer).is_err());
        }
    }
}

#[test]
fn answer_decoder_obeys_the_json_codec_recursion_guard() {
    let request = request(json!(true));
    for depth in [64, 65, 127, 128] {
        let mut value = json!(null);
        for _ in 0..depth {
            value = json!([value]);
        }
        let answer = serde_json::to_string(&value).unwrap();
        if depth < 128 {
            assert_eq!(request.decode_answer(&answer).unwrap(), value);
        } else {
            assert!(request.decode_answer(&answer).is_err());
        }
    }
}

#[test]
fn schema_annotation_depth_obeys_the_64_level_guard() {
    for depth in [64, 65] {
        let mut annotation = Value::Null;
        for _ in 1..depth {
            annotation = json!([annotation]);
        }
        let parsed = InferRequest::parse(json!({
            "task": "answer", "context": null,
            "schema": {"type": "null", "x-depth": annotation}
        }));
        assert_eq!(parsed.is_ok(), depth == 64, "depth {depth}: {parsed:?}");
    }
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
fn a_true_schema_decodes_bounded_generated_json_trees_exactly(tc: TestCase) {
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
fn accepts_generated_required_objects_and_rejects_a_missing_property(
    tc: TestCase,
) {
    let key_indices: Vec<usize> = tc.draw(
        gs::subsequences(vec![0_usize, 1, 2, 3])
            .min_size(1)
            .max_size(4),
    );
    let mut properties = serde_json::Map::new();
    let mut fields = serde_json::Map::new();
    let required: Vec<String> = key_indices
        .iter()
        .map(|index| format!("field{index}"))
        .collect();
    for name in &required {
        let value: i64 =
            tc.draw(gs::integers().min_value(-1000_i64).max_value(1000));
        assert!(
            properties
                .insert(
                    name.clone(),
                    json!({
                        "type": "integer",
                        "minimum": -1000,
                        "maximum": 1000
                    }),
                )
                .is_none()
        );
        assert!(fields.insert(name.clone(), json!(value)).is_none());
    }

    let schema = json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    });
    let accepted = Value::Object(fields);
    let mut rejected_fields = accepted.as_object().unwrap().clone();
    assert!(rejected_fields.remove(&required[0]).is_some());
    let rejected = Value::Object(rejected_fields);
    let required_for_oracle = required.clone();
    assert_schema_pair_matches_predicate(
        schema,
        accepted,
        rejected,
        move |value| {
            value.as_object().is_some_and(|fields| {
                fields.len() == required_for_oracle.len()
                    && required_for_oracle.iter().all(|name| {
                        fields.get(name).is_some_and(|value| {
                            is_json_integer(value)
                                && value.as_f64().is_some_and(|number| {
                                    (-1000.0..=1000.0).contains(&number)
                                })
                        })
                    })
            })
        },
        "one required property is absent",
    );
}

#[hegel::test]
fn accepts_generated_integers_without_coercing_numeric_strings(tc: TestCase) {
    let integer: i64 =
        tc.draw(gs::integers().min_value(-1000_i64).max_value(1000));
    let accepted = json!(integer);
    let rejected = json!(integer.to_string());
    assert_schema_pair_matches_predicate(
        json!({"type": "integer"}),
        accepted,
        rejected,
        is_json_integer,
        "the JSON value is a numeric string, not an integer token",
    );
}

#[test]
fn accepts_booleans_without_coercing_strings() {
    for boolean in [false, true] {
        assert_schema_pair_matches_predicate(
            json!({"type": "boolean"}),
            json!(boolean),
            json!(boolean.to_string()),
            Value::is_boolean,
            "the JSON value is a string, not a boolean",
        );
    }
}

#[hegel::test]
fn accepts_bounded_numbers_and_rejects_outside_the_interval(tc: TestCase) {
    let center = tc.draw(gs::integers::<i64>().min_value(-999).max_value(999));
    let radius = tc.draw(gs::integers::<i64>().min_value(1).max_value(8));
    let minimum = center - radius;
    let maximum = center + radius;
    let accepted = json!(center as f64 + 0.25);
    for invalid in [minimum - 1, maximum + 1] {
        assert_schema_pair_matches_predicate(
            json!({"type": "number", "minimum": minimum, "maximum": maximum}),
            accepted.clone(),
            json!(invalid),
            |value| {
                value.as_f64().is_some_and(|number| {
                    (minimum as f64..=maximum as f64).contains(&number)
                })
            },
            "the number lies just outside the inclusive bounds",
        );
    }
}

#[hegel::test]
fn accepts_bounded_enum_numbers_and_rejects_a_neighbor(tc: TestCase) {
    let number: i64 =
        tc.draw(gs::integers().min_value(-999_i64).max_value(999));
    let accepted = json!(number);
    let rejected = json!(number + 1);
    assert_schema_pair_matches_predicate(
        json!({
            "type": "number",
            "minimum": -1000,
            "maximum": 1000,
            "enum": [number]
        }),
        accepted,
        rejected,
        move |value| {
            value.as_f64().is_some_and(|value| {
                (-1000.0..=1000.0).contains(&value) && value == number as f64
            })
        },
        "the neighboring number is in range but absent from the enum",
    );
}

#[hegel::test]
fn accepts_string_arrays_and_rejects_a_numeric_member(tc: TestCase) {
    let values: Vec<String> = tc.draw(
        gs::vecs(gs::text().alphabet(ASCII_TEXT).max_size(16))
            .min_size(1)
            .max_size(4),
    );
    let accepted = json!(values);
    let mut rejected_values = accepted.as_array().unwrap().clone();
    rejected_values[0] = json!(0);
    let rejected = Value::Array(rejected_values);
    assert_schema_pair_matches_predicate(
        json!({
            "type": "array",
            "items": {"type": "string", "maxLength": 16},
            "minItems": 1,
            "maxItems": 4
        }),
        accepted,
        rejected,
        |value| {
            value.as_array().is_some_and(|items| {
                (1..=4).contains(&items.len())
                    && items.iter().all(|item| {
                        item.as_str().is_some_and(|text| text.len() <= 16)
                    })
            })
        },
        "the first array member is numeric rather than a string",
    );
}

#[hegel::test]
fn accepts_nullable_strings_and_rejects_numbers(tc: TestCase) {
    let text: Option<String> =
        tc.draw(gs::optional(gs::text().alphabet(ASCII_TEXT).max_size(16)));
    let accepted = match text {
        Some(text) => json!(text),
        None => Value::Null,
    };
    assert_schema_pair_matches_predicate(
        json!({"type": ["string", "null"], "maxLength": 16}),
        accepted,
        json!(0),
        |value| {
            value.is_null()
                || value.as_str().is_some_and(|text| text.len() <= 16)
        },
        "a number is neither a string nor null",
    );
}

#[hegel::test]
fn accepts_nonnullable_strings_and_rejects_null(tc: TestCase) {
    let text: String = tc.draw(gs::text().alphabet(ASCII_TEXT).max_size(16));
    assert_schema_pair_matches_predicate(
        json!({"type": "string", "maxLength": 16}),
        json!(text),
        Value::Null,
        |value| value.as_str().is_some_and(|text| text.len() <= 16),
        "null is not a string",
    );
}
