//! Strict JSON Schema rewrite (`tau_agent::schema`).
//!
//! Oracle for the wire shape: pi's `constrained-sampling.ts` and its test
//! file `constrained-sampling.test.ts`, commit `2b0a123`. Oracle for
//! "does this value validate": the `jsonschema` crate, a validator this
//! code does not share with `to_strict`/`strip_nulls_for_optional`.
//!
//! See `docs/reference/testing.md`'s `tau-agent` property inventory for
//! the rules this file checks.

use std::collections::HashSet;

use hegel::TestCase;
use serde_json::{Map, Value, json};
use tau_agent::schema::{
    NotStrict,
    inline_refs,
    strip_nulls_for_optional,
    to_strict,
};
use tau_testing::generators;

// =============================================================================
// Test-only oracles
// =============================================================================

/// Walks `schema` (a [`to_strict`] output) and panics if any object node
/// is missing `additionalProperties: false` or does not list every one
/// of its own properties in `required`. This is the invariant property's
/// oracle: it is written independently of `to_strict`'s own logic, by
/// just reading back the two fields OpenAI's strict mode requires.
fn assert_every_object_is_closed_and_fully_required(schema: &Value) {
    let Some(obj) = schema.as_object() else {
        return;
    };
    if matches!(obj.get("type"), Some(Value::String(t)) if t == "object") {
        assert_eq!(
            obj.get("additionalProperties"),
            Some(&Value::Bool(false)),
            "object schema is not closed: {schema}"
        );
        let properties = obj
            .get("properties")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let mut names: Vec<&str> =
            properties.keys().map(String::as_str).collect();
        names.sort_unstable();
        let mut required: Vec<&str> = obj
            .get("required")
            .and_then(Value::as_array)
            .map(|values| values.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        required.sort_unstable();
        assert_eq!(names, required, "not every property is required: {schema}");
        for property in properties.values() {
            assert_every_object_is_closed_and_fully_required(property);
        }
    }
    if let Some(items) = obj.get("items") {
        assert_every_object_is_closed_and_fully_required(items);
    }
    if let Some(variants) = obj.get("anyOf").and_then(Value::as_array) {
        for variant in variants {
            assert_every_object_is_closed_and_fully_required(variant);
        }
    }
}

/// The metamorphic transform the property inventory names: a value valid
/// under `schema`, with every optional property that is missing from the
/// value filled in as `null` (recursing into properties and array items
/// that are themselves present). This is the input transformation the
/// property tests, not the code under test: it is the inverse of
/// [`strip_nulls_for_optional`], written separately from it.
fn nullify_missing_optionals(schema: &Value, value: &Value) -> Value {
    let Some(schema_obj) = schema.as_object() else {
        return value.clone();
    };
    if let Value::Array(items) = value {
        let item_schema = schema_obj.get("items").filter(|s| !s.is_array());
        return Value::Array(
            items
                .iter()
                .map(|item| match item_schema {
                    Some(s) => nullify_missing_optionals(s, item),
                    None => item.clone(),
                })
                .collect(),
        );
    }
    let Value::Object(value_obj) = value else {
        return value.clone();
    };
    let Some(properties) =
        schema_obj.get("properties").and_then(Value::as_object)
    else {
        return value.clone();
    };
    let required: HashSet<&str> = schema_obj
        .get("required")
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    let mut result: Map<String, Value> = value_obj.clone();
    for key in properties.keys() {
        if !required.contains(key.as_str()) {
            result.entry(key.clone()).or_insert(Value::Null);
        }
    }
    for (key, property_schema) in properties {
        if let Some(present) = result.get(key).cloned()
            && !present.is_null()
        {
            result.insert(
                key.clone(),
                nullify_missing_optionals(property_schema, &present),
            );
        }
    }
    Value::Object(result)
}

// =============================================================================
// Properties
// =============================================================================

/// Strict schema rewrite: every object in the output has all properties
/// required and `additionalProperties: false` (testing.md, `tau-agent`
/// property inventory).
#[hegel::test(test_cases = 300)]
fn strict_output_is_fully_required_and_closed(tc: TestCase) {
    let schema = tc.draw(generators::strict_schema(3));
    let strict = to_strict(&schema)
        .expect("generator only builds strict-convertible schemas");
    assert_every_object_is_closed_and_fully_required(&strict);
}

/// Strict schema rewrite is idempotent.
#[hegel::test(test_cases = 300)]
fn strict_rewrite_is_idempotent(tc: TestCase) {
    let schema = tc.draw(generators::strict_schema(3));
    let once = to_strict(&schema)
        .expect("generator only builds strict-convertible schemas");
    let twice =
        to_strict(&once).expect("a strict schema is itself strict-convertible");
    assert_eq!(once, twice);
}

/// A value valid under the original schema, with missing optional fields
/// set to `null`, is valid under the strict schema.
#[hegel::test(test_cases = 300)]
fn nulled_optional_value_validates_under_strict_schema(tc: TestCase) {
    let (schema, value) = tc.draw(generators::strict_schema_with_value(3));
    let strict = to_strict(&schema)
        .expect("generator only builds strict-convertible schemas");
    let nulled = nullify_missing_optionals(&schema, &value);
    assert!(
        jsonschema::is_valid(&strict, &nulled),
        "nulled value does not validate under the strict schema\nschema: {schema}\nstrict: {strict}\nvalue: {value}\nnulled: {nulled}"
    );
}

/// `strip_nulls_for_optional` reverses the nulled-optional value back to
/// the original value, which validates against the original schema: the
/// round trip pi's tool-argument path (and, on the model's answer side,
/// a typed run) relies on.
#[hegel::test(test_cases = 300)]
fn strip_nulls_round_trips(tc: TestCase) {
    let (schema, value) = tc.draw(generators::strict_schema_with_value(3));
    let nulled = nullify_missing_optionals(&schema, &value);
    let stripped = strip_nulls_for_optional(&schema, &nulled);
    assert_eq!(stripped, value);
    assert!(
        jsonschema::is_valid(&schema, &stripped),
        "stripped value does not validate under the original schema\nschema: {schema}\nstripped: {stripped}"
    );
}

/// A schema pi's strict rewrite does not support is rejected with a
/// `NotStrict` naming one of pi's own reasons, never silently accepted
/// or panicking.
#[hegel::test(test_cases = 300)]
fn unsupported_schema_is_rejected_with_a_reason(tc: TestCase) {
    const KNOWN_REASONS: &[&str] = &[
        "boolean schemas are unsupported",
        "$ref schemas are unsupported",
        "$defs schemas are unsupported",
        "definitions schemas are unsupported",
        "allOf schemas are unsupported",
        "oneOf schemas are unsupported",
        "patternProperties schemas are unsupported",
        "dependentSchemas schemas are unsupported",
        "dependencies schemas are unsupported",
        "unevaluatedProperties schemas are unsupported",
        "propertyNames schemas are unsupported",
        "contains schemas are unsupported",
        "prefixItems schemas are unsupported",
        "not schemas are unsupported",
        "if schemas are unsupported",
        "then schemas are unsupported",
        "else schemas are unsupported",
        "anyOf must contain at least one schema",
        "recursive $ref schemas are unsupported",
        "tuple schemas are unsupported",
        "properties require type object",
        "schema-valued or true additionalProperties is unsupported",
        "object properties must be a schema map",
        "object required must be a string array",
        "required contains an unknown property",
        "root schema must have type object",
    ];

    let schema = tc.draw(generators::unsupported_schema());
    let error = to_strict(&schema)
        .expect_err("generator only builds unsupported schemas");
    assert!(
        KNOWN_REASONS.contains(&error.reason()),
        "unrecognized NotStrict reason {:?} for schema {schema}",
        error.reason()
    );
}

/// `NotStrict` carries its reason and behaves like a normal error type:
/// `Display` prints the reason, it implements `std::error::Error`, and
/// it is `Clone` + structurally comparable.
#[test]
fn not_strict_behaves_like_a_normal_error() {
    let error: NotStrict = to_strict(&json!("not an object")).unwrap_err();
    assert_eq!(error.reason(), "root schema must have type object");
    assert_eq!(error.to_string(), error.reason());
    assert_eq!(error.clone(), error);
    let as_error: &dyn std::error::Error = &error;
    assert_eq!(as_error.to_string(), error.reason());
}

// =============================================================================
// `is_structured_schema` / `schema_allows_null`: the array (`type: [...]`)
// forms of `type`, and each disjunct of `is_structured_schema` in
// isolation. The property generators never draw a `type` array or a
// `properties`/`items`-only node with no `type` at all, so these forms
// need their own examples.
// =============================================================================

/// Object and array variants of an `anyOf` are made strict like any
/// other node (a deliberate difference from pi, which rejects them):
/// every object variant comes out closed and fully required, and an
/// array variant's items too.
#[test]
fn structured_any_of_variants_are_made_strict() {
    let schema = json!({
        "type": "object",
        "properties": {
            "a": {"anyOf": [
                {"type": "object", "properties": {"x": {"type": "string"}}},
                {"type": "null"},
            ]},
            "b": {"anyOf": [
                {"type": "array", "items": {
                    "type": "object",
                    "properties": {"y": {"type": "integer"}},
                    "required": ["y"],
                }},
                {"type": "string"},
            ]},
        },
        "required": ["a", "b"],
    });
    let strict = to_strict(&schema).unwrap();
    assert_every_object_is_closed_and_fully_required(&strict);
    assert_eq!(
        strict["properties"]["a"]["anyOf"][0]["properties"]["x"],
        json!({"anyOf": [{"type": "string"}, {"type": "null"}]})
    );
}

/// `schema_allows_null` recognizes `type` as an array containing
/// `"null"` (not just a bare `"null"` string or an `anyOf` branch): an
/// optional property shaped that way is left alone, not wrapped in a
/// second `anyOf`. A sibling property whose `type` array does *not*
/// contain `"null"` is the negative control: it must still be wrapped.
#[test]
fn optional_property_with_null_in_a_type_array_is_not_wrapped_again() {
    let schema = json!({
        "type": "object",
        "properties": {
            "already_nullable": {"type": ["string", "null"]},
            "not_nullable": {"type": ["string", "boolean"]},
        },
        "required": [],
    });

    let strict =
        to_strict(&schema).expect("both properties are strict-convertible");

    assert_eq!(
        strict["properties"]["already_nullable"],
        json!({"type": ["string", "null"]}),
        "a type array already containing \"null\" must not be re-wrapped: {strict}"
    );
    assert_eq!(
        strict["properties"]["not_nullable"],
        json!({"anyOf": [{"type": ["string", "boolean"]}, {"type": "null"}]}),
        "a type array without \"null\" must still be wrapped: {strict}"
    );
    assert_eq!(
        strict["required"],
        json!(["already_nullable", "not_nullable"])
    );
}

// =============================================================================
// Known cases, ported from pi's constrained-sampling.test.ts (2b0a123)
// =============================================================================

/// Port of "derives strict provider schemas without changing tool
/// definitions" (`constrained-sampling.test.ts`). The TypeBox schema
/// there is written out as the plain JSON Schema it compiles to.
#[test]
fn strict_rewrite_matches_pi_typebox_example() {
    let parameters = json!({
        "type": "object",
        "properties": {
            "path": {"type": "string"},
            "offset": {"type": "number"},
            "metadata": {
                "type": "object",
                "properties": {"enabled": {"type": "boolean"}},
            },
            "nullable": {"anyOf": [{"type": "string"}, {"type": "null"}]},
        },
        "required": ["path", "metadata"],
    });

    let strict =
        to_strict(&parameters).expect("pi's example is strict-convertible");

    assert_eq!(
        strict,
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "offset": {"anyOf": [{"type": "number"}, {"type": "null"}]},
                "metadata": {
                    "type": "object",
                    "properties": {
                        "enabled": {"anyOf": [{"type": "boolean"}, {"type": "null"}]},
                    },
                    "required": ["enabled"],
                    "additionalProperties": false,
                },
                "nullable": {"anyOf": [{"type": "string"}, {"type": "null"}]},
            },
            "required": ["path", "offset", "metadata", "nullable"],
            "additionalProperties": false,
        }),
    );
}

/// Port of "falls back or rejects schemas that cannot be safely
/// converted" (`constrained-sampling.test.ts`): three of the four TypeBox
/// cases there, written out as plain JSON Schema, each rejected for the
/// same reason pi's test asserts (`toThrow` there is a substring match).
#[test]
fn known_unsupported_shapes_from_pi() {
    let cases: &[(Value, &str)] = &[
        (
            json!({
                "type": "object",
                "properties": {
                    "metadata": {
                        "type": "object",
                        "properties": {},
                        "additionalProperties": {"type": "string"},
                    },
                },
                "required": ["metadata"],
            }),
            "additionalProperties is unsupported",
        ),
        (
            json!({
                "allOf": [
                    {"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a"]},
                    {"type": "object", "properties": {"b": {"type": "number"}}, "required": ["b"]},
                ],
            }),
            "allOf schemas are unsupported",
        ),
        // pi's third case, a nullable object, is accepted on purpose:
        // see `structured_any_of_variants_are_made_strict`.
        (
            json!({
                "type": "object",
                "properties": {"child": {"$ref": "https://example.com/child.json"}},
                "required": ["child"],
            }),
            "$ref schemas are unsupported",
        ),
    ];

    for (parameters, expected_substring) in cases {
        match to_strict(parameters) {
            Ok(strict) => {
                panic!("expected {parameters} to be rejected, got {strict}")
            }
            Err(error) => assert!(
                error.reason().contains(*expected_substring),
                "expected reason {:?} to contain {expected_substring:?}, for {parameters}",
                error.reason()
            ),
        }
    }
}

// =============================================================================
// References
// =============================================================================

/// The metamorphic transform for references: moves drawn subschemas of
/// `schema` (property schemas and array items, at any depth) into
/// `defs` and points at them with `$ref`, as schemars does for named
/// types. The moved schemas are themselves refactored first.
fn factor_out(
    tc: &TestCase,
    schema: &Value,
    defs: &mut Map<String, Value>,
) -> Value {
    let Value::Object(obj) = schema else {
        return schema.clone();
    };
    let mut out = obj.clone();
    if let Some(Value::Object(properties)) = obj.get("properties") {
        let mut refactored = Map::new();
        for (name, property) in properties {
            refactored.insert(name.clone(), factor_child(tc, property, defs));
        }
        out.insert("properties".to_owned(), Value::Object(refactored));
    }
    if let Some(items) = obj.get("items") {
        out.insert("items".to_owned(), factor_child(tc, items, defs));
    }
    if let Some(Value::Array(variants)) = obj.get("anyOf") {
        let variants =
            variants.iter().map(|v| factor_child(tc, v, defs)).collect();
        out.insert("anyOf".to_owned(), Value::Array(variants));
    }
    Value::Object(out)
}

fn factor_child(
    tc: &TestCase,
    schema: &Value,
    defs: &mut Map<String, Value>,
) -> Value {
    let inner = factor_out(tc, schema, defs);
    if tc.draw(hegel::generators::booleans()) {
        let name = format!("D{}", defs.len());
        defs.insert(name.clone(), inner);
        json!({"$ref": format!("#/$defs/{name}")})
    } else {
        inner
    }
}

/// Inlining undoes factoring out: a schema with drawn subschemas moved
/// into `$defs` behind `$ref`s inlines back to the original, and so has
/// the same strict form.
#[hegel::test(test_cases = 300)]
fn inlining_references_undoes_factoring_them_out(tc: TestCase) {
    let schema = tc.draw(generators::strict_schema(3));
    let mut defs = Map::new();
    let mut refactored = factor_out(&tc, &schema, &mut defs);
    if !defs.is_empty() {
        refactored["$defs"] = Value::Object(defs);
    }
    assert_eq!(inline_refs(&refactored).unwrap(), schema);
    assert_eq!(to_strict(&refactored), to_strict(&schema));
}

/// Keywords next to a `$ref` are kept and win over the definition's;
/// `const`, `enum`, `default` and `examples` hold data, so a `$ref`
/// inside them is left alone; a property may be named like a keyword.
#[test]
fn inlining_keeps_siblings_and_leaves_data_alone() {
    let schema = json!({
        "type": "object",
        "properties": {
            "a": {"$ref": "#/definitions/A", "description": "outer"},
            "const": {"const": {"$ref": "#/$defs/A"}},
            "e": {"enum": [{"$ref": "x"}], "default": {"$ref": "y"}},
        },
        "definitions": {"A": {"type": "string", "description": "inner", "minLength": 1}},
    });
    assert_eq!(
        inline_refs(&schema).unwrap(),
        json!({
            "type": "object",
            "properties": {
                "a": {"type": "string", "description": "outer", "minLength": 1},
                "const": {"const": {"$ref": "#/$defs/A"}},
                "e": {"enum": [{"$ref": "x"}], "default": {"$ref": "y"}},
            },
        })
    );
}

/// A reference that is not local, names no definition, leads back to
/// itself, or names a boolean schema cannot be inlined.
#[test]
fn references_that_cannot_be_inlined_are_rejected() {
    let cases = [
        (
            json!({"$ref": "https://example.com/a.json"}),
            "$ref schemas are unsupported",
        ),
        (
            json!({"$ref": "#/$defs/Missing"}),
            "$ref schemas are unsupported",
        ),
        (
            json!({"$ref": "#/$defs/A", "$defs": {"A": {"anyOf": [{"$ref": "#/$defs/B"}]}, "B": {"items": {"$ref": "#/$defs/A"}}}}),
            "recursive $ref schemas are unsupported",
        ),
        (
            json!({"$ref": "#/$defs/T", "$defs": {"T": true}}),
            "boolean schemas are unsupported",
        ),
    ];
    for (schema, reason) in cases {
        let error = inline_refs(&schema).unwrap_err();
        assert_eq!(error.reason(), reason, "for {schema}");
    }
}

/// A definition used twice is inlined at both places: only a cycle is
/// recursion, not reuse.
#[test]
fn a_definition_may_be_used_twice() {
    let schema = json!({
        "type": "object",
        "properties": {"a": {"$ref": "#/$defs/S"}, "b": {"$ref": "#/$defs/S"}},
        "$defs": {"S": {"type": "string"}},
    });
    let inlined = inline_refs(&schema).unwrap();
    assert_eq!(inlined["properties"]["a"], json!({"type": "string"}));
    assert_eq!(inlined["properties"]["b"], json!({"type": "string"}));
}

/// A schemars schema with a nested and an optional nested struct gets a
/// strict form: references are inlined, and the optional one becomes a
/// nullable object.
#[test]
fn schemars_nested_structs_have_a_strict_form() {
    #[allow(dead_code)]
    #[derive(schemars::JsonSchema)]
    struct Outer {
        inner: Inner,
        maybe: Option<Inner>,
    }
    #[allow(dead_code)]
    #[derive(schemars::JsonSchema)]
    struct Inner {
        name: String,
    }
    let schema = serde_json::to_value(schemars::schema_for!(Outer)).unwrap();
    assert!(schema.get("$defs").is_some(), "{schema}");
    let strict = to_strict(&schema).unwrap();
    assert_every_object_is_closed_and_fully_required(&strict);
    assert!(!strict.to_string().contains("$ref"), "{strict}");
}
