//! tau-constitution's own database (ADR 0006): each repository's
//! constitution and what a person reviewed, in a SQLite file of the
//! plugin's, apart from tau's store.

use std::{path::Path, str::FromStr, time::Duration};

use sqlx::sqlite::{
    SqliteConnectOptions,
    SqliteJournalMode,
    SqlitePool,
    SqlitePoolOptions,
    SqliteSynchronous,
};

use crate::{Constitution, RuleError, StoredConstitution, StoredRule};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error(transparent)]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
    /// Opens the database at `path`, creating it and its tables when
    /// they are missing.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, DbError> {
        if let Some(dir) = path.as_ref().parent() {
            std::fs::create_dir_all(dir).map_err(sqlx::Error::Io)?;
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(5))
            .foreign_keys(true);
        Self::connect(options, 4).await
    }

    /// An in-memory database, for tests: one connection, since each
    /// connection to `sqlite::memory:` is its own database, and no wait
    /// for it times out, since tests may pause time.
    pub async fn memory() -> Result<Self, DbError> {
        let options = SqliteConnectOptions::from_str("sqlite::memory:")?
            .foreign_keys(true);
        Self::connect(options, 1).await
    }

    async fn connect(
        options: SqliteConnectOptions,
        connections: u32,
    ) -> Result<Self, DbError> {
        let pool = SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(connections)
            .idle_timeout(None)
            .max_lifetime(None)
            .acquire_timeout(Duration::from_secs(u64::MAX / 4))
            .connect_with(options)
            .await?;
        MIGRATOR.run(&pool).await?;
        Ok(Self { pool })
    }

    /// Repository `repo`'s constitution, when one was saved.
    pub async fn constitution(
        &self,
        repo: &str,
    ) -> Result<Option<StoredConstitution>, DbError> {
        let Some(head) = sqlx::query!(
            r#"SELECT on_error AS "on_error!: String", max_holds AS "max_holds!: i64"
               FROM constitutions WHERE repo = ?1"#,
            repo
        )
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };
        let rules = sqlx::query!(
            r#"SELECT id AS "id!: String", text AS "text!: String",
                      targets AS "targets!: String",
                      review AS "review!: f64", block AS "block!: f64"
               FROM constitution_rules WHERE repo = ?1 ORDER BY position"#,
            repo
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| {
            Ok(StoredRule {
                id: row.id,
                text: row.text,
                targets: serde_json::from_str(&row.targets)?,
                review: row.review,
                block: row.block,
            })
        })
        .collect::<Result<Vec<_>, DbError>>()?;
        Ok(Some(StoredConstitution {
            on_error: head.on_error,
            max_holds: u32::try_from(head.max_holds).unwrap_or(u32::MAX),
            rules,
        }))
    }

    /// Replaces repository `repo`'s constitution with `constitution`, in
    /// one transaction: a reader sees the old rules or the new ones.
    pub async fn save_constitution(
        &self,
        repo: &str,
        constitution: &StoredConstitution,
    ) -> Result<(), DbError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let max_holds = i64::from(constitution.max_holds);
        sqlx::query!(
            "INSERT INTO constitutions (repo, on_error, max_holds, updated_at)
             VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
             ON CONFLICT (repo) DO UPDATE SET
               on_error = excluded.on_error,
               max_holds = excluded.max_holds,
               updated_at = excluded.updated_at",
            repo,
            constitution.on_error,
            max_holds,
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query!("DELETE FROM constitution_rules WHERE repo = ?1", repo)
            .execute(&mut *tx)
            .await?;
        for (position, rule) in constitution.rules.iter().enumerate() {
            let position = position as i64;
            let targets = serde_json::to_string(&rule.targets)?;
            sqlx::query!(
                "INSERT INTO constitution_rules
                   (repo, id, position, text, targets, review, block)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                repo,
                rule.id,
                position,
                rule.text,
                targets,
                rule.review,
                rule.block,
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// The flagged calls and answers a person found fine, as `(run, key)`,
    /// in the order they were marked.
    pub async fn reviewed(&self) -> Result<Vec<(String, String)>, DbError> {
        Ok(sqlx::query!(
            r#"SELECT run AS "run!: String", key AS "key!: String"
               FROM reviewed ORDER BY rowid"#
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| (row.run, row.key))
        .collect())
    }

    /// Marks `key` of `run` as reviewed; marking it again changes nothing.
    pub async fn mark_reviewed(
        &self,
        run: &str,
        key: &str,
    ) -> Result<(), DbError> {
        sqlx::query!(
            "INSERT INTO reviewed (run, key) VALUES (?1, ?2)
             ON CONFLICT DO NOTHING",
            run,
            key,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

/// A constitution that could not be loaded or saved.
#[derive(Debug, thiserror::Error)]
pub enum ConstitutionError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("The constitution stored for {repo} is not valid: {rule}")]
    Invalid {
        repo: String,
        #[source]
        rule: RuleError,
    },
}

/// Repository `repo`'s constitution; none saved is no rules.
pub async fn load(
    db: &Db,
    repo: &str,
) -> Result<Constitution, ConstitutionError> {
    match db.constitution(repo).await? {
        Some(stored) => Constitution::from_stored(stored).map_err(|rule| {
            ConstitutionError::Invalid {
                repo: repo.to_owned(),
                rule,
            }
        }),
        None => Ok(Constitution::default()),
    }
}

/// Saves `constitution` as repository `repo`'s, replacing the one before
/// it.
pub async fn save(
    db: &Db,
    repo: &str,
    constitution: &Constitution,
) -> Result<(), ConstitutionError> {
    Ok(db
        .save_constitution(repo, &constitution.to_stored())
        .await?)
}
