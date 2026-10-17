//! Readers for what other coding agents kept on this computer (ADR
//! 0044): Claude Code's memory notes and chats ([`claude`]), and Codex's
//! memories, task summaries and sessions ([`codex`]).
//!
//! Each reader takes the person's home as a parameter, never the
//! process's: the app gives the real one, tests a directory of their
//! own. Readers only read. A scan lists what there is ([`Item`]), cheaply:
//! paths, sizes, the folder each was about. Reading an item gives a note
//! as the tool saved it ([`Read::Note`]), or text for the person's model
//! ([`Read::Text`]). What a tool wrote is data: it is shown to the model
//! fenced as such, and every note made of it goes through the checks
//! every note does.
//!
//! The formats change without notice, so the readers take only what they
//! need and skip what they cannot read: a file that does not read is
//! named, not fatal.

pub mod claude;
pub mod codex;
pub mod remote;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use tau_memory::{
    import::{Material, Place, Tool},
    note::NoteType,
};

/// Where the readers look: the person's home, under which each tool
/// keeps its directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roots {
    pub home: PathBuf,
}

impl Roots {
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self { home: home.into() }
    }

    /// `tool`'s directory: `~/.claude`, `~/.codex`.
    pub fn of(&self, tool: Tool) -> PathBuf {
        let root = tool.root().trim_start_matches("~/");
        self.home.join(root)
    }

    /// `path` as the person writes it: under `~` when it is in their
    /// home.
    pub fn show(&self, path: &Path) -> String {
        match path.strip_prefix(&self.home) {
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => path.display().to_string(),
        }
    }
}

/// How an item stood when it was read, to tell later whether it changed:
/// its size and when it was last written, or for a section of a file, a
/// hash of its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint(pub String);

impl Fingerprint {
    /// A file's size and modification time.
    pub fn of_file(path: &Path) -> std::io::Result<(Self, u64)> {
        let meta = std::fs::metadata(path)?;
        let mtime = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |since| since.as_millis());
        Ok((Self(format!("{}:{mtime}", meta.len())), meta.len()))
    }

    /// A hash of `text`: for a section of a file.
    pub fn of_text(text: &str) -> Self {
        Self(format!("h{:016x}", fnv(text)))
    }
}

/// FNV-1a over `text`'s bytes: stable across runs and builds.
pub fn fnv(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// One thing a scan found to read: a file, or a section of one.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub tool: Tool,
    pub material: Material,
    pub path: PathBuf,
    /// Its key in the checkpoint: the path, and `#` and the section for
    /// a part of a file.
    pub key: String,
    /// The folder it is about; `None` when it is about no one folder,
    /// such as Codex's summary of the person.
    pub place: Option<Place>,
    /// A saved note about the person, which goes to their notes.
    pub about_you: bool,
    pub fingerprint: Fingerprint,
    /// Its size, for a chat: how far a later import has to read.
    pub size: u64,
    pub session: Option<String>,
    /// The day it was written, `2026-10-04`.
    pub date: Option<String>,
    /// A section's text, read with the scan.
    pub section: Option<String>,
}

/// A note as a tool saved it: no model reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedNote {
    pub kind: NoteType,
    pub title: String,
    pub description: String,
    pub body: String,
    pub about_you: bool,
}

/// What reading an item gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Read {
    Note(SavedNote),
    /// Text for the model, and how far into the file it was read: a
    /// chat read again later starts there.
    Text {
        text: String,
        offset: u64,
    },
}

/// Why an item did not read.
#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Format(String),
}

/// What a scan found: the items to read, and the files it could not
/// read, by path.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scan {
    pub items: Vec<Item>,
    pub failed: Vec<PathBuf>,
}

impl Scan {
    pub fn extend(&mut self, other: Scan) {
        self.items.extend(other.items);
        self.failed.extend(other.failed);
    }
}

/// Every item both tools have under `roots`.
pub fn scan(roots: &Roots) -> Scan {
    let mut remotes = Remotes::default();
    let mut scan = claude::scan(&roots.of(Tool::ClaudeCode), &mut remotes);
    scan.extend(codex::scan(&roots.of(Tool::Codex), &mut remotes));
    scan
}

/// Reads `item`, from `offset` for a chat.
pub fn read(item: &Item, offset: u64) -> Result<Read, ReadError> {
    match (item.tool, item.material) {
        (Tool::ClaudeCode, Material::Saved) => {
            let text = std::fs::read_to_string(&item.path)?;
            Ok(Read::Note(claude::parse_note(&text, &item.path)?))
        }
        (Tool::ClaudeCode, Material::Chat) => {
            claude::read_chat(&item.path, offset)
        }
        (Tool::Codex, Material::Saved) => {
            let text = match &item.section {
                Some(section) => section.clone(),
                None => std::fs::read_to_string(&item.path)?,
            };
            Ok(Read::Text {
                offset: text.len() as u64,
                text,
            })
        }
        (Tool::Codex, Material::Chat) => codex::read_chat(&item.path, offset),
    }
}

/// Each folder's place, its remote read once.
#[derive(Debug, Default)]
pub struct Remotes(HashMap<PathBuf, Option<String>>);

impl Remotes {
    /// The place of `folder`: the folder, and its repository's remote.
    pub fn place(&mut self, folder: &Path) -> Place {
        let remote = self
            .0
            .entry(folder.to_owned())
            .or_insert_with(|| remote::of_folder(folder))
            .clone();
        Place {
            folder: Some(folder.display().to_string()),
            remote,
        }
    }
}

/// Cuts `text` into chunks of at most `max` bytes (or one character,
/// when `max` is less), between entries (a blank line before a `[`)
/// where it can, else between lines, else anywhere on a character
/// boundary. Joined, they are `text`.
pub fn chunks(text: &str, max: usize) -> Vec<String> {
    let max = max.max(1);
    let mut out = Vec::new();
    let mut rest = text;
    while rest.len() > max {
        let mut cut = floor_boundary(rest, max);
        let window = &rest[..cut];
        if let Some(at) = window.rfind("\n\n[").filter(|at| *at > 0) {
            cut = at + 2;
        } else if let Some(at) = window.rfind('\n').filter(|at| *at > 0) {
            cut = at + 1;
        }
        if cut == 0 {
            cut = rest.chars().next().map_or(1, char::len_utf8);
        }
        out.push(rest[..cut].to_owned());
        rest = &rest[cut..];
    }
    if !rest.is_empty() {
        out.push(rest.to_owned());
    }
    out
}

/// The largest char boundary in `text` at or below `at`.
fn floor_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// `text` cut to `max` characters, marked when it was.
pub fn cut(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((at, _)) => format!("{}… (cut)", &text[..at]),
        None => text.to_owned(),
    }
}

/// The day of an ISO 8601 time: `2026-10-04` of `2026-10-04T12:00:00Z`.
pub fn day(time: &str) -> Option<String> {
    let day = time.get(..10)?;
    let ok = day.bytes().enumerate().all(|(i, b)| match i {
        4 | 7 => b == b'-',
        _ => b.is_ascii_digit(),
    });
    ok.then(|| day.to_owned())
}
