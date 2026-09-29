//! A note: one idea, as a Markdown file with TOML front matter between
//! `+++` lines (`docs/reference/plugins.md`, `tau-memory`).
//!
//! ```text
//! +++
//! id = "retry-after-http-date"
//! title = "retry-after can be an HTTP date"
//! description = "Parse both forms; a bad one falls back to backoff"
//! type = "gotcha"
//! created = 1790000000000
//! updated = 1790000000000
//! valid_from = 1790000000000
//!
//! [source]
//! by = "agent"
//! run = "01j…"
//!
//! [[links]]
//! to = "retry-policy"
//! type = "refines"
//! +++
//! The body, in full prose: exact versions, flags, paths and errors.
//! ```
//!
//! A bare `[[id]]` in the body is a `relates` link, as if it were listed.

use std::fmt;

use serde::{Deserialize, Serialize};

/// What a note holds. A closed set: the kind decides how the note is
/// written and read, and task progress is not memory.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum NoteType {
    /// How the repository or its tools are.
    Fact,
    Convention,
    /// What was chosen, and why.
    Decision,
    /// A symptom, its cause and the fix.
    Gotcha,
    Procedure,
    /// A task, the approach taken and how it turned out, failures too.
    Case,
    Preference,
    /// A hub note that maps others. The scope's index note is one.
    Index,
}

impl NoteType {
    pub const ALL: [Self; 8] = [
        Self::Fact,
        Self::Convention,
        Self::Decision,
        Self::Gotcha,
        Self::Procedure,
        Self::Case,
        Self::Preference,
        Self::Index,
    ];

    /// The name in files and tool arguments, which is also the note's
    /// directory.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fact => "fact",
            Self::Convention => "convention",
            Self::Decision => "decision",
            Self::Gotcha => "gotcha",
            Self::Procedure => "procedure",
            Self::Case => "case",
            Self::Preference => "preference",
            Self::Index => "index",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == name)
    }
}

impl fmt::Display for NoteType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How one note relates to another. A closed set, so links can be
/// queried: free-text relations drift apart.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum LinkType {
    /// The default, and what a bare `[[id]]` in the body means.
    Relates,
    /// Adds detail to the target.
    Refines,
    /// Replaces the target, which stays, with `valid_to` set.
    Supersedes,
    Contradicts,
    /// A summary or hub, pointing at what it was made from.
    DerivedFrom,
    /// About a file, crate or symbol; the target is its path, not a note.
    About,
}

impl LinkType {
    pub const ALL: [Self; 6] = [
        Self::Relates,
        Self::Refines,
        Self::Supersedes,
        Self::Contradicts,
        Self::DerivedFrom,
        Self::About,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Relates => "relates",
            Self::Refines => "refines",
            Self::Supersedes => "supersedes",
            Self::Contradicts => "contradicts",
            Self::DerivedFrom => "derived_from",
            Self::About => "about",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == name)
    }
}

/// One link out of a note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    /// A note id; for `about`, a path in the repository.
    pub to: String,
    #[serde(rename = "type")]
    pub kind: LinkType,
    /// Why, in a few words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// Who a note's claim comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum By {
    /// The user said it.
    User,
    /// The agent did it or saw it.
    Agent,
    /// Inferred, by the agent or a consolidation pass.
    Inferred,
}

/// Where a note came from: enough to check it against the repository
/// later.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub by: By,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
}

impl Source {
    pub fn new(by: By) -> Self {
        Self {
            by,
            run: None,
            turn: None,
            commit: None,
            files: Vec::new(),
        }
    }
}

/// A note, as its file holds it. Times are milliseconds since the Unix
/// epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    /// Stable, and the file's name: `a-z`, `0-9` and `-`, 1 to 64 long.
    pub id: String,
    /// The claim, on one line.
    pub title: String,
    /// One line, for the index and search results.
    pub description: String,
    #[serde(rename = "type")]
    pub kind: NoteType,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    pub created: u64,
    pub updated: u64,
    pub valid_from: u64,
    /// Set when a newer note superseded this one; the note stays.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_to: Option<u64>,
    /// Why the note may be stale, when something it is about changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale: Option<String>,
    pub source: Source,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<Link>,
    #[serde(skip)]
    pub body: String,
}

/// Why a note or its file is not valid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoteError {
    /// Not an id: see [`is_id`].
    BadId(String),
    /// A field that must be one line held a line break, or was empty.
    BadLine(&'static str),
    /// The file has no `+++` front matter.
    NoFrontMatter,
    /// The front matter did not read as a note.
    FrontMatter(String),
}

impl fmt::Display for NoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadId(id) => write!(
                f,
                "{id:?} is not a note id: use a-z, 0-9 and -, 1 to 64 long"
            ),
            Self::BadLine(field) => {
                write!(f, "the {field} must be one line, and not empty")
            }
            Self::NoFrontMatter => {
                f.write_str("the file does not start with +++ front matter")
            }
            Self::FrontMatter(why) => write!(f, "bad front matter: {why}"),
        }
    }
}

impl std::error::Error for NoteError {}

/// Whether `id` can name a note.
pub fn is_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// An id made from a title: lowercase words joined by `-`, at most 64
/// long; `note` when nothing in the title can be kept.
pub fn slug(title: &str) -> String {
    let mut id = String::new();
    for word in title
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
    {
        let word = word.to_ascii_lowercase();
        let sep = usize::from(!id.is_empty());
        if id.len() + sep + word.len() > 64 {
            break;
        }
        if sep == 1 {
            id.push('-');
        }
        id.push_str(&word);
    }
    if id.is_empty() { "note".into() } else { id }
}

const FENCE: &str = "+++";

impl Note {
    /// Checks what a file must hold for the note to read back the same.
    pub fn validate(&self) -> Result<(), NoteError> {
        if !is_id(&self.id) {
            return Err(NoteError::BadId(self.id.clone()));
        }
        let one_line = |text: &str| {
            !text.trim().is_empty() && !text.contains(['\n', '\r'])
        };
        if !one_line(&self.title) {
            return Err(NoteError::BadLine("title"));
        }
        if !one_line(&self.description) {
            return Err(NoteError::BadLine("description"));
        }
        if !self.links.iter().all(|link| one_line(&link.to)) {
            return Err(NoteError::BadLine("link target"));
        }
        // Everything else in the front matter is one line too, so no
        // value can hold a line that closes it.
        let no_break = |text: &str| !text.contains(['\n', '\r']);
        let source = &self.source;
        let fields = self
            .tags
            .iter()
            .chain(self.links.iter().filter_map(|link| link.why.as_ref()))
            .chain(&source.files)
            .chain(&source.run)
            .chain(&source.commit)
            .chain(&self.stale);
        for field in fields {
            if !no_break(field) {
                return Err(NoteError::BadLine("front matter"));
            }
        }
        Ok(())
    }

    /// The note as its file holds it.
    pub fn render(&self) -> String {
        let front = toml::to_string(self).expect("a note serializes");
        format!("{FENCE}\n{front}{FENCE}\n{}", self.body)
    }

    /// Reads a note file.
    pub fn parse(text: &str) -> Result<Self, NoteError> {
        let rest = text
            .strip_prefix(FENCE)
            .and_then(|rest| rest.strip_prefix('\n'))
            .ok_or(NoteError::NoFrontMatter)?;
        // The front matter ends at the first line that is only the fence.
        let mut at = 0;
        let (front, body) = loop {
            let Some(end) = rest[at..].find('\n') else {
                return Err(NoteError::NoFrontMatter);
            };
            let line = &rest[at..at + end];
            if line == FENCE {
                break (&rest[..at], &rest[at + end + 1..]);
            }
            at += end + 1;
        };
        let mut note: Note = toml::from_str(front)
            .map_err(|error| NoteError::FrontMatter(error.to_string()))?;
        note.body = body.to_owned();
        Ok(note)
    }

    /// Every link out of the note: those listed, then each bare `[[id]]`
    /// in the body that is not listed already, as `relates`.
    pub fn all_links(&self) -> Vec<Link> {
        let mut links = self.links.clone();
        for to in wiki_links(&self.body) {
            if !links.iter().any(|link| link.to == to) {
                links.push(Link {
                    to,
                    kind: LinkType::Relates,
                    why: None,
                });
            }
        }
        links
    }

    /// Whether a newer note replaced this one.
    pub fn is_superseded(&self) -> bool {
        self.valid_to.is_some()
    }
}

/// The ids named by `[[id]]` in `text`, in order, each once. Anything
/// between the brackets that is not an id is left alone.
pub fn wiki_links(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find("[[") {
        let after = &rest[open + 2..];
        let Some(close) = after.find("]]") else { break };
        let inner = &after[..close];
        if is_id(inner) && !found.iter().any(|id| id == inner) {
            found.push(inner.to_owned());
        }
        rest = &after[close + 2..];
    }
    found
}
