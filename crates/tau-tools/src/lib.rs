//! Optional coding tools for tau agents: read, bash, edit, write, grep,
//! find and ls (`docs/reference/tools.md`).
//!
//! Every tool takes a [`path::Root`] at construction, and every path it
//! is given resolves against it. A tool that fails returns `Err`, which
//! the loop turns into an error result.

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
pub mod read;
pub mod truncate;
pub mod write;

/// What every tool returns when the run is cancelled while it works.
pub const ABORTED: &str = "Operation aborted";

/// All seven tools on `root`, in pi's order, for `Agent::tools`.
#[cfg(unix)]
pub fn coding_tools(
    root: &path::Root,
) -> Vec<std::sync::Arc<dyn tau_agent::tool::AgentTool>> {
    use std::sync::Arc;
    vec![
        Arc::new(read::Read::new(root.clone())),
        Arc::new(bash::Bash::new(root.clone())),
        Arc::new(edit::Edit::new(root.clone())),
        Arc::new(write::Write::new(root.clone())),
        Arc::new(grep::Grep::new(root.clone())),
        Arc::new(find::Find::new(root.clone())),
        Arc::new(ls::Ls::new(root.clone())),
    ]
}
