//! tau-memory's UI (ADR 0017): each repository's notes and the user's
//! on their pages, an entry for each in the sidebar, what a run recalled
//! and saved in its transcript, and its line in the run's plugin list.
//!
//! A repository's notes live in tau's directory for it, beside its
//! rules; the user's, across repositories, in tau's data directory.
//! Embeddings are cached beside each. Every scope is opened once and
//! shared by every run.

pub mod page;

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use gpui::{AppContext as _, Context};
use serde::{Deserialize, Serialize};
use tau_agent::plugin::Plugin;
use tau_ui_kit::{assets::Icon, input::TextInput};
use tau_ui_plugin::{
    Fold,
    Handle,
    HostCx,
    Link,
    Manifest,
    NavEntry,
    Page,
    PluginHost,
    PluginInfo,
    PluginUi,
    RepoCtx,
    RunCtx,
    RunCx,
    Seam,
    TurnCommit,
    TurnHooks,
    UiPlugin,
    points::{self, AtApp, AtRepo},
};

use crate::{
    Memory,
    MemoryPlugin,
    Scopes,
    index::{Bm25, Index},
    note::{By, LinkType, Note as MemoryNote},
    plugin::{NAME, Record, Scope},
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

/// tau-memory with its UI: what tau adds to an agent.
#[derive(Debug, Clone, Copy, Default)]
pub struct MemoryUi;

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

/// A scope's notes, as their page shows them.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Notebook {
    /// The notes directory.
    pub path: String,
    /// The current notes, newest first.
    pub notes: Vec<NoteView>,
}

impl Notebook {
    pub fn note(&self, id: &str) -> Option<&NoteView> {
        self.notes.iter().find(|note| note.id == id)
    }

    pub fn by_title(&self, title: &str) -> Option<&NoteView> {
        self.notes.iter().find(|note| note.title == title)
    }

    /// Notes that link to `id`, with why.
    pub fn backlinks<'a>(
        &'a self,
        id: &'a str,
    ) -> impl Iterator<Item = (&'a NoteView, &'a str)> {
        self.notes.iter().filter_map(move |note| {
            note.links
                .iter()
                .find(|link| link.to == id)
                .map(|link| (note, link.why.as_str()))
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoteView {
    pub id: String,
    pub title: String,
    /// Paragraphs, with `code` in backticks.
    pub body: Vec<String>,
    pub links: Vec<LinkView>,
    /// Files the note is about.
    pub paths: Vec<String>,
    pub written_by: String,
    pub edited: String,
}

impl NoteView {
    /// The first sentence, for lists.
    pub fn snippet(&self) -> &str {
        let first = self.body.first().map(String::as_str).unwrap_or("");
        first.split_inclusive(". ").next().unwrap_or(first)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkView {
    pub to: String,
    pub why: String,
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

/// What memory did in one run.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// How many notes the repository had as the run started.
    pub notes: Option<usize>,
    /// Each note in the transcript, by its anchor.
    pub marks: BTreeMap<String, Mark>,
}

/// One of memory's notes in a transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Mark {
    /// Notes found for the run's task as it started, as `(id, title)`.
    Recalled(Vec<(String, String)>),
    /// Notes written or linked from a conversation about to be compacted,
    /// or at the end of a run.
    Saved(usize),
    Failed(String),
}

impl Fold for State {
    type Record = Record;

    /// Folds one of the plugin's records, or what it says as a run
    /// starts.
    fn apply(&mut self, record: Record, run: &mut dyn RunCx) {
        let mark = match record {
            Record::Starting { notes } => {
                self.notes = Some(notes);
                return;
            }
            Record::Recalled { notes } => Mark::Recalled(
                notes
                    .into_iter()
                    .map(|note| (note.id, note.title))
                    .collect(),
            ),
            Record::Saved { calls } => Mark::Saved(calls.len()),
            Record::Error { message } => Mark::Failed(message),
        };
        let key = format!("m{}", self.marks.len());
        self.marks.insert(key.clone(), mark);
        run.transcript(&key);
    }
}

impl State {
    /// The plugin's line in the run's plugin list.
    pub fn status(&self) -> Option<String> {
        Some(match self.notes? {
            0 => "no notes yet".into(),
            1 => "1 note".into(),
            n => format!("{n} notes"),
        })
    }
}

/// The notes page of `repo`; the user's, across repositories, with
/// [`USER`] as its scope.
pub fn notes_link(repo: &str) -> Link {
    Link::page("notes").param("repo", repo.to_owned())
}

/// The scope parameter of the user's notes page.
pub const USER: &str = "you";

impl UiPlugin for MemoryUi {
    type State = State;
    /// The user's notes, across repositories.
    type Data = Notebook;
    type RepoData = Notebook;
    type Settings = ();
    type Host = Host;
    type Ui = page::Ui;

    fn name(&self) -> &'static str {
        NAME
    }

    /// Notes for a run and its sub-agents alike, marked stale when a
    /// turn changes what they are about. Notes that cannot be opened
    /// leave memory out of the run, which goes on.
    fn agent_plugins(
        &self,
        host: &Host,
        run: &RunCtx,
        _settings: &(),
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
        if let Some(hooks) = run.services.get::<TurnHooks>() {
            hooks.on_turn(stale_on_turn(plugin.clone()));
        }
        Ok(vec![Box::new(plugin)])
    }

    fn starting(
        &self,
        host: &Host,
        run: &RunCtx,
        _settings: &(),
    ) -> Vec<Record> {
        let notes = host.memories.notebook(&repo_dir(&run.repo)).notes.len();
        vec![Record::Starting { notes }]
    }

    fn catalog(
        &self,
        _host: &Host,
        _cx: &HostCx,
        _settings: &(),
    ) -> PluginInfo {
        PluginInfo {
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

    fn data(&self, host: &Host, _cx: &HostCx) -> Notebook {
        host.memories.notebook(&host.user)
    }

    fn repo_data(&self, host: &Host, repo: &RepoCtx, _cx: &HostCx) -> Notebook {
        host.memories.notebook(&repo_dir(repo))
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .page(Page::new("notes", page::render).title(page::title))
            .contribute_at(points::SIDEBAR, 10, |_: &AtApp, view| {
                Some(
                    NavEntry::new("Your notes", Icon::Memory, notes_link(USER))
                        .detail(format!("{} notes", view.data.notes.len())),
                )
            })
            .contribute_at(points::SIDEBAR_REPO, -10, |at: &AtRepo, view| {
                let notes =
                    view.repo(&at.repo).map_or(0, |book| book.notes.len());
                Some(
                    NavEntry::new("Memory", Icon::Memory, notes_link(&at.repo))
                        .detail(format!("{notes} notes")),
                )
            })
            .status(State::status)
            .contribute(points::TRANSCRIPT, page::mark)
    }
}

impl PluginHost for Host {
    fn new(cx: &HostCx) -> anyhow::Result<Self> {
        let search = cx.services.get::<Search>().copied().unwrap_or_default();
        Ok(Host {
            memories: Memories::new(search),
            user: cx.dir.join("memory"),
        })
    }
}

impl PluginUi for page::Ui {
    fn new(_handle: Handle, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| TextInput::new("Search notes", cx));
        cx.observe(&search, |_, _, cx| cx.notify()).detach();
        page::Ui { search }
    }
}

/// Where a repository's notes are kept: in tau's directory for it,
/// beside its rules, out of the repository's history.
fn repo_dir(repo: &RepoCtx) -> PathBuf {
    repo.dir.join("memory")
}
