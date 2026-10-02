# Full-file artifacts from `read`

In normal app runs, `CodingTools` stores artifacts under the repository's
private `artifacts` directory. A `read` call still presents the same direct
text, truncation notices, image blocks, and read errors. Its arguments stay
`path`, `offset`, and `limit`; the default display limit remains 2000 lines or
50 KiB. An independently constructed `Read::new(root)` is also supported and
reports that artifact storage is unavailable in its structured value.

When storage is available, `read` streams the **entire original file** to
private byte storage after preparing its display. The artifact does not come
from the displayed text, selected line range, lossy UTF-8 conversion, or
image processing. Text and binary files retain their original bytes. For
images, the direct image block still comes from the existing processor;
the artifact refers to the original image file, including when processing
omits the image. The original bytes are not copied into the script value.

Successful structured text and image values include:

```json
"artifact": {
  "id": "0199b283-f06a-722b-8c75-476700ee3488",
  "digest": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "size_bytes": 70000,
  "source": "/resolved/path/to/file"
}
```

`id` is an opaque reference. `digest` is lowercase SHA-256 of the complete
original bytes; `size_bytes` is their count. `source` is the resolved file
path used for the read. The grant is scoped to the run and its permitted
forks. A script can call `artifact_read` with the ID and byte ranges to page
through the content. See [artifact ranges](codemode-artifact-ranges.md).

If publication cannot finish, the structured value contains
`"artifact_error": "..."` instead of `artifact`. This includes absent
storage or plugin context, file changes, I/O errors, and byte quota failures.
The display still succeeds when only artifact publication fails. A failed
read remains a failed read. Cancellation returns `Operation aborted` and
does not return a structured artifact reference.

The display and artifact pass use the same opened file descriptor. The
reader checks the descriptor and resolved path's file identity, size, and
modification/change times before publication and at the end of streaming.
If either changed, it reports a snapshot mismatch and publishes no grant.
This check detects ordinary edits and path replacement; it does not provide
an atomic filesystem snapshot against a writer that can deliberately restore
all checked metadata. Keep inputs stable while reading when strict snapshot
identity matters.

The default per-artifact limit is 64 MiB and the default total limit is
512 MiB. Publication streams in bounded chunks. A quota breach, interrupted
read, or cancelled publication removes staging bytes without returning an ID
or grant. If a complete object was committed immediately before cancellation,
it may remain as an ungranted orphan; it cannot be read through
`artifact_read` without a scoped grant.

`tests/artifact_ranges.rs` checks bytes beyond both display limits, large
first lines, binary recovery, quota errors, future-drop cancellation, and
run/fork access. Its Hegel property compares complete Unicode pages to the
original source bytes and resolved source path. The property inventory,
generator plan, and CI notes are in the test comments; the workspace
`hegel.toml` controls counts.
