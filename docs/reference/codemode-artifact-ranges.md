# Code Mode artifact ranges

`CodingTools::new(root).with_artifacts(bytes)` adds `artifact_read` as a
**nested** tool. A tool calls it through `ToolCtx::call`; it is not one of the
seven direct model tools. `CodingTools::new(root)` without byte storage still
registers it, but a call returns `artifact storage is unavailable`.

## Arguments

```json
{ "id": "0199b283-f06a-722b-8c75-476700ee3488", "offset": 0, "limit": 32768, "encoding": "utf8" }
```

| Field      | Meaning                                                                                                                                      |
| ---------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| `id`       | Required opaque artifact UUID v7. It is not a path.                                                                                          |
| `offset`   | Optional, zero based **byte** offset; defaults to `0`. It may equal the artifact size for an empty EOF page. An offset beyond the end fails. |
| `limit`    | Optional maximum input bytes to read; defaults to `32768`. Valid values are `1` through `65536`.                                             |
| `encoding` | Optional `utf8` (default) or `base64`.                                                                                                       |

The result has a short text summary and this structured value:

```json
{
  "id": "0199b283-f06a-722b-8c75-476700ee3488",
  "offset": 0,
  "next_offset": 4,
  "size_bytes": 9,
  "encoding": "utf8",
  "data": "text",
  "eof": false,
  "complete": false
}
```

`offset` is the requested byte position. `data` is either strict UTF-8 text or
standard padded base64. `size_bytes` is the whole artifact's recorded size.
`next_offset` is the next byte position to request, or `null` at EOF. `eof`
means this page reached the **end of the artifact**. `complete` is true only
when this one range starts at byte zero and contains the whole artifact.
A tail-only page can reach EOF without being complete. A successful page at
`offset == size_bytes` has empty `data`, `next_offset: null`, and `eof: true`;
it is complete only for an empty artifact. For larger artifacts, follow
`next_offset` and join contiguous pages from byte zero to EOF.

With `utf8`, the starting offset must be a codepoint boundary. If the byte
limit ends inside a codepoint, the page stops before it and `next_offset`
points to that codepoint's first byte. Use `next_offset` for the next page.
A limit too small to include one codepoint fails instead of returning a page
that cannot advance. Invalid UTF-8, including a truncated codepoint at the
artifact's end, fails; use `base64` to preserve arbitrary bytes, including
NUL. Base64 uses the exact requested byte slice and may end inside a UTF-8
codepoint. Both encodings use the same byte offsets.

## Grants and storage

A trusted tool publishes through `publish_artifact(bytes, reader, source,
ctx)`. It needs a live `CodingTools` plugin context and stores an
`artifact_grant` record for that run. The record binds the opaque ID to
immutable digest and size metadata, the owner run ID, and source. Publication
fails if it cannot store the grant. A fork inherits only records at or before
its checkpoint sequence; grants added to the parent later are invisible to
that fork. Resumed runs and later forks can see records within their own
chain. Unrelated conversations cannot read each other's artifacts.

`artifact_read` resolves metadata only from those stored records. Supplying
an ID, digest, metadata, or a path in tool arguments cannot create a grant.
Unknown argument fields are rejected. A guessed ID without a visible grant
fails. Stored IDs must be canonical UUID v7 values, and digests must be
lowercase SHA-256 hex; traversal-like IDs are rejected before path resolution.

The byte store is private and should be supplied by the host. Reads seek and
read at most `limit` bytes on a blocking worker, honor cancellation, and check
the file's size against the grant metadata. A missing file or changed size
fails explicitly, including after a storage restart. The range call does not
rehash the whole file; hosts must protect the private storage directory from
untrusted writes. With artifact storage configured, `read` publishes a
full-file artifact through this API; `bash` does not. See
[full-file artifacts](codemode-artifact-files.md).
