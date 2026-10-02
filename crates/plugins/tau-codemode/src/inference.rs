//! A bounded, independently validated request for a future nested `infer` tool.

use serde_json::{Value, json};
use tau_agent::schema::{inline_refs, strip_nulls_for_optional, to_strict};

mod format;

const MAX_TASK_BYTES: usize = 16 * 1024;
const MAX_CONTEXT_BYTES: usize = 256 * 1024;
const MAX_SCHEMA_BYTES: usize = 64 * 1024;
const MAX_ANSWER_BYTES: usize = 64 * 1024;
const MAX_SCHEMA_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub struct InferRequest {
    pub task: String,
    pub context: Value,
    pub schema: Option<Value>,
}

impl InferRequest {
    /// Parse and validate all supplied arguments before any model call is made.
    pub fn parse(value: Value) -> Result<Self, String> {
        let Value::Object(mut fields) = value else {
            return Err("infer arguments must be an object".into());
        };
        if let Some(key) = fields
            .keys()
            .find(|key| !matches!(key.as_str(), "task" | "context" | "schema"))
        {
            return Err(format!("unknown infer argument: {key}"));
        }
        let task = fields
            .remove("task")
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or("task must be a string")?;
        if task.trim().is_empty() || task.len() > MAX_TASK_BYTES {
            return Err(format!(
                "task must be nonwhitespace and at most {MAX_TASK_BYTES} bytes"
            ));
        }
        let context = fields.remove("context").ok_or("context is required")?;
        check_json_size(&context, MAX_CONTEXT_BYTES, "context")?;
        let schema = fields.remove("schema").filter(|schema| !schema.is_null());
        if let Some(schema) = &schema {
            check_json_size(schema, MAX_SCHEMA_BYTES, "schema")?;
            check_schema_resources(schema, 0)?;
            jsonschema::validator_for(schema)
                .map_err(|error| format!("invalid schema: {error}"))?;
        }
        Ok(Self {
            task,
            context,
            schema,
        })
    }

    /// Parameter schema for the nested tool. Runtime limits are enforced by `parse`.
    pub fn parameters() -> Value {
        json!({
            "type": "object",
            "properties": {
                "task": {"type": "string", "description": "The independent task for inference."},
                "context": {"description": "Explicit JSON context for this task; null is allowed."},
                "schema": {"description": "Optional JSON Schema for the answer, or null."}
            },
            "required": ["task", "context"],
            "additionalProperties": false
        })
    }

    /// Build a self-contained prompt from only this request's data.
    pub fn input_text(&self) -> String {
        let context =
            serde_json::to_string(&self.context).expect("Value serializes");
        let mut input =
            format!("Task:\n{}\n\nContext (JSON):\n{context}", self.task);
        if let Some(schema) = &self.schema
            && self.text_format().is_none()
        {
            let schema =
                serde_json::to_string(schema).expect("Value serializes");
            input.push_str(&format!(
                "\n\nReturn only JSON that satisfies this schema:\n{schema}"
            ));
        }
        input
    }

    /// The provider's strict output format, when the schema has a supported rewrite.
    pub fn text_format(&self) -> Option<Value> {
        let schema = self.schema.as_ref()?;
        format::within_inline_budget(schema).then_some(())?;
        let inlined = inline_refs(schema).ok()?;
        let strict = to_strict(&inlined).ok()?;
        (serde_json::to_vec(&strict).ok()?.len()
            <= format::MAX_STRICT_SCHEMA_BYTES)
            .then_some(())?;
        Some(json!({
            "type": "json_schema",
            "name": "infer_answer",
            "schema": strict,
            "strict": true
        }))
    }

    /// Decode an answer without coercion, then check the original schema.
    pub fn decode_answer(&self, answer: &str) -> Result<Value, String> {
        if answer.len() > MAX_ANSWER_BYTES {
            return Err(format!("answer exceeds {MAX_ANSWER_BYTES} bytes"));
        }
        let Some(schema) = &self.schema else {
            return Ok(Value::String(answer.to_owned()));
        };
        let answer: Value = crate::json::decode(answer.as_bytes())
            .map_err(|error| format!("answer is not valid JSON: {error}"))?;
        let answer = if self.text_format().is_some() {
            let inlined =
                inline_refs(schema).expect("strict schema was inlined");
            strip_nulls_for_optional(&inlined, &answer)
        } else {
            answer
        };
        let validator = jsonschema::validator_for(schema)
            .map_err(|error| format!("invalid schema: {error}"))?;
        validator.validate(&answer).map_err(|error| {
            format!("answer does not match schema: {error}")
        })?;
        Ok(answer)
    }
}

fn check_json_size(
    value: &Value,
    max_bytes: usize,
    name: &str,
) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    if bytes.len() > max_bytes {
        return Err(format!("{name} exceeds {max_bytes} serialized bytes"));
    }
    Ok(())
}

/// Prevent the validator from resolving a file or network resource, while
/// checking the depth of every JSON value, including annotation data.
fn check_schema_resources(value: &Value, depth: usize) -> Result<(), String> {
    check_schema_value(value, depth, SchemaLocation::Schema)
}

#[derive(Clone, Copy)]
enum SchemaLocation {
    Schema,
    NamedSchemas,
    SchemaList,
    Data,
}

fn check_schema_value(
    value: &Value,
    depth: usize,
    location: SchemaLocation,
) -> Result<(), String> {
    if depth > MAX_SCHEMA_DEPTH {
        return Err(format!("schema exceeds {MAX_SCHEMA_DEPTH} levels"));
    }
    match value {
        Value::Object(fields) => {
            for (key, child) in fields {
                if matches!(location, SchemaLocation::Schema) {
                    match key.as_str() {
                        "$ref" => {
                            if !child.as_str().is_some_and(|reference| {
                                reference.starts_with('#')
                            }) {
                                return Err(
                                    "schema permits only local # references"
                                        .into(),
                                );
                            }
                        }
                        "$id" | "$anchor" | "$dynamicAnchor"
                        | "$dynamicRef" | "$recursiveRef"
                        | "$recursiveAnchor" => {
                            return Err(format!(
                                "schema resource keyword {key} is unsupported"
                            ));
                        }
                        _ => {}
                    }
                }
                let child_location = match location {
                    SchemaLocation::Schema => match key.as_str() {
                        "properties" | "patternProperties" | "$defs"
                        | "definitions" | "dependentSchemas"
                        | "dependencies" => SchemaLocation::NamedSchemas,
                        "allOf" | "anyOf" | "oneOf" | "prefixItems" => {
                            SchemaLocation::SchemaList
                        }
                        "additionalProperties"
                        | "unevaluatedProperties"
                        | "propertyNames"
                        | "items"
                        | "contains"
                        | "not"
                        | "if"
                        | "then"
                        | "else"
                        | "additionalItems"
                        | "unevaluatedItems"
                        | "contentSchema" => SchemaLocation::Schema,
                        _ => SchemaLocation::Data,
                    },
                    SchemaLocation::NamedSchemas => SchemaLocation::Schema,
                    SchemaLocation::SchemaList | SchemaLocation::Data => {
                        SchemaLocation::Data
                    }
                };
                check_schema_value(child, depth + 1, child_location)?;
            }
        }
        Value::Array(items) => {
            for item in items {
                let child_location = match location {
                    SchemaLocation::SchemaList | SchemaLocation::Schema => {
                        SchemaLocation::Schema
                    }
                    SchemaLocation::NamedSchemas | SchemaLocation::Data => {
                        SchemaLocation::Data
                    }
                };
                check_schema_value(item, depth + 1, child_location)?;
            }
        }
        _ => {}
    }
    Ok(())
}
