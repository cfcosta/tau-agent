//! One scope's memory: its notes and the index that finds them, kept in
//! step. Every write goes through here, so every write is checked
//! ([`crate::safety`]), indexed, and answered with the notes nearest to
//! it, for the agent to link or supersede instead of repeating.

use std::{fmt, path::PathBuf};

use crate::{
    index::{Index, IndexError, index_text},
    note::{Link, LinkType, Note, NoteType, Source, is_id, slug},
    recall::{Hit, recall},
    safety,
    store::{Notes, StoreError},
};

/// The id of a scope's index note: the map loaded into every run.
pub const INDEX_ID: &str = "index";

/// The index note's size limit, in characters: about 2,000 tokens.
pub const INDEX_BUDGET: usize = 8_000;

/// How many near notes a write reports.
pub const NEAREST: usize = 3;

/// A note to write: new, a new version of one (`id`), or a replacement
/// for one (`supersedes`).
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub kind: NoteType,
    pub title: String,
    pub description: String,
    pub body: String,
    pub tags: Vec<String>,
    pub links: Vec<Link>,
    /// Writes a new version of this note.
    pub id: Option<String>,
    /// Writes a new note that replaces this one.
    pub supersedes: Option<String>,
    pub source: Source,
}

/// What a write did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Created,
    Updated,
    /// Created, replacing this note.
    Superseded(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Written {
    pub id: String,
    pub action: Action,
    /// How many secrets were redacted from it.
    pub redacted: usize,
    /// The notes most like it, itself left out.
    pub nearest: Vec<Hit>,
}

#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    /// The write was not made; the text says why and what to do instead.
    #[error("{0}")]
    Refused(String),
    #[error(transparent)]
    Store(StoreError),
    #[error("index: {0}")]
    Index(#[from] IndexError),
}

impl From<WriteError> for tau_agent::tool::ToolError {
    fn from(error: WriteError) -> Self {
        Self::other(error)
    }
}

/// Why a scope's memory could not open: its notes would not read, or
/// would not index.
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error(transparent)]
    Notes(#[from] std::io::Error),
    #[error(transparent)]
    Index(#[from] IndexError),
}

impl From<StoreError> for WriteError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Note(error) => Self::Refused(error.to_string()),
            other => Self::Store(other),
        }
    }
}

/// A note, with the links into it.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub note: Note,
    pub backlinks: Vec<(String, Link)>,
}

/// One scope's notes and their index.
pub struct Memory {
    notes: Notes,
    index: Box<dyn Index>,
}

impl fmt::Debug for Memory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Memory")
            .field("notes", &self.notes.len())
            .finish()
    }
}

impl Memory {
    /// Reads the notes in `dir` and indexes them all: the index is
    /// derived, and rebuilt from the files on every open.
    pub fn open(
        dir: impl Into<PathBuf>,
        mut index: Box<dyn Index>,
    ) -> Result<Self, OpenError> {
        let notes = Notes::open(dir)?;
        for note in notes.iter() {
            index.upsert(&note.id, &index_text(note))?;
        }
        Ok(Self { notes, index })
    }

    pub fn notes(&self) -> &Notes {
        &self.notes
    }

    /// The scope's index note, when one was written.
    pub fn index_note(&self) -> Option<&Note> {
        self.notes.get(INDEX_ID)
    }

    /// Checks, redacts and stores `draft`, then indexes it.
    pub fn write(
        &mut self,
        draft: Draft,
        now: u64,
    ) -> Result<Written, WriteError> {
        let Draft {
            kind,
            title,
            description,
            body,
            tags,
            links,
            id,
            supersedes,
            source,
        } = draft;
        let whys = links.iter().filter_map(|link| link.why.as_deref());
        for text in [&title, &description, &body]
            .into_iter()
            .map(String::as_str)
            .chain(tags.iter().map(String::as_str))
            .chain(whys)
        {
            if let Some(why) = safety::refusal(text) {
                return Err(WriteError::Refused(format!("not stored: {why}")));
            }
        }
        let mut redacted = 0;
        let mut clean = |text: String| {
            let (text, count) = safety::redact(&text);
            redacted += count;
            text
        };
        let (title, description, body) =
            (clean(title), clean(description), clean(body));
        let tags: Vec<String> = tags.into_iter().map(&mut clean).collect();
        let links: Vec<Link> = links
            .into_iter()
            .map(|link| Link {
                why: link.why.map(&mut clean),
                ..link
            })
            .collect();

        let named = id.is_some();
        let mut target = id.unwrap_or_else(|| {
            if kind == NoteType::Index {
                INDEX_ID.to_owned()
            } else {
                slug(&title)
            }
        });
        // A replacement often keeps the old title; it takes the next free id.
        if supersedes.is_some() && !named {
            let base = target.clone();
            let mut n = 2;
            while self.notes.get(&target).is_some() {
                target = format!("{}-{n}", &base[..base.len().min(60)]);
                n += 1;
            }
        }
        if !is_id(&target) {
            return Err(WriteError::Refused(format!(
                "{target:?} is not a note id: use a-z, 0-9 and -"
            )));
        }
        if (kind == NoteType::Index) != (target == INDEX_ID) {
            return Err(WriteError::Refused(format!(
                "the index note is the only note of type index, and is called {INDEX_ID:?}"
            )));
        }
        if kind == NoteType::Index && body.chars().count() > INDEX_BUDGET {
            return Err(WriteError::Refused(format!(
                "the index note would be {} characters, over its {INDEX_BUDGET}; \
                 rewrite it shorter: one line per note or group",
                body.chars().count()
            )));
        }

        let existing = self.notes.get(&target).cloned();
        // A title that lands on another note's id never overwrites it: the
        // agent names the note it means to change. The index note has one
        // id, so it is always meant.
        if existing.is_some()
            && !named
            && supersedes.is_none()
            && kind != NoteType::Index
        {
            return Err(WriteError::Refused(format!(
                "a note called {target:?} exists; pass id {target:?} to update it, \
                 supersedes {target:?} to replace it, or a different title"
            )));
        }
        let note = Note {
            id: target.clone(),
            title,
            description,
            kind,
            tags,
            created: existing.as_ref().map_or(now, |note| note.created),
            updated: now,
            valid_from: existing.as_ref().map_or(now, |note| note.valid_from),
            valid_to: None,
            stale: None,
            source,
            links,
            body,
        };
        let action = match (&supersedes, &existing) {
            (Some(old), _) => {
                self.notes.supersede(old, note.clone(), now)?;
                Action::Superseded(old.clone())
            }
            (None, Some(_)) => {
                self.notes.update(note.clone())?;
                Action::Updated
            }
            (None, None) => {
                self.notes.create(note.clone())?;
                Action::Created
            }
        };
        let written = self.notes.get(&target).expect("just written").clone();
        self.index.upsert(&target, &index_text(&written))?;
        if let Some(old) = &supersedes
            && let Some(old) = self.notes.get(old)
        {
            self.index.upsert(&old.id, &index_text(old))?;
        }
        let query = format!("{} {}", written.title, written.description);
        let nearest =
            recall(&self.notes, self.index.as_ref(), &query, NEAREST + 1)?
                .into_iter()
                .filter(|hit| hit.id != target)
                .take(NEAREST)
                .collect();
        Ok(Written {
            id: target,
            action,
            redacted,
            nearest,
        })
    }

    /// Links `from` to `to`, typed. The target may be a note not written
    /// yet; for `about`, it is a path.
    pub fn link(
        &mut self,
        from: &str,
        to: &str,
        kind: LinkType,
        why: Option<String>,
        now: u64,
    ) -> Result<(), WriteError> {
        let Some(note) = self.notes.get(from) else {
            return Err(WriteError::Refused(format!(
                "no note is called {from:?}"
            )));
        };
        if let Some(why) = why.as_deref().and_then(safety::refusal) {
            return Err(WriteError::Refused(format!("not stored: {why}")));
        }
        let why = why.map(|why| safety::redact(&why).0);
        if kind != LinkType::About && !is_id(to) {
            return Err(WriteError::Refused(format!(
                "{to:?} is not a note id"
            )));
        }
        let mut note = note.clone();
        note.links.retain(|link| link.to != to);
        note.links.push(Link {
            to: to.to_owned(),
            kind,
            why,
        });
        note.updated = now;
        self.notes.update(note)?;
        Ok(())
    }

    /// Up to `budget` notes for `query`.
    pub fn search(
        &self,
        query: &str,
        budget: usize,
    ) -> Result<Vec<Hit>, IndexError> {
        recall(&self.notes, self.index.as_ref(), query, budget)
    }

    pub fn read(&self, id: &str) -> Option<Reading> {
        let note = self.notes.get(id)?.clone();
        Some(Reading {
            backlinks: self.notes.backlinks(id),
            note,
        })
    }

    /// Marks the notes about any of `paths`, last written at or before
    /// `written_by`, as possibly stale.
    pub fn mark_stale(
        &mut self,
        paths: &[String],
        why: &str,
        written_by: u64,
        now: u64,
    ) -> Result<Vec<String>, StoreError> {
        self.notes.mark_stale(paths, why, written_by, now)
    }
}
