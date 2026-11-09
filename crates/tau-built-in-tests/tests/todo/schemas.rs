//! Every `todo_*` tool's schema is one a provider takes (ADR 0048, "6.
//! Oracles"): made strict, each object is closed and fully required, and
//! drawn arguments the schema accepts, filled in with nulls, the strict
//! one accepts too. So no run is refused for a schema this plugin added.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use hegel::{TestCase, generators as gs};
use serde_json::{Map, Value, json};
use support::Plugin;
use tau_agent::schema::to_strict;

use super::support;

/// Panics if an object of `schema` is open or leaves a property out of
/// `required`: read back as strict mode asks, apart from `to_strict`.
fn closed_and_required(schema: &Value) {
    let Some(obj) = schema.as_object() else {
        return;
    };
    if obj.get("type") == Some(&json!("object")) {
        assert_eq!(
            obj.get("additionalProperties"),
            Some(&json!(false)),
            "open: {schema}"
        );
        let properties = obj
            .get("properties")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let mut names: Vec<&String> = properties.keys().collect();
        names.sort();
        let mut required: Vec<&str> = obj
            .get("required")
            .and_then(Value::as_array)
            .map(|values| values.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        required.sort_unstable();
        assert_eq!(names, required, "not every property is required: {schema}");
        for property in properties.values() {
            closed_and_required(property);
        }
    }
    if let Some(items) = obj.get("items") {
        closed_and_required(items);
    }
    for variant in obj
        .get("anyOf")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        closed_and_required(variant);
    }
}

/// A value `schema` accepts, drawn; each optional property there or not.
fn draw(tc: &TestCase, schema: &Value) -> Value {
    if let Some(choices) = schema.get("enum").and_then(Value::as_array) {
        return tc.draw(gs::sampled_from(choices.clone()));
    }
    match schema["type"].as_str() {
        Some("object") => {
            let required: Vec<&str> = schema["required"]
                .as_array()
                .map(|values| values.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let mut out = Map::new();
            for (name, property) in
                schema["properties"].as_object().into_iter().flatten()
            {
                if required.contains(&name.as_str()) || tc.draw(gs::booleans())
                {
                    out.insert(name.clone(), draw(tc, property));
                }
            }
            Value::Object(out)
        }
        Some("array") => {
            let n: usize = tc.draw(gs::integers().max_value(3));
            Value::Array((0..n).map(|_| draw(tc, &schema["items"])).collect())
        }
        Some("integer") => {
            json!(tc.draw(gs::integers::<i64>().min_value(-3).max_value(9)))
        }
        Some("boolean") => json!(tc.draw(gs::booleans())),
        Some("string") => json!(tc.draw(gs::text().max_size(12))),
        other => panic!("a schema of {other:?}: {schema}"),
    }
}

/// `value` with each optional property it leaves out given as null, as
/// a provider in strict mode sends it.
fn with_nulls(schema: &Value, value: &Value) -> Value {
    match (schema["type"].as_str(), value) {
        (Some("object"), Value::Object(fields)) => {
            let mut out = Map::new();
            for (name, property) in
                schema["properties"].as_object().into_iter().flatten()
            {
                out.insert(
                    name.clone(),
                    fields.get(name).map_or(Value::Null, |field| {
                        with_nulls(property, field)
                    }),
                );
            }
            Value::Object(out)
        }
        (Some("array"), Value::Array(items)) => Value::Array(
            items
                .iter()
                .map(|item| with_nulls(&schema["items"], item))
                .collect(),
        ),
        _ => value.clone(),
    }
}

#[test]
fn every_tool_is_named_for_the_plugin_and_has_a_strict_schema() {
    let tools = Plugin::new().tools();
    assert_eq!(tools.len(), 14, "beads' tool set and steps");
    for tool in tools {
        assert!(tool.name.starts_with("todo_"), "{}", tool.name);
        let strict = to_strict(&tool.parameters)
            .unwrap_or_else(|why| panic!("{}: {why:?}", tool.name));
        closed_and_required(&strict);
    }
}

#[hegel::test(test_cases = 300)]
fn what_a_schema_takes_its_strict_form_takes(tc: TestCase) {
    let tools = Plugin::new().tools();
    let at: usize = tc.draw(gs::integers().max_value(tools.len() - 1));
    let tool = &tools[at];
    let args = draw(&tc, &tool.parameters);
    assert!(
        jsonschema::is_valid(&tool.parameters, &args),
        "{}: {args}",
        tool.name
    );
    let strict = to_strict(&tool.parameters).unwrap();
    let sent = with_nulls(&tool.parameters, &args);
    assert!(
        jsonschema::is_valid(&strict, &sent),
        "{}: {sent}",
        tool.name
    );
}
