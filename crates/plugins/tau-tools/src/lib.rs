//! Optional coding tools for tau agents: read, bash, edit, write, grep,
//! find and ls (`docs/reference/tools.md`).
//!
//! Every tool takes a [`path::Root`] at construction, and every path it
//! is given resolves against it. A tool that fails returns `Err`, which
//! the loop turns into an error result.
//!
//! [`plugin::CodingTools`] adds them to an agent as a plugin:
//! `Agent::plugin(CodingTools::new(root))`.
//!
//! The `terminal` feature runs `bash` under a pseudo-terminal, through
//! tau-terminal (libghostty-vt), and streams its raw output in the
//! results' details (`bash::terminal`;
//! `docs/decisions/0010-terminal-rendering.md`). Without it, `bash` uses
//! pipes and neither is built.

/// Unix only: it runs commands in their own process group.
#[cfg(all(unix, feature = "host"))]
pub mod bash;
pub mod details;
#[cfg(feature = "host")]
pub mod edit;
#[cfg(feature = "host")]
pub mod errno;
#[cfg(feature = "host")]
pub mod find;
#[cfg(feature = "host")]
pub mod grep;
#[cfg(feature = "host")]
pub mod image;
#[cfg(feature = "host")]
pub mod lock;
#[cfg(feature = "host")]
pub mod ls;
#[cfg(feature = "host")]
pub mod path;
#[cfg(all(unix, feature = "host"))]
pub mod plugin;
#[cfg(feature = "host")]
pub mod read;
#[cfg(feature = "host")]
pub mod truncate;
pub mod ui;
#[cfg(feature = "host")]
pub mod write;

pub const ABORTED: &str = "Operation aborted";

/// A search that could not start: its glob or its pattern does not
/// parse. The model reads the parser's own message.
#[cfg(feature = "host")]
#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error(transparent)]
    Glob(#[from] globset::Error),
    #[error(transparent)]
    Filter(#[from] ignore::Error),
    #[error(transparent)]
    Pattern(#[from] grep_regex::Error),
}

#[cfg(feature = "host")]
impl From<SearchError> for tau_agent::tool::ToolError {
    fn from(error: SearchError) -> Self {
        Self::other(error)
    }
}

/// All seven tools on `root`, in pi's order, for `Agent::tools`. The
/// same tools [`plugin::CodingTools`] adds.
#[cfg(all(unix, feature = "host"))]
pub fn coding_tools(
    root: &path::Root,
) -> Vec<std::sync::Arc<dyn tau_agent::tool::AgentTool>> {
    use tau_agent::plugin::Plugin;
    plugin::CodingTools::new(root.clone()).tools()
}
