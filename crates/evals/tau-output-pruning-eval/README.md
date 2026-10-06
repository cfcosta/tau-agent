# tau-output-pruning-eval

The retention evaluation of fast compaction's output pruning. It checks
whether `tau-fast-compaction` keeps the lines a task needs when it trims
a long command output, how much smaller the output gets, and what Jev
costs. It runs the real plugin against the real Jev.

## What it measures

Each workload is a conversation that ends in one long command output,
generated from a seed with synthetic noise (cargo, npm, pip, pytest,
bundler and upload logs; no network). The lines the task needs, the
needles, are each in the output exactly once.

| Workload              | Needles                                                      |
| --------------------- | ------------------------------------------------------------ |
| `needle-error`        | one error line deep in a noisy cargo build                   |
| `needle-detail`       | a bundle hash the user asked to remember, among many like it |
| `summary-line`        | npm's totals line, with post-install noise after it          |
| `structured-json`     | one service's record in a large JSON registry                |
| `all-noise`           | none: pruning should cut most of a pip install               |
| `spilled-middle`      | a failed step far above the 2,000-line tail `bash` keeps     |
| `multi-needle`        | three failing tests far apart in a cargo test run            |
| `earlier-requirement` | a checksum only a large runbook read earlier asks for        |

The runner drives each workload through the agent loop as the app
prunes: a scripted model makes the workload's calls, fake tools answer,
and a fake `bash` truncates and spills like tau's. Fast compaction, with
only output pruning at work, prunes the output before the model's next
request.

Per trial (`metrics::Trial`) it records:

- recall: needles in the result the model saw, each as a whole line (a
  needle only in the archive does not count), and tail recall, the same
  for what `bash` alone would have shown;
- estimated tokens before (what `bash` alone shows) and after, the
  reduction, and the reduction against the whole output;
- whether the result was replaced, Jev requests, input tokens, cost and
  latency;
- Jev's answers by band: noise (at most 0.1), uncertain, and needed (0.5
  or more, so the chunk stays), and each chunk's largest answer, to
  replay another keep rule offline.

The library modules are `workload` (the `Kind`s and `generate`),
`noise`, `rng` (a small deterministic generator), `runner` (`Config`,
`evaluate`, `Report`) and `metrics` (`Trial`, `Summary`, `table`).

## How it fits

It builds on `tau-fast-compaction` and `tau-jev` for the system under
test. It runs them as the app does with `tau-agent`, `tau-ai`,
`tau-testing` (the scripted model), `tau-store`, `tau-store-sqlite` and
`tau-tools-host`. No other crate depends on it.

## Usage

```sh
TYPESAFE_API_KEY=… cargo run -p tau-output-pruning-eval -- \
  --seeds 3 --budget-usd 0.5 --json results.json
```

| Option             | Effect                                                                                |
| ------------------ | ------------------------------------------------------------------------------------- |
| `--workload NAME`  | Only these workloads (repeat, or comma-separated)                                     |
| `--seeds N`        | Seeds 0 to N-1 of each workload (default 1)                                           |
| `--budget-usd USD` | Stop once Jev's spend passes this                                                     |
| `--json PATH`      | Also write every trial and summary to `PATH`                                          |
| `--whole`          | `bash` returns every output whole, with no truncation or spill                        |
| `--work DIR`       | Where archives and spilled outputs go (default: a temp directory, removed afterwards) |
| `--list`           | List the workloads, and exit                                                          |
| `--help`           | Print the usage, and exit                                                             |

It needs `TYPESAFE_API_KEY`; without it, it refuses to run. Each trial's
progress goes to standard error. Standard output gets a table per
workload, the total spent, and the reason when it stopped early.

Jev costs about $0.042 per million input tokens. A workload costs about
half a cent, so three seeds of all eight cost about $0.13.

## Testing

```sh
cargo nextest run --release -p tau-output-pruning-eval
```

The tests use fake Jevs and need no key. `tests/workloads.rs` checks,
with Hegel properties, that generation is deterministic and that each
needle is where it is claimed, exactly once. `tests/runner.rs` runs the
real plugin with a ground-truth Jev, one that keeps nothing, and one
that fails, and checks the budget stop. `tests/metrics.rs` checks the
summaries against a reference.

## Further reading

- [fast-compaction.md](../../../docs/reference/fast-compaction.md):
  "Evaluation" has the workloads, the measures and the latest results
  against the real Jev
