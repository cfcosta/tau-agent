# VCS handoff and conflict properties

The tests use Hegel 0.48.1, scripted models, local fixture repositories,
SQLite, and the production jj-lib workspace APIs. They make no provider
requests. Run each Store on one enabled-I/O runtime.

## Finalization inventory

| Test                                                          | Oracle                                                   | Responsibility                                                                                                                                                |
| ------------------------------------------------------------- | -------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `finalization_lands_exact_bytes_or_keeps_the_child_workspace` | Independent path-to-bytes map and handoff state          | A child either transfers all generated committed and pending files, or transfers nothing and retains its workspace, bookmark, and durable finalization error. |
| `oversized_untracked_child_files_are_not_discarded`           | Fixed native snapshot-size boundaries and original bytes | Files omitted by jj-lib's size limit also prevent handoff and remain recoverable.                                                                             |
| `landing_keeps_a_chat_whose_final_commit_failed`              | Fixed file bytes and unchanged trunk                     | Manual preview and landing cannot delete edits after an unusable final commit message.                                                                        |
| `retained_delegates_stay_open_at_every_call_depth`            | Exhaustive retention flag × call-depth table             | Direct and nested failed handoffs remain open in the UI, without a close action or navigation away.                                                           |

The property generates zero to two prior commits, zero to three extra
writes, bounded Unicode/newline contents, and either a normal stop or a
turn limit. A mandatory pending write remains even after shrinking. Every
history exercises empty and whitespace descriptions, a fatal provider
response, a successful description, and a real jj-lib commit refusal: a
test-only plugin tags the snapshotted working copy before finalization.
No commit result is mocked. File expectations come from the generated
operations, not the VCS implementation.

Shrinking removes prior commits and extra writes, then minimizes the
pending suffix and stop mode. The permanent explicit cases include the
original empty-answer failure and a limited child with earlier committed
work. Failure blobs are for short-lived replay, not permanent regressions.

The production handoff reads jj-lib's native snapshot and its oversized
untracked-file statistics before landing. A failed commit is recorded;
a normal or limited run outcome alone is not evidence of committed work.

## Execution and replay

```sh
nix develop -c cargo test -p tau-vcs --test delegate_finalization
nix develop -c cargo test -p tau-ui --test host --test workspace
```

Keep the shipped `hegel.toml` profiles. Local failures use Hegel's example
database. CI automatically uses fixed seeds and no example database; retain
its printed reproduction blob to replay with the same Hegel version. For
an additional fixed-seed local run:

```sh
HEGEL_DERANDOMIZE=true HEGEL_DATABASE=disabled \
  nix develop -c cargo test -p tau-vcs --test delegate_finalization
```

The owned Agent/Store/jj property has 12 cases because each history runs
five complete native handoffs; it is not a pure string property. The UI
flags are finite and exhaustive, not claimed as a speed improvement.

For disk-constrained Rust verification, use one shared target directory,
run suites sequentially, and set `CARGO_INCREMENTAL=0`,
`CARGO_PROFILE_DEV_DEBUG=0`, and `CARGO_PROFILE_TEST_DEBUG=0`. These build
settings do not reduce property cases or change the shipped CI profiles.
