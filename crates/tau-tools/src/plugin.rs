//! The coding tools as a plugin (`docs/reference/plugins.md`), for
//! `Agent::plugin`.
//!
//! The plugin adds tools and nothing else: it keeps no state per run.
//! The per-path locks that `edit` and `write` hold stay process-wide
//! ([`crate::lock`]), so two agents on one directory still take turns
//! on a file.

use std::sync::Arc;

use tau_agent::{plugin::Plugin, tool::AgentTool};

use crate::{bash, edit, find, grep, ls, path::Root, read, write};

/// One of the coding tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Tool {
    Read,
    Bash,
    Edit,
    Write,
    Grep,
    Find,
    Ls,
}

impl Tool {
    /// Every tool, in pi's order.
    pub const ALL: [Tool; 7] = [
        Tool::Read,
        Tool::Bash,
        Tool::Edit,
        Tool::Write,
        Tool::Grep,
        Tool::Find,
        Tool::Ls,
    ];

    /// The name the model calls the tool by.
    pub fn name(self) -> &'static str {
        match self {
            Tool::Read => "read",
            Tool::Bash => "bash",
            Tool::Edit => "edit",
            Tool::Write => "write",
            Tool::Grep => "grep",
            Tool::Find => "find",
            Tool::Ls => "ls",
        }
    }

    fn build(self, root: &Root) -> Arc<dyn AgentTool> {
        let root = root.clone();
        match self {
            Tool::Read => Arc::new(read::Read::new(root)),
            Tool::Bash => Arc::new(bash::Bash::new(root)),
            Tool::Edit => Arc::new(edit::Edit::new(root)),
            Tool::Write => Arc::new(write::Write::new(root)),
            Tool::Grep => Arc::new(grep::Grep::new(root)),
            Tool::Find => Arc::new(find::Find::new(root)),
            Tool::Ls => Arc::new(ls::Ls::new(root)),
        }
    }
}

/// The coding tools on one root, as a plugin. All seven by default;
/// [`only`](Self::only) and [`without`](Self::without) pick a subset.
/// The tools are offered in pi's order, whatever order they were picked
/// in.
#[derive(Debug, Clone)]
pub struct CodingTools {
    root: Root,
    tools: Vec<Tool>,
}

impl CodingTools {
    pub fn new(root: Root) -> Self {
        Self {
            root,
            tools: Tool::ALL.to_vec(),
        }
    }

    /// Keeps only `tools`.
    pub fn only(mut self, tools: &[Tool]) -> Self {
        self.tools.retain(|tool| tools.contains(tool));
        self
    }

    /// Drops `tool`.
    pub fn without(mut self, tool: Tool) -> Self {
        self.tools.retain(|kept| *kept != tool);
        self
    }

    /// The tools this plugin adds, in pi's order.
    pub fn selected(&self) -> &[Tool] {
        &self.tools
    }
}

impl Plugin for CodingTools {
    fn name(&self) -> &str {
        "coding-tools"
    }

    fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        self.tools
            .iter()
            .map(|tool| tool.build(&self.root))
            .collect()
    }
}
