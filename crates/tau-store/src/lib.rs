//! SQLite storage for runs, messages and fork transcripts, through sqlx.
//!
//! See `docs/reference/storage.md`. Every query goes through sqlx's
//! compile-time-checked macros; the offline metadata in `.sqlx/` lets the
//! crate build without a database.
//!
//! Message bodies are opaque JSON here: this crate stores what the agent
//! loop gives it and knows nothing about message shapes beyond the `role`
//! column that queries may filter on.

use std::{
    path::Path,
    str::FromStr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use sqlx::{
    Connection,
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

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("migration error: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error("stored JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unknown run {0}")]
    UnknownRun(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// How a run relates to other runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunKind {
    Root,
    /// Inherits `parent`'s messages with `seq <= fork_seq`.
    Fork {
        parent: String,
        fork_seq: i64,
    },
    /// Started by `parent` as a tool; inherits nothing.
    Subagent {
        parent: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRun<'a> {
    pub id: &'a str,
    pub workflow_id: Option<&'a str>,
    pub agent: &'a str,
    pub kind: RunKind,
    pub model: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    Running,
    Done,
    Failed,
    Cancelled,
    Limit,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Limit => "limit",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "running" => Self::Running,
            "done" => Self::Done,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            // The CHECK constraint allows nothing else.
            _ => Self::Limit,
        }
    }
}

/// One transcript entry. Its body is JSON text, which the store keeps
/// as given: callers serialize straight to text and parse straight from
/// it, with no `Value` in between.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    Message {
        role: String,
        body: String,
    },
    /// A context rewrite by `plugin`: the transcript restarts after it,
    /// with the messages that follow. Loading a transcript drops
    /// everything before the latest one.
    Context {
        plugin: String,
        body: String,
    },
    /// A record `plugin` keeps with the run. It is never part of the
    /// transcript; [`Store::records`] reads it back.
    Plugin {
        plugin: String,
        body: String,
    },
}

/// Token and cost totals added by one turn.
///
/// Token counts are unsigned and 32-bit: a turn never has negative or
/// billions of tokens, and the run totals, stored as SQLite's 64-bit
/// INTEGER, cannot overflow from any realistic number of turns.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TurnUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cost_usd: f64,
}

/// A run as stored.
#[derive(Debug, Clone, PartialEq)]
pub struct RunRecord {
    pub id: String,
    pub workflow_id: Option<String>,
    pub agent: String,
    pub kind: RunKind,
    pub model: String,
    pub status: Status,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
    pub result: Option<String>,
    pub error: Option<String>,
    /// When the run started, as SQLite's `strftime` writes it
    /// (`2026-09-28T14:03:11.402Z`).
    pub created_at: String,
}

/// The cost of one agent's runs in a workflow.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentCost {
    pub agent: String,
    pub runs: i64,
    pub usd: f64,
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
pub struct Store {
    writer: SqlitePool,
    reader: SqlitePool,
    stats: Arc<Mutex<WriterStats>>,
}

impl Store {
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
            .await?;
        MIGRATOR.run(&writer).await?;
        let reader = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(options.read_only(true))
            .await?;
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
        let options = SqliteConnectOptions::from_str("sqlite::memory:")?
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .acquire_timeout(Duration::from_secs(u64::MAX / 4))
            .connect_with(options)
            .await?;
        MIGRATOR.run(&pool).await?;
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
        let connection = self.writer.acquire().await?;
        self.record_wait(started.elapsed());
        Ok(connection)
    }

    fn record_wait(&self, waited: Duration) {
        let mut stats = self.stats.lock().expect("not poisoned");
        stats.writes += 1;
        stats.waited += waited;
        stats.longest = stats.longest.max(waited);
    }

    /// Records a new run with status `running`.
    pub async fn create_run(&self, run: &NewRun<'_>) -> Result<()> {
        let (kind, parent, fork_seq) = match &run.kind {
            RunKind::Root => ("root", None, None),
            RunKind::Fork { parent, fork_seq } => {
                ("fork", Some(parent.as_str()), Some(*fork_seq))
            }
            RunKind::Subagent { parent } => {
                ("subagent", Some(parent.as_str()), None)
            }
        };
        sqlx::query!(
            "INSERT INTO runs (id, workflow_id, agent, kind, parent_run_id, fork_seq,
                               model, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'running',
                     strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                     strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            run.id,
            run.workflow_id,
            run.agent,
            kind,
            parent,
            fork_seq,
            run.model,
        )
        .execute(&mut *self.writer().await?)
        .await?;
        Ok(())
    }

    /// Appends one turn's entries and adds its usage to the run's totals,
    /// in one write transaction: either all of it is stored or none.
    /// Returns the `seq` of the run's last entry afterwards, or -1 when
    /// the run has none.
    pub async fn append_turn(
        &self,
        run: &str,
        entries: &[Entry],
        usage: TurnUsage,
    ) -> Result<i64> {
        // The wait covers the connection and the write lock, which
        // another process can hold.
        let started = Instant::now();
        let mut connection = self.writer.acquire().await?;
        let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
        self.record_wait(started.elapsed());
        let input_tokens = i64::from(usage.input_tokens);
        let output_tokens = i64::from(usage.output_tokens);

        let updated = sqlx::query!(
            "UPDATE runs SET input_tokens = input_tokens + ?2,
                             output_tokens = output_tokens + ?3,
                             cost_usd = cost_usd + ?4,
                             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1",
            run,
            input_tokens,
            output_tokens,
            usage.cost_usd,
        )
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() == 0 {
            return Err(StoreError::UnknownRun(run.to_owned()));
        }

        let next = sqlx::query_scalar!(
            r#"SELECT COALESCE(MAX(seq), -1) + 1 AS "next!: i64"
               FROM messages WHERE run_id = ?1"#,
            run
        )
        .fetch_one(&mut *tx)
        .await?;

        for (offset, entry) in entries.iter().enumerate() {
            let seq = next + offset as i64;
            let (kind, role, plugin, body) = match entry {
                Entry::Message { role, body } => {
                    ("message", Some(role.as_str()), None, body.as_str())
                }
                Entry::Context { plugin, body } => {
                    ("context", None, Some(plugin.as_str()), body.as_str())
                }
                Entry::Plugin { plugin, body } => {
                    ("plugin", None, Some(plugin.as_str()), body.as_str())
                }
            };
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
            .await?;
        }

        tx.commit().await?;
        Ok(next + entries.len() as i64 - 1)
    }

    /// The run's transcript: the inherited messages of its fork chain,
    /// then its own, from the latest context entry onward.
    /// Plugin records are not part of it.
    pub async fn transcript(&self, run: &str) -> Result<Vec<Entry>> {
        let rows = sqlx::query!(
            r#"WITH RECURSIVE chain(run_id, cutoff, depth) AS (
                   SELECT id, NULL, 0 FROM runs WHERE id = ?1
                   UNION ALL
                   SELECT r.parent_run_id, r.fork_seq, chain.depth + 1
                   FROM chain JOIN runs r ON r.id = chain.run_id
                   WHERE r.kind = 'fork'
               )
               SELECT m.kind AS "kind!: String", m.role AS "role?: String",
                      m.plugin AS "plugin?: String", m.body AS "body!: String"
               FROM chain JOIN messages m ON m.run_id = chain.run_id
               WHERE (chain.cutoff IS NULL OR m.seq <= chain.cutoff)
                 AND m.kind != 'plugin'
               ORDER BY chain.depth DESC, m.seq"#,
            run
        )
        .fetch_all(&self.reader)
        .await?;

        let start = rows
            .iter()
            .rposition(|row| row.kind == "context")
            .unwrap_or(0);
        Ok(rows
            .into_iter()
            .skip(start)
            .map(|row| match row.kind.as_str() {
                "context" => Entry::Context {
                    // The loop sets a plugin on every context row.
                    plugin: row.plugin.unwrap_or_default(),
                    body: row.body,
                },
                _ => Entry::Message {
                    // The schema sets a role on every message row.
                    role: row.role.unwrap_or_default(),
                    body: row.body,
                },
            })
            .collect())
    }

    /// The bodies of `plugin`'s records along the run's fork chain, oldest
    /// first: the inherited ones, then the run's own.
    pub async fn records(
        &self,
        run: &str,
        plugin: &str,
    ) -> Result<Vec<String>> {
        let rows = sqlx::query_scalar!(
            r#"WITH RECURSIVE chain(run_id, cutoff, depth) AS (
                   SELECT id, NULL, 0 FROM runs WHERE id = ?1
                   UNION ALL
                   SELECT r.parent_run_id, r.fork_seq, chain.depth + 1
                   FROM chain JOIN runs r ON r.id = chain.run_id
                   WHERE r.kind = 'fork'
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
        .await?;
        Ok(rows)
    }

    /// Sets a run's final status and result or error.
    pub async fn finish_run(
        &self,
        run: &str,
        status: Status,
        result: Option<&str>,
        error: Option<&str>,
    ) -> Result<()> {
        let status = status.as_str();
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
        .await?;
        if updated.rows_affected() == 0 {
            return Err(StoreError::UnknownRun(run.to_owned()));
        }
        Ok(())
    }

    pub async fn run(&self, run: &str) -> Result<Option<RunRecord>> {
        let row = sqlx::query!(
            r#"SELECT id AS "id!: String", workflow_id, agent, kind,
                      parent_run_id, fork_seq, model, status,
                      input_tokens, output_tokens, cost_usd, result, error,
                      created_at
               FROM runs WHERE id = ?1"#,
            run
        )
        .fetch_optional(&self.reader)
        .await?;
        Ok(row.map(|row| RunRecord {
            kind: run_kind(&row.kind, row.parent_run_id, row.fork_seq),
            id: row.id,
            workflow_id: row.workflow_id,
            agent: row.agent,
            model: row.model,
            status: Status::parse(&row.status),
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cost_usd: row.cost_usd,
            result: row.result,
            error: row.error,
            created_at: row.created_at,
        }))
    }

    /// The latest `limit` runs that are not sub-agents, newest first.
    pub async fn recent_runs(&self, limit: u32) -> Result<Vec<RunRecord>> {
        let rows = sqlx::query!(
            r#"SELECT id AS "id!: String", workflow_id, agent, kind,
                      parent_run_id, fork_seq, model, status,
                      input_tokens, output_tokens, cost_usd, result, error,
                      created_at
               FROM runs WHERE kind != 'subagent'
               ORDER BY created_at DESC, id DESC
               LIMIT ?1"#,
            limit
        )
        .fetch_all(&self.reader)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| RunRecord {
                kind: run_kind(&row.kind, row.parent_run_id, row.fork_seq),
                id: row.id,
                workflow_id: row.workflow_id,
                agent: row.agent,
                model: row.model,
                status: Status::parse(&row.status),
                input_tokens: row.input_tokens,
                output_tokens: row.output_tokens,
                cost_usd: row.cost_usd,
                result: row.result,
                error: row.error,
                created_at: row.created_at,
            })
            .collect())
    }

    /// `plugin`'s records in the run itself, not inherited, with the
    /// `seq` each was stored at, oldest first. A fork at a record's `seq`
    /// inherits that record and everything before it.
    pub async fn plugin_entries(
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
        .await?;
        Ok(rows.into_iter().map(|row| (row.seq, row.body)).collect())
    }

    /// Runs and cost per agent in a workflow.
    pub async fn workflow_cost(
        &self,
        workflow: &str,
    ) -> Result<Vec<AgentCost>> {
        Ok(sqlx::query_as!(
            AgentCost,
            r#"SELECT agent AS "agent!: String", count(*) AS "runs!: i64",
                      sum(cost_usd) AS "usd!: f64"
               FROM runs WHERE workflow_id = ?1
               GROUP BY agent ORDER BY agent"#,
            workflow
        )
        .fetch_all(&self.reader)
        .await?)
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
        ("subagent", Some(parent), _) => RunKind::Subagent { parent },
        _ => RunKind::Root,
    }
}
