//! tau-memory's host half (ADR 0030): every scope open this session,
//! the notes they hold as their pages show them, and the plugin for
//! each run.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use tau_agent::plugin::Plugin;
use tau_memory::{
    MemoryUi,
    note::{By, LinkType, Note as MemoryNote},
    record::Record,
    ui::{LinkView, NoteView, Notebook, ago, notes_link, settings::Settings},
};
use tau_ui_plugin::{
    HostCx,
    HostHalf,
    PluginHost,
    PluginInfo,
    RepoCtx,
    RunCtx,
    Seam,
    TurnCommit,
    TurnHooks,
};

use crate::{
    Memory,
    MemoryPlugin,
    Scopes,
    index::{Bm25, Index},
    plugin::{Scope, Writer},
    store::Notes,
};

/// How a host searches notes: by meaning, with docbert's model, or by
/// keywords alone, as in tests, where no model is loaded. The host puts
/// one in its services; without one, keywords.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Search {
    Semantic,
    #[default]
    Keywords,
}

/// The plugin on the host: every scope open this session, and where the
/// user's notes are.
pub struct Host {
    memories: Memories,
    user: PathBuf,
}

/// The scopes open this session, by their notes directory.
pub struct Memories {
    /// What semantic search encodes with, loaded on first use; `None`
    /// searches by keywords alone.
    #[cfg(feature = "docbert")]
    encoder: Option<crate::colbert::Shared<crate::docbert::Docbert>>,
    open: Mutex<HashMap<PathBuf, Scope>>,
}

impl Memories {
    /// Memories that search as `search` says; by keywords when built
    /// without docbert's model.
    #[cfg_attr(not(feature = "docbert"), allow(unused_variables))]
    pub fn new(search: Search) -> Self {
        Self {
            #[cfg(feature = "docbert")]
            encoder: (search == Search::Semantic).then(|| {
                crate::colbert::Shared::new(crate::docbert::Docbert::new())
            }),
            open: Mutex::default(),
        }
    }

    /// The scope whose notes are in `dir`, opened on first use.
    pub fn scope(&self, dir: &Path) -> anyhow::Result<Scope> {
        let mut open = self.open.lock().expect("not poisoned");
        if let Some(scope) = open.get(dir) {
            return Ok(scope.clone());
        }
        #[cfg(feature = "docbert")]
        let index: Box<dyn Index> = match &self.encoder {
            Some(encoder) => Box::new(
                crate::colbert::Colbert::new(encoder.clone())
                    .cached(embeddings_dir(dir)),
            ),
            None => Box::new(Bm25::new()),
        };
        #[cfg(not(feature = "docbert"))]
        let index: Box<dyn Index> = Box::new(Bm25::new());
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

    /// The notes in `dir` as their page shows them: from the open scope
    /// when there is one, else read from the files.
    pub fn notebook(&self, dir: &Path) -> Notebook {
        let open = self.open.lock().expect("not poisoned").get(dir).cloned();
        let now = now();
        match open {
            Some(scope) => {
                let memory = scope.lock().expect("not poisoned");
                notebook(dir, memory.notes(), now)
            }
            None => match Notes::open(dir) {
                Ok(notes) => notebook(dir, &notes, now),
                Err(_) => Notebook {
                    path: dir.display().to_string(),
                    ..Notebook::default()
                },
            },
        }
    }
}

/// Where the embeddings of the notes in `dir` are cached: beside it, so
/// the notes directory holds notes alone.
#[cfg_attr(not(feature = "docbert"), allow(dead_code))]
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

/// Marks notes about the files each turn changed as possibly stale.
/// Notes written since the previous turn (or since the run began) are
/// left alone: the change may have come before them.
pub fn stale_on_turn(
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
                eprintln!("tau-memory: cannot mark notes stale: {error:#}");
            }
        };
        // Off the async workers: a search may hold the scope a while.
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => drop(runtime.spawn_blocking(mark)),
            Err(_) => mark(),
        }
    }
}

/// The current notes in `notes`, newest first, as their page lists them.
/// Superseded notes stay in their history, off the list.
pub fn notebook(dir: &Path, notes: &Notes, now: u64) -> Notebook {
    let mut current: Vec<&MemoryNote> =
        notes.iter().filter(|note| !note.is_superseded()).collect();
    current.sort_by(|a, b| b.updated.cmp(&a.updated).then(a.id.cmp(&b.id)));
    Notebook {
        path: dir.display().to_string(),
        notes: current.into_iter().map(|note| entry(note, now)).collect(),
    }
}

fn entry(note: &MemoryNote, now: u64) -> NoteView {
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
    NoteView {
        id: note.id.clone(),
        title: note.title.clone(),
        kind: note.kind,
        body,
        links: note
            .all_links()
            .into_iter()
            .filter(|link| link.kind != LinkType::About)
            .map(|link| LinkView {
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
    }
}

/// tau-memory on the host.
#[derive(Debug, Clone, Copy, Default)]
pub struct MemoryHost;

impl HostHalf for MemoryHost {
    type Plugin = MemoryUi;
    type Host = Host;

    /// Notes for a run and its sub-agents alike, marked stale when a
    /// turn changes what they are about. Notes that cannot be opened
    /// leave memory out of the run, which goes on.
    async fn agent_plugins(
        &self,
        host: &Host,
        run: &RunCtx,
        settings: &Settings,
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        let plugin = match host
            .memories
            .plugin(&repo_dir(&run.repo), &host.user)
        {
            Ok(plugin) => plugin,
            Err(error) => {
                eprintln!("tau-memory: memory is off for this run: {error:#}");
                return Ok(Vec::new());
            }
        };
        let plugin = plugin.writer(
            settings
                .model()
                .map(|(model, reasoning)| Writer { model, reasoning }),
        );
        if let Some(hooks) = run.services.get::<TurnHooks>() {
            hooks.on_turn(stale_on_turn(plugin.clone()));
        }
        Ok(vec![Box::new(plugin)])
    }

    async fn starting(
        &self,
        host: &Host,
        run: &RunCtx,
        _settings: &Settings,
    ) -> Vec<Record> {
        let notes = host.memories.notebook(&repo_dir(&run.repo)).notes.len();
        vec![Record::Starting { notes }]
    }

    async fn catalog(
        &self,
        _host: &Host,
        _cx: &HostCx,
        _settings: &Settings,
    ) -> PluginInfo {
        PluginInfo {
            group: tau_ui_plugin::Group::Context,
            description: "Linked notes each repository's runs keep, and yours \
                          across them; searched at the start of a run"
                .into(),
            seams: vec![
                Seam::Start,
                Seam::Tools,
                Seam::AfterTool,
                Seam::Rewrite,
            ],
            page: Some(notes_link("")),
            ..Default::default()
        }
    }

    async fn data(&self, host: &Host, _cx: &HostCx) -> Notebook {
        host.memories.notebook(&host.user)
    }

    async fn repo_data(
        &self,
        host: &Host,
        repo: &RepoCtx,
        _cx: &HostCx,
    ) -> Notebook {
        host.memories.notebook(&repo_dir(repo))
    }
}

impl PluginHost for Host {
    async fn new(cx: &HostCx) -> anyhow::Result<Self> {
        let search = cx.services.get::<Search>().copied().unwrap_or_default();
        Ok(Host {
            memories: Memories::new(search),
            user: cx.dir.join("memory"),
        })
    }
}

/// Where a repository's notes are kept: in tau's directory for it,
/// beside its rules, out of the repository's history.
pub(crate) fn repo_dir(repo: &RepoCtx) -> PathBuf {
    repo.dir.join("memory")
}
