# Code Mode `read` results

The `read` tool returns its usual content blocks for the agent and a
structured value for Code Mode scripts. Text reads stream the file once. The
reader retains only the requested range and enough data to decide the 2000
line and 50 KiB display limits; it does not buffer the whole file.

## Text

A text result has this shape:

```json
{
  "kind": "text",
  "path": "src/main.rs",
  "text": "first line\nsecond line",
  "offset": 1,
  "requested_limit": null,
  "total_lines": 2,
  "returned_lines": 2,
  "next_offset": null,
  "truncated": false,
  "complete": true,
  "truncated_by": null,
  "first_line_exceeds_limit": false
}
```

`text` contains file text only. Direct-output continuation and truncation
notices remain in the normal content block and are not repeated in this field.
`offset` is the effective one-based line number; omitted offsets and an
explicit zero both normalize to line 1. `requested_limit` keeps the supplied
limit, or `null` when none was supplied. `total_lines` follows the reader's
newline-split accounting, including an empty final segment after a trailing
newline.

`returned_lines` counts the lines retained by the 2000-line/50 KiB display
truncator. `truncated` and `truncated_by` describe those display limits;
`truncated_by` is `"lines"`, `"bytes"`, or `null`. A user-requested subset is
reported by `requested_limit`, `next_offset`, and `complete` without marking a
display truncation. `complete` is true only when the read starts at line 1,
covers the file, and is not display-truncated.

`next_offset` is the first line not returned, when more file lines remain. A
zero limit can therefore report the current offset because it returned no
lines. If the first line is larger than 50 KiB, the tool cannot return that
line or advance past it, so `next_offset` is `null` and
`first_line_exceeds_limit` is true.

## Images

Image results preserve the direct content blocks in a JSON-compatible array:

```json
{
  "kind": "image",
  "path": "assets/logo.png",
  "content": [
    { "type": "text", "text": "Read image file [image/png]" },
    { "type": "image", "data": "iVBORw0KGgo...", "mimeType": "image/png" }
  ],
  "omitted": false
}
```

The image data is base64 from the existing image processor and has a MIME type
detected from the bytes. The `type`, `data`, and `mimeType` fields match the
ImageContent shape accepted by Code Mode's `image()` function. When processing
omits an image, `omitted` is true and `content` contains the same explanatory
text block returned directly by the tool.

## Property check

`tests/read.rs` contains a Hegel differential property that generates small,
valid Unicode lines, then compares structured text and continuation ranges to
an independent `split`/`select` oracle. Its generator bounds line count,
characters, offset, and limit without rejecting invalid cases. Hegel's CI
profile derandomizes tests, disables the example database, and suppresses the
`TooSlow` check; the focused property uses a local 100-case override.
