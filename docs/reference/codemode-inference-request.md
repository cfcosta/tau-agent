# Codemode inference request

`tau_codemode::inference::InferRequest` is the host-side request contract for a
future nested `infer` tool. It does not call a model or register a tool yet.

The argument object has `task` (required string), `context` (required JSON
value, including `null`), and `schema` (optional JSON Schema; `null` means
absent). Other keys fail parsing. The task must contain nonwhitespace text and
fit in 16 KiB. Serialized context fits in 256 KiB; serialized schema and answer
fit in 64 KiB each. Schema nesting is limited to 64 levels.

Schema compilation happens during parsing, before a caller can invoke a model.
Only local `#` references are accepted. Resource identifiers, anchors, dynamic
references, and file or network references are rejected. Boolean schemas are
accepted. The decoded JSON answer is validated against the original schema
without type coercion. An invalid or mismatched answer returns an error. With
no schema, the entire answer is returned as a string.

`input_text()` contains only the explicit task and serialized context. For a
schema supported by `tau_agent::schema::inline_refs` and `to_strict`,
`text_format()` supplies the provider's strict JSON schema format. Otherwise
`text_format()` is absent and `input_text()` includes the original schema with
an instruction to return JSON. This avoids sending a known unsupported strict
schema to the provider while retaining final validation.

For strict output, optional fields can arrive as `null` because the strict
rewrite requires every field. Before original-schema validation,
`strip_nulls_for_optional` removes only nulls on fields declared optional in
the original schema when that field does not itself allow null. Required or
genuinely nullable fields keep their null values.
