//! Work and size bounds before the shared schema rewriters allocate an expansion.

use serde_json::{Map, Value};

pub(super) const MAX_STRICT_SCHEMA_BYTES: usize = 256 * 1024;
const MAX_EXPANDED_NODES: usize = 4096;
const MAX_EXPANSION_DEPTH: usize = 64;

/// Count a conservative serialized upper bound without constructing the
/// expanded tree. Counting both a referenced definition and its overriding
/// siblings is deliberate: overwritten fields can only make the result
/// smaller. Every repeated reference consumes budget again.
pub(super) fn within_inline_budget(schema: &Value) -> bool {
    let definitions = schema.get("definitions").and_then(Value::as_object);
    let dollar_defs = schema.get("$defs").and_then(Value::as_object);
    let mut walker = ExpansionBudget {
        nodes: 0,
        bytes: 0,
        references: Vec::new(),
    };
    walker
        .walk(schema, 0, Location::Inline, true, definitions, dollar_defs)
        .is_some()
}

#[derive(Clone, Copy)]
enum Location {
    Inline,
    NamedMap,
    Data,
}

struct ExpansionBudget<'a> {
    nodes: usize,
    bytes: usize,
    references: Vec<&'a str>,
}

impl<'a> ExpansionBudget<'a> {
    fn charge(&mut self, bytes: usize) -> Option<()> {
        self.bytes = self.bytes.checked_add(bytes)?;
        (self.bytes <= MAX_STRICT_SCHEMA_BYTES).then_some(())
    }

    fn walk(
        &mut self,
        value: &'a Value,
        depth: usize,
        location: Location,
        root: bool,
        definitions: Option<&'a Map<String, Value>>,
        dollar_defs: Option<&'a Map<String, Value>>,
    ) -> Option<()> {
        self.nodes += 1;
        if self.nodes > MAX_EXPANDED_NODES || depth > MAX_EXPANSION_DEPTH {
            return None;
        }
        match value {
            Value::Object(fields) => {
                self.charge(2)?; // braces
                if matches!(location, Location::Inline)
                    && let Some(reference) = fields.get("$ref")
                {
                    let name = reference
                        .as_str()?
                        .strip_prefix("#/$defs/")
                        .or_else(|| {
                            reference.as_str()?.strip_prefix("#/definitions/")
                        })?;
                    let definition =
                        dollar_defs.and_then(|defs| defs.get(name)).or_else(
                            || definitions.and_then(|defs| defs.get(name)),
                        )?;
                    if self.references.contains(&name) {
                        return None;
                    }
                    self.references.push(name);
                    let result = self.walk(
                        definition,
                        depth + 1,
                        Location::Inline,
                        false,
                        definitions,
                        dollar_defs,
                    );
                    self.references.pop();
                    result?;
                }
                for (key, child) in fields {
                    // inline_refs removes root definition maps before walking.
                    if root
                        && matches!(key.as_str(), "definitions" | "$defs")
                        && child.is_object()
                    {
                        continue;
                    }
                    self.charge(serde_json::to_string(key).ok()?.len() + 2)?;
                    let child_location = match location {
                        Location::Inline if key == "properties" => {
                            Location::NamedMap
                        }
                        Location::Inline
                            if matches!(
                                key.as_str(),
                                "const" | "enum" | "default" | "examples"
                            ) =>
                        {
                            Location::Data
                        }
                        Location::Inline | Location::NamedMap => {
                            Location::Inline
                        }
                        Location::Data => Location::Data,
                    };
                    self.walk(
                        child,
                        depth + 1,
                        child_location,
                        false,
                        definitions,
                        dollar_defs,
                    )?;
                }
            }
            Value::Array(items) => {
                self.charge(2)?; // brackets
                for item in items {
                    self.charge(1)?; // separator, including one extra
                    self.walk(
                        item,
                        depth + 1,
                        location,
                        false,
                        definitions,
                        dollar_defs,
                    )?;
                }
            }
            primitive => {
                self.charge(serde_json::to_vec(primitive).ok()?.len())?
            }
        }
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::within_inline_budget;

    #[test]
    fn strict_size_fixture_is_within_preflight_budget() {
        let mut properties = serde_json::Map::new();
        for index in 0..500 {
            let name = format!("p{index:04}{}", "x".repeat(55));
            properties.insert(name, json!({"$ref": "#/$defs/leaf"}));
        }
        let schema = json!({
            "$defs": {"leaf": {"type": "string", "description": "x".repeat(350)}},
            "type": "object", "properties": properties,
            "additionalProperties": false
        });
        assert!(within_inline_budget(&schema));
    }
}
