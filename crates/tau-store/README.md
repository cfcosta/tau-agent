# tau-store

Storage for runs, messages and fork transcripts, as an interface. The agent
loop and the plugins write every run here and read it back, but this crate
holds no database driver: it has the shapes and the `Backend` trait, and a
host picks the backend. The SQLite one is in `tau-store-sqlite`. Keeping the
two apart means the loop and the plugins build without waiting for SQLite to
compile.

Message bodies are opaque JSON text. A store keeps what the loop gives it
and knows nothing about a message beyond its role.

## What it provides

| Item                      | What it is                                                                 |
| ------------------------- | -------------------------------------------------------------------------- |
| `Backend`                 | The trait a store implements: create, append, finish, reopen, read back    |
| `Store`                   | `Arc<dyn Backend>`, the shared handle every run and plugin takes           |
| `NewRun`, `RunKind`       | A run to create, and how it relates to others: `Root`, `Fork`, `Subagent`  |
| `RunRecord`, `Status`     | A stored run, and its status (`Running`, `Done`, `Failed`, `Interrupted`…) |
| `Entry`                   | One stored entry: a `Message`, a `Context` rewrite, or a `Plugin` record   |
| `RewriteStats`            | Token counts before and after a context rewrite, and what triggered it     |
| `TurnUsage`               | Tokens, cost and turns that one write adds to a run's totals               |
| `PluginCost`, `AgentCost` | Cost per plugin in a run, and cost per agent in a workflow                 |
| `Table`                   | What `Backend::query` returns: columns and rows as text                    |
| `StoreError`, `Result`    | Errors: database, bad JSON, unknown run, still running, corrupt history    |

The main `Backend` methods:

- `create_run`, `append_turn` / `append_charged`, `finish_run`: write a
  run. A turn's entries and usage go in one transaction.
- `transcript`: the run's transcript, with the messages it inherits from
  its fork chain, as its context rewrites left them.
- `timeline`: everything the fork chain holds in write order, plugin
  records included, for showing a stored run as it happened.
- `records`, `plugin_entries`, `plugin_entries_everywhere`: a plugin's
  records, along one run's chain or across the whole store.
- `reopen_run`: open a finished run again so it can go on.
- `interrupt_running`: mark runs a closed process left `running` as
  `Interrupted`. Call it before any run starts.
- `run`, `recent_runs`, `subagents`, `retained_runs`: look runs up.
- `plugin_costs`, `plugin_spend`, `workflow_cost`: cost reports.
- `query`: a read-only query a person typed, returned as text.

## How it fits

`tau-store` depends on no other tau crate. `tau-store-sqlite` implements
`Backend`. `tau-agent` writes every run through a `Store`, and `tau-ui`,
`tau-ui-plugin`, `tau-ui-remote` and the evals read from one. Most plugin
crates use it in their tests.

## Usage

Open a store with `tau_store_sqlite::open` (or `memory` in tests), then
read from it through the `Store` handle:

```rust
use tau_store::{Entry, Store};

async fn print_transcript(store: &Store, run: &str) -> tau_store::Result<()> {
    for entry in store.transcript(run).await? {
        if let Entry::Message { role, body } = entry {
            println!("{role}: {body}");
        }
    }
    Ok(())
}
```

## Testing

The crate has no tests of its own. The `Backend` contract is tested in
`tau-store-sqlite`, against a model of it:

```sh
cargo nextest run --release -p tau-store-sqlite
```

## Further reading

- [Storage](../../docs/reference/storage.md)
- [Decision 0003: SQLite through stock sqlx](../../docs/decisions/0003-sqlite-via-sqlx-macros.md)
- [Decision 0030: Host halves are crates](../../docs/decisions/0030-host-halves-are-crates.md)
