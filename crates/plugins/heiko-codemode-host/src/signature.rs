//! Luau signatures from JSON Schema, after pi's `schemaToType`.
//!
//! | Schema                        | Luau                                 |
//! | ----------------------------- | ------------------------------------ |
//! | `string`, `number`, `integer` | `string`, `number`, `number`         |
//! | `boolean`, `null`             | `boolean`, `nil`                     |
//! | `array` with `items`          | `{ T }`; tuples `{ any }`            |
//! | `object` with `properties`    | `{ a: T, b: T? }`, sorted            |
//! | `additionalProperties: T`     | `{ [string]: T }`                    |
//! | `enum`, `const`               | `"a" \| "b"`, or the base type       |
//! | `anyOf`, `oneOf`              | `A \| B`                             |
//! | `allOf`                       | `A & B`                              |
//! | local `$ref`                  | expanded, at most 32 times           |
//! | anything else                 | `any`                                |

use std::collections::BTreeSet;

use serde_json::{Map, Value};

use crate::host::ToolEntry;

/// `$ref` expansions one rendering may make.
pub const MAX_REF_EXPANSIONS: usize = 32;

/// An input type longer than this many characters renders as `any`.
pub const MAX_INPUT_TYPE_CHARS: usize = 16_000;

/// Tokens (chars / 4) the run's context spends on signatures.
pub const CATALOG_BUDGET_TOKENS: usize = 3_000;

/// How deep a schema is followed before the rest is `any`.
const MAX_DEPTH: usize = 32;

/// The MCP types `CallToolResult<T>` refers to.
pub const CALL_TOOL_RESULT_TYPES: &str = "\
type Annotations = { audience: { \"user\" | \"assistant\" }?, priority: number?, lastModified: string? }
type TextContent = { type: \"text\", text: string, annotations: Annotations? }
type ImageContent = { type: \"image\", data: string, mimeType: string, annotations: Annotations? }
type AudioContent = { type: \"audio\", data: string, mimeType: string, annotations: Annotations? }
type ResourceLink = { type: \"resource_link\", uri: string, name: string, title: string?, description: string?, mimeType: string?, size: number?, annotations: Annotations? }
type EmbeddedResource = { type: \"resource\", resource: { uri: string, mimeType: string?, text: string?, blob: string? }, annotations: Annotations? }
type ContentBlock = TextContent | ImageContent | AudioContent | ResourceLink | EmbeddedResource
type CallToolResult<T> = { content: { ContentBlock }, structuredContent: T?, isError: boolean? }";

/// A Luau type.
#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Any,
    Nil,
    Name(String),
    /// A string singleton.
    Literal(String),
    Array(Box<Type>),
    Table {
        properties: Vec<Property>,
        indexer: Option<Box<Type>>,
    },
    Union(Vec<Type>),
    Intersection(Vec<Type>),
    Optional(Box<Type>),
    Generic(String, Vec<Type>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Property {
    pub name: String,
    pub ty: Type,
    pub required: bool,
    pub description: Option<String>,
}

/// Turns schemas into [`Type`]s, counting `$ref` expansions.
struct Converter<'a> {
    root: &'a Value,
    expansions: usize,
    stack: Vec<String>,
}

/// The Luau type for `schema`, with `$ref`s resolved against itself.
pub fn schema_type(schema: &Value) -> Type {
    Converter {
        root: schema,
        expansions: 0,
        stack: Vec::new(),
    }
    .convert(schema, 0)
}

impl Converter<'_> {
    fn convert(&mut self, schema: &Value, depth: usize) -> Type {
        if depth > MAX_DEPTH {
            return Type::Any;
        }
        let Value::Object(map) = schema else {
            return Type::Any;
        };
        if let Some(Value::String(reference)) = map.get("$ref") {
            return self.reference(reference, depth);
        }
        if let Some(value) = map.get("const") {
            return literals(std::slice::from_ref(value));
        }
        if let Some(Value::Array(values)) = map.get("enum") {
            return literals(values);
        }
        for key in ["anyOf", "oneOf"] {
            if let Some(Value::Array(options)) = map.get(key) {
                let types = options
                    .iter()
                    .map(|o| self.convert(o, depth + 1))
                    .collect();
                return union(types);
            }
        }
        if let Some(Value::Array(parts)) = map.get("allOf") {
            let mut types: Vec<Type> =
                parts.iter().map(|p| self.convert(p, depth + 1)).collect();
            types.dedup();
            return match types.len() {
                0 => Type::Any,
                1 => types.remove(0),
                _ if types.contains(&Type::Any) => Type::Any,
                _ => Type::Intersection(types),
            };
        }
        match map.get("type") {
            Some(Value::String(name)) => self.typed(name, map, depth),
            Some(Value::Array(names)) => {
                let types = names
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|name| self.typed(name, map, depth))
                    .collect();
                union(types)
            }
            _ if map.contains_key("properties") => {
                self.typed("object", map, depth)
            }
            _ => Type::Any,
        }
    }

    fn typed(
        &mut self,
        name: &str,
        map: &Map<String, Value>,
        depth: usize,
    ) -> Type {
        match name {
            "string" => Type::Name("string".into()),
            "number" | "integer" => Type::Name("number".into()),
            "boolean" => Type::Name("boolean".into()),
            "null" => Type::Nil,
            "array" => match map.get("items") {
                Some(items @ Value::Object(_)) => {
                    Type::Array(Box::new(self.convert(items, depth + 1)))
                }
                _ => Type::Array(Box::new(Type::Any)),
            },
            "object" => self.object(map, depth),
            _ => Type::Any,
        }
    }

    fn object(&mut self, map: &Map<String, Value>, depth: usize) -> Type {
        let required: BTreeSet<&str> = match map.get("required") {
            Some(Value::Array(names)) => {
                names.iter().filter_map(Value::as_str).collect()
            }
            _ => BTreeSet::new(),
        };
        let mut properties = Vec::new();
        // Luau cannot name a table-type key holding NUL: such properties
        // fall to the indexer.
        let mut unnamed = false;
        if let Some(Value::Object(props)) = map.get("properties") {
            let mut names: Vec<&String> = props.keys().collect();
            names.sort();
            for name in names {
                if name.contains('\0') {
                    unnamed = true;
                    continue;
                }
                let schema = &props[name];
                properties.push(Property {
                    name: name.clone(),
                    ty: self.convert(schema, depth + 1),
                    required: required.contains(name.as_str()),
                    description: schema
                        .get("description")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|d| !d.is_empty())
                        .map(str::to_owned),
                });
            }
        }
        let indexer = match map.get("additionalProperties") {
            Some(extra @ Value::Object(_)) => {
                Some(Box::new(self.convert(extra, depth + 1)))
            }
            _ if properties.is_empty() || unnamed => Some(Box::new(Type::Any)),
            _ => None,
        };
        Type::Table {
            properties,
            indexer,
        }
    }

    fn reference(&mut self, reference: &str, depth: usize) -> Type {
        let Some(pointer) = reference.strip_prefix('#') else {
            return Type::Any;
        };
        if self.expansions >= MAX_REF_EXPANSIONS
            || self.stack.iter().any(|r| r == reference)
        {
            return Type::Any;
        }
        let Some(target) = self.root.pointer(pointer) else {
            return Type::Any;
        };
        self.expansions += 1;
        self.stack.push(reference.to_owned());
        let ty = self.convert(target, depth + 1);
        self.stack.pop();
        ty
    }
}

fn literals(values: &[Value]) -> Type {
    let mut types = Vec::new();
    for value in values {
        let ty = match value {
            Value::String(s) => Type::Literal(s.clone()),
            Value::Number(_) => Type::Name("number".into()),
            Value::Bool(_) => Type::Name("boolean".into()),
            Value::Null => Type::Nil,
            _ => Type::Any,
        };
        types.push(ty);
    }
    union(types)
}

/// A union of `types`, flattened and deduplicated; `nil` in it makes
/// it optional.
fn union(types: Vec<Type>) -> Type {
    let mut flat: Vec<Type> = Vec::new();
    let mut nil = false;
    for ty in types {
        let parts = match ty {
            Type::Union(parts) => parts,
            Type::Optional(inner) => {
                nil = true;
                match *inner {
                    Type::Union(parts) => parts,
                    other => vec![other],
                }
            }
            other => vec![other],
        };
        for part in parts {
            if part == Type::Nil {
                nil = true;
            } else if !flat.contains(&part) {
                flat.push(part);
            }
        }
    }
    if flat.contains(&Type::Any) {
        return Type::Any;
    }
    let inner = match flat.len() {
        0 if nil => return Type::Nil,
        0 => return Type::Any,
        1 => flat.remove(0),
        _ => Type::Union(flat),
    };
    if nil {
        Type::Optional(Box::new(inner))
    } else {
        inner
    }
}

const RESERVED: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function",
    "if", "in", "local", "nil", "not", "or", "repeat", "return", "then",
    "true", "until", "while",
];

/// Whether `name` can be a property name as it is.
pub fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !RESERVED.contains(&name)
}

/// `text` as a Luau string literal.
pub fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                // Three digits, so a digit after it is not read into it.
                out.push_str(&format!("\\{:03}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `text` as `--` comment lines at `indent`. Luau ends a comment at
/// `\r` as well as `\n`, and stops reading at NUL, so other control
/// characters become spaces.
fn comment(text: &str, indent: &str) -> String {
    text.replace("\r\n", "\n")
        .split(['\r', '\n'])
        .map(|line| {
            let line: String = line
                .chars()
                .map(|c| if c.is_control() && c != '\t' { ' ' } else { c })
                .collect();
            format!("{indent}-- {}\n", line.trim_end())
        })
        .collect()
}

impl Type {
    /// The type as Luau, at `indent` levels of four spaces.
    pub fn render(&self, indent: usize) -> String {
        match self {
            Self::Any => "any".into(),
            Self::Nil => "nil".into(),
            Self::Name(name) => name.clone(),
            Self::Literal(text) => quote(text),
            Self::Array(item) => format!("{{ {} }}", item.render(indent)),
            Self::Union(parts) => parts
                .iter()
                .map(|p| p.render_atom(indent))
                .collect::<Vec<_>>()
                .join(" | "),
            Self::Intersection(parts) => parts
                .iter()
                .map(|p| p.render_atom(indent))
                .collect::<Vec<_>>()
                .join(" & "),
            Self::Optional(inner) => format!("{}?", inner.render_atom(indent)),
            Self::Generic(name, args) => format!(
                "{name}<{}>",
                args.iter()
                    .map(|a| a.render(indent))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Table {
                properties,
                indexer,
            } => render_table(properties, indexer.as_deref(), indent),
        }
    }

    /// The type where it binds tighter than `|`, `&` and `?`.
    fn render_atom(&self, indent: usize) -> String {
        match self {
            Self::Union(_) | Self::Intersection(_) | Self::Optional(_) => {
                format!("({})", self.render(indent))
            }
            _ => self.render(indent),
        }
    }
}

fn render_table(
    properties: &[Property],
    indexer: Option<&Type>,
    indent: usize,
) -> String {
    if properties.is_empty() {
        let value = indexer.map_or_else(|| "any".into(), |t| t.render(indent));
        return format!("{{ [string]: {value} }}");
    }
    let field = |property: &Property, indent: usize| {
        let key = if is_identifier(&property.name) {
            property.name.clone()
        } else {
            format!("[{}]", quote(&property.name))
        };
        let ty = if property.required {
            property.ty.render(indent)
        } else {
            match &property.ty {
                Type::Optional(_) | Type::Nil | Type::Any => {
                    property.ty.render(indent)
                }
                other => format!("{}?", other.render_atom(indent)),
            }
        };
        format!("{key}: {ty}")
    };
    let multiline = properties.iter().any(|p| p.description.is_some());
    if !multiline {
        let mut fields: Vec<String> =
            properties.iter().map(|p| field(p, indent)).collect();
        if let Some(extra) = indexer {
            fields.push(format!("[string]: {}", extra.render(indent)));
        }
        return format!("{{ {} }}", fields.join(", "));
    }
    let pad = "    ".repeat(indent + 1);
    let mut out = String::from("{\n");
    for property in properties {
        if let Some(description) = &property.description {
            out.push_str(&comment(description, &pad));
        }
        out.push_str(&format!("{pad}{},\n", field(property, indent + 1)));
    }
    if let Some(extra) = indexer {
        out.push_str(&format!(
            "{pad}[string]: {},\n",
            extra.render(indent + 1)
        ));
    }
    out.push_str(&"    ".repeat(indent));
    out.push('}');
    out
}

/// Whether `schema` is an MCP `CallToolResult` wrapper: an object with
/// `content` and `isError` properties.
pub fn is_call_tool_result(schema: &Value) -> bool {
    let Some(Value::Object(properties)) = schema.get("properties") else {
        return false;
    };
    properties.contains_key("content") && properties.contains_key("isError")
}

/// The type a tool's call returns to a script.
pub fn return_type(output_schema: Option<&Value>) -> Type {
    match output_schema {
        None => Type::Name("string".into()),
        Some(schema) if is_call_tool_result(schema) => {
            let structured = schema
                .pointer("/properties/structuredContent")
                .map_or(Type::Any, |inner| {
                    // Resolve `$ref`s inside it against the whole schema.
                    Converter {
                        root: schema,
                        expansions: 0,
                        stack: Vec::new(),
                    }
                    .convert(inner, 0)
                });
            let structured = match structured {
                Type::Optional(inner) => *inner,
                other => other,
            };
            Type::Generic("CallToolResult".into(), vec![structured])
        }
        Some(schema) => schema_type(schema),
    }
}

/// The input type of `tool`, as Luau.
pub fn input_type(tool: &ToolEntry) -> String {
    let rendered = schema_type(&tool.input_schema).render(0);
    if rendered.chars().count() > MAX_INPUT_TYPE_CHARS {
        "any".into()
    } else {
        rendered
    }
}

/// `tool` as a commented Luau function signature.
pub fn render_tool(tool: &ToolEntry) -> String {
    let mut out = comment(tool.description.trim(), "");
    out.push_str(&format!(
        "function tools.{}(args: {}): {}",
        tool.name,
        input_type(tool),
        return_type(tool.output_schema.as_ref()).render(0)
    ));
    out
}

/// Whether `tool`'s signature needs [`CALL_TOOL_RESULT_TYPES`].
pub fn uses_call_tool_result(tool: &ToolEntry) -> bool {
    tool.output_schema.as_ref().is_some_and(is_call_tool_result)
}

/// A tool's description and signature, with the MCP types it uses.
pub fn describe(tool: &ToolEntry) -> String {
    let signature = render_tool(tool);
    if uses_call_tool_result(tool) {
        format!("{signature}\n\n{}", CALL_TOOL_RESULT_TYPES)
    } else {
        signature
    }
}

/// The signatures that fit a budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    /// Luau: the shared types if a listed tool needs them, then each
    /// namespace's signatures.
    pub text: String,
    pub listed: Vec<String>,
    pub omitted: Vec<String>,
}

/// A namespace's tools: index, signature and price in tokens.
type Group = (Option<String>, Vec<(usize, String, usize)>);

/// Chars / 4, rounded up.
fn cost(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

/// Picks the signatures of `tools` that fit `budget_tokens`, as pi's
/// `selectCatalog` does: tools are grouped by namespace (tools without
/// one first, then namespaces by name), each group cheapest first, and
/// groups take turns; a group drops out when its next tool does not
/// fit. Every namespace is listed before any is complete.
pub fn catalog(tools: &[ToolEntry], budget_tokens: usize) -> Catalog {
    let mut groups: Vec<Group> = Vec::new();
    for (i, tool) in tools.iter().enumerate() {
        let text = render_tool(tool);
        let price = cost(&text) + 1;
        match groups.iter_mut().find(|(ns, _)| *ns == tool.namespace) {
            Some((_, members)) => members.push((i, text, price)),
            None => {
                groups.push((tool.namespace.clone(), vec![(i, text, price)]))
            }
        }
    }
    groups.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, members) in &mut groups {
        members.sort_by_key(|(i, _, price)| (*price, *i));
    }
    let shared = cost(CALL_TOOL_RESULT_TYPES) + 1;
    let mut spent = 0;
    let mut types_paid = false;
    let mut taken: Vec<Vec<usize>> = vec![Vec::new(); groups.len()];
    let mut next = vec![0usize; groups.len()];
    let mut active: Vec<bool> = vec![true; groups.len()];
    while active.iter().any(|a| *a) {
        for g in 0..groups.len() {
            if !active[g] {
                continue;
            }
            let Some((i, _, price)) = groups[g].1.get(next[g]) else {
                active[g] = false;
                continue;
            };
            let extra = if !types_paid && uses_call_tool_result(&tools[*i]) {
                shared
            } else {
                0
            };
            let header = if taken[g].is_empty() {
                groups[g].0.as_ref().map_or(0, |ns| cost(ns) + 2)
            } else {
                0
            };
            if spent + price + extra + header > budget_tokens {
                active[g] = false;
                continue;
            }
            spent += price + extra + header;
            types_paid |= extra > 0;
            taken[g].push(next[g]);
            next[g] += 1;
        }
    }
    let mut text = String::new();
    if types_paid {
        text.push_str(CALL_TOOL_RESULT_TYPES);
        text.push_str("\n\n");
    }
    let mut listed = Vec::new();
    for (g, (namespace, members)) in groups.iter().enumerate() {
        if taken[g].is_empty() {
            continue;
        }
        if let Some(namespace) = namespace {
            text.push_str(&format!("-- {namespace}\n\n"));
        }
        for &m in &taken[g] {
            let (i, signature, _) = &members[m];
            text.push_str(signature);
            text.push_str("\n\n");
            listed.push(tools[*i].name.clone());
        }
    }
    let text = text.trim_end().to_owned();
    let omitted = tools
        .iter()
        .map(|t| t.name.clone())
        .filter(|name| !listed.contains(name))
        .collect();
    Catalog {
        text,
        listed,
        omitted,
    }
}
