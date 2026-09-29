//! Tool-argument coercion and validation (`tau_agent::validation`).
//!
//! Oracle for the rules: pi's `validateToolArguments`
//! (`packages/ai/src/utils/validation.ts:317`) and its test file
//! `packages/ai/test/validation.test.ts`. See
//! `docs/reference/testing.md`'s `tau-agent` property inventory (the
//! "Coercion ..." rows) and `docs/reference/agent-loop.md`'s "Coercion
//! before validation" for the rules ported here.

use hegel::{TestCase, extras::serde_json as json_gs, generators as gs};
use serde_json::{Value, json};
use tau_agent::validation::ArgumentSchema;
use tau_testing::generators;

fn compile(schema: &Value) -> ArgumentSchema {
    ArgumentSchema::new(schema)
        .unwrap_or_else(|error| panic!("{schema} did not compile: {error}"))
}

/// Structural equality that treats two JSON numbers as equal whenever
/// they hold the same `f64` value, regardless of whether either is
/// backed by an integer or a float. Used only where a test compares
/// values that reached the same number through different coercion
/// paths (e.g. a string parsed to a float vs. an integer already in
/// canonical form); tests that compare a single coercion's output
/// against its exact input use plain `assert_eq!` instead, since
/// [`tau_agent::validation`] never renumbers a value it does not touch.
fn assert_semantically_equal(a: &Value, b: &Value) {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            assert_eq!(x.as_f64(), y.as_f64(), "{a} != {b}");
        }
        (Value::Array(xs), Value::Array(ys)) => {
            assert_eq!(xs.len(), ys.len(), "{a} != {b}");
            for (x, y) in xs.iter().zip(ys) {
                assert_semantically_equal(x, y);
            }
        }
        (Value::Object(xs), Value::Object(ys)) => {
            assert_eq!(xs.len(), ys.len(), "{a} != {b}");
            for (key, x) in xs {
                let y = ys
                    .get(key)
                    .unwrap_or_else(|| panic!("{b} is missing key {key:?}"));
                assert_semantically_equal(x, y);
            }
        }
        _ => assert_eq!(a, b),
    }
}

// =============================================================================
// Properties (testing.md, `tau-agent` property inventory)
// =============================================================================

/// Coercion leaves a value that already validates unchanged.
#[hegel::test(test_cases = 300)]
fn coercion_leaves_a_valid_value_unchanged(tc: TestCase) {
    let schema = tc.draw(generators::arg_schema(3));
    let value = tc.draw(generators::arg_value_for_schema(schema.clone()));
    let compiled = compile(&schema);
    assert_eq!(compiled.coerce(&value), value);
}

/// Coercion is idempotent, for any value, not only one that already
/// validates: applying it twice gives the same result as applying it
/// once.
#[hegel::test(test_cases = 300)]
fn coercion_is_idempotent(tc: TestCase) {
    let schema = tc.draw(generators::arg_schema(3));
    // An arbitrary JSON value, unrelated to `schema`, so this exercises
    // coercion on inputs that do not already match (including ones
    // coercion cannot fix at all).
    let value = tc.draw(json_gs::values());
    let compiled = compile(&schema);
    let once = compiled.coerce(&value);
    let twice = compiled.coerce(&once);
    assert_eq!(once, twice);
}

fn draw_metamorphic_case(tc: &TestCase) -> (Value, Value, Value) {
    let (leaf, canonical, alternate) =
        match tc.draw(gs::integers::<u8>().max_value(3)) {
            0 => {
                let value = tc.draw(generators::json_number());
                let alternate = Value::String(value.to_string());
                (json!({"type": "number"}), Value::Number(value), alternate)
            }
            1 => {
                let value = tc.draw(
                    gs::integers::<i64>()
                        .min_value(-1_000_000)
                        .max_value(1_000_000),
                );
                let alternate = Value::String(value.to_string());
                (json!({"type": "integer"}), json!(value), alternate)
            }
            2 => {
                let value = tc.draw(gs::booleans());
                let alternate = Value::String(value.to_string());
                (json!({"type": "boolean"}), Value::Bool(value), alternate)
            }
            _ => {
                let element = Value::String(tc.draw(generators::text(8)));
                let canonical = Value::Array(vec![element.clone()]);
                (
                    json!({"type": "array", "items": {"type": "string"}}),
                    canonical,
                    element,
                )
            }
        };
    let schema = json!({
        "type": "object",
        "properties": {"value": leaf},
        "required": ["value"],
    });
    (
        schema,
        json!({"value": canonical}),
        json!({"value": alternate}),
    )
}

/// A number, a boolean or a one-element array gives the same result as
/// its string or scalar form after coercion: `42` and `"42"` under
/// `number`/`integer`, `true`/`false` and `"true"`/`"false"` under
/// `boolean`, and `["x"]` and `"x"` under `array`.
#[hegel::test(test_cases = 300)]
fn string_or_scalar_form_coerces_to_the_same_result(tc: TestCase) {
    let (schema, canonical_args, alternate_args) = draw_metamorphic_case(&tc);
    let compiled = compile(&schema);
    let from_canonical = compiled.coerce(&canonical_args);
    let from_alternate = compiled.coerce(&alternate_args);
    assert_semantically_equal(&from_canonical, &from_alternate);
    // Both forms must actually validate: the point of the metamorphic
    // relation is that they reach the *same valid* result, not merely
    // an equal invalid one.
    assert!(compiled.validate(&canonical_args).is_ok());
    assert!(compiled.validate(&alternate_args).is_ok());
}

fn draw_optional_null_case(tc: &TestCase) -> (Value, Value, Value) {
    // A leaf type whose own schema never accepts `null`, so the
    // property is unambiguously "optional, not nullable".
    let leaf = tc.draw(gs::sampled_from(vec![
        json!({"type": "string"}),
        json!({"type": "number"}),
        json!({"type": "boolean"}),
    ]));
    let schema = json!({
        "type": "object",
        "properties": {"present": {"type": "string"}, "extra": leaf},
        "required": ["present"],
    });
    let present = Value::String(tc.draw(generators::text(8)));

    let without_extra = json!({"present": present});
    let mut with_null = without_extra.clone();
    with_null["extra"] = Value::Null;

    (schema, with_null, without_extra)
}

/// `null` on an optional, non-nullable property is treated as absent:
/// the validated output is the same as if the property had not been
/// sent at all (`docs/reference/agent-loop.md`, "Coercion before
/// validation", rule 2; pi's `normalizeOptionalNulls`).
#[hegel::test(test_cases = 300)]
fn null_on_optional_field_is_treated_as_absent(tc: TestCase) {
    let (schema, with_null, without_extra) = draw_optional_null_case(&tc);
    let compiled = compile(&schema);
    assert_eq!(compiled.coerce(&with_null), compiled.coerce(&without_extra));
}

/// Validation accepts exactly the values whose coerced form the schema
/// accepts, by the `jsonschema` crate asked directly, and hands back that
/// coerced form. Checked on arbitrary values, on values valid under the
/// schema, and on values built for an unrelated schema.
#[hegel::test(test_cases = 300)]
fn validation_accepts_exactly_what_coercion_makes_valid(tc: TestCase) {
    let schema = tc.draw(generators::arg_schema(3));
    let value = match tc.draw(gs::sampled_from(vec![
        "arbitrary",
        "valid",
        "for another schema",
    ])) {
        "arbitrary" => tc.draw(json_gs::values()),
        "valid" => tc.draw(generators::arg_value_for_schema(schema.clone())),
        _ => {
            let other = tc.draw(generators::arg_schema(3));
            tc.draw(generators::arg_value_for_schema(other))
        }
    };
    let compiled = compile(&schema);
    let coerced = compiled.coerce(&value);
    let accepted = jsonschema::is_valid(&schema, &coerced);
    tc.event(if accepted { "accepted" } else { "rejected" });
    match compiled.validate(&value) {
        Ok(validated) => {
            assert!(accepted, "{schema} accepted {value} as {validated}");
            assert_eq!(validated, coerced);
        }
        Err(error) => {
            assert!(!accepted, "{schema} rejected {value}: {error}");
            assert!(
                error.to_string().starts_with("Validation failed:\n  - "),
                "{error}"
            );
        }
    }
}

/// The objects in `value` (valid under `schema`) that hold a required
/// property, found by walking both together through properties and
/// array items: each as the JSON pointer to the object, its path as a
/// validation error spells it, and the required key. Values under an
/// `anyOf` are skipped, since which branch they satisfy is not known.
fn required_slots(
    schema: &Value,
    value: &Value,
    pointer: &str,
    path: &str,
    out: &mut Vec<(String, String, String)>,
) {
    if schema.get("anyOf").is_some() {
        return;
    }
    let join = |segment: &str| {
        if path.is_empty() {
            segment.to_owned()
        } else {
            format!("{path}.{segment}")
        }
    };
    match value {
        Value::Object(entries) => {
            let required = schema
                .get("required")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for key in required.iter().filter_map(Value::as_str) {
                if entries.contains_key(key) {
                    out.push((
                        pointer.to_owned(),
                        path.to_owned(),
                        key.to_owned(),
                    ));
                }
            }
            let properties =
                schema.get("properties").and_then(Value::as_object);
            for (key, entry) in entries {
                if let Some(property) = properties.and_then(|p| p.get(key)) {
                    required_slots(
                        property,
                        entry,
                        &format!("{pointer}/{key}"),
                        &join(key),
                        out,
                    );
                }
            }
        }
        Value::Array(items) => {
            if let Some(item_schema) = schema.get("items") {
                for (i, item) in items.iter().enumerate() {
                    required_slots(
                        item_schema,
                        item,
                        &format!("{pointer}/{i}"),
                        &join(&i.to_string()),
                        out,
                    );
                }
            }
        }
        _ => {}
    }
}

/// An object missing a required property fails validation, and the one
/// error line names that property by its full field path, however deep
/// (pi's `formatValidationPath` special-cases a `required` violation to
/// name the missing property itself, not just its parent).
#[hegel::test(test_cases = 300)]
fn missing_required_property_is_rejected_with_its_path(tc: TestCase) {
    // Two levels of required objects, then whatever `arg_schema` draws.
    let schema = json!({
        "type": "object",
        "properties": {"field": {
            "type": "object",
            "properties": {
                "p0": tc.draw(generators::arg_schema(2)),
                "p1": tc.draw(generators::arg_schema(2)),
            },
            "required": ["p0", "p1"],
        }},
        "required": ["field"],
    });
    let valid = tc.draw(generators::arg_value_for_schema(schema.clone()));
    let compiled = compile(&schema);
    assert!(compiled.validate(&valid).is_ok());

    let mut slots = Vec::new();
    required_slots(&schema, &valid, "", "", &mut slots);
    // The root's `field` is always a slot; lean toward the deeper ones.
    if slots.len() > 1 && tc.draw(gs::weighted_booleans(0.8)) {
        slots.remove(0);
    }
    let (pointer, path, key) = tc.draw(gs::sampled_from(slots));
    let mut broken = valid.clone();
    broken
        .pointer_mut(&pointer)
        .and_then(Value::as_object_mut)
        .expect("the slot is an object")
        .remove(&key);
    tc.event(if path.is_empty() {
        "top level"
    } else {
        "nested"
    });
    let missing = if path.is_empty() {
        key.clone()
    } else {
        format!("{path}.{key}")
    };
    let error = compiled
        .validate(&broken)
        .expect_err("a required property was dropped");
    let want = format!(
        "Validation failed:\n  - {missing}: \"{key}\" is a required property\n\nReceived arguments:\n"
    );
    assert!(
        error.to_string().starts_with(&want),
        "want {want:?}, got: {error}"
    );
}

/// A value of a leaf type that no coercion rule turns into `ty`: an
/// object or an array for any scalar type, and for the non-string types
/// a string or number the coercion rules leave alone.
fn uncoercible(tc: &TestCase, ty: &str) -> Value {
    let mut kinds = vec!["object", "array"];
    match ty {
        "number" | "integer" => kinds.push("word"),
        "boolean" => kinds.extend(["word", "other number"]),
        "null" => kinds.extend(["word", "other number", "true"]),
        _ => {}
    }
    if ty == "integer" {
        kinds.push("fraction");
    }
    match tc.draw(gs::sampled_from(kinds)) {
        "object" => json!({"unexpected": tc.draw(json_gs::values())}),
        "array" => {
            Value::Array(tc.draw(gs::vecs(json_gs::values()).max_size(3)))
        }
        // Letters alone never parse as a finite number, and "true" and
        // "false" are the only strings a boolean takes: a word not
        // starting with `t` or `f` is neither.
        "word" => Value::String(
            tc.draw(gs::from_regex("[a-eg-su-z][a-z]{0,7}").fullmatch(true)),
        ),
        "other number" => json!(tc.draw(hegel::one_of!(
            gs::integers::<i64>().max_value(-1),
            gs::integers::<i64>().min_value(2),
        ))),
        "true" => Value::Bool(true),
        _ => json!(
            tc.draw(gs::integers::<i32>()) as f64
                + tc.draw(gs::sampled_from(vec![0.25, 0.5, 0.75]))
        ),
    }
}

/// A value whose type cannot be coerced into what a required property
/// demands is rejected, and the error names that property's field path,
/// however deep the property sits.
#[hegel::test(test_cases = 300)]
fn wrong_type_that_cannot_coerce_is_rejected_with_its_path(tc: TestCase) {
    let ty = tc.draw(gs::sampled_from(vec![
        "string", "number", "integer", "boolean", "null",
    ]));
    let leaf = json!({"type": ty});
    let bad = uncoercible(&tc, ty);
    assert!(!jsonschema::is_valid(&leaf, &bad), "{bad} is a {ty}");
    let keys: Vec<String> = tc.draw(
        gs::vecs(gs::from_regex("[a-z]{1,4}").fullmatch(true))
            .min_size(1)
            .max_size(4),
    );
    let (mut schema, mut args) = (leaf, bad);
    for key in keys.iter().rev() {
        schema = json!({
            "type": "object",
            "properties": {key.as_str(): schema},
            "required": [key],
        });
        args = json!({key.as_str(): args});
    }
    let compiled = compile(&schema);
    let error = compiled
        .validate(&args)
        .expect_err("the value cannot coerce into the leaf type");
    let want = format!("Validation failed:\n  - {}: ", keys.join("."));
    assert!(
        error.to_string().starts_with(&want),
        "want {want:?}, got: {error}"
    );
    tc.event(ty);
}

// =============================================================================
// Known cases, ported from pi's `validation.test.ts` (commit `2b0a123`)
// =============================================================================

fn wrap(schema: Value) -> Value {
    json!({
        "type": "object",
        "properties": {"value": schema},
        "required": ["value"],
    })
}

/// pi's "coerces serialized plain JSON schemas with AJV-compatible
/// primitive rules" (`validation.test.ts:64`).
#[test]
fn pi_known_case_passing_primitive_coercions() {
    let cases: Vec<(Value, Value, Value)> = vec![
        (json!({"type": "number"}), json!("42"), json!(42)),
        (json!({"type": "number"}), json!(true), json!(1)),
        (json!({"type": "number"}), Value::Null, json!(0)),
        (json!({"type": "integer"}), json!("42"), json!(42)),
        (json!({"type": "boolean"}), json!("true"), json!(true)),
        (json!({"type": "boolean"}), json!("false"), json!(false)),
        (json!({"type": "boolean"}), json!(1), json!(true)),
        (json!({"type": "boolean"}), json!(0), json!(false)),
        (json!({"type": "string"}), Value::Null, json!("")),
        (json!({"type": "string"}), json!(true), json!("true")),
        (json!({"type": "null"}), json!(""), Value::Null),
        (json!({"type": "null"}), json!(0), Value::Null),
        (json!({"type": "null"}), json!(false), Value::Null),
        (
            json!({"type": ["number", "string"]}),
            json!("1"),
            json!("1"),
        ),
        (json!({"type": ["boolean", "number"]}), json!("1"), json!(1)),
    ];
    for (leaf, input, expected) in cases {
        let schema = wrap(leaf.clone());
        let compiled = compile(&schema);
        let args = json!({"value": input});
        let result = compiled
            .validate(&args)
            .unwrap_or_else(|e| panic!("{leaf} rejected {args}: {e}"));
        assert_eq!(result, json!({"value": expected}), "schema {leaf}");
    }
}

/// pi's "treats null as omission for optional non-nullable properties"
/// (`validation.test.ts:101`).
#[test]
fn pi_known_case_null_omission() {
    let schema = json!({
        "type": "object",
        "properties": {
            "path": {"type": "string"},
            "offset": {"type": "number"},
            "nullable": {"anyOf": [{"type": "string"}, {"type": "null"}]},
            "metadata": {
                "type": "object",
                "properties": {"enabled": {"type": "boolean"}},
                "required": [],
            },
        },
        "required": ["path", "metadata"],
    });
    let compiled = compile(&schema);
    let args = json!({
        "path": "file.txt",
        "offset": Value::Null,
        "nullable": Value::Null,
        "metadata": {"enabled": Value::Null},
    });
    let result = compiled.validate(&args).expect("valid after coercion");
    assert_eq!(
        result,
        json!({"path": "file.txt", "nullable": Value::Null, "metadata": {}})
    );
}

/// pi's "preserves optional nulls whose referenced schema is nullable"
/// (`validation.test.ts:126`): the referenced schema allows `null`, so
/// the `null` is kept. pi keeps it because it never follows `$ref`;
/// tau-agent follows it and finds the `null` allowed.
#[test]
fn pi_known_case_ref_property_keeps_null() {
    let schema = json!({
        "type": "object",
        "properties": {"value": {"$ref": "#/$defs/value"}},
        "$defs": {"value": {"anyOf": [{"type": "number"}, {"type": "null"}]}},
    });
    let compiled = compile(&schema);
    let args = json!({"value": Value::Null});
    assert_eq!(compiled.validate(&args).unwrap(), args);
}

/// Unlike pi, the coercion walk follows `$ref`: an optional property
/// inside a referenced object, sent as `null` by a strict-mode model, is
/// dropped as it would be inline. Rust schemas put every nested struct
/// behind a `$ref`, so without this a strict tool with a nested struct
/// could never omit an optional field.
#[test]
fn optional_nulls_inside_referenced_objects_are_dropped() {
    let schema = json!({
        "type": "object",
        "properties": {"inner": {"$ref": "#/$defs/Inner"}},
        "required": ["inner"],
        "$defs": {"Inner": {
            "type": "object",
            "properties": {"name": {"type": "string"}, "limit": {"type": "integer"}},
            "required": ["name"],
        }},
    });
    let compiled = compile(&schema);
    let args = json!({"inner": {"name": "a", "limit": Value::Null}});
    assert_eq!(
        compiled.validate(&args).unwrap(),
        json!({"inner": {"name": "a"}})
    );
}

/// pi's "preserves a value that already matches a nullable union arm"
/// and "... a oneOf nullable union arm" (`validation.test.ts:146,164`).
#[test]
fn pi_known_case_preserves_matching_union_null() {
    for keyword in ["anyOf", "oneOf"] {
        let schema =
            wrap(json!({keyword: [{"type": "number"}, {"type": "null"}]}));
        let compiled = compile(&schema);
        let args = json!({"value": Value::Null});
        assert_eq!(
            compiled.validate(&args).unwrap(),
            args,
            "keyword {keyword}"
        );
    }
}

/// pi's "still coerces nullable unions when the original value does not
/// match any arm" (`validation.test.ts:173`).
#[test]
fn pi_known_case_coerces_unmatched_nullable_union() {
    let schema = wrap(json!({"anyOf": [{"type": "number"}, {"type": "null"}]}));
    let compiled = compile(&schema);
    let args = json!({"value": "42"});
    assert_eq!(compiled.validate(&args).unwrap(), json!({"value": 42}));
}

/// pi's "accepts null for nullable array schemas with items"
/// (`validation.test.ts:182`).
#[test]
fn pi_known_case_null_for_nullable_array() {
    let schema =
        wrap(json!({"type": ["array", "null"], "items": {"type": "string"}}));
    let compiled = compile(&schema);
    let args = json!({"value": Value::Null});
    assert_eq!(compiled.validate(&args).unwrap(), args);
}

/// pi's "rejects invalid coercions for serialized plain JSON schemas"
/// (`validation.test.ts:194`).
#[test]
fn pi_known_case_rejects_invalid_coercions() {
    let cases = vec![
        (json!({"type": "boolean"}), json!("1")),
        (json!({"type": "boolean"}), json!("0")),
        (json!({"type": "null"}), json!("null")),
        (json!({"type": "integer"}), json!("42.1")),
    ];
    for (leaf, input) in cases {
        let schema = wrap(leaf.clone());
        let compiled = compile(&schema);
        let args = json!({"value": input.clone()});
        let error = compiled
            .validate(&args)
            .expect_err(&format!("{leaf} should reject {input}"));
        assert!(error.to_string().contains("Validation failed"));
    }
}

/// The `Value.Convert`/`FromArray` rule tau-agent adds on top of pi's
/// custom coercion (`docs/reference/agent-loop.md`, "Coercion before
/// validation", rule 3): a bare value becomes a one-element array where
/// the schema says `array`.
#[test]
fn single_value_becomes_one_element_array() {
    let schema = wrap(json!({"type": "array", "items": {"type": "string"}}));
    let compiled = compile(&schema);
    let args = json!({"value": "solo"});
    assert_eq!(
        compiled.validate(&args).unwrap(),
        json!({"value": ["solo"]})
    );
}

// =============================================================================
// Golden error messages
// =============================================================================

/// The envelope around a validation failure: a `- path: message` line
/// per violation, then the pretty-printed original arguments (not the
/// coerced ones), exactly as pi's own message pairs its errors with the
/// call's original arguments (`validation.test.ts:347`), minus the tool
/// name pi's version opens with (`ArgumentSchema` does not know it; see
/// the module doc comment).
#[test]
fn error_message_names_the_path_and_shows_original_arguments() {
    let schema = json!({
        "type": "object",
        "properties": {"count": {"type": "integer"}},
        "required": ["count"],
    });
    let compiled = compile(&schema);
    let args = json!({"count": "not-a-number"});
    let error = compiled.validate(&args).unwrap_err();
    let message = error.to_string();
    assert!(
        message.starts_with("Validation failed:\n  - count: "),
        "message: {message}"
    );
    assert!(
        message.ends_with(
            "\n\nReceived arguments:\n{\n  \"count\": \"not-a-number\"\n}"
        ),
        "message: {message}"
    );
}

/// A missing required property is named by the property itself, not by
/// its parent path, matching pi's `formatValidationPath` special case
/// for the `required` keyword.
#[test]
fn error_message_names_a_nested_missing_required_property() {
    let schema = json!({
        "type": "object",
        "properties": {
            "outer": {
                "type": "object",
                "properties": {"inner": {"type": "string"}},
                "required": ["inner"],
            },
        },
        "required": ["outer"],
    });
    let compiled = compile(&schema);
    let args = json!({"outer": {}});
    let error = compiled.validate(&args).unwrap_err();
    assert!(
        error.to_string().contains("- outer.inner:"),
        "error: {error}"
    );
}

/// A schema `jsonschema` cannot compile — here, a `$ref` to a document
/// this crate never provides a resolver for (`jsonschema` is built with
/// `default-features = false`: no `resolve-http`/`resolve-file`, so even
/// a reachable URL cannot be fetched) — is a
/// [`tau_agent::validation::SchemaError`], not a panic.
#[test]
fn incompatible_schema_is_a_schema_error_not_a_panic() {
    let unresolvable =
        json!({"$ref": "https://tau-agent.invalid/does-not-exist.json"});
    let error = ArgumentSchema::new(&unresolvable)
        .expect_err("an unresolvable $ref must not compile");
    assert!(!error.to_string().is_empty());
}

/// Every generated schema over the shapes [`generators::arg_schema`]
/// draws compiles: the generator never has to fall back to `tc.assume`
/// (`docs/reference/testing.md`, "Build valid values directly").
#[hegel::test(test_cases = 300)]
fn every_generated_schema_compiles(tc: TestCase) {
    let schema = tc.draw(generators::arg_schema(3));
    let _ = compile(&schema);
}

/// `SchemaError` and `ValidationError` behave like ordinary errors:
/// `Display` gives a human-readable, non-empty message, and both
/// implement `std::error::Error`.
#[test]
fn errors_implement_display_and_std_error() {
    fn assert_error<E: std::error::Error>(error: &E) {
        assert!(!error.to_string().is_empty());
    }
    let schema_error = ArgumentSchema::new(&json!({
        "$ref": "https://tau-agent.invalid/missing.json"
    }))
    .unwrap_err();
    assert_error(&schema_error);

    let schema = compile(&json!({"type": "string"}));
    let validation_error = schema
        .validate(&json!({"nested": {"object": true}}))
        .unwrap_err();
    assert_error(&validation_error);
}

// =============================================================================
// Mutation-covering tests
//
// Each of these pins a rule the property tests above happen not to reach:
// either because the shapes `generators::arg_schema` draws never include
// it (a JSON-Schema `"type"` array combining a scalar keyword with
// another type, an `additionalProperties` schema, a tuple `items`), or
// because the property tests only check "is this rejected", not the
// exact coerced value, which is what a few of these guards actually
// change. `cargo mutants` found the gap in each case; see the module's
// mutants run for the full list.
// =============================================================================

/// `ArgumentSchema`'s `Debug` impl is not the default placeholder: it
/// shows the schema it was built from.
#[test]
fn debug_impl_shows_the_schema() {
    let schema = compile(&json!({"type": "string"}));
    let text = format!("{schema:?}");
    assert!(text.contains("ArgumentSchema"), "{text}");
    assert!(text.contains("string"), "{text}");
}

/// `null` on an optional, non-nullable field is stripped inside array
/// elements too, not just at the top level: `normalizeOptionalNulls`
/// recurses through a single-schema `items`.
#[test]
fn null_omission_recurses_into_array_items() {
    let schema = json!({
        "type": "object",
        "properties": {
            "items": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {"present": {"type": "string"}, "extra": {"type": "number"}},
                    "required": ["present"],
                },
            },
        },
        "required": ["items"],
    });
    let compiled = compile(&schema);
    let args = json!({"items": [{"present": "a", "extra": Value::Null}, {"present": "b"}]});
    let result = compiled.validate(&args).expect("valid after coercion");
    assert_eq!(
        result,
        json!({"items": [{"present": "a"}, {"present": "b"}]})
    );
}

/// The same, for a tuple `items` (`schema.items` itself an array of
/// per-position schemas). Draft-07 shape: see the note on `$schema`
/// below and on [`tuple_items_are_coerced_positionally`].
#[test]
fn null_omission_recurses_into_tuple_items() {
    let schema = json!({
        // Tuple validation via an array-valued `items` is draft-07
        // (2020-12 uses `prefixItems` instead, which this port does not
        // handle — see the doc comment on this test).
        "$schema": "http://json-schema.org/draft-07/schema#",
        "type": "object",
        "properties": {
            "pair": {
                "type": "array",
                "items": [
                    {
                        "type": "object",
                        "properties": {"present": {"type": "string"}, "extra": {"type": "number"}},
                        "required": ["present"],
                    },
                    {"type": "string"},
                ],
            },
        },
        "required": ["pair"],
    });
    let compiled = compile(&schema);
    let args = json!({"pair": [{"present": "a", "extra": Value::Null}, "tag"]});
    let result = compiled.validate(&args).expect("valid after coercion");
    assert_eq!(result, json!({"pair": [{"present": "a"}, "tag"]}));
}

/// A value that already matches one member of a multi-type `"type"`
/// array is left alone, for every scalar keyword, not only the ones the
/// known cases above happen to combine. This is what actually exercises
/// `matches_json_type`'s `"number"`/`"integer"`/`"boolean"` arms and
/// `is_integer_number`: a value paired with a *non-matching* second type
/// (`"string"`) is only left unchanged if the matching arm is checked
/// correctly, since `coerce_string` would otherwise stringify it.
#[test]
fn already_matching_union_member_is_left_alone() {
    let cases = vec![
        (json!(["number", "string"]), json!(5)),
        // A negative integer: `is_i64()` is true but `is_u64()` is
        // false, so this also exercises `is_i64() || is_u64()`.
        (json!(["integer", "string"]), json!(-5)),
        (json!(["boolean", "string"]), json!(true)),
    ];
    for (ty, value) in cases {
        let schema = wrap(json!({"type": ty}));
        let compiled = compile(&schema);
        let args = json!({"value": value});
        assert_eq!(compiled.coerce(&args), args, "type {ty}");
    }
}

/// A number that is not integer-valued does *not* match `"integer"`, so
/// it still coerces to its string form under `["integer", "string"]` —
/// the mirror image of [`already_matching_union_member_is_left_alone`],
/// which exercises `is_integer_number` returning `true` incorrectly;
/// this one exercises it returning `false` incorrectly.
#[test]
fn non_integer_number_under_integer_union_still_stringifies() {
    let schema = wrap(json!({"type": ["integer", "string"]}));
    let compiled = compile(&schema);
    let args = json!({"value": 5.5});
    assert_eq!(compiled.coerce(&args), json!({"value": "5.5"}));
}

/// An `f64`-backed whole number (as opposed to one already stored as an
/// integer, which never reaches `Number::fract`) still counts as an
/// integer: `is_integer_number`'s float branch checks `fract() == 0.0`,
/// not `!= 0.0`.
#[test]
fn f64_backed_whole_number_matches_integer_union_member() {
    let schema = wrap(json!({"type": ["integer", "string"]}));
    let compiled = compile(&schema);
    let value = Value::Number(serde_json::Number::from_f64(5.0).unwrap());
    assert!(
        !value.is_i64() && !value.is_u64(),
        "value must be float-backed"
    );
    let args = json!({"value": value});
    assert_eq!(compiled.coerce(&args), args);
}

/// A string that parses to exactly `2^53` stays a float: at the
/// boundary [`whole_number_value`] checks, `9007199254740992` is
/// converted to `i64` only when strictly under it.
#[test]
fn whole_number_boundary_stays_float_backed() {
    let compiled = compile(&json!({"type": "number"}));
    let result = compiled.coerce(&json!("9007199254740992"));
    assert_eq!(result.as_f64(), Some(9_007_199_254_740_992.0));
    assert!(
        !result.is_i64() && !result.is_u64(),
        "{result} must stay float-backed"
    );
}

/// A string that cannot become a valid integer (a fractional value) is
/// left exactly as it was received, not silently truncated: coercion's
/// job is to try a conversion and fall back, never to lose data no
/// legitimate reading of the input supports.
#[test]
fn fractional_string_is_left_unchanged_under_integer_type() {
    let compiled = compile(&wrap(json!({"type": "integer"})));
    let args = json!({"value": "42.1"});
    assert_eq!(compiled.coerce(&args), args);
}

/// A non-finite numeric string (`"Infinity"`, which `str::parse::<f64>`
/// happily accepts) is left alone under `"number"` too: `coerce_number`
/// requires *both* a finite parse and (for `"integer"`) no fractional
/// part, not either.
#[test]
fn non_finite_string_is_left_unchanged_under_number_type() {
    let compiled = compile(&wrap(json!({"type": "number"})));
    let args = json!({"value": "Infinity"});
    assert_eq!(compiled.coerce(&args), args);
}

/// A number becomes a string where the schema says `string` — the
/// [`pi_known_case_passing_primitive_coercions`] table happens to only
/// cover the boolean and null sources for this rule.
#[test]
fn number_becomes_string() {
    let compiled = compile(&wrap(json!({"type": "string"})));
    let args = json!({"value": 42});
    assert_eq!(compiled.coerce(&args), json!({"value": "42"}));
}

/// `additionalProperties`' schema coerces the extra keys, and *only*
/// the extra keys: a property already listed under `properties` keeps
/// going through its own schema, never the `additionalProperties` one.
#[test]
fn additional_properties_schema_coerces_only_undeclared_keys() {
    let schema = json!({
        "type": "object",
        "properties": {"known": {"type": "string"}},
        "additionalProperties": {"type": "number"},
        "required": ["known"],
    });
    let compiled = compile(&schema);
    let args = json!({"known": "hello", "extra": "42"});
    let result = compiled.validate(&args).expect("valid after coercion");
    assert_eq!(result, json!({"known": "hello", "extra": 42}));
}

/// An array's elements are coerced against a single-schema `items`, not
/// merely wrapped: each element that needs converting is converted.
#[test]
fn array_items_are_coerced_against_a_single_schema() {
    let schema = wrap(json!({"type": "array", "items": {"type": "number"}}));
    let compiled = compile(&schema);
    let args = json!({"value": ["42"]});
    assert_eq!(compiled.validate(&args).unwrap(), json!({"value": [42]}));
}

/// A tuple `items` coerces each position against its own schema.
///
/// **Deviation from tau-agent's own schemas, faithful to pi:** tuple
/// validation via an array-valued `items` is draft-07 (`schema.items`
/// as `Array.isArray` checks it in pi's `coerceWithJsonSchema`).
/// `schemars` 1.2.2, which generates every real tool schema in this
/// crate, defaults to 2020-12 and expresses a tuple with `prefixItems`
/// instead — a keyword neither pi's original code nor this port
/// recognizes. This test pins the ported behavior on the shape pi
/// actually handles, via an explicit `$schema`; a `prefixItems` tuple
/// coerces no differently than an untyped `items: true` schema would
/// (no per-position coercion), matching pi's own gap.
#[test]
fn tuple_items_are_coerced_positionally() {
    // `$schema` only takes effect at the document root, so (unlike the
    // other cases here) this cannot use `wrap`.
    let schema = json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "type": "object",
        "properties": {
            "value": {"type": "array", "items": [{"type": "number"}, {"type": "string"}]},
        },
        "required": ["value"],
    });
    let compiled = compile(&schema);
    let args = json!({"value": ["42", 5]});
    assert_eq!(
        compiled.validate(&args).unwrap(),
        json!({"value": [42, "5"]})
    );
}

/// Tuples coerce position by position, differentially: coercing
/// `[a, b, c]` under a tuple of `S1, S2` with `R` for the rest equals
/// coercing `a` under `S1`, `b` under `S2` and `c` under `R` one at a
/// time. It holds for both spellings: 2020-12's `prefixItems` + `items`
/// (what `schemars` emits for tuples) and the positional `items` array
/// + `additionalItems`. Each element is drawn loose (a number as a
/// string, a scalar where an array is expected) or valid.
#[hegel::test(test_cases = 200)]
fn tuples_coerce_each_position_under_its_schema(tc: TestCase) {
    let schemas: Vec<Value> =
        (0..3).map(|_| tc.draw(generators::arg_schema(1))).collect();
    let values: Vec<Value> = schemas
        .iter()
        .map(|schema| {
            let value =
                tc.draw(generators::arg_value_for_schema(schema.clone()));
            match (tc.draw(gs::booleans()), &value) {
                (true, Value::Number(n)) => Value::String(n.to_string()),
                (true, Value::Array(items)) if items.len() == 1 => {
                    items[0].clone()
                }
                _ => value,
            }
        })
        .collect();
    // The positional `items` array is draft 7; `$schema` only takes
    // effect at the root.
    let one = |value: &Value, schema: &Value| {
        compile(&json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "properties": {"v": schema},
            // Required, so a `null` element is coerced, not dropped as
            // an omitted optional.
            "required": ["v"],
        }))
        .coerce(&json!({"v": value}))["v"]
            .clone()
    };
    let expected: Vec<Value> = values
        .iter()
        .zip(&schemas)
        .map(|(v, s)| one(v, s))
        .collect();
    let spellings = [
        json!({"type": "array", "prefixItems": [schemas[0], schemas[1]], "items": schemas[2]}),
        json!({"type": "array", "items": [schemas[0], schemas[1]], "additionalItems": schemas[2]}),
    ];
    for tuple in spellings {
        let coerced = one(&Value::Array(values.clone()), &tuple);
        assert_eq!(coerced, Value::Array(expected.clone()), "under {tuple}");
    }
}

/// A Rust tuple argument, as `schemars` describes it, is coerced item
/// by item and then validates: `("3", "true", 5)` becomes `[3, true,
/// "5"]`. pi does not coerce `prefixItems` at all.
#[test]
fn schemars_tuples_are_coerced_item_by_item() {
    #[allow(dead_code)]
    #[derive(schemars::JsonSchema)]
    struct Args {
        point: (u32, bool, String),
    }
    let schema = serde_json::to_value(schemars::schema_for!(Args)).unwrap();
    assert!(schema.to_string().contains("prefixItems"), "{schema}");
    let args = json!({"point": ["3", "true", 5]});
    assert_eq!(
        compile(&schema).validate(&args).unwrap(),
        json!({"point": [3, true, "5"]})
    );
}

/// Positions past a tuple with no schema for the rest, and positions
/// whose schema is a boolean, are left alone.
#[test]
fn tuple_positions_without_a_schema_are_left_alone() {
    let cases = [
        json!({"type": "array", "prefixItems": [{"type": "number"}]}),
        json!({"type": "array", "items": [{"type": "number"}]}),
        json!({"type": "array", "prefixItems": [{"type": "number"}], "items": true}),
    ];
    for schema in cases {
        let wrapped = json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "properties": {"v": schema},
        });
        assert_eq!(
            compile(&wrapped).coerce(&json!({"v": ["1", "2"]})),
            json!({"v": [1, "2"]}),
            "under {schema}"
        );
    }
    let boolean = json!({"type": "object", "properties": {"v": {"type": "array", "prefixItems": [true, {"type": "number"}]}}});
    assert_eq!(
        compile(&boolean).coerce(&json!({"v": ["1", "2"]})),
        json!({"v": ["1", 2]})
    );
}

/// Optional nulls inside tuple positions are dropped like anywhere
/// else: the walk follows `prefixItems` too.
#[test]
fn optional_nulls_inside_tuple_positions_are_dropped() {
    let schema = json!({
        "type": "object",
        "properties": {"v": {
            "type": "array",
            "prefixItems": [{
                "type": "object",
                "properties": {"a": {"type": "string"}},
            }],
        }},
    });
    assert_eq!(
        compile(&schema).coerce(&json!({"v": [{"a": null}]})),
        json!({"v": [{}]})
    );
}
