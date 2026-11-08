# tau-output-pruning-eval

The retention evaluation of fast compaction's output pruning. It checks
whether `tau-fast-compaction` keeps the lines a task needs when it trims
a long command output, how much smaller the output gets, and what Jev
costs. It runs the real plugin, the Luau one tau comes with (ADR 0050),
against the real Jev.

## What it measures

Each workload is a conversation that ends in one long command output,
generated from a seed with synthetic noise (cargo, npm, pip, pytest,
bundler and upload logs; no network). The lines the task needs, the
needles, are each in the output exactly once.

| Workload              | Needles                                                          |
| --------------------- | ---------------------------------------------------------------- |
| `needle-error`        | one error line deep in a noisy cargo build                       |
| `needle-detail`       | a bundle hash the user asked to remember, among many like it     |
| `summary-line`        | npm's totals line, with post-install noise after it              |
| `structured-json`     | one service's record in a large JSON registry                    |
| `all-noise`           | none: pruning should cut most of a pip install                   |
| `cut-middle`          | a failed step far above the 2,000-line tail `exec_command` keeps |
| `multi-needle`        | three failing tests far apart in a cargo test run                |
| `earlier-requirement` | a checksum only a large runbook read earlier asks for            |

The runner drives each workload through the agent loop as the app
prunes: a scripted model makes the workload's calls, fake tools answer,
and a fake `exec_command` cuts its output to the tail like tau's. Fast
compaction, loaded by `tau-luau-plugins-host` as tau loads it, with only
output pruning at work, prunes the output before the model's next
request. What it made of the output comes from the plugin's state
(`outputs`) in the run's records; a failure from its log.

With `--whole`, an output over 256 KiB reaches the model unpruned:
`tau-luau-plugins` gives a plugin's hook at most that much of a result,
and the plugin leaves an output it could not read whole as it was. Of
the workloads, only some seeds of `all-noise` are that large.

Per trial (`metrics::Trial`) it records:

- recall: needles in the result the model saw, each as a whole line (a
  needle only in the archive does not count), and tail recall, the same
  for what `exec_command` alone would have shown;
- estimated tokens before (what `exec_command` alone shows) and after, the
  reduction, and the reduction against the whole output;
- whether the result was replaced, Jev requests, input tokens, cost and
  latency;
- Jev's answers by band: noise (at most 0.1), uncertain, and needed (0.5
  or more, so the chunk stays), and each chunk's largest answer, to
  replay another keep rule offline.

The library modules are `workload` (the `Kind`s and `generate`),
`noise`, `rng` (a small deterministic generator), `runner` (`Config`,
`evaluate`, `Report`), `metrics` (`Trial`, `Summary`, `table`) and
`measure`: the eval's own copy of how the plugin splits an output into
lines and chunks, renders it and estimates tokens, ported from the Rust
the plugin had before, so the layout and token counts do not depend on
the plugin under test.

`Config::settings` is the plugin's settings over its defaults, by its
flat keys, such as `output_chunk_lines` or `output_keep_threshold`.

## How it fits

It builds on `tau-fast-compaction` (the plugin's Luau) and `tau-jev` for
the system under test. It runs them as the app does with
`tau-luau-plugins-host` (which loads and runs the Luau),
`tau-luau-plugins` (its records), `tau-agent`, `tau-ai`, `tau-testing`
(the scripted model), `tau-store`, `tau-store-sqlite` and
`tau-tools-host`. No other crate depends on it.

## Usage

```sh
TYPESAFE_API_KEY=… cargo run -p tau-output-pruning-eval -- \
  --seeds 3 --budget-usd 0.5 --json results.json
```

| Option             | Effect                                                            |
| ------------------ | ----------------------------------------------------------------- |
| `--workload NAME`  | Only these workloads (repeat, or comma-separated)                 |
| `--seeds N`        | Seeds 0 to N-1 of each workload (default 1)                       |
| `--budget-usd USD` | Stop once Jev's spend passes this                                 |
| `--json PATH`      | Also write every trial and summary to `PATH`                      |
| `--whole`          | `exec_command` returns every output whole, with no truncation     |
| `--work DIR`       | Where archives go (default: a temp directory, removed afterwards) |
| `--list`           | List the workloads, and exit                                      |
| `--help`           | Print the usage, and exit                                         |

It needs `TYPESAFE_API_KEY`; without it, it refuses to run. Each trial's
progress goes to standard error. Standard output gets a table per
workload, the total spent, and the reason when it stopped early.

Jev costs about $0.042 per million input tokens. A workload costs about
half a cent, so three seeds of all eight cost about $0.13.

## Testing

```sh
cargo nextest run --release -p tau-output-pruning-eval
```

The tests use fake Jevs and need no key, and run on real time: a Luau
hook's limit is a timer that paused time lets run out. `tests/workloads.rs` checks,
with Hegel properties, that generation is deterministic and that each
needle is where it is claimed, exactly once. `tests/runner.rs` runs the
real plugin with a ground-truth Jev, one that keeps nothing, and one
that fails, checks it against `measure`'s render, and checks the budget
stop. `tests/metrics.rs` checks the
summaries against a reference.

## Further reading

- [fast-compaction.md](../../../docs/reference/fast-compaction.md):
  "Evaluation" has the workloads, the measures and the latest results
  against the real Jev
