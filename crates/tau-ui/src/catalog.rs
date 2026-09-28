//! What the workspace shows beyond single runs: the agent's plugins, the
//! repositories with their memory notes and constitutions, and the
//! store. The host fills it from its agents and plugin crates;
//! [`crate::demo`] has an example.

use crate::view::Proposal;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Catalog {
    /// The agent the plugin screen describes.
    pub agent: String,
    /// Where the agent is defined, for the header.
    pub agent_source: Option<String>,
    /// In registration order.
    pub plugins: Vec<PluginInfo>,
    pub jev: Option<JevStats>,
    /// The repositories runs work on, in the sidebar's order. Each has
    /// its own notes and rules.
    pub repos: Vec<Repo>,
    /// The repositories the sidebar had open when the user last left it.
    pub open_repos: Vec<String>,
    /// Conversations the user closed: History lists them, the sidebar
    /// does not.
    pub closed_runs: Vec<tau_agent::tool::RunId>,
    /// Flagged calls someone looked at, as `(run, call id)`: off the
    /// review queue.
    pub reviewed: Vec<(tau_agent::tool::RunId, String)>,
    pub store: StoreInfo,
    /// Whether the host can open pull requests from runs.
    pub pull_requests: bool,
    /// Where runs work.
    pub project: ProjectStatus,
    /// What the latest update of a repository found, for the status bar:
    /// `tau-agent is up to date`.
    pub update: Option<String>,
    /// The models the picker offers, and the user's choices about them.
    pub models: crate::models::Models,
}

impl Catalog {
    pub fn repo(&self, name: &str) -> Option<&Repo> {
        self.repos.iter().find(|repo| repo.name == name)
    }

    pub fn repo_mut(&mut self, name: &str) -> Option<&mut Repo> {
        self.repos.iter_mut().find(|repo| repo.name == name)
    }
}

/// A repository runs work on: its memory and constitution belong to it
/// alone.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Repo {
    /// What the sidebar calls it; unique in the catalog.
    pub name: String,
    /// The checkout it came from.
    pub path: String,
    pub memory: Memory,
    pub constitution: Constitution,
}

impl Repo {
    pub fn new(name: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            path: path.into(),
            ..Self::default()
        }
    }

    /// The letter on the repository's mark.
    pub fn letter(&self) -> String {
        self.name
            .chars()
            .find(|c| c.is_alphanumeric())
            .map_or("?".into(), |c| c.to_lowercase().to_string())
    }
}

/// Where the host's runs work, for the status bar.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ProjectStatus {
    /// No host, or one that says nothing.
    #[default]
    Unknown,
    /// The checkout is being copied into a project named this.
    Importing(String),
    /// Runs get a workspace each in the project named this.
    Ready(String),
    /// The repository named this is taking in new commits from its
    /// source.
    Updating(String),
    /// Runs work in the checkout itself, for this reason.
    Checkout(String),
}

/// The seams a plugin can use, in the order the loop reaches them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seam {
    Start,
    Tools,
    BeforeTool,
    AfterTool,
    Rewrite,
    BeforeStop,
    Finish,
}

impl Seam {
    pub const ALL: [Self; 7] = [
        Self::Start,
        Self::Tools,
        Self::BeforeTool,
        Self::AfterTool,
        Self::Rewrite,
        Self::BeforeStop,
        Self::Finish,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Tools => "tools",
            Self::BeforeTool => "before_tool",
            Self::AfterTool => "after_tool",
            Self::Rewrite => "rewrite",
            Self::BeforeStop => "before_stop",
            Self::Finish => "finish",
        }
    }
}

/// Which screen explains a plugin's work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginScreen {
    /// The run plan: what `start` decided.
    Plan,
    Memory,
    Constitution,
    /// The pruning ledger.
    Ledger,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PluginInfo {
    pub name: String,
    pub description: String,
    pub seams: Vec<Seam>,
    /// What the plugin cost over the store's recent window.
    pub spend: f64,
    pub screen: Option<PluginScreen>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct JevStats {
    pub model: String,
    /// Where the key comes from.
    pub key_env: String,
    pub price: String,
    pub requests: u64,
    pub input_tokens: u64,
    pub spent: f64,
    pub latency_p50_ms: u32,
    /// Requests that got no answer.
    pub failed: u32,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Memory {
    /// The notes directory.
    pub path: String,
    pub collection: String,
    pub notes: Vec<Note>,
}

impl Memory {
    pub fn note(&self, id: &str) -> Option<&Note> {
        self.notes.iter().find(|note| note.id == id)
    }

    pub fn by_title(&self, title: &str) -> Option<&Note> {
        self.notes.iter().find(|note| note.title == title)
    }

    /// Notes that link to `id`, with why.
    pub fn backlinks<'a>(
        &'a self,
        id: &'a str,
    ) -> impl Iterator<Item = (&'a Note, &'a str)> {
        self.notes.iter().filter_map(move |note| {
            note.links
                .iter()
                .find(|link| link.to == id)
                .map(|link| (note, link.why.as_str()))
        })
    }

    /// Adds a suggested note, keeping the id unique.
    pub fn keep(&mut self, proposal: &Proposal, from_run: &str) -> String {
        let id = format!("n-{:04}", 1000 + self.notes.len());
        self.notes.insert(
            0,
            Note {
                id: id.clone(),
                title: proposal.title.clone(),
                body: vec![proposal.detail.clone()],
                links: Vec::new(),
                paths: Vec::new(),
                written_by: format!("kept from {from_run}"),
                edited: "just now".into(),
                used_by_runs: 0,
            },
        );
        id
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub id: String,
    pub title: String,
    /// Paragraphs, with `code` in backticks.
    pub body: Vec<String>,
    pub links: Vec<Link>,
    /// Files the note is about.
    pub paths: Vec<String>,
    pub written_by: String,
    pub edited: String,
    /// How many runs got the note at start.
    pub used_by_runs: u32,
}

impl Note {
    /// The first sentence, for lists.
    pub fn snippet(&self) -> &str {
        let first = self.body.first().map(String::as_str).unwrap_or("");
        first.split_inclusive(". ").next().unwrap_or(first)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub to: String,
    pub why: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Constitution {
    pub path: String,
    pub rules: Vec<Rule>,
    pub max_continuations: u32,
    /// Why the file could not be read, if it could not: runs fail until
    /// it is fixed.
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub id: String,
    pub text: String,
    /// `tool.field` names, or `final answer`.
    pub applies_to: Vec<String>,
    pub review: f32,
    pub block: f32,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct StoreInfo {
    pub path: String,
    pub size: String,
    /// A query the history screen offers to run.
    pub sample_query: String,
}
