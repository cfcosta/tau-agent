# Code Mode artifact retention and inspection

## Inspection

Finished `read` and `bash` cards report the artifact ID, SHA-256 digest, byte
size, source, and observed source completeness. A failed publication reports
its error; an empty `bash` output reports that it has no artifact. A terminal
card keeps its rendered terminal view while its artifact refers to the raw PTY
byte stream. The model and card receive bounded output, not the complete
artifact bytes.

The run inspector lists the run's scoped grants from its serialized `tau-tools`
state. Live record folding and reloaded record folding use the same serde
state. A preview action names the run, artifact ID, byte offset, and UTF-8 or
base64 encoding. The host resolves metadata from that run's persisted fork
grants, then returns at most 1,024 source bytes. UTF-8 previews end at a
codepoint boundary; base64 previews cover binary bytes. Each inspector entry
keeps only its latest bounded preview. Missing storage, missing bytes, invalid
UTF-8, and unauthorized IDs return an explicit error. A phone can deserialize
the grant state without the filesystem artifact crate.

`source_complete` describes the observed source boundary. For `read`, `true`
means the display reader reached its selected range and continuation boundary;
it does not establish that immutable bytes were published. For `bash`, it
describes the captured stream drain and does not indicate command exit success.
Only an artifact reference with a persisted grant identifies readable bytes.
`artifact_error` is authoritative when publication fails, including missing
storage or a file changed after display. New grants store source completeness;
older grants without the field show completion as unknown.

## Maintenance API

`Host::prune_artifacts(repo)` is explicit maintenance for one repository. It
is not called on startup or after a tool call. It holds the artifact
publication lock while it enumerates every persisted run, validates each
run's repository record and reachable fork parent, reads the original message
bodies (including those hidden by later context rewrites), scans all original
`tau-tools` grant records, and sweeps. Structured tool results, artifact grant
records, and codemode return/store values containing artifact metadata can
retain an object. The complete grant scan retains an original scoped grant
even when a current card or script store no longer displays it. Any retained
fork prefix keeps the grants it inherited from its parent. An absent parent,
malformed body or grant, mismatched grant owner, missing referenced file,
unreadable bytes, or digest mismatch aborts before deletion.

`Bytes::prune_with_roots` is the lower-level maintenance API. Its caller
supplies trusted `Artifact` metadata and a `PublicationLease`; that API cannot
establish whether the caller included complete retained history. The host API
supplies the complete persisted inventory. Marking validates each root's ID,
size, and SHA-256 digest before sweeping unreferenced, atomically published
`.blob` objects. Quota accounting scans remaining published files after a
restart, counting separate object IDs even if their digests match. Private
staging files are not swept because an active writer can own one.

`publish_leased_reader` keeps the publication lock through the following
durable grant record write. A maintenance pass therefore observes either the
recorded grant or no published object from that writer. A cancelled operation
may leave an ungranted published object; a later explicit maintenance pass
can reclaim it.
