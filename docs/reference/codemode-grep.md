# Structured grep output

`tau-tools`' `grep` keeps its current text response for the model and also
returns structured JSON to callers that use `AgentTool::output_schema`, such
as `codemode`. The output schema is an object with these required fields:

```json
{
  "root": "<resolved search path>",
  "lines": [
    {
      "path": "src/example.rs",
      "line": 12,
      "text": "matching source line",
      "kind": "match",
      "truncated": false
    }
  ],
  "match_count": 1,
  "truncated": false,
  "complete": true,
  "limit_reached": false,
  "bytes_truncated": false,
  "lines_truncated": false,
  "skipped": 0
}
```

Each line record comes from the native search result. `path` is relative to the
search root for a directory search, or the file name for a single-file search.
It stays a separate string even when it contains punctuation, Unicode, or
newlines. `line` is the one-based line number. `kind` is `match`, `before`, or
`after`. `text` is the matched/context line without its line terminator,
bounded to the existing 500-character limit. A record's `truncated` flag says
that this line was cut at that limit.

`match_count` counts matching lines selected before the 50 KiB presentation
cut. `lines` contains only complete records whose legacy formatted display
fits in that byte-limited presentation. A presentation cut never creates a
partial structured record.

- `limit_reached` is true only when an additional match beyond the requested
  limit was found. Files never started after the search settled at that limit
  are not counted as skipped.
- `bytes_truncated` reports the 50 KiB presentation cut.
- `lines_truncated` reports one or more lines cut to 500 characters.
- `truncated` is true when a match limit, presentation-byte limit, or
  per-line limit cut the result.
- `skipped` counts walker errors and files that could not be read or decoded
  as UTF-8. These inputs leave the result incomplete but do not change its
  existing text response.
- `complete` is false if a file was skipped, another match existed beyond the
  match limit, or either output limit cut the represented result. Otherwise,
  it is true.

When there are no matches, the text response remains `No matches found` and
the structured response still has `lines: []`, `match_count: 0`, and the
completion and skip fields for the search.
