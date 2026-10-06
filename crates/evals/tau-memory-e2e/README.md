# tau-memory-e2e

The end-to-end evaluation of tau-memory. Each trial runs a coding agent
twice on a small generated repository: the first run finds a fact while
doing one task, and the second run needs that fact for another. It
compares what the second run knows of the first under five arms, and
what each costs in calls, tokens, money and time. It makes real model
calls.

## What it measures

Scenarios (`scenario::SCENARIOS`) are small made-up repositories of bash
scripts, so a run needs only bash and coreutils: no network, no
toolchain. A check script on the repository decides whether each run's
task is done.

| Scenario      | The fact the first run finds                              |
| ------------- | --------------------------------------------------------- |
| `test-mode`   | the test suite runs only with an environment variable set |
| `generated`   | a file is generated from another by a script              |
| `config-keys` | settings keys take a prefix                               |
| `release`     | a release sets the version in three files                 |
| `registry`    | a command runs only when it is listed in an index         |

Each scenario has two variants (`scenario::Variant`). In `stable` the
fact holds for the second run. In `changed` a commit between the runs
changes it, memory marks the notes about the changed files stale as the
app does, and a second run that acts on the old fact is counted.

The arms (`arm::Arm`) are what the second run knows of the first:

| Arm                  | What carries over                                              |
| -------------------- | -------------------------------------------------------------- |
| `none`               | nothing                                                        |
| `memory_md`          | the agent keeps `MEMORY.md`, and the second run starts with it |
| `transcripts`        | the best matches of a search over the first run's transcript   |
| `memory`             | tau-memory, consolidation off, as the app runs it              |
| `memory_consolidate` | tau-memory with its consolidation pass after each run          |

Each arm's agent is built as tau-ui builds one (the coding tools on the
repository, compaction, the memory plugin), less what needs the app:
version control, Jev and the constitution.

Per run (`metrics::RunMetrics`) it records success, turns, tool calls
and failed calls, input, output and cached tokens, cost and wall time.
Per trial (`metrics::Trial`) it adds whether memory was saved, given
and read, and in the changed variant whether the old fact was used.
`metrics::Summary` holds the means per arm and variant.

## How it fits

It builds on `tau-memory-host` (the memory plugin, its notes and its
indexes), `tau-compaction`, `tau-tools-host`, `tau-agent`, `tau-ai`,
`tau-store` and `tau-store-sqlite`. No other crate depends on it.

## Usage

```sh
cargo run --release -p tau-memory-e2e -- --budget-usd 2 --json results.json
cargo run --release -p tau-memory-e2e --features docbert -- --trials 3
```

| Option             | Effect                                                          |
| ------------------ | --------------------------------------------------------------- |
| `--scenario NAME`  | Only these scenarios (repeat, or comma-separated)               |
| `--arm NAME`       | Only these arms (repeat, or comma-separated)                    |
| `--variant NAME`   | `stable` or `changed` (default: both)                           |
| `--trials N`       | Repetitions of each scenario, variant and arm (default 1)       |
| `--model ID`       | The model (default `gpt-5.5`, tau-ui's)                         |
| `--budget-usd USD` | Stop once this much is spent                                    |
| `--json PATH`      | Also write every trial and summary to `PATH`                    |
| `--chatgpt ID`     | Use this ChatGPT account's plan, from tau's sign-ins            |
| `--max-turns N`    | Turns a run may take (default 40)                               |
| `--work DIR`       | Where trial repositories are made (default: the temp directory) |
| `--keywords`       | Search with BM25 even when built with `docbert`                 |
| `--list`           | List the scenarios and arms, and exit                           |
| `--help`           | Print the usage, and exit                                       |

Access is a saved Sign in with ChatGPT with plan usage: the account
named by `--chatgpt`, or else tau's active account, both from
`$XDG_CONFIG_HOME/tau/chatgpt`. Sign in with tau first; without a
sign-in, nothing runs. There is no API key option.

Every trial is two agent runs, which cost money; use `--budget-usd` to
cap the spend. Each trial's progress goes to standard error. Standard
output gets a table per arm and variant, the total spent, and the reason
when it stopped early.

## Features

- `docbert` (off by default): search memory and transcripts with
  docbert's ColBERT model, as the app does, through
  `tau-memory-host/docbert`. Without it, the search is BM25.

## Testing

```sh
cargo nextest run --release -p tau-memory-e2e
```

The tests make no model calls. `tests/runner.rs` runs the trials with
`ScriptedModel`s: each arm carries memory into the second run as it
should, a changed fact reaches memory as a stale mark and its use is
caught, checks decide success, and the budget stops the run.
`tests/scenarios.rs` builds every scenario: its checks fail on a fresh
repository and pass once the task's solution runs, and its change
touches exactly the files it lists. `tests/metrics.rs` (with Hegel properties)
and `tests/access.rs` cover the metrics and how access is found. The
scenario tests need `bash`.

## Further reading

- [plugins.md](../../../docs/reference/plugins.md): the `tau-memory`
  section, "Evaluation first"
- [0012-chatgpt-sign-in-only.md](../../../docs/decisions/0012-chatgpt-sign-in-only.md):
  why the evaluation uses a ChatGPT sign-in
