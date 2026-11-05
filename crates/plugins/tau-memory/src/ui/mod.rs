//! tau-memory's UI (ADR 0017): each repository's notes and the user's
//! on their pages, an entry for each in the sidebar and in the
//! repository's menu, a key to the repository's (ctrl-2), what a run
//! recalled and saved in its transcript, its line in the run's plugin
//! list, and its settings: the model it writes notes with.
//!
//! A repository's notes live in tau's directory for it, beside its
//! rules; the user's, across repositories, in tau's data directory.
//! Embeddings are cached beside each. Every scope is opened once and
//! shared by every run.
//!
//! Everything it draws is a view tree (ADR 0045); its buttons and
//! switches name their tree's action ([`BANNER`], [`REVIEW`], [`SETUP`],
//! [`SETTINGS`]) and carry a [`Do`] in their arguments, carried out in
//! [`UiPlugin::on_action`].

pub mod banner;
pub mod page;
pub mod review;
pub mod settings;
pub mod setup;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_ui_kit::assets::Icon;
use tau_ui_plugin::{
    Fold,
    Handle,
    Link,
    Manifest,
    NavEntry,
    Page,
    RunCx,
    UiPlugin,
    ViewCx,
    points::{self, AtApp, AtRepo},
    tree::{Kind, Node, Tone},
};

use crate::{
    import::{Act, Kept, Request},
    note::NoteType,
    record::{NAME, Record},
};

/// tau-memory with its UI: what tau adds to an agent.
#[derive(Debug, Clone, Copy, Default)]
pub struct MemoryUi;

/// What memory knows across repositories: the person's notes, and what
/// imports found and propose (ADR 0044).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryData {
    /// The person's notes, across repositories.
    pub you: Notebook,
    pub imports: crate::import::Imports,
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
            // Bookkeeping for the next run: nothing to show.
            Record::Given { .. } | Record::Forgotten => return,
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

/// The review page of what an import proposes: every proposal, or with
/// `repo`, a repository's or ([`USER`]) the person's.
pub fn review_link(repo: Option<&str>) -> Link {
    let link = Link::page("import");
    match repo {
        Some(repo) => link.param("repo", repo.to_owned()),
        None => link,
    }
}

/// What memory reads outside repositories: what Claude Code and Codex
/// kept, when the person imports it (ADR 0044).
pub const READS: &[&str] = &["~/.claude", "~/.codex"];

impl UiPlugin for MemoryUi {
    type State = State;
    /// The user's notes, across repositories, and what imports found.
    type Data = MemoryData;
    type RepoData = Notebook;
    type Settings = settings::Settings;

    fn name(&self) -> &'static str {
        NAME
    }

    /// What keeping proposals did, for the review page to say.
    fn reply(&self, reply: serde_json::Value, handle: &Handle) {
        if serde_json::from_value::<Kept>(reply.clone()).is_ok() {
            handle.set_ui(KEPT, Some(reply));
        }
    }

    /// Carries out the [`Do`] a button or a switch of one of its trees
    /// holds: saving settings only from the settings pane, choosing and
    /// starting only from the setup step.
    fn on_action(
        &self,
        action: &str,
        args: &Value,
        view: &mut ViewCx<'_, Self>,
    ) {
        if ![BANNER, REVIEW, SETUP, SETTINGS].contains(&action) {
            return;
        }
        let Ok(what) = serde_json::from_value::<Do>(args.clone()) else {
            return;
        };
        let handle = &view.handle;
        match what {
            Do::Act { act } => handle.act(act),
            Do::Import { request } => {
                handle.act(Act::Import(request));
                handle.navigate(review_link(None));
            }
            Do::Open { link } => handle.navigate(link),
            Do::Choose { choice } if action == SETUP => {
                handle.set_ui(setup::CHOICE, serde_json::to_value(choice).ok())
            }
            Do::Start { request } if action == SETUP => {
                if let Some(request) = request {
                    handle.act(Act::Import(request));
                }
                handle.setup_done();
            }
            Do::Save { settings } if action == SETTINGS => {
                view.save_settings(&settings)
            }
            Do::Choose { .. } | Do::Start { .. } | Do::Save { .. } => {}
        }
    }

    fn declaration(&self) -> tau_ui_plugin::Declaration {
        tau_ui_plugin::Declaration::new()
            .seams(&[
                tau_ui_plugin::Seam::Tools,
                tau_ui_plugin::Seam::Start,
                tau_ui_plugin::Seam::StartIdle,
                tau_ui_plugin::Seam::BeforeRequest,
                tau_ui_plugin::Seam::AfterTool,
                tau_ui_plugin::Seam::Rewritten,
                tau_ui_plugin::Seam::Finish,
            ])
            .services(&[
                tau_ui_plugin::services::names::TURN_HOOKS,
                crate::SEARCH,
                tau_ui_plugin::services::names::MODEL,
            ])
            .reads(READS)
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .page(Page::new("notes", page::render).title(page::title).framed())
            .page(
                Page::new("import", review::render)
                    .title(review::title)
                    .framed(),
            )
            .contribute_at(points::SIDEBAR, 10, |_: &AtApp, view| {
                Some(
                    NavEntry::new("Your notes", Icon::Memory, notes_link(USER))
                        .detail(format!("{} notes", view.data.you.notes.len())),
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
            // The first of the plugins' entries, right after tau's own.
            .contribute_at(points::REPO_MENU, -10, page::menu_entry)
            .contribute_at(points::REPO_MENU, -9, banner::menu_entry)
            // The repository's notes, from anywhere in it.
            .key("ctrl-2", |at, view| {
                if let Some(repo) = &at.repo {
                    view.handle.navigate(notes_link(repo));
                }
            })
            .contribute(points::SETUP, setup::step)
            .status(State::status)
            .contribute(points::TRANSCRIPT, page::mark)
            .settings(settings::pane)
    }
}

/// The notes page's search input.
pub const SEARCH: &str = "search";

/// The UI state key of what keeping proposals last did, as the host half
/// answered ([`Kept`]).
pub const KEPT: &str = "kept";

/// The review page's search input.
pub const REVIEW_SEARCH: &str = "review-search";

/// The action of the banner's buttons and the ways into an import, on
/// the notes page and in a repository's menu.
pub const BANNER: &str = "banner";

/// The action of the review page's buttons and switches.
pub const REVIEW: &str = "review";

/// The action of the setup step's buttons and switches.
pub const SETUP: &str = "setup";

/// The action of the settings pane's buttons and switches.
pub const SETTINGS: &str = "settings";

/// What a button or a switch of memory's asks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum Do {
    /// Asks the host half for `act`.
    Act { act: Act },
    /// Starts an import, and opens the review page to watch it.
    Import { request: Request },
    /// Opens a page.
    Open { link: Link },
    /// Keeps what the setup step has chosen so far.
    Choose { choice: setup::Choice },
    /// Starts `request`, when there is one, and ends the setup step.
    Start { request: Option<Request> },
    /// Saves the settings pane's settings.
    Save { settings: settings::Settings },
}

impl Do {
    pub fn args(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

/// A button of the tree whose action is `action`, that asks for `what`.
pub fn button(
    action: &str,
    label: impl Into<String>,
    kind: Kind,
    what: &Do,
) -> Node {
    Node::Button {
        label: label.into(),
        action: action.into(),
        args: what.args(),
        kind,
        icon: None,
    }
}

/// An icon of tau's, by its file's name.
pub fn icon(name: &str, tone: Tone) -> Node {
    Node::Icon {
        name: name.into(),
        tone,
    }
}
