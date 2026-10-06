# tau-codemode-eval

Deterministic workloads and oracles for Codemode's capability
evaluation. It runs fixed Luau programs and modules in the real Codemode
VM over the real coding tools, then grades the results against answers
written by hand, not computed by the code under test. Nothing here calls
a provider: the evaluation is offline and gives the same report on every
run.

## What it measures

The workloads (`fixtures`) are files plus an `expected` answer: a search
over paths with Unicode, colons and spaces; a long test log with
failures at its start, middle and end; a set of breaking semantic
changes beside a nonbreaking distractor; and a repeated workflow whose
config input changes. Large logs put evidence where a head or tail
display cannot reach it, so only full access finds it all. Only the
grading step reads `expected`.

The default report (`runner::EvalReport`, `format_version: 1`) has six
cases: each of the search, log and semantic workloads run once with the
display-only text tools (`text_only_reference`) and once with the
improved path (`structured_tools`, or `scripted_infer_simulator`, a
scripted `infer` that answers only exact inputs it has a record for).
Each `CaseReport` says whether the result is correct and complete, its
tool calls, simulated round trips, repairs and failure.

With `--matrix`, the report adds a `matrix` member
(`matrix::MatrixReport`). It runs original and changed inputs through
the policy cases and through an `Agent` with `ScriptedModel`, production
`Codemode` and the coding tools, which defines, tests, selects and
reuses a module. It reports per-stage call counts, amortization over
reuse runs, and scope checks on artifact grants. `--module-manifest`
adds one module whose exact source the caller supplies.

Provider metrics (attempts, round trips, usage, cost) are zero or null
in every report: no model is asked.

## How it fits

It builds on `tau-codemode` and `tau-codemode-host` (the VM and the
plugin under test), `tau-tools-host` (the `grep` and `read` tools),
`tau-artifacts`, `tau-agent`, `tau-ai`, `tau-store`, `tau-store-sqlite`
and `tau-testing`. No other crate depends on it.

## Usage

```sh
cargo run -p tau-codemode-eval -- --output report.json
cargo run -p tau-codemode-eval -- --matrix --output matrix.json
```

| Option                   | Effect                                                              |
| ------------------------ | ------------------------------------------------------------------- |
| `--output PATH`          | Write pretty JSON to `PATH`; otherwise to standard output           |
| `--timings`              | Include observed VM wall milliseconds; they vary by machine         |
| `--matrix`               | Add the matrix of policy cases and the Agent-owned reference module |
| `--module-manifest PATH` | With `--matrix`, add one externally supplied module source          |
| `--live`                 | Rejected: no guarded provider transport exists                      |

`--live` also requires all four budget flags, `--max-provider-attempts`,
`--max-usd`, `--max-seconds` and `--max-output-tokens`, each positive;
even then it fails closed and opens nothing. Budget flags without
`--live` are rejected, and so is `--module-manifest` without `--matrix`.
No credentials are needed.

## Testing

```sh
cargo nextest run --release -p tau-codemode-eval
```

The tests compare the actual report with the checked-in goldens,
`tests/goldens/offline.json` and `tests/goldens/matrix.json`, so a
change to the corpus, the scripts or Codemode's output shows up as a
golden diff. They also check changed inputs, broken or stale external
modules, manifest validation and the CLI's live gate. `tests/fixtures.rs`
holds Hegel properties over generated corpora, governed by the
workspace's `hegel.toml`.

## Further reading

- [codemode-evaluation.md](../../../docs/reference/codemode-evaluation.md):
  the CLI, the matrix modes, the module manifest, every report field and
  the expected regressions
- [codemode.md](../../../docs/reference/codemode.md): Codemode itself
- [0018-codemode-and-mcp.md](../../../docs/decisions/0018-codemode-and-mcp.md)
