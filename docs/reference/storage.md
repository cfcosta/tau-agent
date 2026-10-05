# Storage

Every run and message is written to an embedded SQLite database through
stock `sqlx`, using the `sqlite`, `macros` and `migrate` features. SQLite
is bundled into the build. Every query uses `sqlx::query!`, `query_as!`
or `query_scalar!`, and migrations run through `sqlx::migrate!`. See
[decision 0003](../decisions/0003-sqlite-via-sqlx-macros.md).

Credentials are not in the database. tau-ui keeps them in
`$XDG_CONFIG_HOME/tau/`, each file readable only by the user: ChatGPT
sign-ins in `chatgpt/` (see
[`chatgpt-sign-in.md`](chatgpt-sign-in.md#storage)), GitHub's token in
`github.json`, TypeSafe's key in `typesafe-key`, and model choices (with
whether the plan note was read) in `models.json`. The old `codex.json`
and `openai-key` are no longer read, and tau does not delete them
([0012](../decisions/0012-chatgpt-sign-in-only.md)).

## Model

Runs are two tables, `runs` and `messages`. A plugin that keeps more
than its records keeps it in a SQLite file of its own, under
`HostCx::plugin_dir`, never here: tau-constitution's rules are in its
own `constitution.db` ([constitution.md](constitution.md)).

- **A run** is a flat list of messages.
- **A fork** is a run whose `parent_run_id` points at another run. It
  inherits the parent's messages with `seq <= fork_seq`, by reference,
  without copying them.
- **A sub-agent run** also sets `parent_run_id`. With a `fork_seq`
  (every `spawn` sub-agent sets one), it inherits as a fork does;
  without (`Agent::as_tool`), it inherits nothing. Inheritance follows
  any run that has a `fork_seq`, whatever its kind.
- **A context rewrite** is a row in `messages` with `kind = 'context'`,
  naming the plugin that made it (compaction is one). Its body is
  `{"details": …, "layout": […], "stats": {…}}`: the plugin's details,
  the transcript the rewrite leaves, one slot per message, and what it
  did: `tokens_before` and `tokens_after`, the loop's estimate of the
  context, and the `trigger` (`turn_end`, `start` or `overflow`). A number keeps
  the message at that index of the transcript before the rewrite, by
  reference. `null` takes the next message stored after the row: one
  the rewrite made or changed. Only those are stored again, so a
  rewrite costs what it changed, not a copy of the whole transcript.
  `Store::transcript` replays the rewrites and starts with the latest
  one. `Store::timeline` keeps everything as it happened, with each
  rewrite in place and without the messages it made.
- **A plugin record** is a row with `kind = 'plugin'`, naming its
  plugin. It is never part of the transcript. `Store::records` reads a
  plugin's records along a run's fork chain.

## Schema: `migrations/0001_runs.sql`

```sql
CREATE TABLE runs (
  id            TEXT PRIMARY KEY,           -- uuidv7
  workflow_id   TEXT,                       -- groups the runs of one workflow invocation
  agent         TEXT NOT NULL,              -- Agent::name
  kind          TEXT NOT NULL CHECK (kind IN ('root', 'fork', 'subagent')),
  parent_run_id TEXT REFERENCES runs (id),  -- fork source or calling agent
  fork_seq      INTEGER,                    -- fork: inherit parent messages with seq <= fork_seq
  model         TEXT NOT NULL,
  status        TEXT NOT NULL CHECK (status IN ('running', 'done', 'failed', 'cancelled', 'limit', 'interrupted')),
  input_tokens  INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  cost_usd      REAL    NOT NULL DEFAULT 0,
  result        TEXT,                       -- final output; JSON when typed
  error         TEXT,
  created_at    TEXT NOT NULL,
  updated_at    TEXT NOT NULL
) STRICT;
CREATE INDEX runs_by_workflow ON runs (workflow_id, created_at);
CREATE INDEX runs_by_parent   ON runs (parent_run_id);

CREATE TABLE messages (
  run_id     TEXT    NOT NULL REFERENCES runs (id),
  seq        INTEGER NOT NULL,
  kind       TEXT    NOT NULL CHECK (kind IN ('message', 'context', 'plugin')),
  role       TEXT,                          -- user | assistant | toolResult
  plugin     TEXT,                          -- the plugin of a context or plugin entry
  body       TEXT    NOT NULL CHECK (json_valid(body)),
  created_at TEXT    NOT NULL,
  PRIMARY KEY (run_id, seq)
) STRICT;
```

`body` holds the message as JSON, in the same shape as the `tau-ai`
message types. Only the fields that queries filter on get their own
columns.

```sql
CREATE TABLE plugin_costs (
  run_id        TEXT    NOT NULL REFERENCES runs (id),
  plugin        TEXT    NOT NULL,
  input_tokens  INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  cost_usd      REAL    NOT NULL DEFAULT 0,
  PRIMARY KEY (run_id, plugin)
) STRICT;
```

- What each plugin charged to a run (`PluginCtx::charge`): a breakdown
  of the run's own totals, which already count it. It is written in
  the same transaction as the turn that adds it to the totals
  (`Store::append_charged`).
- `Store::plugin_costs` reads one run's breakdown;
  `Store::plugin_spend` sums each plugin's cost over the runs started
  since a time, for the Plugins screen.

## Runs left running

A run's status is `running` while a process runs it. When that process
closes in its middle, nothing finishes the run: it stays `running`. A
process opening the store calls `Store::interrupt_running` before it
starts any run, which marks every such run `interrupted` (without
touching when it was last active) and returns their ids. An interrupted
run reopens like any finished one (`Store::reopen_run`). tau-ui shows it
as interrupted and offers to resume it, telling the model it was cut
off.

## Connections

| Pool                      | Size                              | Settings                                                                                                                     |
| ------------------------- | --------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| writer                    | 1 connection                      | WAL, `synchronous = NORMAL`, `busy_timeout = 5s`, `foreign_keys = ON`; every write transaction starts with `BEGIN IMMEDIATE` |
| reader                    | 8 connections                     | same settings, plus `read_only(true)`                                                                                        |
| tests (`Store::memory()`) | 1 connection shared by both roles | `sqlite::memory:`                                                                                                            |

- **One writer at a time.** SQLite allows only one writer, so parallel
  runs queue on the writer connection. A run makes one write
  transaction per turn, taking milliseconds, and turns take seconds.
  Time spent waiting for the writer connection is a tracked metric:
  `Store::writer_stats()` returns the number of writes, the total wait
  and the longest single wait, shared by every clone of the store. For
  an append the wait includes taking the write lock, which another
  process can hold.
- **Other processes can read.** WAL mode lets them read the file while
  workflows run: a dashboard, `sqlite3`, or another worker. This only
  works on a local disk, not on network filesystems.
- **Durability.** `synchronous = NORMAL` in WAL mode survives an
  application crash. A power loss can drop the last few commits, which
  is acceptable for run traces.

## Queries

### Append a turn (atomic)

```rust
let mut tx = self.writer.begin_with("BEGIN IMMEDIATE").await?;

let next = sqlx::query_scalar!(
    r#"SELECT COALESCE(MAX(seq), -1) + 1 AS "next!: i64" FROM messages WHERE run_id = ?1"#,
    run
)
.fetch_one(&mut *tx)
.await?;

for (i, m) in msgs.iter().enumerate() {
    let (seq, role, body) = (next + i as i64, m.role(), serde_json::to_string(m)?);
    sqlx::query!(
        "INSERT INTO messages (run_id, seq, kind, role, body, created_at)
         VALUES (?1, ?2, 'message', ?3, ?4, ?5)",
        run, seq, role, body, now
    )
    .execute(&mut *tx)
    .await?;
}

sqlx::query!(
    "UPDATE runs SET input_tokens = input_tokens + ?2, output_tokens = output_tokens + ?3,
                     cost_usd = cost_usd + ?4, updated_at = ?5
     WHERE id = ?1",
    run, input, output, cost, now
)
.execute(&mut *tx)
.await?;

tx.commit().await?;
```

### Transcript, including fork ancestors

```rust
sqlx::query!(
    r#"WITH RECURSIVE chain(run_id, cutoff, depth) AS (
           SELECT id, NULL, 0 FROM runs WHERE id = ?1
           UNION ALL
           SELECT r.parent_run_id, r.fork_seq, chain.depth + 1
           FROM chain JOIN runs r ON r.id = chain.run_id
           WHERE r.fork_seq IS NOT NULL
       )
       SELECT m.kind AS "kind!: String", m.body AS "body!: String"
       FROM chain JOIN messages m ON m.run_id = chain.run_id
       WHERE chain.cutoff IS NULL OR m.seq <= chain.cutoff
       ORDER BY chain.depth DESC, m.seq"#,
    run
)
.fetch_all(&self.reader)
.await?
```

The caller keeps the rows from the last `compaction` row onward.

### Cost of a workflow

```rust
sqlx::query_as!(
    AgentCost,
    r#"SELECT agent AS "agent!: String", count(*) AS "runs!: i64", sum(cost_usd) AS "usd!: f64"
       FROM runs WHERE workflow_id = ?1 GROUP BY agent"#,
    workflow
)
```

## sqlx workflow

```sh
export DATABASE_URL=sqlite://target/tau-store-dev.db
cargo sqlx database setup                    # create the database and run migrations/
cargo sqlx prepare -- -p tau-store           # after changing SQL or migrations; commit .sqlx/
cargo sqlx prepare --check -- -p tau-store   # CI
```

A plugin with its own database, such as tau-constitution, keeps its
migrations and `.sqlx/` in its crate, with a dated migration version so
it never clashes with tau-store's. Its metadata is prepared against a
database with both schemas, since its build compiles tau-store's
queries too:

```sh
sqlx database setup --source crates/tau-store/migrations
sqlx migrate run --ignore-missing --source crates/plugins/tau-constitution/migrations
(cd crates/plugins/tau-constitution && cargo sqlx prepare)
```

- **Commit `.sqlx/`.** Crates that depend on `tau-store` don't set
  `DATABASE_URL`, so the macros read the committed metadata instead.
  Building `tau-store` needs no database.
- **Override uncertain types.** Where SQLite's nullability inference is
  unsure, such as aggregates and CTE columns, write the type out with an
  override like `"next!: i64"`. Nullability is then decided in the SQL.
- **Never edit an applied migration.** Add a new numbered file instead.
