//! A Luau plugin's own store (ADR 0048, the first seam): keys and JSON
//! values, one SQLite database per plugin and repository, kept by the
//! host, apart from tau's store. Luau sees it as `ctx.store`, never a
//! file or SQL.
//!
//! A hook works in a [`Session`]: hooks on one store take turns, what a
//! hook writes it reads back at once, and it all goes in one transaction
//! when the hook ends, or none of it when the hook fails. So a hook that
//! reads and then writes, as claiming an issue does, is one step.

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    str::FromStr,
    sync::{
        Arc,
        Mutex,
        atomic::{AtomicI64, Ordering},
    },
    time::Duration,
};

use serde_json::Value;
use sqlx::{
    Row,
    sqlite::{
        SqliteConnectOptions,
        SqliteJournalMode,
        SqlitePool,
        SqlitePoolOptions,
        SqliteSynchronous,
    },
};

/// Lets a number of writes through, then fails every one after: as if
/// tau died there. For the crash model.
#[derive(Debug, Default)]
pub struct Fuse {
    /// Writes left; negative is no limit.
    left: AtomicI64,
}

impl Fuse {
    /// A fuse that lets `writes` through.
    pub fn after(writes: i64) -> Arc<Self> {
        Arc::new(Self {
            left: AtomicI64::new(writes),
        })
    }

    /// A fuse that never blows.
    pub fn never() -> Arc<Self> {
        Self::after(-1)
    }

    fn take(&self) -> Result<(), String> {
        let left = self.left.load(Ordering::SeqCst);
        if left < 0 {
            return Ok(());
        }
        if left == 0 {
            return Err("the store cannot write: tau closed".into());
        }
        self.left.fetch_sub(1, Ordering::SeqCst);
        Ok(())
    }

    /// Whether it blew.
    pub fn blown(&self) -> bool {
        self.left.load(Ordering::SeqCst) == 0
    }
}

/// Hears that a store changed: what draws from it draws again.
pub type Changed = Arc<dyn Fn() + Send + Sync>;

/// One plugin's store in one repository.
pub struct Kv {
    pool: SqlitePool,
    /// Hooks take turns.
    turn: Arc<tokio::sync::Mutex<()>>,
    fuse: Arc<Fuse>,
    changed: Option<Changed>,
}

impl std::fmt::Debug for Kv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Kv").finish_non_exhaustive()
    }
}

impl Kv {
    /// The store at `path`, made when missing.
    pub async fn open(path: &Path, fuse: Arc<Fuse>) -> Result<Self, String> {
        if let Some(dir) = path.parent() {
            tokio::fs::create_dir_all(dir)
                .await
                .map_err(|error| error.to_string())?;
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(5));
        Self::connect(options, 0, fuse).await
    }

    /// A store in memory, for tests.
    pub async fn memory(fuse: Arc<Fuse>) -> Result<Self, String> {
        let options = SqliteConnectOptions::from_str("sqlite::memory:")
            .map_err(|error| error.to_string())?;
        Self::connect(options, 1, fuse).await
    }

    async fn connect(
        options: SqliteConnectOptions,
        kept: u32,
        fuse: Arc<Fuse>,
    ) -> Result<Self, String> {
        let pool = SqlitePoolOptions::new()
            .min_connections(kept)
            .max_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .acquire_timeout(Duration::from_secs(u64::MAX / 4))
            .connect_with(options)
            .await
            .map_err(|error| error.to_string())?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS kv (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        )
        .execute(&pool)
        .await
        .map_err(|error| error.to_string())?;
        Ok(Self {
            pool,
            turn: Arc::default(),
            fuse,
            changed: None,
        })
    }

    /// The store, telling `changed` after each commit that wrote.
    pub fn with_changed(mut self, changed: Option<Changed>) -> Self {
        self.changed = changed;
        self
    }

    /// A hook's session: its turn, from now until it is dropped.
    pub async fn session(self: &Arc<Self>) -> Session {
        let turn = self.turn.clone().lock_owned().await;
        Session {
            kv: self.clone(),
            _turn: turn,
            written: BTreeMap::new(),
        }
    }

    async fn read(&self, key: &str) -> Result<Option<Value>, String> {
        let row = sqlx::query("SELECT value FROM kv WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .map_err(|error| error.to_string())?;
        row.map(|row| decode(&row.get::<String, _>(0))).transpose()
    }

    async fn read_prefix(
        &self,
        prefix: &str,
    ) -> Result<BTreeMap<String, Value>, String> {
        let rows = sqlx::query(
            "SELECT key, value FROM kv WHERE substr(key, 1, ?) = ?",
        )
        .bind(prefix.chars().count() as i64)
        .bind(prefix)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| error.to_string())?;
        rows.iter()
            .map(|row| {
                Ok((row.get::<String, _>(0), decode(&row.get::<String, _>(1))?))
            })
            .collect()
    }

    /// Every key and value, for tests and export.
    pub async fn everything(&self) -> Result<BTreeMap<String, Value>, String> {
        self.read_prefix("").await
    }
}

fn decode(text: &str) -> Result<Value, String> {
    serde_json::from_str(text)
        .map_err(|error| format!("a stored value: {error}"))
}

/// What one hook reads and writes of a store, in its turn.
#[derive(Debug)]
pub struct Session {
    kv: Arc<Kv>,
    _turn: tokio::sync::OwnedMutexGuard<()>,
    /// What it wrote: a value, or `None` for a key it deleted.
    written: BTreeMap<String, Option<Value>>,
}

impl Session {
    pub async fn get(&self, key: &str) -> Result<Option<Value>, String> {
        match self.written.get(key) {
            Some(written) => Ok(written.clone()),
            None => self.kv.read(key).await,
        }
    }

    pub fn put(&mut self, key: &str, value: Value) {
        self.written.insert(key.to_owned(), Some(value));
    }

    pub fn delete(&mut self, key: &str) {
        self.written.insert(key.to_owned(), None);
    }

    /// Every key starting with `prefix`, with its value, in key order.
    pub async fn list(
        &self,
        prefix: &str,
    ) -> Result<Vec<(String, Value)>, String> {
        let mut found = self.kv.read_prefix(prefix).await?;
        for (key, value) in &self.written {
            if !key.starts_with(prefix) {
                continue;
            }
            match value {
                Some(value) => {
                    found.insert(key.clone(), value.clone());
                }
                None => {
                    found.remove(key);
                }
            }
        }
        Ok(found.into_iter().collect())
    }

    /// Writes what the hook wrote, in one transaction.
    pub async fn commit(self) -> Result<(), String> {
        if self.written.is_empty() {
            return Ok(());
        }
        let fail = |error: sqlx::Error| error.to_string();
        let mut tx = self.kv.pool.begin().await.map_err(fail)?;
        for (key, value) in &self.written {
            self.kv.fuse.take()?;
            match value {
                Some(value) => {
                    sqlx::query(
                        "INSERT INTO kv (key, value) VALUES (?, ?) \
                         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                    )
                    .bind(key)
                    .bind(value.to_string())
                    .execute(&mut *tx)
                    .await
                    .map_err(fail)?;
                }
                None => {
                    sqlx::query("DELETE FROM kv WHERE key = ?")
                        .bind(key)
                        .execute(&mut *tx)
                        .await
                        .map_err(fail)?;
                }
            }
        }
        self.kv.fuse.take()?;
        tx.commit().await.map_err(fail)?;
        if let Some(changed) = &self.kv.changed {
            changed();
        }
        Ok(())
    }
}

/// The name of a plugin's store of the whole app, not of a repository:
/// what its `app`, its jobs and its page of the app keep (ADR 0050, gap
/// H). No repository's name starts with a dot.
pub const APP: &str = ".app";

/// Every Luau plugin's stores, opened as they are first asked for: in
/// `dir`, `<plugin>/<repo>.db`, or in memory.
#[derive(Default)]
pub struct Stores {
    dir: Option<PathBuf>,
    open: tokio::sync::Mutex<HashMap<(String, String), Arc<Kv>>>,
    /// By plugin, the repositories its folder held when first looked at:
    /// what it gets since is opened here, so a job that asks each second
    /// reads the folder once.
    on_disk: tokio::sync::Mutex<HashMap<String, Vec<String>>>,
    fuse: Mutex<Option<Arc<Fuse>>>,
    changed: Option<Changed>,
}

impl std::fmt::Debug for Stores {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stores")
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

impl Stores {
    pub fn at(dir: PathBuf) -> Self {
        Self {
            dir: Some(dir),
            ..Self::default()
        }
    }

    /// Stores in memory, for tests.
    pub fn memory() -> Self {
        Self::default()
    }

    /// Stores that tell `changed` after each commit that wrote.
    pub fn with_changed(mut self, changed: Changed) -> Self {
        self.changed = Some(changed);
        self
    }

    /// Stores opened from now on write through `fuse`: for the crash
    /// model.
    pub fn with_fuse(self, fuse: Arc<Fuse>) -> Self {
        *self.fuse.lock().expect("not poisoned") = Some(fuse);
        self
    }

    /// `plugin`'s store in the repository `repo`.
    pub async fn of(
        &self,
        plugin: &str,
        repo: &str,
    ) -> Result<Arc<Kv>, String> {
        let key = (plugin.to_owned(), repo.to_owned());
        let mut open = self.open.lock().await;
        if let Some(kv) = open.get(&key) {
            return Ok(kv.clone());
        }
        let fuse = self
            .fuse
            .lock()
            .expect("not poisoned")
            .clone()
            .unwrap_or_else(Fuse::never);
        let kv = match &self.dir {
            Some(dir) => {
                let file = dir.join(plugin).join(format!("{}.db", safe(repo)));
                Kv::open(&file, fuse).await?
            }
            None => Kv::memory(fuse).await?,
        };
        let kv = Arc::new(kv.with_changed(self.changed.clone()));
        open.insert(key, kv.clone());
        Ok(kv)
    }

    /// The repositories `plugin` keeps a store in: on disk, and opened;
    /// its store of the app ([`APP`]) is no repository's.
    /// The folder is read the first time; stores made since are opened.
    pub async fn repos_of(&self, plugin: &str) -> Vec<String> {
        let mut repos: Vec<String> = self
            .open
            .lock()
            .await
            .keys()
            .filter(|(of, _)| of == plugin)
            .map(|(_, repo)| repo.clone())
            .collect();
        let mut on_disk = self.on_disk.lock().await;
        if !on_disk.contains_key(plugin) {
            let mut found = Vec::new();
            if let Some(dir) = &self.dir
                && let Ok(mut files) =
                    tokio::fs::read_dir(dir.join(plugin)).await
            {
                while let Ok(Some(file)) = files.next_entry().await {
                    let name = file.file_name().to_string_lossy().into_owned();
                    if let Some(repo) =
                        name.strip_suffix(".db").and_then(unsafe_name)
                    {
                        found.push(repo);
                    }
                }
            }
            on_disk.insert(plugin.to_owned(), found);
        }
        for repo in &on_disk[plugin] {
            if !repos.contains(repo) {
                repos.push(repo.clone());
            }
        }
        repos.retain(|repo| repo != APP);
        repos.sort();
        repos
    }

    /// Forgets the stores opened, as a new process would.
    pub async fn close(&self) {
        self.open.lock().await.clear();
        self.on_disk.lock().await.clear();
    }
}

/// The repository a file's name `safe` stands for: [`safe`] read back.
fn unsafe_name(safe: &str) -> Option<String> {
    let mut bytes = Vec::new();
    let mut rest = safe.bytes();
    while let Some(byte) = rest.next() {
        if byte == b'%' {
            let hex = [rest.next()?, rest.next()?];
            bytes.push(
                u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?,
            );
        } else {
            bytes.push(byte);
        }
    }
    String::from_utf8(bytes).ok()
}

/// `repo` as a file's name: what is not a letter, a digit, `-`, `_` or
/// `.` written as `%xx`.
fn safe(repo: &str) -> String {
    repo.bytes()
        .map(|byte| match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02x}"),
        })
        .collect()
}
