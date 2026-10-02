# Code Mode command artifacts

When `CodingTools` is configured with `with_artifacts(bytes)`, `bash` captures
the complete output it observes into a private spill file. Pipe mode captures
the merged stdout and stderr bytes in arrival order. Terminal mode captures
the original raw PTY bytes; its rendered plain text remains the model's view.
The displayed tail, truncation limits, spill path and `Full output:` notice
retain their existing behavior. The original spill remains in place after
publication.

After the output accumulator finishes, `bash` flushes and syncs the spill and
checks that its stored length and successful write count equal the number of
raw bytes observed. The copy also checks a SHA-256 digest of the observed
stream as it reads the spill, rejecting equal-length edits before publication.
It then records a grant for the current CodingTools run. An inherited fork
can read the grant; an unrelated run cannot. `artifact_read` returns bounded
byte ranges by opaque ID, with UTF-8 or base64 encoding. Read successive
`next_offset` values until it is `null` to recover the whole output. A short
unspilled command without configured storage has no artifact; with storage,
even short output is captured from its original bytes rather than rebuilt
from displayed text.

The structured `bash` result adds:

| Field             | Meaning                                                                                                                           |
| ----------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| `artifact`        | `{id, digest, size_bytes, source}` after a successful publication; otherwise `null`.                                              |
| `artifact_error`  | Reason a nonempty observed output has no artifact, including missing storage, spill failure or quota rejection; otherwise `null`. |
| `source_complete` | `true` when the observed stream reached the tool's existing drain boundary. This says nothing about exit success.                 |

`status` and `exit_code` remain authoritative for command completion. A
nonzero command or a timed out command can have a complete artifact of the
bytes observed before its drain boundary. Timeout and cancellation are never
reported as command success merely because an artifact can be read. A failed
pipe read or terminal stream error sets `source_complete` to `false` and
prevents publication. Cancellation leaves no partial grant or partially
published object. A complete object committed just before cancellation can
remain ungranted until trusted retention maintenance removes it. Quota, absent spill, incomplete write, sync, and storage
errors leave `artifact` null and preserve the direct tool result and any
existing spill path. No rolling tail is published as full output.

The tests use scripted models only. A generated TSV property splits Unicode
bytes across arbitrary chunks and compares both the full bytes and parsed
rows with independent oracles. An integration fixture retrieves a 2,405-line,
over-150-KiB log through `artifact_read` and checks its first, middle, and
last markers in pipe and terminal modes. It also covers failed commands,
timeouts, quota failure, missing storage, and run versus fork scope. Hegel
case counts and CI behavior come from the workspace `hegel.toml` profile.
