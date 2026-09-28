//! Optional coding tools for tau agents: read, bash, edit, write, grep,
//! find and ls (`docs/reference/tools.md`).
//!
//! Every tool takes a [`path::Root`] at construction, and every path it
//! is given resolves against it. A tool that fails returns `Err`, which
//! the loop turns into an error result.
//!
//! [`plugin::CodingTools`] adds them to an agent as a plugin:
//! `Agent::plugin(CodingTools::new(root))`.

/// Unix only: it runs commands in their own process group.
#[cfg(unix)]
pub mod bash;
pub mod edit;
pub mod errno;
pub mod find;
pub mod grep;
pub mod image;
pub mod lock;
pub mod ls;
pub mod path;
/// Unix only, like `bash`.
#[cfg(unix)]
pub mod plugin;
pub mod read;
pub mod truncate;
pub mod write;

/// What every tool returns when the run is cancelled while it works.
pub const ABORTED: &str = "Operation aborted";

/// All seven tools on `root`, in pi's order, for `Agent::tools`. The
/// same tools [`plugin::CodingTools`] adds.
#[cfg(unix)]
pub fn coding_tools(
    root: &path::Root,
) -> Vec<std::sync::Arc<dyn tau_agent::tool::AgentTool>> {
    use tau_agent::plugin::Plugin;
    plugin::CodingTools::new(root.clone()).tools()
}
