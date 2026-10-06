# VCS handoff and conflict properties

The tests use Hegel 0.48.1, scripted models, local fixture repositories,
SQLite, and the production jj-lib workspace APIs. They make no provider
requests. Run each Store on one enabled-I/O runtime.

## Property Inventory

### Finalization

| Test                                                          | Oracle                                                   | Responsibility                                                                                                                                                |
| ------------------------------------------------------------- | -------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `finalization_lands_exact_bytes_or_keeps_the_child_workspace` | Independent path-to-bytes map and handoff state          | A child either transfers all generated committed and pending files, or transfers nothing and retains its workspace, bookmark, and durable finalization error. |
| `oversized_untracked_child_files_are_not_discarded`           | Fixed native snapshot-size boundaries and original bytes | Files omitted by jj-lib's size limit also prevent handoff and remain recoverable.                                                                             |
| `landing_keeps_a_chat_whose_final_commit_failed`              | Fixed file bytes and unchanged trunk                     | Manual preview and landing cannot delete edits after an unusable final commit message.                                                                        |
| `retained_sub_agents_stay_open_at_every_call_depth`           | Exhaustive retention flag × call-depth table             | A sub-agent a direct or nested `wait` lists as retained stays open in the UI, without a close action or navigation away; one it landed closes.                |

### Conflicts

| Test                                                                                                  | Oracle                                                                          | Responsibility                                                                                                                                                                                                                                                                                                   |
| ----------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `tree_values_match_the_signed_term_model`                                                             | Independent signed-term map, plus cancellation/reordering and metadata partners | Native delta comparison ignores redundant pairs and addend order without ignoring file presence, blob identity, executable bits or copy provenance.                                                                                                                                                              |
| `snapshots_keep_conflicts_until_changed_or_explicitly_resolved`                                       | Constructed file-presence/bytes histories and exact unresolved sets             | Resolving one file does not decide its sibling; unrelated commits, reopen, same-byte writes, and transient delete/recreate preserve remaining conflicts. Explicit acceptance and committed-side restore select only named paths; mixed marked/clean acceptance fails atomically, and undo restores the conflict. |
| `runs_behave_like_the_model`, `chats_land_like_the_model`, `chats_follow_an_update_like_the_model`    | Independent merge-term/history model                                            | Stored conflict sets and changed-path lists follow the reference model. No observed SUT conflict set changes expected state.                                                                                                                                                                                     |
| `a_markerless_landed_conflict_can_be_explicitly_accepted`, `a_turn_keeps_a_conflict_it_did_not_touch` | Retained literal regressions                                                    | Markerless contents remain unresolved through same-byte and transient rewrites; accepting them is explicit. Resolving another path does not list or resolve the untouched conflict.                                                                                                                              |
| `accepting_markerless_contents_keeps_native_executable_choice`                                        | Exhaustive base/child executable-bit table in native modified/deleted histories | Explicit acceptance retains native materialized bytes and mode, including its non-executable default for a mode conflict.                                                                                                                                                                                        |
| Shared path/cancellation and immutable-working-copy checks                                            | Finite authorization boundaries                                                 | The new resolution tool retains the writing tools' path confinement, cancellation, immutable refusal, sequential scheduling, and read-only exclusion.                                                                                                                                                            |

## Generator Plan

The finalization property generates zero to two prior commits, zero to
three extra writes, bounded Unicode/newline contents, and either a normal
stop or a turn limit. A mandatory pending write remains after shrinking.
Every history exercises empty and whitespace descriptions, a fatal
provider response, a successful description, and a real jj-lib commit
refusal: a test-only plugin tags the snapshotted working copy before
finalization. No commit result is mocked. File expectations come from
the generated operations, not the VCS implementation.

Shrinking removes prior commits and extra writes, then minimizes the
pending suffix and stop mode. Permanent explicit cases retain the original
empty-answer failure and a limited child with earlier committed work.
Failure blobs are for short-lived replay, not permanent regressions.

The pure native-term comparison has 200 cases: two independently drawn
odd-length term sequences, cancelling-pair and addend-reordering partners,
and a mandatory nontrivial swapped-addend witness. An independent signed
map is the oracle; new-blob, presence, mode and provenance partners remain
changes. Shrinking reduces sequence lengths and identities without
removing the fixed structural witness.

The native history property has 20 cases. Its minimum structure contains
an empty-base file that the parent deletes and child modifies, plus a
nonempty-base file that the parent empties and child deletes. The child
has two commits: edits first, deletions second. A flat single-commit
fixture did not reproduce the redundant-term regression.

The generator adds zero to two independent conflict kinds, up to five
operations, bounded Unicode/newline contents, and an explicit-acceptance
choice. There are no rejected cases. Shrinking removes extra paths and
operations but keeps the two-file, two-commit witness. Permanent explicit
cases cover the minimal witness and additions, real markers with literal
marker-like source lines, binary bytes, transient rewrites, and reopen.

The snapshot model compares final bytes to its last observed reference
tree. A delete/recreate sequence before a snapshot is not an intermediate
resolution. Native results are assertion inputs, never reference-state
updates.

## Rust Code

The complete implementations are in
[`sub_agent_finalization.rs`](../../crates/plugins/tau-vcs-host/tests/sub_agent_finalization.rs),
[`conflict_snapshots.rs`](../../crates/plugins/tau-vcs-host/tests/conflict_snapshots.rs),
[`runs_model.rs`](../../crates/plugins/tau-vcs-host/tests/runs_model.rs), and
[`diff.rs`](../../crates/plugins/tau-vcs-host/src/diff.rs).
The independent history algebra is in
[`common/merge.rs`](../../crates/plugins/tau-vcs-host/tests/common/merge.rs).

### Native implementation boundary

The production handoff reads jj-lib's native snapshot and oversized
untracked-file statistics before landing. A failed commit is recorded;
a normal or limited run outcome alone is not evidence of committed work.

Snapshots, merge resolution, conflict materialization, tree construction,
transactions, checkout, path matching, and undo remain jj-lib operations.
Tau does not parse markers, rewrite working-copy state, or implement a
merge engine. Diff comparison flattens the native delta
`before - after + absent` and uses `Merge::simplify()` to cancel terms.
This handles both redundant pairs and reordered addends without custom
counting or sorting in production. Explicit acceptance uses
`try_materialize_file_conflict_value`, `files::merge_hunks`, and
`MergedTreeBuilder`. Committed-side selection uses native `restore_tree`.

The original untouched-path probe checked only its reported paths. The
follow-up native-state probe showed that the conflict remained unresolved:
five terms became three after cancelling a redundant pair. The stronger
history model also found a reordered-addend case; a named catch-up
regression retains it. The defect was a representation-only change being
reported as an edited path, not proven implicit resolution. The retained
tests now check both paths and conflict state.

## CI Configuration Guidance

### Execution and replay

```sh
nix develop -c cargo nextest run --release -p tau-vcs-host --lib tree_values_match_the_signed_term_model
nix develop -c cargo nextest run --release -p tau-vcs-host --test sub_agent_finalization
nix develop -c cargo nextest run --release -p tau-vcs-host --test conflict_snapshots --test runs_model
nix develop -c cargo nextest run --release -p tau-ui --test host --test workspace
```

The shipped `hegel.toml` profiles are unchanged. Local failures use Hegel's
example database. CI automatically uses fixed seeds and no example database; retain
its printed reproduction blob to replay with the same Hegel version. For
an additional fixed-seed local run:

```sh
HEGEL_DERANDOMIZE=true HEGEL_DATABASE=disabled \
  nix develop -c cargo nextest run --release -p tau-vcs-host --test sub_agent_finalization
```

The owned Agent/Store/jj property has 12 cases because each history runs
five complete native handoffs; it is not a pure string property. The UI
flags are finite and exhaustive, not claimed as a speed improvement.

For disk-constrained Rust verification, use one shared target directory,
run suites sequentially, and set `CARGO_INCREMENTAL=0`,
`CARGO_PROFILE_DEV_DEBUG=0`, and `CARGO_PROFILE_TEST_DEBUG=0`. These build
settings do not reduce property cases or change the shipped CI profiles.

### Checked result

The combined fixes passed 438 tests: 139 VCS, 146 UI, and 153 Agent tests.
Seven existing opt-in tests remain ignored. The VCS total includes the
final executable-choice table added after the full suite and checked with
the conflict history property. Strict all-target Clippy, the UI build,
supported host-disabled VCS/remote-UI library Clippy, and canonical
formatting passed. No live-provider or performance claim is made.
