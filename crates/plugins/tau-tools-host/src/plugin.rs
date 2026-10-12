//! The coding tools as a plugin (`docs/reference/plugins.md`), for
//! `Agent::plugin`.
//!
//! The plugin adds tools and nothing else. The commands `exec_command`
//! starts live in its [`Processes`], which the host can share between
//! the plugin's instances, so a run's processes outlive a turn. The
//! per-path locks that `edit` and `write` hold stay process-wide
//! ([`crate::lock`]), so two agents on one directory still take turns
//! on a file.

use std::sync::Arc;

use tau_agent::{launch::Launcher, plugin::Plugin, tool::AgentTool};
use tau_artifacts::Bytes;

use crate::{
    artifact_read,
    edit,
    exec::{self, Processes},
    find,
    grep,
    http,
    ls,
    path::Root,
    read,
    secrets::SecretStore,
    write,
};

/// One of the coding tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Tool {
    Read,
    ExecCommand,
    WriteStdin,
    Edit,
    Write,
    Grep,
    Find,
    Ls,
}

impl Tool {
    /// Every tool, in pi's order.
    pub const ALL: [Tool; 8] = [
        Tool::Read,
        Tool::ExecCommand,
        Tool::WriteStdin,
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
            Tool::ExecCommand => "exec_command",
            Tool::WriteStdin => "write_stdin",
            Tool::Edit => "edit",
            Tool::Write => "write",
            Tool::Grep => "grep",
            Tool::Find => "find",
            Tool::Ls => "ls",
        }
    }

    pub(crate) fn build(
        self,
        root: &Root,
        processes: &Processes,
    ) -> Arc<dyn AgentTool> {
        let root = root.clone();
        match self {
            Tool::Read => Arc::new(read::Read::new(root)),
            Tool::ExecCommand => {
                Arc::new(exec::ExecCommand::new(root, processes.clone()))
            }
            Tool::WriteStdin => {
                Arc::new(exec::WriteStdin::new(processes.clone()))
            }
            Tool::Edit => Arc::new(edit::Edit::new(root)),
            Tool::Write => Arc::new(write::Write::new(root)),
            Tool::Grep => Arc::new(grep::Grep::new(root)),
            Tool::Find => Arc::new(find::Find::new(root)),
            Tool::Ls => Arc::new(ls::Ls::new(root)),
        }
    }
}

/// The coding tools on one root, as a plugin. All eight by default;
/// [`only`](Self::only) and [`without`](Self::without) pick a subset.
/// The tools are offered in pi's order, whatever order they were picked
/// in.
#[derive(Clone)]
pub struct CodingTools {
    root: Root,
    tools: Vec<Tool>,
    artifacts: Option<Bytes>,
    launcher: Option<Arc<dyn Launcher>>,
    http: Option<SecretStore>,
    processes: Processes,
    notifier: Option<exec::Notifier>,
}

impl std::fmt::Debug for CodingTools {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodingTools")
            .field("root", &self.root)
            .field("tools", &self.tools)
            .field("artifacts", &self.artifacts)
            .field("launcher", &self.launcher.is_some())
            .field("http", &self.http)
            .finish()
    }
}

impl CodingTools {
    pub fn new(root: Root) -> Self {
        Self {
            root,
            tools: Tool::ALL.to_vec(),
            artifacts: None,
            launcher: None,
            http: None,
            processes: Processes::new(),
            notifier: None,
        }
    }

    /// Tells `notifier` when a command started with `notify` ends and no
    /// call had reported it.
    pub fn with_notifier(mut self, notifier: exec::Notifier) -> Self {
        self.notifier = Some(notifier);
        self
    }

    /// Adds `http` (ADR 0037), for scripts only, its secrets from
    /// `store`: an agent's runs have it, no others.
    pub fn with_http(mut self, store: SecretStore) -> Self {
        self.http = Some(store);
        self
    }

    /// Keeps the commands `exec_command` starts in `processes`, which
    /// outlive this plugin: a run's next turn finds them there.
    pub fn with_processes(mut self, processes: Processes) -> Self {
        self.processes = processes;
        self
    }

    /// Starts `exec_command`'s commands through `launcher`: the environment
    /// plugins give the run (`tau_agent::launch`).
    pub fn with_launcher(mut self, launcher: Arc<dyn Launcher>) -> Self {
        self.launcher = Some(launcher);
        self
    }

    /// Attach private byte storage for the nested artifact reader.
    pub fn with_artifacts(mut self, bytes: Bytes) -> Self {
        self.artifacts = Some(bytes);
        self
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
        crate::ui::NAME
    }

    fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        let exec = || {
            let exec = exec::ExecCommand::new(
                self.root.clone(),
                self.processes.clone(),
            );
            let exec = match &self.notifier {
                Some(notifier) => exec.with_notifier(notifier.clone()),
                None => exec,
            };
            match &self.launcher {
                Some(launcher) => exec.with_launcher(launcher.clone()),
                None => exec,
            }
        };
        let mut tools: Vec<Arc<dyn AgentTool>> = self
            .tools
            .iter()
            .map(|tool| match (tool, &self.artifacts) {
                (Tool::Read, Some(bytes)) => Arc::new(
                    read::Read::new(self.root.clone())
                        .with_artifacts(bytes.clone()),
                )
                    as Arc<dyn AgentTool>,
                (Tool::ExecCommand, _) => {
                    Arc::new(exec()) as Arc<dyn AgentTool>
                }
                _ => tool.build(&self.root, &self.processes),
            })
            .collect();
        tools.push(Arc::new(artifact_read::ArtifactRead::new(
            self.artifacts.clone(),
        )));
        if let Some(store) = &self.http {
            let http = http::Http::new(store.clone());
            tools.push(Arc::new(match &self.artifacts {
                Some(bytes) => http.with_artifacts(bytes.clone()),
                None => http,
            }));
        }
        tools
    }
}
