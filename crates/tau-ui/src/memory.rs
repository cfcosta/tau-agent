//! tau-memory in the app: each repository's notes and the user's, open
//! once and shared by every run, the Memory screen's view of them, and
//! stale marks from each turn's commit.
//!
//! A repository's notes live in tau's directory for it, beside its
//! constitution; the user's, across repositories, in tau's data
//! directory. Embeddings are cached beside each.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use tau_memory::{
    Memory,
    MemoryPlugin,
    Scopes,
    colbert::{Colbert, Shared},
    docbert::Docbert,
    index::{Bm25, Index},
    note::{By, LinkType, Note as MemoryNote},
    plugin::Scope,
    store::Notes,
};
use tau_vcs::TurnCommit;

use crate::catalog::{Link, Memory as CatalogMemory, Note};

/// The scopes open this session, by their notes directory.
pub struct Memories {
    /// What semantic search encodes with; `None` searches by keywords
    /// alone, as in tests, where no model is loaded.
    encoder: Option<Shared<Docbert>>,
    open: Mutex<HashMap<PathBuf, Scope>>,
}

impl Memories {
    /// Memories that search with docbert's model, loaded on first use.
    pub fn semantic() -> Self {
        Self::with_encoder(Some(Shared::new(Docbert::new())))
    }

    /// Memories that search by keywords alone.
    pub fn keywords() -> Self {
        Self::with_encoder(None)
    }

    fn with_encoder(encoder: Option<Shared<Docbert>>) -> Self {
        Self {
            encoder,
            open: Mutex::default(),
        }
    }

    /// The scope whose notes are in `dir`, opened on first use.
    pub fn scope(&self, dir: &Path) -> anyhow::Result<Scope> {
        let mut open = self.open.lock().expect("not poisoned");
        if let Some(scope) = open.get(dir) {
            return Ok(scope.clone());
        }
        let index: Box<dyn Index> = match &self.encoder {
            Some(encoder) => Box::new(
                Colbert::new(encoder.clone()).cached(embeddings_dir(dir)),
            ),
            None => Box::new(Bm25::new()),
        };
        let scope = Arc::new(Mutex::new(Memory::open(dir, index)?));
        open.insert(dir.to_owned(), scope.clone());
        Ok(scope)
    }

    /// The plugin for a run in the repository whose notes are in `repo`,
    /// with the user's notes in `user`.
    pub fn plugin(
        &self,
        repo: &Path,
        user: &Path,
    ) -> anyhow::Result<MemoryPlugin> {
        Ok(MemoryPlugin::new(Scopes {
            repo: self.scope(repo)?,
            user: Some(self.scope(user)?),
            clock: Arc::new(now),
        }))
    }

    /// The notes in `dir` as the Memory screen shows them: from the open
    /// scope when there is one, else read from the files.
    pub fn catalog(&self, dir: &Path) -> CatalogMemory {
        let open = self.open.lock().expect("not poisoned").get(dir).cloned();
        let now = now();
        match open {
            Some(scope) => {
                let memory = scope.lock().expect("not poisoned");
                catalog(dir, memory.notes(), now)
            }
            None => match Notes::open(dir) {
                Ok(notes) => catalog(dir, &notes, now),
                Err(_) => CatalogMemory {
                    path: dir.display().to_string(),
                    ..CatalogMemory::default()
                },
            },
        }
    }
}

/// Where the embeddings of the notes in `dir` are cached: beside it, so
/// the notes directory holds notes alone.
fn embeddings_dir(dir: &Path) -> PathBuf {
    let name = dir.file_name().map_or_else(
        || "memory".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    dir.with_file_name(format!("{name}-embeddings"))
}

/// Milliseconds since the epoch.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

/// Marks notes about the files each turn's commit changed as possibly
/// stale, for [`tau_vcs::RunWorkspace::on_commit`]. Notes written since
/// the previous commit (or since the run began) are left alone: the
/// change may have come before them.
pub fn stale_on_commit(
    plugin: MemoryPlugin,
) -> impl Fn(&TurnCommit) + Send + Sync + 'static {
    let since = Arc::new(Mutex::new(plugin.now()));
    move |commit| {
        let written_by = std::mem::replace(
            &mut *since.lock().expect("not poisoned"),
            plugin.now(),
        );
        if commit.paths.is_empty() {
            return;
        }
        let why = format!(
            "commit {} changed it",
            commit.change_id.get(..12).unwrap_or(&commit.change_id)
        );
        let (plugin, paths) = (plugin.clone(), commit.paths.clone());
        let mark = move || {
            if let Err(error) = plugin.mark_stale(&paths, &why, written_by) {
                eprintln!("tau-ui: cannot mark notes stale: {error:#}");
            }
        };
        // Off the async workers: a search may hold the scope a while.
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => drop(runtime.spawn_blocking(mark)),
            Err(_) => mark(),
        }
    }
}

/// The current notes in `notes`, newest first, as the Memory screen
/// lists them. Superseded notes stay in their history, off the list.
pub fn catalog(dir: &Path, notes: &Notes, now: u64) -> CatalogMemory {
    let mut current: Vec<&MemoryNote> =
        notes.iter().filter(|note| !note.is_superseded()).collect();
    current.sort_by(|a, b| b.updated.cmp(&a.updated).then(a.id.cmp(&b.id)));
    CatalogMemory {
        path: dir.display().to_string(),
        collection: String::new(),
        notes: current.into_iter().map(|note| entry(note, now)).collect(),
    }
}

fn entry(note: &MemoryNote, now: u64) -> Note {
    let mut body = Vec::new();
    if let Some(why) = &note.stale {
        body.push(format!("May be stale: {why}."));
    }
    if !note.description.is_empty() {
        body.push(note.description.clone());
    }
    body.extend(
        note.body
            .split("\n\n")
            .map(|paragraph| paragraph.trim().replace('\n', " "))
            .filter(|paragraph| !paragraph.is_empty()),
    );
    let mut paths: Vec<String> = note
        .links
        .iter()
        .filter(|link| link.kind == LinkType::About)
        .map(|link| link.to.clone())
        .chain(note.source.files.iter().cloned())
        .collect();
    paths.sort();
    paths.dedup();
    let by = match note.source.by {
        By::User => "from the user",
        By::Agent => "by the agent",
        By::Inferred => "inferred",
    };
    Note {
        id: note.id.clone(),
        title: note.title.clone(),
        body,
        links: note
            .all_links()
            .into_iter()
            .filter(|link| link.kind != LinkType::About)
            .map(|link| Link {
                why: link
                    .why
                    .clone()
                    .unwrap_or_else(|| link.kind.as_str().replace('_', " ")),
                to: link.to,
            })
            .collect(),
        paths,
        written_by: format!("{} {by}", note.kind.as_str()),
        edited: ago(now, note.updated),
        used_by_runs: 0,
    }
}

/// How long before `now` the time `then` was, both in milliseconds, in
/// the largest whole unit: `just now`, `5 minutes ago`, `2 days ago`.
pub fn ago(now: u64, then: u64) -> String {
    let seconds = now.saturating_sub(then) / 1000;
    let (count, unit) = match seconds {
        0..60 => return "just now".into(),
        60..3_600 => (seconds / 60, "minute"),
        3_600..86_400 => (seconds / 3_600, "hour"),
        86_400..2_592_000 => (seconds / 86_400, "day"),
        2_592_000..31_536_000 => (seconds / 2_592_000, "month"),
        _ => (seconds / 31_536_000, "year"),
    };
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {unit}{plural} ago")
}
