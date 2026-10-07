#!/bin/sh
# Mutation testing on the modules where a silent bug costs the most
# (docs/reference/testing.md, "Mutation testing"). Run it by hand, from
# the repository's root, in the dev shell; extra arguments go to cargo
# mutants (`--shard 1/4`, `--in-diff changes.diff`, `-j 4`). A full pass
# is about 1400 mutants. Not in CI: on a hosted runner that is about
# eight hours.
#
# cargo mutants builds a copy of the tree with its own target
# directory; a shared CARGO_TARGET_DIR would leave mutated artifacts
# behind for later builds.
unset CARGO_TARGET_DIR
exec cargo mutants --workspace --test-tool nextest \
  --file crates/tau-ai/src/ws/proto/continuation.rs \
  --file crates/tau-ai/src/ws/proto/lane.rs \
  --file crates/tau-ai/src/ws/proto/pool.rs \
  --file crates/tau-ai/src/responses/input.rs \
  --file crates/tau-ai/src/event.rs \
  --file crates/tau-agent/src/runner.rs \
  --file crates/tau-agent/src/plugin.rs \
  --file crates/tau-agent/src/agent.rs \
  --file crates/tau-agent/src/validation.rs \
  --file crates/tau-agent/src/schema.rs \
  --file crates/plugins/tau-compaction/src/lib.rs \
  --file crates/plugins/tau-compaction/src/plugin.rs \
  --file crates/plugins/tau-fast-compaction/src/state.rs \
  --file crates/plugins/tau-fast-compaction/src/decide.rs \
  --file crates/plugins/tau-fast-compaction/src/ledger.rs \
  --file crates/plugins/tau-fast-compaction/src/lib.rs \
  --file crates/plugins/tau-jev/src/lib.rs \
  --file crates/plugins/tau-tools-host/src/edit.rs \
  --file crates/plugins/tau-tools-host/src/truncate.rs \
  --file crates/plugins/tau-tree-compaction/src/tree.rs \
  --file crates/plugins/tau-tree-compaction/src/build.rs \
  --file crates/plugins/tau-tree-compaction/src/plugin.rs \
  --file crates/tau-store-sqlite/src/lib.rs \
  "$@"
