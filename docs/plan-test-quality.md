# Property-test quality work

Status: implementation in progress. The source audit selected 40 of 392 Hegel
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
- [ ] **20 — Goal commands:** consolidate parsed/general condition laws without rejection (#29).
- [ ] **21 — Inference and slugs:** schema/value acceptance pairs and exact word/length models (#35–38).

## Validation

Canonical formatting is `nix fmt`; Rust checks run through `nix develop`.
Affected tests and strict Clippy run before each commit. Common/UI changes
also receive host-disabled library checks where supported. Filesystem and
Store tests use enabled I/O; a SQLx Store stays on one persistent runtime.
No live provider calls or pushes are required or authorized by this work.

The finished record will distinguish executed checks/probes from proposed
ones. It will not claim exhaustive mutation coverage or unmeasured savings.

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

Shared Cargo target artifacts can have newer timestamps than sources copied
into another workspace. Checks refresh local Rust source timestamps before
building; ambiguous probe artifacts require a clean rebuild. This changes no
source content. The cost probe required such a rebuild.
