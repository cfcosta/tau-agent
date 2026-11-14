# heiko-store-sqlite

The SQLite backend for `heiko-store`. It keeps runs, messages, fork
transcripts and costs in one database file, through stock `sqlx`. Every
query uses sqlx's compile-time-checked macros, and the query metadata is
committed in `.sqlx/`, so the crate builds without a database.

## What it provides

| Item          | What it is                                                         |
| ------------- | ------------------------------------------------------------------ |
| `open(path)`  | Opens or creates the database at `path`, migrates it, as a `Store` |
| `memory()`    | An in-memory database as a `Store`, for tests                      |
| `SqliteStore` | The `Backend` itself, with `open`, `memory` and `writer_stats`     |
| `WriterStats` | How many writes went through the writer, and how long they waited  |

A file database runs in WAL mode with one writer connection and a pool of
read-only connections. SQLite has one writer, so parallel runs queue on it;
`SqliteStore::writer_stats` reports what that queue costs. The in-memory
database uses one connection for both roles and sets no timeouts, so it
also works under tokio's paused clock.

The schema is in `migrations/0001_runs.sql`: `runs`, `messages` (message,
context and plugin entries, keyed by run and `seq`), `plugin_costs` and
`plugin_usage` (what plugins spent outside any run).
Transcripts of forks are read with a recursive CTE up the fork chain.

## How it fits

It implements `heiko_store::Backend`, and it is the only heiko crate that
brings in SQLite. `heiko-ui` and the evals (`heiko-codemode-eval`,
`heiko-memory-e2e`, `heiko-output-pruning-eval`) open stores with it.
`heiko-agent`, `heiko-ui-plugin` and most plugin crates use it only in their
tests, usually through `memory()`.

## Usage

```rust
use heiko_store::Store;

let store: Store = heiko_store_sqlite::open("runs.db").await?;
// In tests:
let test_store: Store = heiko_store_sqlite::memory().await?;
```

To see how much parallel runs waited on the writer, keep the concrete
type. Its clones share one set of counters:

```rust
use std::sync::Arc;

use heiko_store::Store;
use heiko_store_sqlite::SqliteStore;

let sqlite = SqliteStore::open("runs.db").await?;
let store: Store = Arc::new(sqlite.clone());
// ... run agents against `store` ...
let stats = sqlite.writer_stats();
println!("{} writes, {:?} waited", stats.writes, stats.waited);
```

## Testing

```sh
cargo nextest run --release -p heiko-store-sqlite
```

`tests/store.rs` is a Hegel state machine: it creates roots, forks and
sub-agents, appends turns (context rewrites and plugin records included),
finishes, reopens, interrupts and names runs, and checks every read against
a `Vec`-based model. Store tests run on a real current-thread runtime
(`heiko_testing::block_on_io`), not a paused one, because sqlx waits on its
own worker threads.

A deeper run of the same machine is ignored by default:

```sh
cargo nextest run --release -p heiko-store-sqlite --run-ignored only
```

After changing SQL or migrations, regenerate and commit `.sqlx/`:

```sh
export DATABASE_URL=sqlite://target/heiko-store-dev.db
cargo sqlx database setup --source crates/heiko-store-sqlite/migrations
cargo sqlx prepare -- -p heiko-store-sqlite
cargo sqlx prepare --check -- -p heiko-store-sqlite
```

## Further reading

- [Storage](../../docs/reference/storage.md)
- [Testing](../../docs/reference/testing.md)
- [Decision 0003: SQLite through stock sqlx](../../docs/decisions/0003-sqlite-via-sqlx-macros.md)
- [Decision 0030: Host halves are crates](../../docs/decisions/0030-host-halves-are-crates.md)
