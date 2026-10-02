# Structured \`find\` results

\`find\` keeps its existing text output for the model and also returns a
structured result for callers such as Codemode. The output schema is an object:

\`\`\`json
{
"root": "string",
"paths": ["string"],
"truncated": false,
"complete": true,
"limit_reached": false,
"bytes_truncated": false,
"skipped": 0
}
\`\`\`

\`root\` is the resolved path searched by the call. \`paths\` contains matched
records relative to that path in the existing sorted order. A search whose
\`path\` argument names a file returns its basename. The entry limit applies
before the byte limit, so \`paths\` contains at most the selected entry limit.
The 50 KiB byte limit affects only the text shown to the model: structured
paths are kept as complete strings even when the text display is cut. This
preserves valid names containing Unicode, colons, punctuation, or newlines.

\`limit_reached\` is true only when another matching path exists beyond the entry
limit. \`bytes_truncated\` is true when the formatted path text exceeds 50 KiB.
\`truncated\` is true when either limit cuts the result. \`skipped\` counts entries
the filesystem walker could not read; those entries remain omitted from both
outputs, as they were from the prior text-only output. \`complete\` is true only
when neither limit was reached and \`skipped\` is zero. A successful search with
no matches returns \`paths: []\` and the unchanged text \`No files found matching
pattern\`.

The structured payload is independent of formatted display lines. Callers
should consume \`paths\` directly and must not split the text output to recover
path records.
