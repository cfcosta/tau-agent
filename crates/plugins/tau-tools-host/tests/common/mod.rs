//! Helpers the tool tests share.

use serde_json::Value;
use tau_agent::{tool::AgentTool, validation::ArgumentSchema};

/// `structured` is a result `tool`'s declared output schema accepts as
/// it is: valid, and with nothing for the lenient coercion to change
/// (`AgentTool::output_schema`).
pub fn assert_matches_output_schema(tool: &dyn AgentTool, structured: &Value) {
    let schema = ArgumentSchema::new(
        tool.output_schema()
            .expect("the tool declares an output schema"),
    )
    .expect("the output schema compiles");
    let accepted = schema
        .validate(structured)
        .unwrap_or_else(|error| panic!("{error}: {structured}"));
    assert_eq!(&accepted, structured, "the schema had to coerce it");
}
