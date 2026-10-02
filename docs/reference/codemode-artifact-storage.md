# Code Mode artifact storage

`tau-artifacts` stores raw byte artifacts in a private directory. `Bytes::new`
opens a directory with a 64 MiB per-artifact limit and a 512 MiB total limit
by default. Callers can supply smaller `Quotas`. Each `Bytes` handle is cheap
to clone.

`publish_reader` reads at most 64 KiB per call, checks cancellation and the
per-artifact limit while streaming, and hashes the original bytes with SHA-256.
It returns immutable `Artifact` metadata: an opaque UUID v7 `id`, lowercase
SHA-256 `digest`, and `size_bytes`. The digest is descriptive; it cannot locate
an object without its ID. Repeated publications of equal bytes get distinct
IDs.

Staging files are private and removed after ordinary errors, cancellation, or
quota rejection. A successful publication syncs the staged file, persists it
without replacing any existing object, and syncs the object directory. The
published file is read-only to its owner. A file lock serializes the final
quota check and publication across handles and processes. The quota is the sum
of retained published object bytes, recomputed from object files after restart;
staging bytes and display text do not count. A crash may leave an unreferenced
staging file, which is ignored by accounting and is not removed automatically
because another process may still be writing it.

Artifact deserialization validates the canonical ID and lowercase digest
before any internal file resolution. The only byte-read method is crate-private
for tests and the later authorized range API. This crate does not expose an
unscoped read or wire a Tau tool or plugin yet.

The Hegel property in `src/tests.rs` generates bounded raw bytes, Unicode
bytes, and varied read splits, then checks the private read against the input,
the size against its length, and the digest against a separate SHA-256 oracle.
Its inventory, generator plan, and CI profile notes live beside the property.
