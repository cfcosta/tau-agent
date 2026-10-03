//! Luau signatures and the catalog that fits the run's context.
//!
//! | Property | Oracle |
//! | --- | --- |
//! | supported schemas preserve recursive and normalized types | independently constructed `Type` |
//! | broader schema syntax renders to Luau that parses; `$ref` cycles end | Luau's parser |
//! | objects keep every property; required ones have no `?` | construction |
//! | the catalog fits its budget and lists or omits every tool | construction |

use hegel::{TestCase, generators as gs};
use mlua::Lua;
use serde_json::{Map, Value, json};
use tau_codemode::{
    ToolEntry,
    describe,
    description::{self, description},
    signature::{
        CALL_TOOL_RESULT_TYPES,
        MAX_INPUT_TYPE_CHARS,
        Property,
        Type,
        catalog,
        input_type,
        quote,
        render_tool,
        schema_type,
    },
};

fn tool(
    name: &str,
    namespace: Option<&str>,
    schema: serde_json::Value,
) -> ToolEntry {
    ToolEntry {
        name: name.into(),
        description: format!("Does {name}."),
        input_schema: schema,
        output_schema: None,
        namespace: namespace.map(str::to_owned),
        sequential: false,
    }
}

/// `signature` parses as Luau once it has a body.
fn signature_parses(signature: &str) -> bool {
    let code = format!(
        "{CALL_TOOL_RESULT_TYPES}\nlocal tools = {{}}\n{signature}\nend"
    );
    Lua::new().load(code).into_function().is_ok()
}

#[test]
fn a_tool_renders_as_a_commented_function() {
    let read = ToolEntry {
        description: "Read a file.".into(),
        ..tool(
            "read",
            None,
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "offset": { "type": "integer" },
                    "limit": { "type": "integer" },
                },
                "required": ["path"],
            }),
        )
    };
    let signature = render_tool(&read);
    assert_eq!(
        signature,
        "-- Read a file.\nfunction tools.read(args: { limit: number?, offset: number?, path: string }): string"
    );
    assert!(signature_parses(&signature));
}

#[test]
fn described_properties_go_on_their_own_lines() {
    let edit = tool(
        "edit",
        None,
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "The file.\nRelative to the root." },
                "mode": { "enum": ["append", "replace"] },
                "tags": { "type": "array", "items": { "type": ["string", "null"] } },
                "meta": { "type": "object", "additionalProperties": { "type": "number" } },
                "weird key": { "const": 3 },
            },
            "required": ["path", "mode"],
        }),
    );
    let signature = render_tool(&edit);
    assert_eq!(
        signature,
        "-- Does edit.\nfunction tools.edit(args: {\n    \
             meta: { [string]: number }?,\n    \
             mode: \"append\" | \"replace\",\n    \
             -- The file.\n    \
             -- Relative to the root.\n    \
             path: string,\n    \
             tags: { string? }?,\n    \
             [\"weird key\"]: number?,\n\
         }): string"
    );
    assert!(signature_parses(&signature));
}

#[test]
fn mcp_results_render_as_call_tool_result() {
    let mut issues =
        tool("mcp__linear__list_issues", Some("mcp__linear"), json!({}));
    issues.output_schema = Some(json!({
        "type": "object",
        "properties": {
            "content": { "type": "array" },
            "isError": { "type": "boolean" },
            "structuredContent": { "$ref": "#/$defs/list" },
        },
        "$defs": { "list": { "type": "array", "items": { "type": "string" } } },
    }));
    let signature = render_tool(&issues);
    assert!(signature.ends_with("(args: any): CallToolResult<{ string }>"));
    assert!(signature_parses(&signature));
    assert!(describe(&issues).ends_with(CALL_TOOL_RESULT_TYPES));
}

#[test]
fn an_oversized_input_type_is_any() {
    let properties: serde_json::Map<_, _> = (0..2_000)
        .map(|i| (format!("field_{i}"), json!({ "type": "string" })))
        .collect();
    let big = tool(
        "big",
        None,
        json!({ "type": "object", "properties": properties }),
    );
    const { assert!(MAX_INPUT_TYPE_CHARS < 2_000 * 10) };
    assert_eq!(input_type(&big), "any");
}

#[test]
fn the_catalog_shares_its_budget_across_namespaces() {
    let mut tools = Vec::new();
    for ns in ["mcp__a", "mcp__b"] {
        for i in 0..40 {
            tools.push(tool(&format!("{ns}__t{i}"), Some(ns), json!({})));
        }
    }
    tools.push(tool("read", None, json!({})));
    let picked = catalog(&tools, 200);
    assert!(picked.text.chars().count().div_ceil(4) <= 200);
    assert_eq!(picked.listed[0], "read");
    let from =
        |ns: &str| picked.listed.iter().filter(|n| n.starts_with(ns)).count();
    assert!(from("mcp__a").abs_diff(from("mcp__b")) <= 1);
    assert_eq!(picked.listed.len() + picked.omitted.len(), tools.len());
    assert!(picked.text.contains("-- mcp__a\n"));
}

#[hegel::test]
fn the_catalog_fits_its_budget_and_accounts_for_every_tool(tc: TestCase) {
    let count = tc.draw(gs::integers::<usize>().max_value(30));
    let budget = tc.draw(gs::integers::<usize>().max_value(600));
    let tools: Vec<ToolEntry> = (0..count)
        .map(|i| {
            let ns = tc.draw(gs::sampled_from(vec![None, Some("mcp__x"), Some("mcp__y")]));
            let mut t = tool(&format!("t{i}"), ns, json!({ "type": "object", "properties": { "p": { "type": "string" } } }));
            if tc.draw(gs::booleans()) {
                t.output_schema = Some(json!({ "type": "object", "properties": { "content": {}, "isError": {} } }));
            }
            t
        })
        .collect();
    let picked = catalog(&tools, budget);
    assert!(
        picked.text.chars().count().div_ceil(4) <= budget,
        "{}",
        picked.text
    );
    let mut all: Vec<_> = picked
        .listed
        .iter()
        .chain(&picked.omitted)
        .cloned()
        .collect();
    all.sort();
    let mut names: Vec<_> = tools.iter().map(|t| t.name.clone()).collect();
    names.sort();
    assert_eq!(all, names);
    if picked.listed.iter().any(|name| {
        tools
            .iter()
            .any(|t| &t.name == name && t.output_schema.is_some())
    }) {
        assert!(picked.text.starts_with(CALL_TOOL_RESULT_TYPES));
    }
}

#[test]
fn the_description_is_fixed() {
    let with = description(true);
    let without = description(false);
    assert!(
        with.starts_with(
            "Run Luau code to orchestrate and compose tool calls\n"
        )
    );
    assert!(with.contains("\n\nJev:\n"));
    assert!(without.contains(description::NO_JEV));
    assert!(with.ends_with(description::NOT_DECLARED));
    assert!(without.ends_with(description::NOT_DECLARED));
}

#[hegel::composite]
fn leaf_schema(tc: &TestCase) -> Value {
    match tc.draw(gs::integers::<u8>().max_value(7)) {
        0 => json!({ "type": "string" }),
        1 => json!({ "type": "integer" }),
        2 => json!({ "type": "number" }),
        3 => json!({ "type": "boolean" }),
        4 => json!({ "type": "null" }),
        5 => {
            let values: Vec<String> =
                tc.draw(gs::vecs(gs::text().max_size(8)).max_size(4));
            json!({ "enum": values })
        }
        6 => json!({ "const": tc.draw(gs::text().max_size(8)) }),
        _ => json!({}),
    }
}

/// A schema `depth` levels deep at most, whose `$ref`s point at
/// `#/$defs/d0` to `#/$defs/d3`, which may not exist.
#[hegel::composite]
fn schema(tc: &TestCase, depth: u32) -> Value {
    if depth == 0 || tc.draw(gs::weighted_booleans(0.3)) {
        return tc.draw(leaf_schema());
    }
    let inner = || schema(depth - 1);
    match tc.draw(gs::integers::<u8>().max_value(6)) {
        0 => json!({ "type": "array", "items": tc.draw(inner()) }),
        1 => {
            let names: Vec<String> =
                tc.draw(gs::vecs(gs::text().max_size(10)).max_size(4));
            let mut properties = Map::new();
            let mut required = Vec::new();
            for name in names {
                let mut property = tc.draw(inner());
                if tc.draw(gs::booleans()) {
                    property["description"] =
                        json!(tc.draw(gs::text().max_size(30)));
                }
                if tc.draw(gs::booleans()) {
                    required.push(name.clone());
                }
                properties.insert(name, property);
            }
            let mut object = json!({ "type": "object", "properties": properties, "required": required });
            if tc.draw(gs::booleans()) {
                object["additionalProperties"] = tc.draw(inner());
            }
            object
        }
        2 => json!({ "anyOf": tc.draw(gs::vecs(inner()).max_size(3)) }),
        3 => json!({ "allOf": tc.draw(gs::vecs(inner()).max_size(3)) }),
        4 => {
            json!({ "type": ["string", "null", "array"], "items": tc.draw(inner()) })
        }
        5 => {
            json!({ "$ref": format!("#/$defs/d{}", tc.draw(gs::integers::<u8>().max_value(4))) })
        }
        _ => json!({ "oneOf": tc.draw(gs::vecs(inner()).max_size(3)) }),
    }
}

fn parses(lua: &Lua, ty: &str) -> bool {
    lua.load(format!("local x: {ty} = nil"))
        .into_function()
        .is_ok()
}

struct SupportedSchema {
    schema: Value,
    expected: Type,
}

fn primitive_schema(tc: &TestCase) -> SupportedSchema {
    let name = tc.draw(gs::sampled_from(vec![
        "string", "number", "integer", "boolean", "null",
    ]));
    let expected = match name {
        "string" => Type::Name("string".into()),
        "number" | "integer" => Type::Name("number".into()),
        "boolean" => Type::Name("boolean".into()),
        "null" => Type::Nil,
        _ => unreachable!(),
    };
    SupportedSchema {
        schema: json!({ "type": name }),
        expected,
    }
}

fn string_literal_schema(tc: &TestCase) -> SupportedSchema {
    let stem = tc.draw(gs::text().max_size(8));
    if tc.draw(gs::booleans()) {
        SupportedSchema {
            schema: json!({ "const": stem }),
            expected: Type::Literal(stem),
        }
    } else {
        // The suffixes keep enum members distinct through every shrink.
        let literals = [format!("{stem}0"), format!("{stem}1")];
        SupportedSchema {
            schema: json!({ "enum": literals }),
            expected: Type::Union(
                literals.into_iter().map(Type::Literal).collect(),
            ),
        }
    }
}

fn distinct_primitive_union(tc: &TestCase) -> SupportedSchema {
    let names = ["string", "number", "boolean", "null"];
    let first = tc.draw(gs::integers::<usize>().max_value(2));
    let second =
        tc.draw(gs::integers::<usize>().min_value(first + 1).max_value(3));
    let members = [first, second];
    let mut expected = Vec::new();
    let mut has_nil = false;
    for index in members {
        match names[index] {
            "null" => has_nil = true,
            name => expected.push(Type::Name(name.into())),
        }
    }
    let expected = if expected.len() == 1 {
        expected.remove(0)
    } else {
        Type::Union(expected)
    };
    SupportedSchema {
        schema: json!({ "anyOf": members.map(|index| json!({ "type": names[index] })) }),
        expected: if has_nil {
            Type::Optional(Box::new(expected))
        } else {
            expected
        },
    }
}

/// At depth two, a binary object has at most seven nodes; an anyOf has three.
fn supported_schema(tc: &TestCase, depth: u8) -> SupportedSchema {
    if depth == 0 {
        return if tc.draw(gs::booleans()) {
            primitive_schema(tc)
        } else {
            string_literal_schema(tc)
        };
    }
    match tc.draw(gs::integers::<u8>().max_value(3)) {
        0 => primitive_schema(tc),
        1 => string_literal_schema(tc),
        2 => {
            let item = supported_schema(tc, depth - 1);
            SupportedSchema {
                schema: json!({ "type": "array", "items": item.schema }),
                expected: Type::Array(Box::new(item.expected)),
            }
        }
        _ if tc.draw(gs::booleans()) => distinct_primitive_union(tc),
        _ => {
            let left = supported_schema(tc, depth - 1);
            let right = supported_schema(tc, depth - 1);
            let left_required = tc.draw(gs::booleans());
            let right_required = tc.draw(gs::booleans());
            let mut required = Vec::new();
            if left_required {
                required.push("zeta");
            }
            if right_required {
                required.push("alpha");
            }
            SupportedSchema {
                schema: json!({
                    "type": "object",
                    "properties": { "zeta": left.schema, "alpha": right.schema },
                    "required": required,
                }),
                expected: Type::Table {
                    properties: vec![
                        Property {
                            name: "alpha".into(),
                            ty: right.expected,
                            required: right_required,
                            description: None,
                        },
                        Property {
                            name: "zeta".into(),
                            ty: left.expected,
                            required: left_required,
                            description: None,
                        },
                    ],
                    indexer: None,
                },
            }
        }
    }
}

// Property inventory: supported schemas map to an independently built Type and
// render as Luau. The root forces primitive, object, and local-ref properties;
// its definition is a generated leaf/subtree. Depth <= 2 and <= 16 schema nodes
// are guaranteed by construction; shrinking shortens text and branches while
// preserving distinct enum/anyOf members and a valid backward definition ref.
#[hegel::test]
fn supported_schemas_preserve_recursive_types_and_local_references(
    tc: TestCase,
) {
    let primitive = primitive_schema(&tc);
    let object_child = supported_schema(&tc, 0);
    let object_required = tc.draw(gs::booleans());
    let object = SupportedSchema {
        schema: json!({
            "type": "object",
            "properties": { "inside": object_child.schema },
            "required": if object_required { vec!["inside"] } else { vec![] },
        }),
        expected: Type::Table {
            properties: vec![Property {
                name: "inside".into(),
                ty: object_child.expected,
                required: object_required,
                description: None,
            }],
            indexer: None,
        },
    };
    let definition = supported_schema(&tc, 2);
    let primitive_required = tc.draw(gs::booleans());
    let object_required = tc.draw(gs::booleans());
    let ref_required = tc.draw(gs::booleans());
    let mut required = Vec::new();
    if primitive_required {
        required.push("zeta");
    }
    if object_required {
        required.push("alpha");
    }
    if ref_required {
        required.push("middle");
    }
    let schema = json!({
        "type": "object",
        "properties": {
            "zeta": primitive.schema,
            "alpha": object.schema,
            "middle": { "$ref": "#/$defs/target" },
        },
        "required": required,
        "$defs": { "target": definition.schema },
    });
    let expected = Type::Table {
        properties: vec![
            Property {
                name: "alpha".into(),
                ty: object.expected,
                required: object_required,
                description: None,
            },
            Property {
                name: "middle".into(),
                ty: definition.expected,
                required: ref_required,
                description: None,
            },
            Property {
                name: "zeta".into(),
                ty: primitive.expected,
                required: primitive_required,
                description: None,
            },
        ],
        indexer: None,
    };
    let actual = schema_type(&schema);
    assert_eq!(actual, expected, "schema: {schema}");
    let rendered = actual.render(0);
    assert!(
        parses(&Lua::new(), &rendered),
        "does not parse:\n{rendered}"
    );
}

#[hegel::test(test_cases = 300)]
fn broader_schema_syntax_renders_to_luau_that_parses(tc: TestCase) {
    let mut root = tc.draw(schema(3));
    // Definitions that refer to each other, cycles included.
    let defs: Vec<Value> = tc.draw(gs::vecs(schema(2)).max_size(4));
    if let Value::Object(map) = &mut root {
        let defs: Map<String, Value> = defs
            .into_iter()
            .enumerate()
            .map(|(i, d)| (format!("d{i}"), d))
            .collect();
        map.insert("$defs".into(), Value::Object(defs));
    }
    let rendered = schema_type(&root).render(0);
    let lua = Lua::new();
    assert!(parses(&lua, &rendered), "does not parse:\n{rendered}");
}

#[test]
fn self_referential_schemas_end() {
    let root = json!({
        "$defs": { "node": { "type": "object", "properties": {
            "next": { "$ref": "#/$defs/node" },
            "kids": { "type": "array", "items": { "$ref": "#/$defs/node" } },
        } } },
        "$ref": "#/$defs/node",
    });
    assert_eq!(
        schema_type(&root).render(0),
        "{ kids: { any }?, next: any }"
    );
}

#[hegel::composite]
fn simple_schema(tc: &TestCase) -> Value {
    let leaf = tc.draw(gs::sampled_from(vec![
        "string", "number", "integer", "boolean",
    ]));
    if tc.draw(gs::booleans()) {
        json!({ "type": "array", "items": { "type": leaf } })
    } else {
        json!({ "type": leaf })
    }
}

#[hegel::test]
fn objects_keep_every_property_and_mark_only_optional_ones(tc: TestCase) {
    // Luau cannot write a key holding NUL; those go to the indexer.
    let names: Vec<String> = tc.draw(
        gs::vecs(gs::text().exclude_characters("\0").max_size(12))
            .unique(true)
            .max_size(6),
    );
    let mut properties = Map::new();
    let mut required = Vec::new();
    for name in &names {
        properties.insert(name.clone(), tc.draw(simple_schema()));
        if tc.draw(gs::booleans()) {
            required.push(name.clone());
        }
    }
    let schema = json!({ "type": "object", "properties": properties, "required": required });
    let rendered = schema_type(&schema).render(0);
    assert!(parses(&Lua::new(), &rendered), "{rendered}");
    for name in &names {
        let key = if tau_codemode::signature::is_identifier(name) {
            name.clone()
        } else {
            format!("[{}]", quote(name))
        };
        let ty = schema_type(&properties[name]).render(0);
        let field = if required.contains(name) {
            format!("{key}: {ty},")
        } else {
            format!("{key}: {ty}?,")
        };
        // Every field is followed by `,` or ` }`.
        let field_end = field.trim_end_matches(',').to_owned() + " }";
        assert!(
            rendered.contains(&field) || rendered.ends_with(&field_end),
            "{field} missing from {rendered}"
        );
    }
}
