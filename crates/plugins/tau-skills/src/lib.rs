//! tau-skills: instructions the agent loads when a task calls for them.
//!
//! A skill is a folder under the person's skills folder
//! (`~/.agents/skills`) holding a `SKILL.md`: YAML frontmatter with a
//! `name` and a `description`, then the instructions. A run's
//! instructions list each skill by name and description; the model loads
//! one with the `skill` tool when a task fits it, and reads or runs the
//! folder's other files with the usual tools. The list costs a line per
//! skill; a skill's instructions cost nothing until it is loaded.
//!
//! - [`Skill`], [`Problem`], [`Skills`]: what the folder holds, as every
//!   interface shows it.
//! - `scan` (feature `host`): reading the folder.
//! - `host` (feature `host`): the list in a run's instructions, and the
//!   tool.
//! - [`ui`]: the plugin, with its UI.

#[cfg(feature = "demo")]
pub mod demo;
#[cfg(feature = "host")]
mod half;
#[cfg(feature = "host")]
pub mod host;
#[cfg(feature = "host")]
pub mod scan;
pub mod ui;

use std::path::PathBuf;

#[cfg(feature = "host")]
pub use half::SkillsHost;
use serde::{Deserialize, Serialize};
pub use ui::SkillsUi;

/// The plugin's name, for both its halves.
pub const NAME: &str = "tau-skills";

/// The tool's name, as the model calls it.
pub const TOOL: &str = "skill";

/// The file in a skill's folder that holds its frontmatter and
/// instructions.
pub const SKILL_FILE: &str = "SKILL.md";

/// Where the person's skills are, relative to their home: the folder
/// other agents read them from too.
pub const SKILLS_DIR: &str = ".agents/skills";

/// Where plugin crates put the skills tau ships, under tau's data
/// directory: one folder each, as in the person's folder. A skill of
/// the person's with the same name is offered instead.
pub const BUILTIN_DIR: &str = "skills";

/// Where the host reads skills from: the person's skills folder, or a
/// test's own. A host service ([`tau_ui_plugin::Services`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillsDir(pub PathBuf);

/// A skill the model can load.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Skill {
    /// What the model loads it by, and `/name` in the composer.
    pub name: String,
    /// When it applies: what the model picks it by.
    pub description: String,
    /// Its folder.
    pub dir: PathBuf,
    /// How many files the folder holds, `SKILL.md` among them.
    pub files: usize,
    /// Frontmatter keys tau does not act on, such as `allowed-tools`.
    pub ignored: Vec<String>,
    /// Whether tau ships it ([`BUILTIN_DIR`]).
    #[serde(default)]
    pub builtin: bool,
}

/// A folder that looks like a skill but is not offered to the model,
/// and why.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Problem {
    pub dir: PathBuf,
    pub reason: String,
}

/// What the skills folders hold: the skills, by name, and the folders
/// that could not be read as one.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Skills {
    /// Where they were read from; none when the host has no folder.
    pub dir: Option<PathBuf>,
    pub found: Vec<Skill>,
    pub problems: Vec<Problem>,
}

impl Skills {
    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.found.iter().find(|skill| skill.name == name)
    }
}

/// What a call of the tool returns in its details, for its card.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Loaded {
    pub name: String,
    pub description: String,
    /// Its `SKILL.md`, as read.
    pub file: PathBuf,
}
