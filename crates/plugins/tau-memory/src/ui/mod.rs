//! tau-memory's UI (ADR 0017): each repository's notes and the user's
//! on their pages, an entry for each in the sidebar, what a run recalled
//! and saved in its transcript, and its line in the run's plugin list.
//!
//! A repository's notes live in tau's directory for it, beside its
//! rules; the user's, across repositories, in tau's data directory.
//! Embeddings are cached beside each. Every scope is opened once and
//! shared by every run.

pub mod page;

use std::collections::BTreeMap;

use gpui::{AppContext as _, Context};
use serde::{Deserialize, Serialize};
use tau_ui_kit::{assets::Icon, input::TextInput};
use tau_ui_plugin::{
    Fold,
    Handle,
    Link,
    Manifest,
    NavEntry,
    Page,
    PluginUi,
    RunCx,
    UiPlugin,
    points::{self, AtApp, AtRepo},
};

use crate::{
    note::NoteType,
    record::{NAME, Record},
};

/// tau-memory with its UI: what tau adds to an agent.
#[derive(Debug, Clone, Copy, Default)]
pub struct MemoryUi;

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
    /// What the note holds: a fact, a decision, …
    pub kind: NoteType,
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
    type Ui = page::Ui;

    fn name(&self) -> &'static str {
        NAME
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

impl PluginUi for page::Ui {
    fn new(_handle: Handle, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| TextInput::new("Search notes", cx));
        cx.observe(&search, |_, _, cx| cx.notify()).detach();
        page::Ui { search }
    }
}
