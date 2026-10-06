# tau-artifacts

Private byte storage for artifacts that are referred to by opaque ids. A
tool can publish output too large to hand the model, such as a command's
full log or a file's bytes, and pass on a small reference instead. The
model, or the code that holds the reference, then reads it back one range
at a time.

The directory and every published file are readable only by their owner,
and the storage has quotas per artifact and in total. No public method
turns an arbitrary id into bytes: a caller reads only from `Artifact`
metadata it already holds, so access is decided by whoever granted that
metadata. The crate is Unix-only.

## What it provides

| Item                                     | What it is                                                                 |
| ---------------------------------------- | -------------------------------------------------------------------------- |
| `Bytes`                                  | A handle on one storage directory. Cheap to clone                          |
| `Bytes::new(path, quotas)`               | Opens the directory, creating it with owner-only permissions               |
| `Bytes::publish_reader`                  | Streams a reader into a new artifact and returns its `Artifact`            |
| `Bytes::publish_leased_reader`           | The same, keeping the publication lock until the caller records its grant  |
| `Bytes::read_range`                      | Reads up to `limit` bytes at an offset, as UTF-8 text or base64            |
| `Bytes::lock_publication`                | Takes the publication lock, to collect roots before a prune                |
| `Bytes::prune_with_roots`                | Deletes published objects that no given root refers to                     |
| `Artifact`                               | Immutable metadata: a UUID v7 `id`, a SHA-256 `digest`, and `size_bytes`   |
| `Quotas`                                 | Limits per artifact and in total (64 MiB and 512 MiB by default)           |
| `Range`, `Encoding`                      | One read: its data, offsets, whether it hit the end, and how it is encoded |
| `PublicationLease`, `PruneReport`        | The held lock, and what a prune removed                                    |
| `MAX_RANGE_BYTES`, `DEFAULT_RANGE_BYTES` | 64 KiB, the most one read returns, and 32 KiB, the usual read              |
| `Error`, `Result`                        | Quota, cancellation, invalid artifact, offset and limit errors, and I/O    |

Publishing reads in 64 KiB chunks into a private staging file, checks the
cancellation token and the size limit as it goes, then moves the file into
place without replacing anything. A directory-wide file lock serializes the
quota check and the move across handles and processes. A failed or
cancelled publication leaves no object. Equal bytes published twice get two
ids.

Deserializing an `Artifact` checks that its id is a canonical UUID v7 and
its digest is lowercase hex, so a path can never pass as an id.

## How it fits

It depends on no other tau crate. `tau-tools-host` publishes `bash` and `read`
output through it and reads ranges back for the model. `tau-ui` keeps the
storage in each project and prunes it against the retained run history.
`tau-codemode-eval` uses it in its evaluation runs.

## Usage

```rust
use std::io::Cursor;

use tau_artifacts::{Bytes, DEFAULT_RANGE_BYTES, Encoding, Quotas};
use tokio_util::sync::CancellationToken;

let bytes = Bytes::new("/path/to/artifacts", Quotas::default())?;
let cancel = CancellationToken::new();

let log = Cursor::new(b"a long log".to_vec());
let artifact = bytes.publish_reader(log, &cancel)?;
println!("{} ({} bytes)", artifact.id(), artifact.size_bytes());

let range = bytes.read_range(
    &artifact,
    0,
    DEFAULT_RANGE_BYTES,
    Encoding::Utf8,
    &cancel,
)?;
assert!(range.eof);
```

`Bytes` does blocking file I/O. In async code, call it from
`tokio::task::spawn_blocking`.

## Testing

```sh
cargo nextest run --release -p tau-artifacts
```

The tests are in `src/tests.rs`, against a temporary directory. Hegel
properties check that published bytes read back whole across any split of
reads, in base64 and in UTF-8 pages that never cut a code point, with the
digest checked against a separate SHA-256. Example tests cover quotas after
a restart, concurrent handles, cancellation, file permissions, and pruning.

## Further reading

- [Code Mode artifact storage](../../docs/reference/codemode-artifact-storage.md)
- [Code Mode artifact ranges](../../docs/reference/codemode-artifact-ranges.md)
- [Code Mode artifact retention](../../docs/reference/codemode-artifact-retention.md)
- [Decision 0028: Async all the way](../../docs/decisions/0028-async-all-the-way-blocking-only-in-spawn-blocking.md)
