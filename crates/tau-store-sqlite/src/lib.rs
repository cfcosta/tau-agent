//! SQLite storage for runs, messages and fork transcripts, through sqlx:
//! the [`Backend`] behind a desktop's [`Store`].
//!
//! See `docs/reference/storage.md`. Every query goes through sqlx's
//! compile-time-checked macros; the offline metadata in `.sqlx/` lets the
//! crate build without a database.
//!
//! Message bodies are opaque JSON here: this crate stores what the agent
//! loop gives it and knows nothing about message shapes beyond the `role`
//! column that queries may filter on.

use std::{
    borrow::Cow,
    mem,
    path::Path,
    str::FromStr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use sqlx::{
    Column as _,
    Connection,
    Row as _,
    Sqlite,
    SqlitePool,
    pool::PoolConnection,
    sqlite::{
        SqliteConnectOptions,
        SqliteJournalMode,
        SqlitePoolOptions,
        SqliteSynchronous,
    },
};
use tau_store::{
    AgentCost,
    Backend,
    Entry,
    NewRun,
    PluginCost,
    Result,
    RewriteStats,
    RunKind,
    RunRecord,
    Status,
    Store,
    StoreError,
    Table,
    TurnUsage,
};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Opens (creating if needed) the database at `path` and runs the
/// migrations.
pub async fn open(path: impl AsRef<Path>) -> Result<Store> {
    Ok(Arc::new(SqliteStore::open(path).await?))
}

/// An in-memory database, for tests: see [`SqliteStore::memory`].
pub async fn memory() -> Result<Store> {
    Ok(Arc::new(SqliteStore::memory().await?))
}

/// A database error, as the store reports it.
fn database(
    error: impl Into<Box<dyn std::error::Error + Send + Sync>>,
) -> StoreError {
    StoreError::Database(error.into())
}

/// A status as the `runs.status` column holds it.
fn status_name(status: Status) -> &'static str {
    match status {
        Status::Running => "running",
        Status::Done => "done",
        Status::Failed => "failed",
        Status::Cancelled => "cancelled",
        Status::Limit => "limit",
        Status::Interrupted => "interrupted",
    }
}

/// The status a `runs.status` value names.
fn parse_status(value: &str) -> Status {
    match value {
        "running" => Status::Running,
        "done" => Status::Done,
        "failed" => Status::Failed,
        "cancelled" => Status::Cancelled,
        "interrupted" => Status::Interrupted,
        // The CHECK constraint allows nothing else.
        _ => Status::Limit,
    }
}

/// One value of a row, as text: `NULL` for none, numbers as written,
/// and blobs by their size.
fn cell(row: &sqlx::sqlite::SqliteRow, i: usize) -> String {
    use sqlx::{Row as _, ValueRef as _};
    match row.try_get_raw(i) {
        Ok(raw) if raw.is_null() => return "NULL".into(),
        Err(_) => return String::new(),
        Ok(_) => {}
    }
    if let Ok(value) = row.try_get::<i64, _>(i) {
        return value.to_string();
    }
    if let Ok(value) = row.try_get::<f64, _>(i) {
        return value.to_string();
    }
    if let Ok(value) = row.try_get::<String, _>(i) {
        return value;
    }
    match row.try_get::<Vec<u8>, _>(i) {
        Ok(bytes) => format!("<{} bytes>", bytes.len()),
        Err(_) => String::new(),
    }
}

/// How long writes waited for the writer connection
/// (`docs/reference/storage.md`, "Connections"). SQLite has one writer,
/// so parallel runs queue on it; this is how much that queue costs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WriterStats {
    /// Writes made through this store (and its clones).
    pub writes: u64,
    /// Time spent waiting for the writer, over every write.
    pub waited: Duration,
    /// The longest single wait.
    pub longest: Duration,
}

#[derive(Debug, Clone)]
pub struct SqliteStore {
    writer: SqlitePool,
    reader: SqlitePool,
    stats: Arc<Mutex<WriterStats>>,
}

impl SqliteStore {
    /// Opens (creating if needed) the database at `path` and runs the
    /// migrations.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(5))
            .foreign_keys(true);
        let writer = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options.clone())
            .await
            .map_err(database)?;
        MIGRATOR.run(&writer).await.map_err(database)?;
        let reader = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(options.read_only(true))
            .await
            .map_err(database)?;
        Ok(Self {
            writer,
            reader,
            stats: Arc::default(),
        })
    }

    /// An in-memory database for tests: one connection serves both roles,
    /// because each connection to `sqlite::memory:` is its own database.
    ///
    /// That connection must never close, or the database goes with it, so
    /// the pool keeps it with no idle timeout and no maximum lifetime.
    /// Acquiring it never times out either: callers queue for the one
    /// connection, and a timeout would only fire spuriously, for example
    /// under tokio's paused clock.
    pub async fn memory() -> Result<Self> {
        let options = SqliteConnectOptions::from_str("sqlite::memory:")
            .map_err(database)?
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .acquire_timeout(Duration::from_secs(u64::MAX / 4))
            .connect_with(options)
            .await
            .map_err(database)?;
        MIGRATOR.run(&pool).await.map_err(database)?;
        Ok(Self {
            writer: pool.clone(),
            reader: pool,
            stats: Arc::default(),
        })
    }

    /// Totals of the time writes spent waiting for the writer, shared by
    /// every clone of this store.
    pub fn writer_stats(&self) -> WriterStats {
        *self.stats.lock().expect("not poisoned")
    }

    /// The writer connection, with the wait for it recorded.
    async fn writer(&self) -> Result<PoolConnection<Sqlite>> {
        let started = Instant::now();
        let connection = self.writer.acquire().await.map_err(database)?;
        self.record_wait(started.elapsed());
        Ok(connection)
    }

    fn record_wait(&self, waited: Duration) {
        let mut stats = self.stats.lock().expect("not poisoned");
        stats.writes += 1;
        stats.waited += waited;
        stats.longest = stats.longest.max(waited);
    }

    async fn entries(&self, run: &str, records: bool) -> Result<Vec<Entry>> {
        let rows = sqlx::query!(
            r#"WITH RECURSIVE chain(run_id, cutoff, depth) AS (
                   SELECT id, NULL, 0 FROM runs WHERE id = ?1
                   UNION ALL
                   SELECT r.parent_run_id, r.fork_seq, chain.depth + 1
                   FROM chain JOIN runs r ON r.id = chain.run_id
                   WHERE r.fork_seq IS NOT NULL
               )
               SELECT m.kind AS "kind!: String", m.role AS "role?: String",
                      m.plugin AS "plugin?: String", m.body AS "body!: String"
               FROM chain JOIN messages m ON m.run_id = chain.run_id
               WHERE (chain.cutoff IS NULL OR m.seq <= chain.cutoff)
                 AND (m.kind != 'plugin' OR ?2)
               ORDER BY chain.depth DESC, m.seq"#,
            run,
            records
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;

        let entries = rows.into_iter().map(|row| match row.kind.as_str() {
            "context" => {
                let (body, layout, stats) = read_context(&row.body)?;
                Ok(Entry::Context {
                    // The loop sets a plugin on every context row.
                    plugin: row.plugin.unwrap_or_default(),
                    body,
                    layout,
                    stats,
                })
            }
            "plugin" => Ok(Entry::Plugin {
                // The schema sets a plugin on every plugin row.
                plugin: row.plugin.unwrap_or_default(),
                body: row.body,
            }),
            _ => Ok(Entry::Message {
                // The schema sets a role on every message row.
                role: row.role.unwrap_or_default(),
                body: row.body,
            }),
        });
        let entries = entries.collect::<Result<Vec<_>>>()?;
        if records {
            Ok(happened(entries))
        } else {
            rewritten(entries)
        }
    }
}

#[async_trait]
impl Backend for SqliteStore {
    async fn query(&self, sql: &str, limit: usize) -> Result<Table> {
        let sql = sql.trim().trim_end_matches(';').trim();
        let mut connection = self.reader.acquire().await.map_err(database)?;
        sqlx::query("PRAGMA query_only = ON")
            .execute(&mut *connection)
            .await
            .map_err(database)?;
        // One row past the limit says whether there are more.
        let wrapped = format!("SELECT * FROM ({sql}) LIMIT {}", limit + 1);
        let rows = sqlx::query(sqlx::AssertSqlSafe(wrapped))
            .fetch_all(&mut *connection)
            .await;
        sqlx::query("PRAGMA query_only = OFF")
            .execute(&mut *connection)
            .await
            .map_err(database)?;
        let rows = rows.map_err(database)?;
        let columns = rows
            .first()
            .map(|row| {
                row.columns()
                    .iter()
                    .map(|column| column.name().to_owned())
                    .collect()
            })
            .unwrap_or_default();
        let truncated = rows.len() > limit;
        let rows = rows
            .iter()
            .take(limit)
            .map(|row| (0..row.columns().len()).map(|i| cell(row, i)).collect())
            .collect();
        Ok(Table {
            columns,
            rows,
            truncated,
        })
    }

    async fn create_run(&self, run: &NewRun<'_>) -> Result<()> {
        let (kind, parent, fork_seq) = match &run.kind {
            RunKind::Root => ("root", None, None),
            RunKind::Fork { parent, fork_seq } => {
                ("fork", Some(parent.as_str()), Some(*fork_seq))
            }
            RunKind::Subagent { parent, fork_seq } => {
                ("subagent", Some(parent.as_str()), *fork_seq)
            }
        };
        sqlx::query!(
            "INSERT INTO runs (id, workflow_id, agent, kind, parent_run_id, fork_seq,
                               model, status, turns, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'running', ?8,
                     strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                     strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            run.id,
            run.workflow_id,
            run.agent,
            kind,
            parent,
            fork_seq,
            run.model,
            run.turns,
        )
        .execute(&mut *self.writer().await?)
        .await.map_err(database)?;
        Ok(())
    }

    async fn append_charged(
        &self,
        run: &str,
        entries: &[Entry],
        usage: TurnUsage,
        plugins: &[(&str, TurnUsage)],
    ) -> Result<i64> {
        // The wait covers the connection and the write lock, which
        // another process can hold.
        let started = Instant::now();
        let mut connection = self.writer.acquire().await.map_err(database)?;
        let mut tx = connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(database)?;
        self.record_wait(started.elapsed());
        let input_tokens = i64::from(usage.input_tokens);
        let output_tokens = i64::from(usage.output_tokens);
        let turns = i64::from(usage.turns);

        let updated = sqlx::query!(
            "UPDATE runs SET input_tokens = input_tokens + ?2,
                             output_tokens = output_tokens + ?3,
                             cost_usd = cost_usd + ?4,
                             turns = turns + ?5,
                             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1",
            run,
            input_tokens,
            output_tokens,
            usage.cost_usd,
            turns,
        )
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        if updated.rows_affected() == 0 {
            return Err(StoreError::UnknownRun(run.to_owned()));
        }

        let next = sqlx::query_scalar!(
            r#"SELECT COALESCE(MAX(seq), -1) + 1 AS "next!: i64"
               FROM messages WHERE run_id = ?1"#,
            run
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(database)?;

        for (offset, entry) in entries.iter().enumerate() {
            let seq = next + offset as i64;
            let (kind, role, plugin, body) = match entry {
                Entry::Message { role, body } => (
                    "message",
                    Some(role.as_str()),
                    None,
                    Cow::Borrowed(body.as_str()),
                ),
                Entry::Context {
                    plugin,
                    body,
                    layout,
                    stats,
                } => (
                    "context",
                    None,
                    Some(plugin.as_str()),
                    Cow::Owned(context_row(body, layout, stats.as_ref())?),
                ),
                Entry::Plugin { plugin, body } => (
                    "plugin",
                    None,
                    Some(plugin.as_str()),
                    Cow::Borrowed(body.as_str()),
                ),
            };
            let body: &str = &body;
            sqlx::query!(
                "INSERT INTO messages (run_id, seq, kind, role, plugin, body, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                run,
                seq,
                kind,
                role,
                plugin,
                body,
            )
            .execute(&mut *tx)
            .await.map_err(database)?;
        }

        for (plugin, usage) in plugins {
            let input_tokens = i64::from(usage.input_tokens);
            let output_tokens = i64::from(usage.output_tokens);
            sqlx::query!(
                "INSERT INTO plugin_costs (run_id, plugin, input_tokens, output_tokens, cost_usd)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (run_id, plugin) DO UPDATE SET
                   input_tokens = input_tokens + excluded.input_tokens,
                   output_tokens = output_tokens + excluded.output_tokens,
                   cost_usd = cost_usd + excluded.cost_usd",
                run,
                plugin,
                input_tokens,
                output_tokens,
                usage.cost_usd,
            )
            .execute(&mut *tx)
            .await.map_err(database)?;
        }

        tx.commit().await.map_err(database)?;
        Ok(next + entries.len() as i64 - 1)
    }

    async fn plugin_costs(&self, run: &str) -> Result<Vec<PluginCost>> {
        Ok(sqlx::query_as!(
            PluginCost,
            r#"SELECT plugin AS "plugin!: String",
                      input_tokens AS "input_tokens!: i64",
                      output_tokens AS "output_tokens!: i64",
                      cost_usd AS "cost_usd!: f64"
               FROM plugin_costs WHERE run_id = ?1 ORDER BY plugin"#,
            run
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?)
    }

    async fn plugin_spend(&self, since: &str) -> Result<Vec<(String, f64)>> {
        let rows = sqlx::query!(
            r#"SELECT plugin_costs.plugin AS "plugin!: String",
                      sum(plugin_costs.cost_usd) AS "usd!: f64"
               FROM plugin_costs JOIN runs ON runs.id = plugin_costs.run_id
               WHERE runs.created_at >= ?1
               GROUP BY plugin_costs.plugin ORDER BY plugin_costs.plugin"#,
            since
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        Ok(rows.into_iter().map(|row| (row.plugin, row.usd)).collect())
    }

    async fn transcript(&self, run: &str) -> Result<Vec<Entry>> {
        self.entries(run, false).await
    }

    async fn first_prompt(&self, run: &str) -> Result<Option<String>> {
        let row = sqlx::query_scalar!(
            "SELECT body FROM messages
             WHERE run_id = ?1 AND kind = 'message' AND role = 'user'
             ORDER BY seq LIMIT 1",
            run
        )
        .fetch_optional(&self.reader)
        .await
        .map_err(database)?;
        Ok(row)
    }

    async fn timeline(&self, run: &str) -> Result<Vec<Entry>> {
        self.entries(run, true).await
    }

    async fn records(&self, run: &str, plugin: &str) -> Result<Vec<String>> {
        let rows = sqlx::query_scalar!(
            r#"WITH RECURSIVE chain(run_id, cutoff, depth) AS (
                   SELECT id, NULL, 0 FROM runs WHERE id = ?1
                   UNION ALL
                   SELECT r.parent_run_id, r.fork_seq, chain.depth + 1
                   FROM chain JOIN runs r ON r.id = chain.run_id
                   WHERE r.fork_seq IS NOT NULL
               )
               SELECT m.body AS "body!: String"
               FROM chain JOIN messages m ON m.run_id = chain.run_id
               WHERE (chain.cutoff IS NULL OR m.seq <= chain.cutoff)
                 AND m.kind = 'plugin' AND m.plugin = ?2
               ORDER BY chain.depth DESC, m.seq"#,
            run,
            plugin
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        Ok(rows)
    }

    async fn finish_run(
        &self,
        run: &str,
        status: Status,
        result: Option<&str>,
        error: Option<&str>,
    ) -> Result<()> {
        let status = status_name(status);
        let updated = sqlx::query!(
            "UPDATE runs SET status = ?2, result = ?3, error = ?4,
                             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1",
            run,
            status,
            result,
            error,
        )
        .execute(&mut *self.writer().await?)
        .await
        .map_err(database)?;
        if updated.rows_affected() == 0 {
            return Err(StoreError::UnknownRun(run.to_owned()));
        }
        Ok(())
    }

    async fn interrupt_running(&self) -> Result<Vec<String>> {
        let mut ids: Vec<String> = sqlx::query_scalar!(
            r#"UPDATE runs SET status = 'interrupted'
               WHERE status = 'running'
               RETURNING id AS "id!: String""#
        )
        .fetch_all(&mut *self.writer().await?)
        .await
        .map_err(database)?;
        ids.sort();
        Ok(ids)
    }

    async fn set_title(&self, run: &str, title: &str) -> Result<()> {
        let updated = sqlx::query!(
            "UPDATE runs SET title = ?2 WHERE id = ?1",
            run,
            title
        )
        .execute(&mut *self.writer().await?)
        .await
        .map_err(database)?;
        if updated.rows_affected() == 0 {
            return Err(StoreError::UnknownRun(run.to_owned()));
        }
        Ok(())
    }

    async fn reopen_run(&self, run: &str, model: &str) -> Result<RunRecord> {
        let updated = sqlx::query!(
            "UPDATE runs SET status = 'running', result = NULL, error = NULL,
                             model = ?2,
                             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1 AND status != 'running'",
            run,
            model,
        )
        .execute(&mut *self.writer().await?)
        .await
        .map_err(database)?;
        let record = self
            .run(run)
            .await?
            .ok_or_else(|| StoreError::UnknownRun(run.to_owned()))?;
        if updated.rows_affected() == 0 {
            return Err(StoreError::StillRunning(run.to_owned()));
        }
        Ok(record)
    }

    async fn subagents(&self, parent: &str) -> Result<Vec<RunRecord>> {
        let rows = sqlx::query!(
            r#"SELECT id AS "id!: String", workflow_id, agent, kind,
                      parent_run_id, fork_seq, model, status,
                      input_tokens, output_tokens, cost_usd, turns, result,
                      error, title, created_at
               FROM runs WHERE kind = 'subagent' AND parent_run_id = ?1
               ORDER BY created_at, id"#,
            parent
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        Ok(rows
            .into_iter()
            .map(|row| RunRecord {
                kind: run_kind(&row.kind, row.parent_run_id, row.fork_seq),
                id: row.id,
                workflow_id: row.workflow_id,
                agent: row.agent,
                model: row.model,
                status: parse_status(&row.status),
                input_tokens: row.input_tokens,
                output_tokens: row.output_tokens,
                cost_usd: row.cost_usd,
                turns: row.turns,
                result: row.result,
                error: row.error,
                title: row.title,
                created_at: row.created_at,
            })
            .collect())
    }

    async fn run(&self, run: &str) -> Result<Option<RunRecord>> {
        let row = sqlx::query!(
            r#"SELECT id AS "id!: String", workflow_id, agent, kind,
                      parent_run_id, fork_seq, model, status,
                      input_tokens, output_tokens, cost_usd, turns, result,
                      error, title, created_at
               FROM runs WHERE id = ?1"#,
            run
        )
        .fetch_optional(&self.reader)
        .await
        .map_err(database)?;
        Ok(row.map(|row| RunRecord {
            kind: run_kind(&row.kind, row.parent_run_id, row.fork_seq),
            id: row.id,
            workflow_id: row.workflow_id,
            agent: row.agent,
            model: row.model,
            status: parse_status(&row.status),
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cost_usd: row.cost_usd,
            turns: row.turns,
            result: row.result,
            error: row.error,
            title: row.title,
            created_at: row.created_at,
        }))
    }

    async fn recent_runs(&self, limit: u32) -> Result<Vec<RunRecord>> {
        let rows = sqlx::query!(
            r#"SELECT id AS "id!: String", workflow_id, agent, kind,
                      parent_run_id, fork_seq, model, status,
                      input_tokens, output_tokens, cost_usd, turns, result,
                      error, title, created_at
               FROM runs WHERE kind != 'subagent'
               ORDER BY updated_at DESC, id DESC
               LIMIT ?1"#,
            limit
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        Ok(rows
            .into_iter()
            .map(|row| RunRecord {
                kind: run_kind(&row.kind, row.parent_run_id, row.fork_seq),
                id: row.id,
                workflow_id: row.workflow_id,
                agent: row.agent,
                model: row.model,
                status: parse_status(&row.status),
                input_tokens: row.input_tokens,
                output_tokens: row.output_tokens,
                cost_usd: row.cost_usd,
                turns: row.turns,
                result: row.result,
                error: row.error,
                title: row.title,
                created_at: row.created_at,
            })
            .collect())
    }

    async fn retained_runs(&self) -> Result<Vec<RunRecord>> {
        let rows = sqlx::query!(
            r#"SELECT id AS "id!: String", workflow_id, agent, kind,
                      parent_run_id, fork_seq, model, status,
                      input_tokens, output_tokens, cost_usd, turns, result,
                      error, title, created_at
               FROM runs ORDER BY id"#
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        rows.into_iter()
            .map(|row| {
                let valid = matches!(
                    (&*row.kind, &row.parent_run_id, row.fork_seq),
                    ("root", None, None)
                        | ("fork", Some(_), Some(_))
                        | ("subagent", Some(_), _)
                );
                if !valid {
                    return Err(StoreError::CorruptHistory(row.id));
                }
                Ok(RunRecord {
                    kind: run_kind(&row.kind, row.parent_run_id, row.fork_seq),
                    id: row.id,
                    workflow_id: row.workflow_id,
                    agent: row.agent,
                    model: row.model,
                    status: parse_status(&row.status),
                    input_tokens: row.input_tokens,
                    output_tokens: row.output_tokens,
                    cost_usd: row.cost_usd,
                    turns: row.turns,
                    result: row.result,
                    error: row.error,
                    title: row.title,
                    created_at: row.created_at,
                })
            })
            .collect()
    }

    async fn retention_entries(&self, run: &str) -> Result<Vec<(i64, String)>> {
        let rows = sqlx::query!(
            r#"SELECT seq AS "seq!: i64", body AS "body!: String"
               FROM messages WHERE run_id = ?1 ORDER BY seq"#,
            run
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        Ok(rows.into_iter().map(|row| (row.seq, row.body)).collect())
    }

    async fn plugin_entries(
        &self,
        run: &str,
        plugin: &str,
    ) -> Result<Vec<(i64, String)>> {
        let rows = sqlx::query!(
            r#"SELECT seq AS "seq!: i64", body AS "body!: String"
               FROM messages
               WHERE run_id = ?1 AND kind = 'plugin' AND plugin = ?2
               ORDER BY seq"#,
            run,
            plugin
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        Ok(rows.into_iter().map(|row| (row.seq, row.body)).collect())
    }

    async fn plugin_entries_everywhere(
        &self,
        plugin: &str,
    ) -> Result<Vec<(String, String)>> {
        let rows = sqlx::query!(
            r#"SELECT run_id AS "run!: String", body AS "body!: String"
               FROM messages
               WHERE kind = 'plugin' AND plugin = ?1
               ORDER BY run_id, seq"#,
            plugin
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?;
        Ok(rows.into_iter().map(|row| (row.run, row.body)).collect())
    }

    async fn workflow_cost(&self, workflow: &str) -> Result<Vec<AgentCost>> {
        Ok(sqlx::query_as!(
            AgentCost,
            r#"SELECT agent AS "agent!: String", count(*) AS "runs!: i64",
                      sum(cost_usd) AS "usd!: f64"
               FROM runs WHERE workflow_id = ?1
               GROUP BY agent ORDER BY agent"#,
            workflow
        )
        .fetch_all(&self.reader)
        .await
        .map_err(database)?)
    }
}

fn run_kind(
    kind: &str,
    parent: Option<String>,
    fork_seq: Option<i64>,
) -> RunKind {
    match (kind, parent, fork_seq) {
        ("fork", Some(parent), Some(fork_seq)) => {
            RunKind::Fork { parent, fork_seq }
        }
        ("subagent", Some(parent), fork_seq) => {
            RunKind::Subagent { parent, fork_seq }
        }
        _ => RunKind::Root,
    }
}

/// A context row's body: the rewrite's details, the layout of the
/// transcript it leaves, and what it did.
#[derive(Serialize, Deserialize)]
struct ContextRow<'a> {
    #[serde(borrow, default)]
    details: Option<&'a RawValue>,
    #[serde(default)]
    layout: Vec<Option<usize>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stats: Option<RewriteStats>,
}

/// The body of the row that stores a rewrite.
fn context_row(
    details: &str,
    layout: &[Option<usize>],
    stats: Option<&RewriteStats>,
) -> Result<String> {
    let details: &RawValue = serde_json::from_str(details)?;
    Ok(serde_json::to_string(&ContextRow {
        details: Some(details),
        layout: layout.to_vec(),
        stats: stats.cloned(),
    })?)
}

/// A context row's details, layout and stats, from its body.
fn read_context(
    body: &str,
) -> Result<(String, Vec<Option<usize>>, Option<RewriteStats>)> {
    let row: ContextRow = serde_json::from_str(body)?;
    let details = row.details.map_or("null", RawValue::get).to_owned();
    Ok((details, row.layout, row.stats))
}

/// The transcript `entries` leave, with the latest rewrite first: each
/// rewrite replaces the transcript before it with its layout, taking the
/// messages it kept from that transcript and the ones it made from the
/// messages stored after it.
fn rewritten(entries: Vec<Entry>) -> Result<Vec<Entry>> {
    let mut latest = None;
    let mut transcript: Vec<Entry> = Vec::new();
    // The latest rewrite's layout while messages stored after it still
    // fill its open slots, and how many are open.
    let mut filling: Vec<Option<Entry>> = Vec::new();
    let mut open = 0;
    for entry in entries {
        match entry {
            Entry::Context { ref layout, .. } => {
                transcript
                    .extend(mem::take(&mut filling).into_iter().flatten());
                let before = mem::take(&mut transcript);
                filling = layout
                    .iter()
                    .map(|slot| match slot {
                        Some(index) => {
                            before.get(*index).cloned().map(Some).ok_or_else(
                                || {
                                    StoreError::CorruptHistory(format!(
                                        "a rewrite keeps message {index} of {}",
                                        before.len()
                                    ))
                                },
                            )
                        }
                        None => Ok(None),
                    })
                    .collect::<Result<_>>()?;
                open = layout.iter().filter(|slot| slot.is_none()).count();
                latest = Some(entry);
            }
            message if open > 0 => {
                let slot = filling
                    .iter_mut()
                    .find(|slot| slot.is_none())
                    .expect("an open slot is left");
                *slot = Some(message);
                open -= 1;
            }
            message => {
                transcript
                    .extend(mem::take(&mut filling).into_iter().flatten());
                transcript.push(message);
            }
        }
    }
    transcript.extend(filling.into_iter().flatten());
    Ok(latest.into_iter().chain(transcript).collect())
}

/// `entries` as they happened: each rewrite stays in place, and the
/// messages it made, stored right after it, are left out.
fn happened(entries: Vec<Entry>) -> Vec<Entry> {
    let mut made = 0;
    entries
        .into_iter()
        .filter(|entry| match entry {
            Entry::Context { layout, .. } => {
                made = layout.iter().filter(|slot| slot.is_none()).count();
                true
            }
            Entry::Message { .. } if made > 0 => {
                made -= 1;
                false
            }
            _ => true,
        })
        .collect()
}
