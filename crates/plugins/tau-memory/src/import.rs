//! What memory brings over from other coding agents on this computer
//! (ADR 0044): the tools it reads, the notes it proposes from them,
//! where each belongs, and what the interface is told about it. Every
//! interface reads this, a phone's too; reading the tools' files is
//! `tau-memory-host`'s.
//!
//! An import never writes a note. It proposes, and a proposal becomes a
//! note only when the person keeps it. A proposal belongs to the person
//! ([`Scope::You`]) or to the folder the tool worked in ([`Place`]),
//! which a repository claims by its folder or its remote, whenever it is
//! listed: proposals for a folder tau does not have yet wait for it.

use std::{collections::BTreeMap, path::Path};

use serde::{Deserialize, Serialize};

use crate::note::NoteType;

/// A coding agent memory reads.
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
pub enum Tool {
    ClaudeCode,
    Codex,
}

impl Tool {
    pub const ALL: [Self; 2] = [Self::ClaudeCode, Self::Codex];

    /// Its name, for the person.
    pub fn label(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
        }
    }

    /// Where it keeps what it learned, under the person's home.
    pub fn root(self) -> &'static str {
        match self {
            Self::ClaudeCode => "~/.claude",
            Self::Codex => "~/.codex",
        }
    }
}

/// What a tool kept: what it saved as memory, or its chats.
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
pub enum Material {
    /// Claude Code's memory notes, Codex's memories and task summaries.
    Saved,
    /// Transcripts of its sessions.
    Chat,
}

/// The folder a tool worked in: its path, and the remote its repository
/// pushes to, normalised ([`normalize_remote`]), when it has one.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Default,
    Serialize,
    Deserialize,
)]
pub struct Place {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
}

impl Place {
    /// Whether a repository with `folder` and `remote` is this place: the
    /// folder is the repository's or inside it, or both push to the same
    /// remote.
    pub fn matches(&self, folder: Option<&Path>, remote: Option<&str>) -> bool {
        let in_folder = match (&self.folder, folder) {
            (Some(mine), Some(theirs)) => Path::new(mine).starts_with(theirs),
            _ => false,
        };
        let same_remote = match (&self.remote, remote) {
            (Some(mine), Some(theirs)) => {
                normalize_remote(mine).is_some_and(|mine| {
                    normalize_remote(theirs)
                        .is_some_and(|theirs| mine == theirs)
                })
            }
            _ => false,
        };
        in_folder || same_remote
    }

    /// Its folder, else its remote, for the person.
    pub fn label(&self) -> String {
        self.folder
            .clone()
            .or_else(|| self.remote.clone())
            .unwrap_or_else(|| "an unknown folder".to_owned())
    }
}

/// A remote's URL as one string whichever way it is written: the host
/// lowercased, then the path, without a scheme, user, port, `.git` or
/// trailing slash. `git@github.com:o/n.git`, `ssh://git@github.com/o/n`
/// and `https://github.com/o/n` are all `github.com/o/n`. A local path
/// stays a path. `None` for an empty one.
pub fn normalize_remote(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    let (host, path) = if let Some((_, rest)) = url.split_once("://") {
        // scheme://[user@]host[:port]/path
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        let host = authority.rsplit('@').next().unwrap_or(authority);
        let host = host.split(':').next().unwrap_or(host);
        (Some(host), path)
    } else if !url.starts_with('/')
        && let Some((authority, path)) = url.split_once(':')
        && !authority.contains('/')
    {
        // scp-like: [user@]host:path
        let host = authority.rsplit('@').next().unwrap_or(authority);
        (Some(host), path)
    } else {
        (None, url)
    };
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let path = path.trim_end_matches('/');
    Some(match host {
        Some(host) if !host.is_empty() => {
            format!("{}/{path}", host.to_ascii_lowercase())
        }
        _ => format!("/{path}"),
    })
}

/// Where a proposal goes once kept: the person's own notes, or the
/// repository that claims the place.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    You,
    Place(Place),
}

/// Where a proposal came from: the tool, what of it, the file, the
/// session, and when, as the tool said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    pub from: Tool,
    pub material: Material,
    /// The file, under the tool's root as the person writes it:
    /// `~/.claude/projects/…/memory/no-git-binary.md`.
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// The day it was written, `2026-10-04`, when the file says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
}

impl Origin {
    /// One line for the person: `Claude Code · memory/no-git-binary.md`,
    /// `Codex · chat on 2026-09-22`.
    pub fn describe(&self) -> String {
        let what = match self.material {
            Material::Saved => {
                let mut parts = self.path.rsplit('/');
                let file = parts.next().unwrap_or(&self.path);
                let dir = parts.next().unwrap_or("");
                match self.from {
                    Tool::ClaudeCode => format!("{dir}/{file}"),
                    Tool::Codex if dir == "rollout_summaries" => {
                        match &self.date {
                            Some(date) => format!("task summary, {date}"),
                            None => "task summary".to_owned(),
                        }
                    }
                    Tool::Codex => format!("memories/{file}"),
                }
            }
            Material::Chat => match &self.date {
                Some(date) => format!("chat on {date}"),
                None => "a chat".to_owned(),
            },
        };
        format!("{} · {what}", self.from.label())
    }
}

/// What about a proposal wants the person's eye before it is kept.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Flags {
    /// It held a secret, now redacted.
    pub secret: bool,
    /// The id of a note in its scope it disagrees with, as the model
    /// read them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disagrees: Option<String>,
    /// The id of a note in its scope with the same title: keeping the
    /// proposal supersedes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replaces: Option<String>,
}

impl Flags {
    /// Whether the proposal waits under "Need a look", unticked.
    pub fn need_a_look(&self) -> bool {
        self.secret || self.disagrees.is_some() || self.replaces.is_some()
    }
}

/// A note an import proposes, not written until the person keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    /// `p-12`: its own, never reused.
    pub id: String,
    #[serde(rename = "type")]
    pub kind: NoteType,
    pub title: String,
    pub description: String,
    pub body: String,
    pub scope: Scope,
    /// Every source that said it, the first one's words kept.
    pub origins: Vec<Origin>,
    #[serde(default)]
    pub flags: Flags,
    /// Ticked to keep: unless it needs a look.
    pub keep: bool,
}

impl Proposal {
    /// Whether more than one tool said it.
    pub fn in_both(&self) -> bool {
        self.origins.iter().any(|o| o.from == Tool::ClaudeCode)
            && self.origins.iter().any(|o| o.from == Tool::Codex)
    }
}

/// Which parts of the tools an import reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Parts {
    pub claude_saved: bool,
    pub claude_chats: bool,
    pub codex_saved: bool,
    pub codex_chats: bool,
}

impl Default for Parts {
    fn default() -> Self {
        Self {
            claude_saved: true,
            claude_chats: true,
            codex_saved: true,
            codex_chats: true,
        }
    }
}

impl Parts {
    pub fn reads(&self, tool: Tool, material: Material) -> bool {
        match (tool, material) {
            (Tool::ClaudeCode, Material::Saved) => self.claude_saved,
            (Tool::ClaudeCode, Material::Chat) => self.claude_chats,
            (Tool::Codex, Material::Saved) => self.codex_saved,
            (Tool::Codex, Material::Chat) => self.codex_chats,
        }
    }

    pub fn set(&mut self, tool: Tool, material: Material, on: bool) {
        match (tool, material) {
            (Tool::ClaudeCode, Material::Saved) => self.claude_saved = on,
            (Tool::ClaudeCode, Material::Chat) => self.claude_chats = on,
            (Tool::Codex, Material::Saved) => self.codex_saved = on,
            (Tool::Codex, Material::Chat) => self.codex_chats = on,
        }
    }

    pub fn any(&self) -> bool {
        self.claude_saved
            || self.claude_chats
            || self.codex_saved
            || self.codex_chats
    }
}

/// An import the person asked for.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Request {
    pub parts: Parts,
    /// The places to read: the repositories picked in setup, or one
    /// repository. Empty reads every repository listed as the import
    /// goes.
    pub only: Vec<Place>,
    /// Also reads folders no repository claims, keeping their proposals
    /// for when one does.
    pub others: bool,
}

/// What the interface asks memory's host half (`HostHalf::act`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "act", rename_all = "snake_case")]
pub enum Act {
    /// Starts an import, or says one is going.
    Import(Request),
    /// Stops the import going on; what it read stays read.
    Stop,
    /// Ticks or unticks a proposal.
    Tick { id: String, keep: bool },
    /// Writes these proposals as notes, through the checks every note
    /// goes through, and drops them from the list.
    Keep { ids: Vec<String> },
    /// Drops these proposals without writing them.
    Drop { ids: Vec<String> },
    /// Hides the repository's banner until something new is found for
    /// it.
    NotNow { repo: String },
}

/// What memory's host half answers an [`Act::Keep`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Kept {
    pub kept: usize,
    /// Each proposal not written, and why.
    pub refused: Vec<(String, String)>,
}

/// What one tool has on this computer, as an import would read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolFound {
    pub tool: Tool,
    /// Notes or summaries it saved.
    pub saved: usize,
    pub chats: usize,
    /// Saved notes about the person, which go to their notes whatever
    /// the folder.
    pub about_you: usize,
    /// Each folder it worked in, with what it has of it.
    pub folders: Vec<FolderFound>,
}

/// A folder a tool worked in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderFound {
    pub place: Place,
    /// The folder as the person writes it: `~/work/shop`.
    #[serde(default)]
    pub label: String,
    pub saved: usize,
    pub chats: usize,
    /// What an import has not read yet of it.
    #[serde(default)]
    pub unread_saved: usize,
    #[serde(default)]
    pub unread_chats: usize,
}

/// A proposal as the review page lists it, with the repository that
/// claims it now (`None`: the person's notes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: NoteType,
    pub title: String,
    pub description: String,
    pub repo: Option<String>,
    /// Each source, described ([`Origin::describe`]).
    pub origins: Vec<String>,
    pub in_both: bool,
    pub flags: Flags,
    pub keep: bool,
}

/// What the Memory tab of a repository says the tools know of it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Banner {
    /// The folder the tools worked in, as the person writes it.
    pub folder: String,
    /// Every place of the tools' that the repository claims: what an
    /// import of it reads.
    pub places: Vec<Place>,
    /// What each tool saved for it.
    pub saved: Vec<(Tool, usize)>,
    /// Its chats an import has not read.
    pub unread_chats: usize,
    /// What they saved for it that an import has not read.
    pub unread_saved: usize,
    /// Proposals waiting for it.
    pub ready: usize,
}

/// How an import went, over every run of it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Tally {
    /// What was read, by tool and material.
    pub read: BTreeMap<String, usize>,
    /// Drafts about folders no repository claims, left out.
    pub left_out: usize,
    /// Drafts that said what another proposal says, merged into it.
    pub merged: usize,
    /// Drafts that said what a note tau has says, dropped.
    pub known: usize,
    /// Drafts refused because they read as instructions to the model.
    pub refused: usize,
}

impl Tally {
    /// The key [`Self::read`] counts `tool`'s `material` under.
    pub fn key(tool: Tool, material: Material) -> String {
        let material = match material {
            Material::Saved => "saved",
            Material::Chat => "chats",
        };
        let tool = match tool {
            Tool::ClaudeCode => "claude_code",
            Tool::Codex => "codex",
        };
        format!("{tool}.{material}")
    }

    pub fn read_of(&self, tool: Tool, material: Material) -> usize {
        self.read
            .get(&Self::key(tool, material))
            .copied()
            .unwrap_or(0)
    }
}

/// An import going on, as its job says.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Progress {
    pub fraction: f32,
    pub line: String,
}

/// Everything the interface shows of imports: what is on this computer,
/// what waits for the person, and how the last import went.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Imports {
    /// Each tool found, in [`Tool::ALL`]'s order; empty when neither is.
    pub found: Vec<ToolFound>,
    /// Proposals a listed repository or the person's notes claim.
    pub pending: Vec<Pending>,
    /// Proposals for folders no listed repository claims yet.
    pub held: usize,
    pub progress: Option<Progress>,
    pub tally: Tally,
    /// Files that would not read, by path: skipped.
    pub failed: Vec<String>,
    /// What each listed repository's Memory tab says, by name.
    pub banners: BTreeMap<String, Banner>,
}

impl Imports {
    /// Whether Claude Code or Codex is on this computer.
    pub fn any_found(&self) -> bool {
        !self.found.is_empty()
    }

    pub fn tool(&self, tool: Tool) -> Option<&ToolFound> {
        self.found.iter().find(|found| found.tool == tool)
    }
}
