# Codemode module records

Codemode module definitions are immutable records. This page describes their
storage and fold contract. See [module loading](codemode-module-loading.md)
for the VM's `require` behavior. No definition tools are provided yet.

The outer plugin record has `kind: "module"`. Its payload has `op: "define"`
with a `definition`, or `op: "select"` with `name` and `version`. A definition
contains `name`, `version`, `source`, `signatures`, and `dependencies`. Define
registers the version and selects it. Select changes the current version of
that name only when that exact version is already registered under the name.
Old definitions remain registered, so a later selection can roll back. A fork
folds its inherited prefix before its own records.

`version` is lowercase SHA-256 hex of the compact JSON tuple
`[name, source, signatures, dependencies]`. Object keys in `signatures` are
sorted recursively; dependencies are sorted by name. Folding recomputes the
digest and skips a record whose version differs. Neither time nor randomness
enters the digest.

Names are ASCII identifiers of 1–64 bytes: a letter or underscore followed by
letters, digits, or underscores. Source is at most 64 KiB of UTF-8. Signatures
take at most 16 KiB as compact JSON. At most 32 dependencies are accepted;
each dependency version is exactly 64 lowercase hexadecimal characters.
The library holds at most 128 distinct versions and 1 MiB of serialized
definition JSON across registered versions. Invalid, malformed, and oversized
records leave the folded library unchanged. Records are folded oldest first.

`modules::fold` accepts the plugin's JSON records. `store::fold` continues to
return only script store values. The UI state serializes a default-empty module
library alongside the script store for replay and resume.

`tests/modules.rs` checks digest normalization, content changes, validation,
quotas, rollback, JSON round trips, and ordered prefix/fork behavior. Its Hegel
property generates valid define/select histories without rejection and compares
every prefix with an independent `BTreeMap` version and alias oracle. The
workspace `hegel.toml` provides development and CI profiles; no additional
per-test case count is needed.
