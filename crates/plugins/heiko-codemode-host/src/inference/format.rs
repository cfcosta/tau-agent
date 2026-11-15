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
    use hegel::{TestCase, generators as gs};
    use heiko_agent::schema::inline_refs;
    use serde_json::{Map, Value, json};

    use super::{
        MAX_EXPANDED_NODES,
        MAX_EXPANSION_DEPTH,
        MAX_STRICT_SCHEMA_BYTES,
        within_inline_budget,
    };

    /// Why a schema cannot be expanded within the limits.
    #[derive(Debug)]
    struct Unexpandable;

    /// A naive expansion: every `$ref` in a schema is replaced by its
    /// definition, kept beside the reference's own fields under the key
    /// `#`, so nothing an override drops is lost. Values under `const`,
    /// `enum`, `default` and `examples` are data, never expanded; the
    /// keys of `properties` are names, never schemas.
    struct Naive<'a> {
        defs: &'a Map<String, Value>,
        definitions: &'a Map<String, Value>,
        nodes: usize,
        refs: usize,
        stack: Vec<String>,
    }

    impl Naive<'_> {
        fn visit(&mut self, depth: usize) -> Result<(), Unexpandable> {
            self.nodes += 1;
            if self.nodes > MAX_EXPANDED_NODES || depth > MAX_EXPANSION_DEPTH {
                return Err(Unexpandable);
            }
            Ok(())
        }

        fn schema(
            &mut self,
            value: &Value,
            depth: usize,
            root: bool,
        ) -> Result<Value, Unexpandable> {
            self.visit(depth)?;
            match value {
                Value::Object(fields) => {
                    let mut out = Map::new();
                    if let Some(reference) = fields.get("$ref") {
                        let reference =
                            reference.as_str().ok_or(Unexpandable)?;
                        let definition = if let Some(name) =
                            reference.strip_prefix("#/$defs/")
                        {
                            self.defs.get(name)
                        } else if let Some(name) =
                            reference.strip_prefix("#/definitions/")
                        {
                            self.definitions.get(name)
                        } else {
                            None
                        }
                        .ok_or(Unexpandable)?;
                        if self.stack.iter().any(|seen| seen == reference) {
                            return Err(Unexpandable);
                        }
                        self.stack.push(reference.to_owned());
                        self.refs += 1;
                        let expanded =
                            self.schema(definition, depth + 1, false);
                        self.stack.pop();
                        out.insert("#".into(), expanded?);
                    }
                    for (key, child) in fields {
                        if root
                            && matches!(key.as_str(), "$defs" | "definitions")
                            && child.is_object()
                        {
                            continue;
                        }
                        let child = match key.as_str() {
                            "properties" => self.names(child, depth + 1)?,
                            "const" | "enum" | "default" | "examples" => {
                                self.data(child, depth + 1)?
                            }
                            _ => self.schema(child, depth + 1, false)?,
                        };
                        out.insert(key.clone(), child);
                    }
                    Ok(Value::Object(out))
                }
                Value::Array(items) => items
                    .iter()
                    .map(|item| self.schema(item, depth + 1, false))
                    .collect(),
                primitive => Ok(primitive.clone()),
            }
        }

        /// A `properties` map: each value a schema.
        fn names(
            &mut self,
            value: &Value,
            depth: usize,
        ) -> Result<Value, Unexpandable> {
            let Value::Object(fields) = value else {
                return self.schema(value, depth, false);
            };
            self.visit(depth)?;
            let mut out = Map::new();
            for (name, child) in fields {
                out.insert(name.clone(), self.schema(child, depth + 1, false)?);
            }
            Ok(Value::Object(out))
        }

        fn data(
            &mut self,
            value: &Value,
            depth: usize,
        ) -> Result<Value, Unexpandable> {
            self.visit(depth)?;
            match value {
                Value::Object(fields) => {
                    for child in fields.values() {
                        self.data(child, depth + 1)?;
                    }
                }
                Value::Array(items) => {
                    for item in items {
                        self.data(item, depth + 1)?;
                    }
                }
                _ => {}
            }
            Ok(value.clone())
        }
    }

    /// Non-empty objects and arrays in `value`.
    fn containers(value: &Value) -> usize {
        match value {
            Value::Object(fields) if !fields.is_empty() => {
                1 + fields.values().map(containers).sum::<usize>()
            }
            Value::Array(items) if !items.is_empty() => {
                1 + items.iter().map(containers).sum::<usize>()
            }
            _ => 0,
        }
    }

    /// What the naive expansion says of `schema`: `Some(true)` when it
    /// fits whatever the count's slack, `Some(false)` when it cannot,
    /// `None` within the slack. The count may charge each `,`/`:` and an
    /// extra separator per container, so up to one byte more per
    /// container; it never charges the `"#":` and separator the naive
    /// tree adds per reference, five bytes.
    fn oracle(schema: &Value) -> Option<bool> {
        let empty = Map::new();
        let map = |key: &str| {
            schema.get(key).and_then(Value::as_object).unwrap_or(&empty)
        };
        let mut naive = Naive {
            defs: map("$defs"),
            definitions: map("definitions"),
            nodes: 0,
            refs: 0,
            stack: Vec::new(),
        };
        let Ok(tree) = naive.schema(schema, 0, true) else {
            return Some(false);
        };
        let bytes = serde_json::to_string(&tree).unwrap().len();
        if bytes.saturating_sub(5 * naive.refs) > MAX_STRICT_SCHEMA_BYTES {
            Some(false)
        } else if bytes + containers(&tree) <= MAX_STRICT_SCHEMA_BYTES {
            Some(true)
        } else {
            None
        }
    }

    /// A schema with `$defs` and `definitions`, root properties that
    /// refer to them again and again, definitions that refer on to
    /// later ones, and sometimes a cycle, a missing reference, a chain
    /// too deep, a reference in data, a property named `$ref`, or an
    /// override beside a reference. Its size and node count are drawn
    /// around the limits.
    #[hegel::composite]
    fn schema(tc: &TestCase) -> Value {
        let count = tc.draw(gs::integers::<usize>().min_value(1).max_value(4));
        let in_defs: Vec<bool> =
            (0..count).map(|_| tc.draw(gs::booleans())).collect();
        let reference = |index: usize| {
            if in_defs[index] {
                format!("#/$defs/d{index}")
            } else {
                format!("#/definitions/d{index}")
            }
        };
        let cycle = tc.draw(gs::integers::<u8>().max_value(9)) == 0;
        let mut defs = Map::new();
        let mut definitions = Map::new();
        for (index, &in_dollar_defs) in in_defs.iter().enumerate() {
            let length = tc.draw(gs::integers::<usize>().max_value(1_500));
            let mut definition = json!({
                "type": "string",
                "description": "x".repeat(length),
            });
            let next = if cycle && index == count - 1 {
                Some(0)
            } else {
                (index + 1 < count && tc.draw(gs::booleans()))
                    .then_some(index + 1)
            };
            if let Some(next) = next {
                definition = json!({
                    "type": "object",
                    "description": "x".repeat(length),
                    "properties": {"inner": {"$ref": reference(next)}},
                });
            }
            let nesting = if tc.draw(gs::integers::<u8>().max_value(9)) == 0 {
                tc.draw(gs::integers::<usize>().min_value(55).max_value(70))
            } else {
                tc.draw(gs::integers::<usize>().max_value(3))
            };
            for _ in 0..nesting {
                definition = json!({"type": "array", "items": definition});
            }
            if in_dollar_defs {
                defs.insert(format!("d{index}"), definition);
            } else {
                definitions.insert(format!("d{index}"), definition);
            }
        }
        let refs = tc.draw(gs::integers::<usize>().max_value(900));
        let padding = tc.draw(gs::integers::<usize>().max_value(60));
        let overrides = tc.draw(gs::booleans());
        let mut properties = Map::new();
        for index in 0..refs {
            let mut property = json!({"$ref": reference(index % count)});
            if overrides && index % 3 == 0 {
                property["description"] = json!("override");
            }
            properties.insert(
                format!("p{index:04}{}", "y".repeat(padding)),
                property,
            );
        }
        if tc.draw(gs::integers::<u8>().max_value(9)) == 0 {
            properties
                .insert("missing".into(), json!({"$ref": "#/$defs/nope"}));
        }
        if tc.draw(gs::booleans()) {
            properties.insert("$ref".into(), json!({"type": "string"}));
        }
        let mut root = json!({
            "type": "object",
            "properties": properties,
            "additionalProperties": false,
        });
        if tc.draw(gs::booleans()) {
            root["default"] = json!({"$ref": "#/$defs/nope"});
        }
        if !defs.is_empty() {
            root["$defs"] = Value::Object(defs);
        }
        if !definitions.is_empty() {
            root["definitions"] = Value::Object(definitions);
        }
        root
    }

    /// The budget says yes to every schema whose naive expansion fits
    /// with room for the count's slack, and no to every one whose
    /// expansion is too large, too many nodes, too deep, cyclic or
    /// unresolved. A yes always leaves the real expansion within the
    /// size limit.
    #[hegel::test(test_cases = 300)]
    fn the_budget_agrees_with_a_naive_expansion(tc: TestCase) {
        let schema = tc.draw(schema());
        let within = within_inline_budget(&schema);
        if let Some(expected) = oracle(&schema) {
            assert_eq!(within, expected);
        }
        if within && let Ok(inlined) = inline_refs(&schema) {
            assert!(
                serde_json::to_vec(&inlined).unwrap().len()
                    <= MAX_STRICT_SCHEMA_BYTES
            );
        }
    }

    /// Five hundred references to one 350-character leaf: about 230 KB
    /// expanded, within the budget.
    fn repeated_leaf(refs: usize) -> Value {
        let mut properties = Map::new();
        for index in 0..refs {
            let name = format!("p{index:04}{}", "x".repeat(55));
            properties.insert(name, json!({"$ref": "#/$defs/leaf"}));
        }
        json!({
            "$defs": {"leaf": {"type": "string", "description": "x".repeat(350)}},
            "type": "object", "properties": properties,
            "additionalProperties": false
        })
    }

    #[test]
    fn strict_size_fixture_is_within_preflight_budget() {
        let schema = repeated_leaf(500);
        assert_eq!(oracle(&schema), Some(true));
        assert!(within_inline_budget(&schema));
    }

    /// Six hundred of them are past it.
    #[test]
    fn more_references_to_the_leaf_are_past_the_budget() {
        let schema = repeated_leaf(600);
        assert_eq!(oracle(&schema), Some(false));
        assert!(!within_inline_budget(&schema));
    }
}
