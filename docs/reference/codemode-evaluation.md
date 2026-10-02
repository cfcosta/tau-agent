# Codemode evaluation

`tau-codemode-eval` runs a fixed offline corpus through the production codemode
VM. Its host calls the production `tau-tools` `grep` and `read` implementations
on staged fixture files. The semantic mode uses a labeled, scripted `infer`
simulator with a handcrafted response record and the production `InferRequest`
schema validator and fixed infer reply shape. It does not exercise the Agent
inference transport, durable budget or private trace pipeline; those have
separate integration tests. No provider is opened by the offline command.

## Commands

From the repository root:

```sh
nix develop -c cargo run -p tau-codemode-eval --bin tau-codemode-eval
nix develop -c cargo run -p tau-codemode-eval --bin tau-codemode-eval -- --output /tmp/codemode-evaluation.json
nix develop -c cargo run -p tau-codemode-eval --bin tau-codemode-eval -- --timings
```

The default writes a pretty-printed `EvalReport` JSON object to standard
output. `--output PATH` writes the same bytes, ending in a newline, to `PATH`.
The report is deterministic for the fixed corpus. The golden is
`crates/evals/tau-codemode-eval/tests/goldens/offline.json`.
`--timings` fills each case's `latency_ms` with the production VM outcome's
observed wall time. It changes `evidence` from `offline_deterministic_scripts`
to `offline_observed_host_runtime`; that report varies with the host and does
not match the deterministic golden.

## Modes and evidence

| Mode                       | Program and evidence                                                                                                                                                                                                                                                                    |
| -------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `text_only_reference`      | Assistant-scripted Luau programs consume the tools' display text. Search parses ordinary `path:line: text` records, including a colon in a path. The log program reads one display window and marks the partial result incomplete. The semantic program is a task-specific text parser. |
| `structured_tools`         | Assistant-scripted Luau programs consume production structured `grep` records and `complete` flags. Search transforms records with codemode `map`; log filters all failure lines with grep.                                                                                             |
| `scripted_infer_simulator` | An assistant-scripted Luau program reads structured text, then calls a scripted `infer` tool. The simulator accepts one exact document and task, validates its response against the supplied schema, and rejects changed documents.                                                     |

The fixture `expected` value is read only when grading the completed VM
output. Programs and simulator responses are fixed independently of that
value. `correct` requires an exact answer and `incomplete == false`; a partial
answer is not credited as success. A changed fixture answer in the wrong-answer
test is detected. The golden test compares actual VM and tool output to a
checked-in reference JSON report.

The text-only mode is a display-policy reference, **not** an upper bound on
older codemode capability. An older program could use `bash`/`grep` or page
`read` to inspect the complete log despite a single truncated display. A
difference between these fixed programs establishes their behavior on these
fixtures; it does not establish that older codemode could not solve them.

## Report fields

`format_version` is `1`. `cases` contains one row per fixture and mode.

| Field                                                           | Meaning                                                                                                                                                                                                                                                    |
| --------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `fixture`, `mode`, `result`, `correct`, `incomplete`, `failure` | Case identity, decoded VM answer, grading, partial-state flag, and VM/tool error.                                                                                                                                                                          |
| `tool_calls`                                                    | Observed nested codemode call count, including scripted `infer`.                                                                                                                                                                                           |
| `provider_round_trips`                                          | Actual provider attempts observed; always zero offline.                                                                                                                                                                                                    |
| `simulated_round_trips`                                         | Observed calls named `infer` to the scripted semantic simulator; each call is counted separately from provider attempts.                                                                                                                                   |
| `repairs`                                                       | Observed argument or schema repairs; zero in these fixed programs.                                                                                                                                                                                         |
| `latency_ms`                                                    | `null` by default. With `--timings`, the production VM's `Outcome.wall` rounded down to milliseconds, from an `Instant` spanning script execution and awaited tool I/O. It excludes fixture staging and report serialization. Small runs can round to `0`. |
| `reported_usage`                                                | Separate uncached input, cached input, cache-write, output, and USD fields. Each is `null` offline because no provider reported usage.                                                                                                                     |

The default report provides deterministic correctness evidence. The optional
timed report adds nondeterministic host runtime evidence, not provider latency.
Neither report measures provider token savings, cost savings, or latency
savings. Prompt byte counts
and scripted answers are not substituted for provider usage. Repository,
artifact, and module-amortization experiments are outside this corpus.

## Live gate

`--live` requires positive values for `--max-provider-attempts`, `--max-usd`,
`--max-seconds`, and `--max-output-tokens`. A complete guarded provider
transport is not installed, so a fully budgeted `--live` invocation returns
`live evaluation is unsupported` before constructing a provider. There is no
automatic live fallback.

## Checks

From the workspace:

```sh
nix develop -c cargo test -p tau-codemode-eval -p tau-codemode -p tau-tools
nix develop -c cargo clippy -p tau-codemode-eval --all-targets -- -D warnings
nix develop -c cargo check -p tau-codemode-eval --no-default-features
```
