//! Optional coding tools for tau agents: read, exec_command,
//! write_stdin, edit, write, grep, find and ls
//! (`docs/reference/tools.md`): tau-tools' host half. Their
//! cards, and the shapes they read, are `tau-tools`'s (ADR 0030).
//!
//! Every tool takes a [`path::Root`] at construction, and every path it
//! is given resolves against it. A tool that fails returns `Err`, which
//! the loop turns into an error result.
//!
//! [`plugin::CodingTools`] adds them to an agent as a plugin:
//! `Agent::plugin(CodingTools::new(root))`.
//!
//! `exec_command` runs commands under a pseudo-terminal, through
//! tau-terminal (libghostty-vt), and streams their raw output in the
//! results' details (`exec`; `docs/decisions/0010-terminal-rendering.md`).
//! A command that outlives its call keeps running, and `write_stdin`
//! goes on with it (`docs/decisions/0037-commands-run-in-the-background.md`).

pub mod artifact_grant;
pub mod artifact_read;
pub mod edit;
/// Unix only: it runs commands in their own process group.
#[cfg(unix)]
pub mod exec;
pub mod errno;
pub mod find;
pub mod grep;
mod half;
pub mod http;
pub mod image;
pub mod lock;
pub mod ls;
pub mod path;
#[cfg(unix)]
pub mod plugin;
pub mod read;
pub mod secrets;
pub mod truncate;
pub mod write;

pub use half::ToolsHost;
// The shapes the tools return, from the interface half.
pub use tau_tools::details;
use tau_tools::ui;

pub const ABORTED: &str = "Operation aborted";

/// A search that could not start: its glob or its pattern does not
/// parse. The model reads the parser's own message.
#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error(transparent)]
    Glob(#[from] globset::Error),
    #[error(transparent)]
    Filter(#[from] ignore::Error),
    #[error(transparent)]
    Pattern(#[from] grep_regex::Error),
}

impl From<SearchError> for tau_agent::tool::ToolError {
    fn from(error: SearchError) -> Self {
        Self::other(error)
    }
}

/// All eight tools on `root`, in pi's order, for `Agent::tools`. The
/// same tools [`plugin::CodingTools`] adds. The commands they start
/// share one [`exec::Processes`].
#[cfg(unix)]
pub fn coding_tools(
    root: &path::Root,
) -> Vec<std::sync::Arc<dyn tau_agent::tool::AgentTool>> {
    let processes = exec::Processes::new();
    plugin::Tool::ALL
        .iter()
        .map(|tool| tool.build(root, &processes))
        .collect()
}
