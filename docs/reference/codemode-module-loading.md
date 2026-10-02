# Codemode module loading

`require(name, version?)` loads a registered Luau module. `name` must be an
ASCII identifier of 1–64 bytes. `version`, when supplied, must be the exact
64-character lowercase SHA-256 digest of a registered definition. Without a
version, the host uses the selected version in this conversation's folded
module records. Unknown names, paths such as `../secret`, unknown versions,
and mismatched name/version pairs raise string errors catchable with `pcall`.

The plugin reads its conversation records once per codemode call and folds
them into a module library. Its host resolves modules only from that library;
it never treats names as file paths. The host API defaults to no modules for
embedders that do not provide a library. No module definition tools are
exposed yet.

A definition's dependency map pins each imported name to an exact digest.
The loader validates the complete dependency graph and rejects missing pins,
invalid definitions, and cycles before evaluating source. Dependencies load
first, in graph order. Inside module source and exported functions,
`require` accepts only declared names. Passing a version must match the pin;
omitting it still uses the pin, even if the selected version has changed.

Each source runs as a text chunk named `module:<name>@<version>` in the
current VM. It inherits sandbox globals and may use `tools` through the usual
host pipeline. It must return exactly one nonnil function or table. Syntax,
runtime, export, and resolution failures are string errors. The same memory
limit, interrupt, cancellation, and timeout cover the entire script and its
module sources. There are no file, process, network, or package globals.

Loaded values are cached by exact version within one VM. Concurrent imports
serialize initialization and share the same value. The cache is cleared when
the VM closes, including after timeout or cancellation. A later codemode call
starts a new VM and new mutable module heaps.

`tests/module_loading.rs` covers host resolution, pins, sandbox boundaries,
timeouts, cancellation, cache behavior, and generated arithmetic inputs.
