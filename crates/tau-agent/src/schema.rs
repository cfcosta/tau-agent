//! OpenAI strict-mode JSON Schema rewrite.
//!
//! Ported from pi's `constrained-sampling.ts`
//! (`packages/ai/src/api/constrained-sampling.ts`, commit `2b0a123`):
//! `makeStrictJsonSchema` / `makeJsonSchemaNodeStrict` become [`to_strict`].
//! Only the `json_schema` strict rewrite is ported here; pi's grammar
//! (`lark`/`regex`) constrained sampling is a different code path and is
//! out of scope for tau-agent.
//!
//! The strict form OpenAI requires of a `json_schema` tool or output
//! format is a subset of JSON Schema: every object must list every one
//! of its properties in `required` and set `additionalProperties:
//! false`. Since a strict schema cannot express "this property is
//! optional", pi keeps optional properties by making them nullable
//! instead (`anyOf: [original, {"type": "null"}]`) and requiring them,
//! so the model can send `null` to mean "omitted". [`to_strict`] performs
//! that rewrite; [`strip_nulls_for_optional`] reverses it on a value, so
//! a model's strict-mode answer can be checked against the *original*
//! schema.
//!
//! Rust schemas put every named type in the root's `$defs` and point at
//! it with `$ref`, which pi's rewrite (built for TypeBox schemas, which
//! inline everything) does not support. [`inline_refs`] replaces each
//! local reference with its definition first, so a nested struct does
//! not cost a tool its strict mode. A recursive type cannot be inlined
//! and stays unsupported.
//!
//! **Deliberate difference from pi:** pi rejects an `anyOf` with an
//! object or array variant, because some of the providers it serves
//! cannot take one. OpenAI's strict mode can, and tau-agent talks to
//! nothing else, so each variant is made strict like any other node.
//! That is what gives `Option<SomeStruct>` a strict form. And every
//! object gets a `properties` map, empty when it had none, since OpenAI
//! rejects an object schema without one.
//!
//! Not every schema can be rewritten this way. A schema that uses a
//! keyword strict mode does not support (`allOf`, `oneOf`, a
//! schema-valued `additionalProperties`, a tuple `items`, a `$ref` that
//! cannot be inlined, ...) is rejected with [`NotStrict`] rather than
//! silently reinterpreted.

use serde_json::{Map, Value};

/// Schema keywords pi's strict rewrite does not support. Present on any
/// schema node, they make [`to_strict`] fail with a `NotStrict` naming
/// the key.
///
/// Source: `constrained-sampling.ts` `UNSUPPORTED_STRICT_SCHEMA_KEYS`.
const UNSUPPORTED_KEYS: &[&str] = &[
    "$ref",
    "$defs",
    "definitions",
    "allOf",
    "oneOf",
    "patternProperties",
    "dependentSchemas",
    "dependencies",
    "unevaluatedProperties",
    "propertyNames",
    "contains",
    "prefixItems",
    "not",
    "if",
    "then",
    "else",
];

/// Why a schema could not be rewritten into OpenAI's strict form.
///
/// The reason is one of pi's own error messages (see
/// `constrained-sampling.ts`'s `UnsupportedStrictJsonSchemaError` call
/// sites), so a message pinned in a test there stays pinned here.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{reason}")]
pub struct NotStrict {
    reason: String,
}

impl NotStrict {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    /// The reason the schema was rejected.
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// `schemaAllowsNull`: whether `null` is already a valid instance of
/// `schema`, by `type`, `const`, `enum`, or (recursively) an `anyOf`
/// branch. A property that already allows null is left as pi leaves it:
/// required, but not wrapped in another `anyOf`.
fn schema_allows_null(schema: &Value) -> bool {
    let Some(obj) = schema.as_object() else {
        return false;
    };
    match obj.get("type") {
        Some(Value::String(s)) if s == "null" => return true,
        Some(Value::Array(types))
            if types.iter().any(|t| t.as_str() == Some("null")) =>
        {
            return true;
        }
        _ => {}
    }
    if matches!(obj.get("const"), Some(Value::Null)) {
        return true;
    }
    if let Some(Value::Array(values)) = obj.get("enum")
        && values.iter().any(Value::is_null)
    {
        return true;
    }
    if let Some(Value::Array(variants)) = obj.get("anyOf") {
        return variants.iter().any(schema_allows_null);
    }
    false
}

/// `makeJsonSchemaNodeStrict`, as a value-to-value rewrite instead of an
/// in-place mutation: returns the strict form of one schema node, or the
/// first [`NotStrict`] reason encountered, recursing depth-first exactly
/// as pi does (unsupported keys, then `anyOf`, then `items`, then the
/// object-only checks).
fn make_strict_node(schema: &Value) -> Result<Value, NotStrict> {
    let Some(obj) = schema.as_object() else {
        return Err(NotStrict::new("boolean schemas are unsupported"));
    };
    let mut obj = obj.clone();

    for key in UNSUPPORTED_KEYS {
        if obj.contains_key(*key) {
            return Err(NotStrict::new(format!(
                "{key} schemas are unsupported"
            )));
        }
    }

    if let Some(any_of) = obj.get("anyOf").cloned() {
        let variants =
            any_of.as_array().filter(|v| !v.is_empty()).ok_or_else(|| {
                NotStrict::new("anyOf must contain at least one schema")
            })?;
        let mut strict_variants = Vec::with_capacity(variants.len());
        for variant in variants {
            strict_variants.push(make_strict_node(variant)?);
        }
        obj.insert("anyOf".to_owned(), Value::Array(strict_variants));
    }

    if let Some(items) = obj.get("items").cloned() {
        if items.is_array() {
            return Err(NotStrict::new("tuple schemas are unsupported"));
        }
        obj.insert("items".to_owned(), make_strict_node(&items)?);
    }

    let is_object_schema =
        matches!(obj.get("type"), Some(Value::String(s)) if s == "object");

    if obj.contains_key("properties") && !is_object_schema {
        return Err(NotStrict::new("properties require type object"));
    }
    if !is_object_schema {
        return Ok(Value::Object(obj));
    }

    match obj.get("additionalProperties") {
        None | Some(Value::Bool(false)) => {}
        Some(_) => {
            return Err(NotStrict::new(
                "schema-valued or true additionalProperties is unsupported",
            ));
        }
    }
    if let Some(properties) = obj.get("properties")
        && !properties.is_object()
    {
        return Err(NotStrict::new("object properties must be a schema map"));
    }
    if let Some(required) = obj.get("required") {
        let valid = required
            .as_array()
            .is_some_and(|values| values.iter().all(Value::is_string));
        if !valid {
            return Err(NotStrict::new(
                "object required must be a string array",
            ));
        }
    }

    let properties = obj
        .get("properties")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    // Checked above: `properties`, if present, is a schema map.
    let properties = properties
        .as_object()
        .expect("properties is an object")
        .clone();
    let property_names: Vec<String> = properties.keys().cloned().collect();
    let required: std::collections::HashSet<String> = obj
        .get("required")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    if required.iter().any(|key| !property_names.contains(key)) {
        return Err(NotStrict::new("required contains an unknown property"));
    }

    // Unlike pi, an object with no properties gets an empty map: OpenAI
    // rejects an object schema without `properties`, which is what a
    // tool with no arguments (an empty struct) generates.
    {
        let mut strict_properties = Map::new();
        for (key, value) in properties {
            let strict_value = make_strict_node(&value)?;
            let final_value = if !required.contains(&key)
                && !schema_allows_null(&strict_value)
            {
                let mut nullable = Map::new();
                nullable.insert(
                    "anyOf".to_owned(),
                    Value::Array(vec![
                        strict_value,
                        serde_json::json!({"type": "null"}),
                    ]),
                );
                Value::Object(nullable)
            } else {
                strict_value
            };
            strict_properties.insert(key, final_value);
        }
        obj.insert("properties".to_owned(), Value::Object(strict_properties));
    }

    obj.insert(
        "required".to_owned(),
        Value::Array(property_names.into_iter().map(Value::String).collect()),
    );
    obj.insert("additionalProperties".to_owned(), Value::Bool(false));

    Ok(Value::Object(obj))
}

/// Rewrites a JSON schema into OpenAI's strict form, or explains why it
/// cannot be.
///
/// Ported from pi's `makeStrictJsonSchema`: every object in the result
/// has `additionalProperties: false` and lists every one of its
/// properties in `required`; a property that was optional becomes
/// nullable (`anyOf: [original, {"type": "null"}]`) unless it already
/// allowed `null`. The root schema must itself have `type: "object"`.
pub fn to_strict(schema: &Value) -> Result<Value, NotStrict> {
    if !schema.is_object() {
        return Err(NotStrict::new("root schema must have type object"));
    }
    let strict = make_strict_node(&inline_refs(schema)?)?;
    if !matches!(strict.get("type"), Some(Value::String(s)) if s == "object") {
        return Err(NotStrict::new("root schema must have type object"));
    }
    Ok(strict)
}

/// Reverses [`to_strict`]'s nullable-optional rewrite on a *value*.
///
/// A model answering a strict schema sends `null` for a property that
/// was optional in the original schema, since strict mode has no way to
/// omit it. This walks `value` alongside `original_schema` (not the
/// strict one) and drops exactly the properties that are `null`, not in
/// `original_schema`'s `required`, and whose *original* schema does not
/// itself accept `null` — so a field that was genuinely nullable in the
/// original schema keeps its `null`, and only the null strict mode
/// invented is removed. The result validates against `original_schema`.
///
/// Ported from pi's `normalizeOptionalNulls`
/// (`packages/ai/src/utils/validation.ts:240`), which tau-agent's
/// argument-coercion pass (`validation.rs`) also runs before validating
/// tool-call arguments.
pub fn strip_nulls_for_optional(
    original_schema: &Value,
    value: &Value,
) -> Value {
    let Some(schema_obj) = original_schema.as_object() else {
        return value.clone();
    };

    if let Value::Array(items) = value {
        let stripped = match schema_obj.get("items") {
            Some(Value::Array(item_schemas)) => items
                .iter()
                .enumerate()
                .map(|(i, item)| match item_schemas.get(i) {
                    Some(item_schema) => {
                        strip_nulls_for_optional(item_schema, item)
                    }
                    None => item.clone(),
                })
                .collect(),
            Some(item_schema) => items
                .iter()
                .map(|item| strip_nulls_for_optional(item_schema, item))
                .collect(),
            None => items.clone(),
        };
        return Value::Array(stripped);
    }

    let Value::Object(value_obj) = value else {
        return value.clone();
    };
    let Some(Value::Object(properties)) = schema_obj.get("properties") else {
        return value.clone();
    };

    let required: std::collections::HashSet<&str> = schema_obj
        .get("required")
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    let mut result = Map::new();
    for (key, entry) in value_obj {
        let Some(property_schema) = properties.get(key) else {
            result.insert(key.clone(), entry.clone());
            continue;
        };
        let has_ref = property_schema.get("$ref").is_some();
        let drop = entry.is_null()
            && !required.contains(key.as_str())
            && !has_ref
            && !jsonschema::is_valid(property_schema, &Value::Null);
        if drop {
            continue;
        }
        result.insert(
            key.clone(),
            strip_nulls_for_optional(property_schema, entry),
        );
    }
    Value::Object(result)
}

/// Keywords whose values are data, not schemas: references inside them
/// are left alone.
const DATA_KEYS: &[&str] = &["const", "enum", "default", "examples"];

/// Replaces every local `$ref` (`#/$defs/<name>` or
/// `#/definitions/<name>`) with the definition it names, and drops the
/// root's definitions. Keywords next to a `$ref` (a `description`, say)
/// are kept, and win over the definition's own.
///
/// A schema with no definitions and no references comes back unchanged.
/// A reference that is not local, names no definition, or is recursive
/// fails with `NotStrict`.
pub fn inline_refs(schema: &Value) -> Result<Value, NotStrict> {
    let mut root = schema.clone();
    let mut defs = Map::new();
    if let Value::Object(obj) = &mut root {
        for key in ["definitions", "$defs"] {
            match obj.remove(key) {
                Some(Value::Object(found)) => defs.extend(found),
                Some(other) => {
                    obj.insert(key.to_owned(), other);
                }
                None => {}
            }
        }
    }
    inline_node(&root, &defs, &mut Vec::new())
}

fn inline_node(
    node: &Value,
    defs: &Map<String, Value>,
    stack: &mut Vec<String>,
) -> Result<Value, NotStrict> {
    match node {
        Value::Object(obj) => {
            let mut out = match obj.get("$ref") {
                Some(reference) => {
                    let name = reference
                        .as_str()
                        .and_then(|r| {
                            r.strip_prefix("#/$defs/")
                                .or_else(|| r.strip_prefix("#/definitions/"))
                        })
                        .filter(|name| defs.contains_key(*name))
                        .ok_or_else(|| {
                            NotStrict::new("$ref schemas are unsupported")
                        })?;
                    if stack.iter().any(|seen| seen == name) {
                        return Err(NotStrict::new(
                            "recursive $ref schemas are unsupported",
                        ));
                    }
                    stack.push(name.to_owned());
                    let resolved = inline_node(&defs[name], defs, stack)?;
                    stack.pop();
                    let Value::Object(resolved) = resolved else {
                        return Err(NotStrict::new(
                            "boolean schemas are unsupported",
                        ));
                    };
                    resolved
                }
                None => Map::new(),
            };
            for (key, value) in obj {
                if key == "$ref" {
                    continue;
                }
                let value = match (key.as_str(), value) {
                    // Property names are names, not keywords.
                    ("properties", Value::Object(properties)) => Value::Object(
                        properties
                            .iter()
                            .map(|(name, schema)| {
                                Ok((
                                    name.clone(),
                                    inline_node(schema, defs, stack)?,
                                ))
                            })
                            .collect::<Result<_, NotStrict>>()?,
                    ),
                    (key, value) if DATA_KEYS.contains(&key) => value.clone(),
                    (_, value) => inline_node(value, defs, stack)?,
                };
                out.insert(key.clone(), value);
            }
            Ok(Value::Object(out))
        }
        Value::Array(items) => items
            .iter()
            .map(|item| inline_node(item, defs, stack))
            .collect(),
        other => Ok(other.clone()),
    }
}
