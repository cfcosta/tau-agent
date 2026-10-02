# Codemode inference request

`tau_codemode::inference::InferRequest` is the host-side request contract for
the nested `infer` tool.

The argument object has `task` (required string), `context` (required JSON
value, including `null`), and `schema` (optional JSON Schema; `null` means
absent). Other keys fail parsing. The task must contain nonwhitespace text and
fit in 16 KiB. Serialized context fits in 256 KiB; serialized schema and answer
fit in 64 KiB each. Schema nesting is limited to 64 levels.

Schema compilation happens during parsing, before a caller can invoke a model.
Only local `#` references are accepted. Resource identifiers, anchors, dynamic
references, and file or network references at schema locations are rejected;
property names and literal values in `const`, `enum`, `default`, and `examples`
are data. The resource check visits schema-valued keywords and named schema
maps, while the 64-level input limit also covers data. The JSON Schema
validator has remote-resolution features disabled. Boolean schemas are
accepted. The decoded JSON answer is validated against the original schema
without type coercion. An invalid or mismatched answer returns an error. With
no schema, the entire answer is returned as a string.
Integer literals in schema-bearing answers outside Luau's exact ±2^53 range
also fail before conversion to Luau.

`input_text()` contains only the explicit task and serialized context. For a
schema supported by `tau_agent::schema::inline_refs` and `to_strict`,
`text_format()` supplies the provider's strict JSON schema format. Before
inlining, a nonmaterializing walk caps the estimated expansion at 4,096 nodes,
64 levels, and 256 KiB of conservative serialized bytes. It follows local
`#/$defs/<name>` and `#/definitions/<name>` references using the combined
root definitions, detects cycles, and stops when a budget is exhausted. The
final serialized strict schema is also capped at 256 KiB. Overbudget,
recursive, or unsupported schemas have no strict format: `input_text()` then
includes the original schema with an instruction to return JSON, and answer
validation still uses the original schema.

The strict rewrite requires an object root. It supports simple local
references to root definitions, object properties, `items`, and `anyOf`;
it makes optional object properties nullable and requires all properties.
It rejects boolean schema nodes, recursive or unresolved references,
`allOf`, `oneOf`, `patternProperties`, `dependentSchemas`, `dependencies`,
schema-valued or true `additionalProperties`, tuple `items`, and other
unsupported strict keywords. For string-valued nodes it accepts only the
strict format names supported by the shared rewrite. These limits affect
provider formatting, not the original answer validator.

For strict output, optional fields can arrive as `null` because the strict
rewrite requires every field. Before original-schema validation,
`strip_nulls_for_optional` removes only nulls on fields declared optional in
the original schema when that field does not itself allow null. Required or
genuinely nullable fields keep their null values.
