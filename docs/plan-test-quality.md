# Property-test quality work

Status: complete. All 21 implementation batches have checked Jujutsu commits.
The source audit selected 40 of 392 Hegel
property entry points (10.2%). Ordinary example tests are outside this pass.
The replacements use behavioral laws, independent oracles, valid constructed
inputs, readable shrinking, and deterministic execution. Known regressions
remain. A lower test count or a speedup is not itself the goal.

Each batch receives its own checked Jujutsu commit. Replacements land before
old coverage is removed. Focused fault probes run against the intended
replacement; probe results are distinct from full-suite mutation scores.

## Batches

Audit numbers refer to the approved 40-test shortlist.

- [x] **01 — MCP names:** independent plain/collision/length naming oracle (#2).
- [x] **02 — Recovery:** independent status/body/precedence expectations (#3).
- [x] **03 — Token estimates:** independent Unicode/image/per-message accounting (#4).
- [x] **04 — Store UI:** compare durable and UI folds to an operation model (#5).
- [x] **05 — Request frames:** preserve original generated maps through both encodings (#6).
- [x] **06 — Costs:** combine independent formula/apply/increment coverage and exhaust zero usage (#7, #21, #39).
- [x] **07 — Schema generators:** replace compile-only coverage with valid/invalid value laws (#8).
- [x] **08 — Evaluator and fixtures:** execute real owned module/tools/artifact paths on generated inputs; retain corpus-integrity checks (#1, #30–34).
- [x] **09 — Markup:** require exact generated span text and marks (#9).
- [x] **10 — Signatures:** verify semantic type structure as well as Luau syntax (#10).
- [x] **11 — Module imports:** dependency/version/import histories and initialization counts (#11).
- [x] **12 — Fake module tests:** ordered fixtures, exact initialization and failure propagation (#12).
- [x] **13 — Services:** clone/mutate/query histories against immutable snapshots (#13).
- [x] **14 — Paths:** exact independent path model with idempotence secondary (#14–15).
- [x] **15 — Retry rules:** exhaustive statuses, richer known/unknown header generation and preserved boundary regressions (#16–20).
- [x] **16 — Compaction UI:** exhaustive activation and exact decision labels (#22–23).
- [x] **17 — Reasoning/goal UI:** exhaustive activation combinations (#24–25).
- [x] **18 — Callback defaults:** explicit configured/listening port boundaries (#26).
- [x] **19 — Session reasoning:** model-class tables and change/no-op/unset histories (#27–28, #40).
- [x] **20 — Goal commands:** consolidate parsed/general condition laws without rejection (#29).
- [x] **21 — Inference and slugs:** schema/value acceptance pairs and exact word/length models (#35–38).

## Validation

Canonical formatting is `nix fmt`; Rust checks run through `nix develop`.
Affected tests and strict Clippy run before each commit. Common/UI changes
also receive host-disabled library checks where supported. Filesystem and
Store tests use enabled I/O; a SQLx Store stays on one persistent runtime.
No live provider calls or pushes are required or authorized by this work.

Merged validation passed:

- 1,147 library/integration tests across the 13 affected crates; 8 existing
  opt-in tests ignored.
- 25 `tau-ui` library tests: **1,172 passed in total**.
- Canonical formatting and affected-crate all-target strict Clippy.
- Host-disabled library strict Clippy for `tau-codemode`, `tau-tools`, and
  `tau-ui-plugin`; `tau-ui` compilation.
- 24 deliberately injected faults detected by the named replacements below.

The final source census is 379 Hegel attributes and 1,237 ordinary
Rust/Tokio/SQLx attributes. These count entry points, not generated cases or
executed tests. The 40 audited responsibilities remain covered through stronger
properties, consolidated models, or exhaustive tables. No production behavior,
dependencies, evaluation goldens, or unrelated regressions changed.

This is not a full mutation campaign or runtime comparison. The larger checked
suite does not establish a speedup, mutation-score increase, or provider savings.
Ordinary example tests had a pass of their own afterwards: see
[Second pass: example tests](#second-pass-example-tests).

### CI and failure replay

Keep the shipped `hegel.toml` settings: development locally and fixed-seed CI
without an example database. Generated domains are bounded in the test
inventories; slow owned VM/session properties have explicit 12/24-case budgets.
Do not substitute timing thresholds or disabled-I/O runtimes. Keep a Store on
one persistent enabled-I/O runtime.

Run affected library/integration tests through `nix develop -c cargo test
-p CRATE --lib --tests`, and strict checks through `nix develop -c cargo clippy
-p CRATE --all-targets -- -D warnings`. For host-disabled builds, check libraries
only, not existing host-only integration targets. Run `nix fmt` before committing.
Use Hegel's reported shrunk inputs and the focused test name to replay failures;
retain confirmed regressions as explicit cases. See [Testing](reference/testing.md)
for the repository-wide profiles and replay conventions.

### Executed focused fault probes

These are individual injected faults in disposable workspaces, not a full
mutation campaign. Only failure of the named replacement counts as detection.

| Batch | Injected fault                                       | Replacement that failed                                               |
| ----- | ---------------------------------------------------- | --------------------------------------------------------------------- |
| 01    | Plain-name prefix loses one underscore               | `short_ascii_pairs_keep_exact_plain_names`                            |
| 02    | Recovery treats HTTP 429 as a request fault          | `status_boundaries_apply_to_every_unrecognized_body_shape`            |
| 03    | Character estimation counts UTF-8 bytes              | `context_estimate_rounds_semantic_characters_per_unreported_message`  |
| 04    | Durable writes stop applying deletes                 | `store_and_ui_folds_match_the_operation_model_at_each_prefix`         |
| 05    | Both encoders receive a decoder that discards input  | `a_json_body_round_trips`                                             |
| 06    | Cost application overwrites reported total tokens    | `cost_matches_the_reference_formula` (clean rebuild)                  |
| 07    | Argument validation accepts every coerced value      | `removing_a_required_leaf_is_rejected_at_its_key_path`                |
| 08    | Native grep ignores the Rust glob                    | `reference_module_search_matches_sorted_rust_line_scan`               |
| 08    | Artifact pages replace `é` with ASCII `e`            | `owned_utf8_artifact_pages_reconstruct_source_bytes`                  |
| 09    | Markup parser treats all nonempty text as plain      | `spans_recognize_generated_bold_and_code_sections`                    |
| 10    | Supported object schemas always become `any`         | `supported_schemas_preserve_recursive_types_and_local_references`     |
| 11    | Exact imports bypass the initialized-export cache    | `generated_exact_definition_graphs_preserve_values_pins_and_vm_cache` |
| 12    | Fake tools stop checking exact arguments             | `ordered_fake_calls_match_the_fixture_transcript_model`               |
| 13    | Cloning services drops all typed values              | `cloned_snapshots_isolate_typed_writes_and_keep_last_values`          |
| 14    | Every nonempty path resolves to `/`                  | `resolves_generated_paths_to_the_modeled_normalized_absolute_path`    |
| 15    | Millisecond retry headers are interpreted as seconds | `parse_retry_after_is_a_trimmed_whole_number`                         |
| 16    | Dropped results display as kept                      | `each_decision_has_its_exact_card_label`                              |
| 17    | Goal UI enables the plugin in subagents              | `starting_checks_and_plugin_presence_follow_key_and_run_kind`         |
| 17    | Reasoning UI ignores a manual effort                 | `startup_status_and_plugin_presence_follow_key_and_effort`            |
| 18    | Redirect URIs hard-code port 7                       | `the_default_callback_binds_loopback_and_uses_the_listener_port`      |
| 19    | Session effort changes are ignored                   | `session_reasoning_history_follows_effective_wire_fields`             |
| 20    | Goal input never quotes ambiguous conditions         | `any_condition_reads_back_from_the_models_input`                      |
| 21    | Inference answers bypass original-schema validation  | `accepts_generated_integers_without_coercing_numeric_strings`         |
| 21    | Every nonempty title becomes `note`                  | `slug_matches_the_independent_whole_word_prefix`                      |

Shared Cargo target artifacts can have newer timestamps than sources copied
into another workspace. Checks refresh local Rust source timestamps before
building; ambiguous probe artifacts require a clean rebuild. This changes no
source content. The cost probe required such a rebuild.

## Second pass: example tests

Status: complete (2026-10-15). Six audits read every test the release suite
runs, 2,210 in all, and nominated 354 weak ones with a severity from 1 to 5.
The cut was severity 3 and up: 237 tests, 10.7%. Each one was deleted when a
property or model already checked it (the covering test was read again before
deleting), merged into the property that covers it, or replaced by a property
with an oracle of its own. A cluster of examples of one law was done whole.

229 test functions went and 20 came; the suite runs 2,005 tests. What went:

- `Display` and `Debug` pins, setter read-backs and constants checked against
  themselves.
- Examples a property or model already checks: most of `tau-vcs-host`'s tool,
  landing, sub-agent and project examples, `tau-tools-host`'s pi edit and
  listing examples, `tau-ai`'s ChatGPT store, HTTP and cost examples.
- Clusters of one law, now one property each: leaf coercion against pi's
  table, side asks against an attempt model, text phases, the summary
  request, agent names, relative times, bounded seen-key sets, glob matching,
  the restored inference budget, Luau formatting, number and clock formats,
  note and card toggles, idle compaction.
- Wall-clock waits: the MCP progress timeout and the memory plugin's commit
  marks run on a paused or injected clock, and the sandbox tests check peak
  concurrency without elapsed-time bounds. The find and grep tests of a global
  gitignore set `GIT_CONFIG_GLOBAL` instead of reading the person's.

Ten nominated tests stay, each the only test of its behaviour or needing a
seam in production code:

- `tau-schedule` `the_system_clock_has_an_offset`, the only run of
  `SystemClock::offset`'s `localtime_r`.
- `tau-tree-compaction` `the_builders_set_their_fields`, the only test of
  `view_bytes()` and `jobs()`.
- `tau-codemode-host` `a_call_that_never_ran_says_why` (the never-ran branch of
  `ui::failure`) and `infinite_loop_times_out_with_a_persisted_failure` (the
  only test that `module_test` has a time limit, which is not injectable).
- `tau-ui` `reduced_motion_stops_onboardings_loops`,
  `settings_start_with_phones_off`, and three slow ones that need a clock seam:
  `what_waits_lands_after_a_main_turn_that_finishes_slowly` and the two
  reasoning-timer tests, which read `std::time::Instant`.
- `tau-remote` `nothing_listening_is_unreachable`, the only test of
  `ServerHandle::stop`.

78 injected faults, one or more per new or strengthened property, each failed
the test named for it and was reverted. Several were chosen so that the old
examples would pass and only the new assertion catches them: an assistant
message counted by its first block only, `cap` counting bytes instead of
characters, ties between a write and a commit in the same millisecond,
refreshed OAuth tokens not saved, and a declined promotion activating.

Open findings:

- `worth_compacting` (idle compaction in `tau-ui`) compares against
  `window / 2` in integers, so for an odd window less than half counts. The
  property draws even windows only until the rule is decided.
- `read_equals_pis_whole_file_read` rarely reaches the 2,000-line cut: a
  fault in `truncated_by: "lines"` survived it, and only
  `structured_text_tracks_exact_and_exceeded_bounds` caught it.
- `sp()`, `stagger()` and `SetupStep::stage()` have no test left; the old ones
  restated their formulas.
- `tau-constitution`'s rule edits are tested from `tau-constitution-host`, so
  `cargo mutants -p tau-constitution` alone shows survivors there.

### Round two

Status: complete (2026-10-16). The same six audits read the suite again,
2,005 tests, and nominated 280 with fresh severities. Little was plainly
wasted this time: 38 rows reached severity 3 and 189 were severity 2. The cut
was severity 2 and up, 227 tests, 11%, with clusters done whole and the same
rules as the first round.

159 test functions went and 41 came; the suite runs 1,886 tests. Besides
redundant examples, the round went after tests that could not fail:

- `concurrent_writes_never_interleave` passed without the per-path lock: one
  write of a few KB is one system call. `concurrent_writes_run_one_after_another`
  checks that each call's diff starts from what the one before it left, and
  fails with the lock removed.
- `read_equals_pis_whole_file_read` never reached its 2,000-line cut; its
  generator now draws line counts and offsets around it.
- Assertions the input already met: `contains("dep")` held through
  "dependency", `contains("missing")` matched the model's own tool arguments,
  and `contains("\"maxItems\":4")` matched another field with the same limit.
- A fold whose comment promised "folding the stored records again gives the
  same state" now asserts it, and grep, find and ls check every generated
  result against their output schemas, without coercion.
- Wall-clock waits gave way to the events they stood for: a fake direnv waits
  for a file the test writes, exec's "not told" tests wait for the watcher to
  end, a too-long hook uses the 200 ms card limit instead of the 2 s one, and
  the system clock's offset is checked under fixed time zones in child
  processes.

Eight rows stay, each the only test of its behaviour or needing a production
seam: `jitter_samples_the_unit_interval`, the
host's `Act::Prompt` reply path and the older protocol's `prompts/get` in
`tau-mcp-host`, `every_demo_screen_opens`, the no-AGENTS-file host test,
`Type::mono()`'s only test, the clone into a missing parent with a token, and
the write debounce in `tau-agents-host`.

Fault probes: about 120, each caught by the test named for it, except two
noted by their agents: an artificial wrap in the backoff exponent at attempts
past 1,000, and an equivalent `>`/`>=` at the skip cap. Several probes were
only caught after a generator learned to draw a rare shape (several values in
one list, a century turn, leading white space before a bare query), which is
now a rule in [Testing](reference/testing.md#generators).

Two agents' probes once restored each other's backups through a shared file,
and one wrote into the default workspace; both were caught before anything was
committed, and the guide now says how to revert a probe.

Findings:

- **Fixed:** `chat_text` cut only the first `ultra` of a word, so
  `Ultraultrathink` reached the composer as a live keyword (`tau-watcher`).
- **Fixed, to match pi:** a signature id was shortened when longer than 64
  bytes (`tau-ai/src/responses/input.rs`), where pi counts JavaScript string
  length, so 33 × `é` was kept by pi and shortened by tau. It now counts
  UTF-16 code units, and `a_signature_id_is_kept_up_to_64_utf16_units` checks
  it.
- An optional `null` inside an object held under `additionalProperties` is
  coerced, not dropped: `normalize_optional_nulls` does not walk
  `additionalProperties` (`tau-agent`).
- `field()` in `tau-agents/src/event.rs` is not idempotent: an id holding
  `· ·` keeps a `·` after the replace and breaks its line.
- A `/goal` prompt's branch slug comes from the goal's first line only.
- `Commit::label` (`tau-vcs`) has no test of its own; a demo test is its only
  coverage.
- The intro in [Codemode](reference/codemode.md) "The description" is older
  than `tau_codemode::description::INTRO`, so only the closing paragraph is
  pinned to the doc.
