//! Tool-argument coercion and validation.
//!
//! Ports pi's `validateToolArguments`
//! (`packages/ai/src/utils/validation.ts:317`), which the loop runs
//! before `before_tool` hooks
//! (`docs/reference/agent-loop.md`, "Coercion before validation" and
//! "Tool execution").
//!
//! [`ArgumentSchema::validate`] does what pi does, in order:
//!
//! 1. Clone the arguments.
//! 2. [`normalize_optional_nulls`]: a `null` on a property that is not
//!    required, has no `$ref`, and whose own schema rejects `null`, is
//!    removed rather than kept (pi's `normalizeOptionalNulls`). A `$ref`
//!    property is never touched here — pi cannot cheaply tell whether
//!    the referenced schema accepts `null` without resolving it against
//!    the whole document, so (like pi) we conservatively keep the
//!    `null`. When the sub-schema fails to compile at all, we also keep
//!    it, for the same reason.
//! 3. [`coerce_with_json_schema`]: pi's own `coerceWithJsonSchema`, plus
//!    one rule from typebox's `Value.Convert` (`FromArray`) that pi
//!    always runs first (`Value.Convert(tool.parameters, args)`):
//!    a value that is not an array becomes a one-element array where the
//!    schema says `array`, with the element itself then coerced against
//!    `items` (`docs/reference/agent-loop.md`, "Coercion before
//!    validation", rule 3). Real typebox schemas built with `Type.*`
//!    carry a `~kind`/`Symbol.for('TypeBox.Kind')` tag that routes them
//!    through `Value.Convert`'s real per-kind logic; every tau-agent
//!    schema is a plain `serde_json::Value` (schemars or hand-written),
//!    which is the "no such tag" case pi's own code guards
//!    (`!Object.getOwnPropertySymbols(tool.parameters).includes(TYPEBOX_KIND)`).
//!    For that case `Value.Convert` only contributes the array rule in
//!    practice (every other typebox kind check requires the tag), so
//!    that is the only piece of it this module ports; everything else
//!    below is `coerceWithJsonSchema` translated directly:
//!    - a string that parses as a number becomes a number where the
//!      schema says `number`; the same holds for `integer`, but only
//!      when the parsed value has no fractional part (`"42"` converts,
//!      `"42.1"` does not: it is left as a string, which then fails
//!      validation, matching pi's own known-failing case);
//!    - `null` becomes `0` where the schema says `number`/`integer`,
//!      `false` where it says `boolean`, and `""` where it says
//!      `string`;
//!    - `"true"`/`"false"` become booleans where the schema says
//!      `boolean` (any other string, including `"1"`/`"0"`, is left
//!      alone and fails validation); `1`/`0` do too, but no other
//!      number;
//!    - a number or a boolean becomes a string where the schema says
//!      `string`;
//!    - `""`, `0` or `false` become `null` where the schema says `null`
//!      (`"null"` the string is left alone and fails validation);
//!    - when the schema's `type` lists more than one type and the value
//!      already matches one of them, none of the above runs for that
//!      value (a value that is valid under a union member is left as
//!      is, even if another member could also accept a coerced form);
//!    - `allOf` coerces through every branch in sequence; `anyOf`/`oneOf`
//!      first check whether the value already validates under any
//!      member (if so, it is returned unchanged, even if a different
//!      member would also accept a coerced form — this is how a
//!      nullable union keeps an explicit `null`), and otherwise try
//!      coercing against each member in turn, keeping the first
//!      coerced form that then validates against that member; if none
//!      does, the value is returned unchanged;
//!    - objects and arrays recurse into their properties/items (and, for
//!      objects, into extra keys when `additionalProperties` is itself a
//!      schema). Tuples are read in both spellings, 2020-12's
//!      `prefixItems` + `items` and draft 7's `items` array +
//!      `additionalItems`; pi reads only the positional `items` array,
//!      so `schemars` tuples were not coerced there.
//! 4. Validate the result against the compiled schema. On failure, the
//!    error text lists each violation's field path and message, in the
//!    same shape pi's does (`formatValidationPath` plus
//!    `validator.Errors`), followed by the original (uncoerced)
//!    arguments as pretty JSON — this is what the model sees as the
//!    tool's error result (`docs/reference/agent-loop.md`, "Tool
//!    execution": "a tool that returns `Err` produces a result with
//!    `is_error = true`. Its text is the error message.").
//!
//! **Deviation from pi:** pi's message opens with
//! `Validation failed for tool "name":`. [`ArgumentSchema`] is scoped to
//! one schema, not a tool, so it does not know the tool's name; the
//! message opens with `Validation failed:` instead. A caller that wants
//! the tool name back (as `runner.rs` does, via `error.to_string()` as
//! the tool result text) can prepend it itself. The per-error message
//! text also differs from pi's: pi validates with ajv/typebox, whose
//! error strings this crate cannot reproduce without depending on them;
//! `jsonschema`'s own `Display` is used instead. The envelope
//! (`Validation failed:\n  - path: message\n\nReceived arguments:\n...`)
//! and the field-path format match pi exactly.

use std::{collections::HashSet, fmt};

use jsonschema::{Validator, error::ValidationErrorKind};
use serde_json::{Map, Value};

use crate::schema::inline_refs;

/// A compiled tool-argument schema: the raw JSON Schema alongside a
/// [`jsonschema::Validator`] compiled from it.
pub struct ArgumentSchema {
    schema: Value,
    validator: Validator,
}

impl fmt::Debug for ArgumentSchema {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ArgumentSchema")
            .field("schema", &self.schema)
            .finish_non_exhaustive()
    }
}

/// A schema that failed to compile.
#[derive(Debug, Clone, thiserror::Error)]
#[error("invalid tool argument schema: {0}")]
pub struct SchemaError(String);

/// Tool arguments that still fail to validate after coercion.
///
/// `Display` (and so `to_string()`) is the exact text the model sees:
/// tau-agent's tool executor uses it verbatim as the tool call's error
/// result (`docs/reference/agent-loop.md`, "Tool execution").
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ValidationError {
    message: String,
}

impl ArgumentSchema {
    /// Compiles `schema`. Fails only if `schema` is not a valid JSON
    /// Schema document.
    pub fn new(schema: &Value) -> Result<Self, SchemaError> {
        let validator = jsonschema::validator_for(schema)
            .map_err(|error| SchemaError(error.to_string()))?;
        // The coercion walks follow the schema's shape, so they get it
        // with references inlined; a schema that cannot be inlined is
        // walked as it is, and its references are not followed.
        Ok(Self {
            schema: inline_refs(schema).unwrap_or_else(|_| schema.clone()),
            validator,
        })
    }

    /// Runs the coercion pass alone: normalizes optional nulls, then
    /// applies pi's lenient conversion. Does not validate the result.
    pub fn coerce(&self, args: &Value) -> Value {
        let mut value = args.clone();
        normalize_optional_nulls(&mut value, &self.schema);
        coerce_with_json_schema(value, &self.schema)
    }

    /// pi's `validateToolArguments`: coerces, then validates. Returns
    /// the coerced arguments on success.
    pub fn validate(&self, args: &Value) -> Result<Value, ValidationError> {
        let coerced = self.coerce(args);
        if self.validator.is_valid(&coerced) {
            Ok(coerced)
        } else {
            Err(self.build_error(args, &coerced))
        }
    }

    fn build_error(
        &self,
        original: &Value,
        coerced: &Value,
    ) -> ValidationError {
        let lines: Vec<String> = self
            .validator
            .iter_errors(coerced)
            .map(|error| format!("  - {}: {error}", format_error_path(&error)))
            .collect();
        let errors_text = if lines.is_empty() {
            "Unknown validation error".to_owned()
        } else {
            lines.join("\n")
        };
        let received = serde_json::to_string_pretty(original)
            .unwrap_or_else(|_| original.to_string());
        ValidationError {
            message: format!(
                "Validation failed:\n{errors_text}\n\nReceived arguments:\n{received}"
            ),
        }
    }
}

/// pi's `formatValidationPath`: the instance path with the leading `/`
/// dropped and the rest turned from JSON-pointer segments into dotted
/// ones, or `root` for the empty path. A missing-required-property error
/// names the property after its parent's path, since the instance path
/// itself points at the object, not at the (absent) property.
fn format_error_path(error: &jsonschema::ValidationError<'_>) -> String {
    let instance_path = error.instance_path().as_str();
    let base_path = instance_path.trim_start_matches('/').replace('/', ".");
    if let ValidationErrorKind::Required { property } = error.kind()
        && let Some(name) = property.as_str()
    {
        return if base_path.is_empty() {
            name.to_owned()
        } else {
            format!("{base_path}.{name}")
        };
    }
    if base_path.is_empty() {
        "root".to_owned()
    } else {
        base_path
    }
}

/// Whether `schema` compiles and rejects `Value::Null`. `false` both
/// when it accepts `null` and when it fails to compile at all — pi
/// treats an uncheckable sub-schema (`getSubSchemaValidator` returning
/// `undefined`) the same as one that accepts `null`, i.e. it never
/// deletes the property.
fn schema_rejects_null(schema: &Value) -> bool {
    jsonschema::validator_for(schema)
        .map(|validator| !validator.is_valid(&Value::Null))
        .unwrap_or(false)
}

/// Whether `schema` compiles and accepts `value`. `false` when it fails
/// to compile, matching pi's `validator?.Check(value)`.
fn validator_accepts(schema: &Value, value: &Value) -> bool {
    jsonschema::validator_for(schema)
        .map(|validator| validator.is_valid(value))
        .unwrap_or(false)
}

/// The schema for each element of an array, by position. Tuples are
/// read in both spellings: JSON Schema 2020-12's `prefixItems` followed
/// by `items` for the rest (which `schemars` emits for Rust tuples), and
/// the older positional `items` array followed by `additionalItems`.
/// pi reads only the older spelling, and never `additionalItems`.
fn item_schema(schema: &Value, index: usize) -> Option<&Value> {
    let (positional, rest) =
        match (schema.get("prefixItems"), schema.get("items")) {
            (Some(Value::Array(prefix)), items) => {
                (prefix.as_slice(), object_schema(items))
            }
            (_, Some(Value::Array(items))) => (
                items.as_slice(),
                object_schema(schema.get("additionalItems")),
            ),
            (_, items) => (&[][..], object_schema(items)),
        };
    match positional.get(index) {
        Some(item) => object_schema(Some(item)),
        None => rest,
    }
}

/// A schema this module can walk: an object, not a boolean schema.
fn object_schema(schema: Option<&Value>) -> Option<&Value> {
    schema.filter(|schema| schema.is_object())
}

/// pi's `normalizeOptionalNulls`.
fn normalize_optional_nulls(value: &mut Value, schema: &Value) {
    match value {
        Value::Array(items) => {
            for (index, item) in items.iter_mut().enumerate() {
                if let Some(item_schema) = item_schema(schema, index) {
                    normalize_optional_nulls(item, item_schema);
                }
            }
        }
        Value::Object(map) => {
            let Some(properties) =
                schema.get("properties").and_then(Value::as_object)
            else {
                return;
            };
            let required: HashSet<&str> = schema
                .get("required")
                .and_then(Value::as_array)
                .map(|values| values.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();

            let keys: Vec<String> = properties.keys().cloned().collect();
            for key in keys {
                if !map.contains_key(&key) {
                    continue;
                }
                let property_schema = &properties[&key];
                let is_null = matches!(map.get(&key), Some(Value::Null));
                let has_ref = property_schema
                    .get("$ref")
                    .and_then(Value::as_str)
                    .is_some();
                let should_delete = is_null
                    && !required.contains(key.as_str())
                    && !has_ref
                    && schema_rejects_null(property_schema);
                if should_delete {
                    map.remove(&key);
                } else if let Some(item) = map.get_mut(&key) {
                    normalize_optional_nulls(item, property_schema);
                }
            }
        }
        _ => {}
    }
}

/// The schema's `type` keyword, normalized to a list: a single string
/// becomes a one-element list, a list of strings is filtered to just
/// the string entries, and anything else (including no `type` at all)
/// is empty.
fn schema_types(schema: &Value) -> Vec<&str> {
    match schema.get("type") {
        Some(Value::String(single)) => vec![single.as_str()],
        Some(Value::Array(many)) => {
            many.iter().filter_map(Value::as_str).collect()
        }
        _ => Vec::new(),
    }
}

fn is_integer_number(number: &serde_json::Number) -> bool {
    if number.is_i64() || number.is_u64() {
        return true;
    }
    number
        .as_f64()
        .is_some_and(|f| f.is_finite() && f.fract() == 0.0)
}

/// pi's `matchesJsonType`.
fn matches_json_type(value: &Value, ty: &str) -> bool {
    match ty {
        "number" => value.is_number(),
        "integer" => matches!(value, Value::Number(n) if is_integer_number(n)),
        "boolean" => value.is_boolean(),
        "string" => value.is_string(),
        "null" => value.is_null(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => false,
    }
}

/// A JSON number holding a whole value close enough to fit an `i64`
/// canonicalizes to one, so `"42"` and `42` coerce to the identical
/// `Value` representation.
fn whole_number_value(n: f64) -> Value {
    if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 {
        Value::from(n as i64)
    } else {
        Value::from(n)
    }
}

fn coerce_number(value: Value, integer_only: bool) -> Value {
    match &value {
        Value::Null => Value::from(0),
        Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return value;
            }
            match trimmed.parse::<f64>() {
                Ok(n)
                    if n.is_finite() && (!integer_only || n.fract() == 0.0) =>
                {
                    whole_number_value(n)
                }
                _ => value,
            }
        }
        Value::Bool(b) => Value::from(i64::from(*b)),
        _ => value,
    }
}

fn coerce_boolean(value: Value) -> Value {
    if value.is_null() {
        return Value::Bool(false);
    }
    if let Value::String(s) = &value {
        if s == "true" {
            return Value::Bool(true);
        }
        if s == "false" {
            return Value::Bool(false);
        }
    }
    if let Value::Number(n) = &value
        && let Some(f) = n.as_f64()
    {
        if f == 1.0 {
            return Value::Bool(true);
        }
        if f == 0.0 {
            return Value::Bool(false);
        }
    }
    value
}

fn coerce_string(value: Value) -> Value {
    match &value {
        Value::Null => Value::String(String::new()),
        Value::Number(n) => Value::String(n.to_string()),
        Value::Bool(b) => Value::String(b.to_string()),
        _ => value,
    }
}

fn coerce_to_null(value: Value) -> Value {
    let becomes_null = matches!(&value, Value::String(s) if s.is_empty())
        || matches!(&value, Value::Number(n) if n.as_f64() == Some(0.0))
        || matches!(&value, Value::Bool(false));
    if becomes_null { Value::Null } else { value }
}

/// typebox's `Value.Convert` `FromArray`, the one piece of it pi's
/// custom coercion does not otherwise cover for a plain JSON schema
/// (see the module doc comment): a value that is not already an array
/// becomes a one-element array holding it.
fn coerce_to_array(value: Value) -> Value {
    match value {
        Value::Array(_) => value,
        other => Value::Array(vec![other]),
    }
}

fn coerce_primitive_by_type(value: Value, ty: &str) -> Value {
    match ty {
        "number" => coerce_number(value, false),
        "integer" => coerce_number(value, true),
        "boolean" => coerce_boolean(value),
        "string" => coerce_string(value),
        "null" => coerce_to_null(value),
        "array" => coerce_to_array(value),
        _ => value,
    }
}

/// pi's `applySchemaObjectCoercion`.
fn apply_schema_object_coercion(map: &mut Map<String, Value>, schema: &Value) {
    let properties = schema.get("properties").and_then(Value::as_object);
    if let Some(properties) = properties {
        for (key, property_schema) in properties {
            if let Some(existing) = map.get(key).cloned() {
                let coerced =
                    coerce_with_json_schema(existing, property_schema);
                if let Some(slot) = map.get_mut(key) {
                    *slot = coerced;
                }
            }
        }
    }

    if let Some(additional_schema) = schema.get("additionalProperties")
        && additional_schema.is_object()
    {
        let defined: HashSet<&str> = properties
            .map(|p| p.keys().map(String::as_str).collect())
            .unwrap_or_default();
        let extra_keys: Vec<String> = map
            .keys()
            .filter(|key| !defined.contains(key.as_str()))
            .cloned()
            .collect();
        for key in extra_keys {
            if let Some(existing) = map.get(&key).cloned() {
                let coerced =
                    coerce_with_json_schema(existing, additional_schema);
                if let Some(slot) = map.get_mut(&key) {
                    *slot = coerced;
                }
            }
        }
    }
}

/// pi's `applySchemaArrayCoercion`, with tuples read as
/// [`item_schema`] reads them.
fn apply_schema_array_coercion(items: &mut [Value], schema: &Value) {
    for (index, slot) in items.iter_mut().enumerate() {
        if let Some(item_schema) = item_schema(schema, index) {
            let existing = std::mem::replace(slot, Value::Null);
            *slot = coerce_with_json_schema(existing, item_schema);
        }
    }
}

/// pi's `coerceWithUnionSchema`, for `anyOf`/`oneOf`.
fn coerce_with_union_schema(value: Value, schemas: &[Value]) -> Value {
    for schema in schemas {
        if validator_accepts(schema, &value) {
            return value;
        }
    }
    for schema in schemas {
        let candidate = coerce_with_json_schema(value.clone(), schema);
        if validator_accepts(schema, &candidate) {
            return candidate;
        }
    }
    value
}

/// pi's `coerceWithJsonSchema`, plus the `Value.Convert` array rule (see
/// the module doc comment).
fn coerce_with_json_schema(value: Value, schema: &Value) -> Value {
    let mut next_value = value;

    if let Some(all_of) = schema.get("allOf").and_then(Value::as_array) {
        for nested in all_of {
            next_value = coerce_with_json_schema(next_value, nested);
        }
    }
    if let Some(any_of) = schema.get("anyOf").and_then(Value::as_array) {
        next_value = coerce_with_union_schema(next_value, any_of);
    }
    if let Some(one_of) = schema.get("oneOf").and_then(Value::as_array) {
        next_value = coerce_with_union_schema(next_value, one_of);
    }

    let types = schema_types(schema);
    let matches_union_member = types.len() > 1
        && types.iter().any(|ty| matches_json_type(&next_value, ty));
    if !types.is_empty() && !matches_union_member {
        for ty in &types {
            let candidate = coerce_primitive_by_type(next_value.clone(), ty);
            if candidate != next_value {
                next_value = candidate;
                break;
            }
        }
    }

    if types.contains(&"object")
        && let Value::Object(map) = &mut next_value
    {
        apply_schema_object_coercion(map, schema);
    }
    if types.contains(&"array")
        && let Value::Array(items) = &mut next_value
    {
        apply_schema_array_coercion(items, schema);
    }

    next_value
}
