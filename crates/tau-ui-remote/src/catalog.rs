//! What the workspace shows beyond single runs: the agent's plugins, the
//! repositories with each plugin's data for them, and the store. The
//! host fills it from its agents and plugin crates; [`crate::demo`] has
//! an example.

use serde::{Deserialize, Serialize};
use tau_ui_plugin::PluginValue;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
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
    /// Each plugin's data across repositories, by plugin
    /// (`UiPlugin::data`).
    #[serde(default)]
    pub plugin_data: std::collections::BTreeMap<String, PluginValue>,
    /// Each plugin's settings, as JSON, by plugin.
    #[serde(default)]
    pub plugin_settings: std::collections::BTreeMap<String, PluginValue>,
}

impl Catalog {
    pub fn repo(&self, name: &str) -> Option<&Repo> {
        self.repos.iter().find(|repo| repo.name == name)
    }

    pub fn repo_mut(&mut self, name: &str) -> Option<&mut Repo> {
        self.repos.iter_mut().find(|repo| repo.name == name)
    }
}

/// A repository runs work on: its memory and its plugins' data belong
/// to it alone.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Repo {
    /// What the sidebar calls it; unique in the catalog.
    pub name: String,
    /// Where its clone is.
    pub path: String,
    /// Its main chat, which every other chat in it forks from and which
    /// cannot be closed.
    #[serde(default)]
    pub main: Option<tau_agent::tool::RunId>,
    /// Each plugin's data for the repository, by plugin
    /// (`UiPlugin::repo_data`).
    #[serde(default)]
    pub plugins: std::collections::BTreeMap<String, PluginValue>,
    /// How many of trunk's changes GitHub does not have yet: what the
    /// main chat would push (ADR 0023). Zero for a repository that did
    /// not come from GitHub.
    #[serde(default)]
    pub unpushed: u32,
    /// Trunk's branch, which `origin/<trunk>` names on GitHub.
    #[serde(default)]
    pub trunk: Option<String>,
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
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ProjectStatus {
    /// No host, or one that says nothing.
    #[default]
    Unknown,
    /// The clone named this is being made a project.
    Importing(String),
    /// The repository named this is taking in new commits from GitHub.
    Updating(String),
    /// The clone named this could not be made a project; runs cannot
    /// start in it.
    Failed(String),
}

pub use tau_ui_plugin::Seam;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginInfo {
    pub name: String,
    pub description: String,
    pub seams: Vec<Seam>,
    /// What the plugin cost over the store's recent window.
    pub spend: f64,
    /// The page that explains its work, for a plugin with its UI (ADR
    /// 0017).
    #[serde(default)]
    pub page: Option<tau_ui_plugin::Link>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct StoreInfo {
    pub path: String,
    pub size: String,
    /// A query the history screen offers to run.
    pub sample_query: String,
}
