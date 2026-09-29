//! A scope's notes: a directory with one file per note at
//! `<type>/<id>.md`, and `.history/<id>/<n>.md` holding every version a
//! write replaced, so any change can be looked at and undone.
//!
//! The files are the truth: [`Notes::open`] reads them all, and anything
//! derived from them (links, backlinks, search) is rebuilt from them. A
//! file that does not read as a note is reported, not dropped.

use std::{
    collections::BTreeMap,
    fmt,
    fs,
    io,
    path::{Path, PathBuf},
};

use crate::note::{Link, LinkType, Note, NoteError, NoteType, is_id};

const HISTORY: &str = ".history";

/// Why a store operation failed.
#[derive(Debug)]
pub enum StoreError {
    Io(io::Error),
    Note(NoteError),
    /// No note has this id.
    Missing(String),
    /// A note with this id exists; writing it again needs an update.
    Exists(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "notes: {error}"),
            Self::Note(error) => write!(f, "{error}"),
            Self::Missing(id) => write!(f, "no note is called {id:?}"),
            Self::Exists(id) => write!(f, "a note is already called {id:?}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<io::Error> for StoreError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<NoteError> for StoreError {
    fn from(error: NoteError) -> Self {
        Self::Note(error)
    }
}

/// A file in the notes directory that did not read as a note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unreadable {
    pub path: PathBuf,
    pub why: String,
}

/// One scope's notes, read from their directory.
#[derive(Debug)]
pub struct Notes {
    dir: PathBuf,
    notes: BTreeMap<String, Note>,
    unreadable: Vec<Unreadable>,
}

impl Notes {
    /// Reads every note under `dir`, creating it when missing.
    pub fn open(dir: impl Into<PathBuf>) -> io::Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        let mut notes = BTreeMap::new();
        let mut unreadable = Vec::new();
        for kind in NoteType::ALL {
            let folder = dir.join(kind.as_str());
            let Ok(entries) = fs::read_dir(&folder) else {
                continue;
            };
            let mut paths: Vec<PathBuf> = entries
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
                .collect();
            paths.sort();
            for path in paths {
                match read_note(&path) {
                    Ok(note) if note.kind != kind => {
                        unreadable.push(Unreadable {
                            why: format!(
                                "a {} note in the {kind} folder",
                                note.kind
                            ),
                            path,
                        })
                    }
                    Ok(note) if notes.contains_key(&note.id) => unreadable
                        .push(Unreadable {
                            why: format!("a second note called {:?}", note.id),
                            path,
                        }),
                    Ok(note) => {
                        notes.insert(note.id.clone(), note);
                    }
                    Err(why) => unreadable.push(Unreadable { path, why }),
                }
            }
        }
        Ok(Self {
            dir,
            notes,
            unreadable,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn get(&self, id: &str) -> Option<&Note> {
        self.notes.get(id)
    }

    pub fn len(&self) -> usize {
        self.notes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }

    /// Every note, by id.
    pub fn iter(&self) -> impl Iterator<Item = &Note> {
        self.notes.values()
    }

    /// Files that did not read as notes when the store was opened.
    pub fn unreadable(&self) -> &[Unreadable] {
        &self.unreadable
    }

    /// Where `note`'s file lives.
    pub fn path_of(&self, note: &Note) -> PathBuf {
        self.dir
            .join(note.kind.as_str())
            .join(format!("{}.md", note.id))
    }

    /// Writes a new note. Refused when its id is taken.
    pub fn create(&mut self, note: Note) -> Result<(), StoreError> {
        if self.notes.contains_key(&note.id) {
            return Err(StoreError::Exists(note.id));
        }
        self.put(note)
    }

    /// Writes a new version of an existing note, keeping the one it
    /// replaces in the history. A change of type moves the file.
    pub fn update(&mut self, note: Note) -> Result<(), StoreError> {
        let Some(old) = self.notes.get(&note.id) else {
            return Err(StoreError::Missing(note.id));
        };
        self.snapshot(old)?;
        if old.kind != note.kind {
            let old_path = self.path_of(old);
            fs::remove_file(old_path)?;
        }
        self.put(note)
    }

    /// Writes `new`, which replaces `old`: `new` gets a `supersedes` link
    /// to it, and `old` stays with `valid_to` set to `now`.
    pub fn supersede(
        &mut self,
        old: &str,
        mut new: Note,
        now: u64,
    ) -> Result<(), StoreError> {
        let Some(previous) = self.notes.get(old) else {
            return Err(StoreError::Missing(old.to_owned()));
        };
        new.validate()?;
        if new.id == old || self.notes.contains_key(&new.id) {
            return Err(StoreError::Exists(new.id));
        }
        let mut previous = previous.clone();
        if !new
            .links
            .iter()
            .any(|link| link.to == old && link.kind == LinkType::Supersedes)
        {
            new.links.push(Link {
                to: old.to_owned(),
                kind: LinkType::Supersedes,
                why: None,
            });
        }
        previous.valid_to = Some(now);
        previous.updated = now;
        self.create(new)?;
        self.update(previous)
    }

    /// Marks each note about one of `paths` as possibly stale: an
    /// `about` link to it, or a source file that is it. Returns the ids
    /// marked. Superseded notes are left alone.
    pub fn mark_stale(
        &mut self,
        paths: &[String],
        why: &str,
        now: u64,
    ) -> Result<Vec<String>, StoreError> {
        let touched: Vec<String> = self
            .notes
            .values()
            .filter(|note| !note.is_superseded() && note.stale.is_none())
            .filter(|note| {
                let about = note.links.iter().any(|link| {
                    link.kind == LinkType::About && paths.contains(&link.to)
                });
                about
                    || note.source.files.iter().any(|file| paths.contains(file))
            })
            .map(|note| note.id.clone())
            .collect();
        for id in &touched {
            let mut note = self.notes[id].clone();
            note.stale = Some(why.to_owned());
            note.updated = now;
            self.update(note)?;
        }
        Ok(touched)
    }

    /// The versions a note had before its latest write, oldest first.
    pub fn history(&self, id: &str) -> Vec<Note> {
        let folder = self.dir.join(HISTORY).join(id);
        let mut versions: Vec<(u64, Note)> = fs::read_dir(folder)
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                let n = path.file_stem()?.to_str()?.parse().ok()?;
                // Versions are named by number, not by id.
                let text = fs::read_to_string(&path).ok()?;
                Some((n, Note::parse(&text).ok()?))
            })
            .collect();
        versions.sort_by_key(|(n, _)| *n);
        versions.into_iter().map(|(_, note)| note).collect()
    }

    /// Makes the note as it was at `version` (an index into
    /// [`Self::history`]) the latest again; the current one goes into the
    /// history, so this can be undone too.
    pub fn revert(
        &mut self,
        id: &str,
        version: usize,
    ) -> Result<(), StoreError> {
        let Some(old) = self.history(id).into_iter().nth(version) else {
            return Err(StoreError::Missing(format!("{id} version {version}")));
        };
        self.update(old)
    }

    /// Links into the note `id`, as `(from, link)`.
    pub fn backlinks(&self, id: &str) -> Vec<(String, Link)> {
        self.notes
            .values()
            .flat_map(|note| {
                note.all_links()
                    .into_iter()
                    .filter(|link| {
                        link.to == id && link.kind != LinkType::About
                    })
                    .map(|link| (note.id.clone(), link))
            })
            .collect()
    }

    /// The notes linked to or from `id`, each once, by id: what one hop
    /// along links reaches.
    pub fn neighbours(&self, id: &str) -> Vec<String> {
        let mut found: Vec<String> = Vec::new();
        let mut add = |other: &str| {
            if other != id
                && self.notes.contains_key(other)
                && !found.iter().any(|seen| seen == other)
            {
                found.push(other.to_owned());
            }
        };
        if let Some(note) = self.notes.get(id) {
            for link in note.all_links() {
                if link.kind != LinkType::About {
                    add(&link.to);
                }
            }
        }
        for (from, _) in self.backlinks(id) {
            add(&from);
        }
        found
    }

    /// Links to notes that do not exist yet, as `(from, to)`. They resolve
    /// once a note with that id is written.
    pub fn dangling(&self) -> Vec<(String, String)> {
        self.notes
            .values()
            .flat_map(|note| {
                note.all_links()
                    .into_iter()
                    .filter(|link| {
                        link.kind != LinkType::About
                            && !self.notes.contains_key(&link.to)
                    })
                    .map(|link| (note.id.clone(), link.to))
            })
            .collect()
    }

    fn put(&mut self, note: Note) -> Result<(), StoreError> {
        note.validate()?;
        let path = self.path_of(&note);
        write_atomically(&path, &note.render())?;
        self.notes.insert(note.id.clone(), note);
        Ok(())
    }

    /// Keeps `note` as it is now in the history, as its next version.
    fn snapshot(&self, note: &Note) -> io::Result<()> {
        let folder = self.dir.join(HISTORY).join(&note.id);
        fs::create_dir_all(&folder)?;
        let next = fs::read_dir(&folder)?
            .filter_map(|entry| {
                entry
                    .ok()?
                    .path()
                    .file_stem()?
                    .to_str()?
                    .parse::<u64>()
                    .ok()
            })
            .max()
            .map_or(0, |n| n + 1);
        write_atomically(&folder.join(format!("{next}.md")), &note.render())
    }
}

fn read_note(path: &Path) -> Result<Note, String> {
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let note = Note::parse(&text).map_err(|error| error.to_string())?;
    let stem = path.file_stem().and_then(|stem| stem.to_str());
    if stem != Some(note.id.as_str()) || !is_id(&note.id) {
        return Err(format!(
            "the file is not named after its id {:?}",
            note.id
        ));
    }
    Ok(note)
}

/// Writes through a temporary file and a rename, so a crash never leaves
/// half a note.
fn write_atomically(path: &Path, text: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("md.tmp");
    fs::write(&tmp, text)?;
    fs::rename(&tmp, path)
}
