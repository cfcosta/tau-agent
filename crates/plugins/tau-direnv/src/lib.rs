//! tau-direnv: agent commands run in the repository's direnv
//! environment (`docs/reference/environment.md`, ADR 0025).
//!
//! A repository whose workspace has an `.envrc` gets its tools from
//! direnv, as it would in the person's shell. The first time a run
//! starts in such a repository, the person says whether tau loads it
//! (in the composer's place); the answer is kept per repository and
//! changes on the repository's menu. Once allowed, each workspace's
//! environment loads in the background, and `bash`'s commands and the
//! repository's MCP servers start through `direnv exec <workspace>`.
//!
//! - [`Record`]: what the plugin publishes about a run's workspace,
//!   folded by its UI.
//! - [`ui`]: the question, the loading and failure cards, and the
//!   repository menu's toggle.
//! - `host` (feature `host`): direnv, tau's direnv configuration, the
//!   loads, and the launcher commands start through.

#[cfg(feature = "host")]
pub mod config;
#[cfg(feature = "host")]
pub mod host;
#[cfg(feature = "host")]
pub mod launch;
pub mod ui;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
pub use ui::DirenvUi;

/// The plugin's name, for both its halves.
pub const NAME: &str = "tau-direnv";

/// Everything tau-direnv publishes about a run's workspace: where its
/// environment stands now. Each record replaces the one before.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Record {
    /// The repository has an `.envrc` and the person has not said
    /// whether tau loads it: its text. Commands wait for the answer.
    Asked { repo: String, envrc: String },
    /// The environment is loading, since `since` (Unix milliseconds).
    /// Commands wait for it.
    Loading { since: u64 },
    /// It loaded: commands run in it.
    Loaded,
    /// It did not load: how direnv ended, and the end of what it said.
    /// Commands run without it.
    Failed { status: String, output: String },
    /// The person denied this `.envrc` with `direnv deny`: tau does not
    /// load it either.
    Denied,
    /// Commands run without it: the person said so for the repository.
    Off,
}

/// What the person decided, by repository: whether tau loads its
/// `.envrc`. A repository missing has not been asked.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub repos: BTreeMap<String, bool>,
}

/// What the host knows of a repository, for its menu.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoData {
    /// Its main workspace has an `.envrc`.
    pub envrc: bool,
    /// direnv is on tau's `PATH`.
    pub direnv: bool,
}

/// What the UI asks the host half.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "act", rename_all = "snake_case")]
pub enum Act {
    /// Load `repo`'s `.envrc` for agent commands, or stop.
    Decide { repo: String, load: bool },
    /// Load `run`'s workspace's environment again, after it failed.
    Reload { run: String },
}
