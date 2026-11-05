//! tau-tools' host half (ADR 0030).

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::Context as _;
use serde_json::Value;
use tau_agent::plugin::Plugin;
use tau_artifacts::{Artifact, Bytes, Encoding, Quotas};
use tau_tools::{
    ToolsUi,
    artifact_grant::fold_grants,
    ui::{Action, ActionReply, NAME},
};
use tau_ui_plugin::{
    AgentRepo,
    HostCx,
    HostHalf,
    PluginHost,
    PluginInfo,
    Prompt,
    RepoCtx,
    RunCommands,
    RunCtx,
    RunKind,
    RunWake,
    WorkspaceDir,
};

#[cfg(unix)]
use crate::{
    exec::{Ended, Processes},
    path::Root,
    plugin::{CodingTools, Tool},
    secrets::SecretStore,
};

/// tau-tools on the host.
#[derive(Debug, Clone, Copy, Default)]
pub struct ToolsHost;

/// What tau-tools keeps on the host: the commands each workspace's runs
/// started and left running, by directory, which outlive a turn and end
/// as the workspace closes; and tau's configuration directory, where an
/// agent's secrets are.
pub struct Tools {
    #[cfg(unix)]
    processes: Mutex<HashMap<PathBuf, Processes>>,
    config: Option<PathBuf>,
}

impl PluginHost for Tools {
    async fn new(cx: &HostCx) -> anyhow::Result<Self> {
        Ok(Self {
            #[cfg(unix)]
            processes: Mutex::default(),
            config: cx.config_dir().map(Path::to_owned),
        })
    }
}

/// What tau-tools adds to every run's instructions.
const INSTRUCTIONS: &str =
    "Use the tools to read and change files and to run commands.";

impl HostHalf for ToolsHost {
    type Plugin = ToolsUi;
    type Host = Tools;

    /// None here: the tools are the run's ([`Self::run_tools`]).
    async fn agent_plugins(
        &self,
        _host: &Tools,
        _run: &RunCtx,
        _settings: &(),
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        Ok(Vec::new())
    }

    /// The coding tools on the run's workspace, with its commands left
    /// running there, its repository's artifacts, how its commands start
    /// ([`RunCommands`]), and `http` for an agent's run (ADR 0037). A
    /// command left running that ends wakes the run that asked to be
    /// told ([`RunWake`]); a sub-agent has none, as it ends and lands.
    #[cfg(unix)]
    async fn run_tools(
        &self,
        host: &Tools,
        run: &RunCtx,
        _settings: &(),
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        let Some(WorkspaceDir(dir)) = run.services.get::<WorkspaceDir>() else {
            return Ok(Vec::new());
        };
        // tau's directory for the repository, not the project's root: a
        // local repository's is the person's folder.
        let artifacts =
            Bytes::new(run.repo.dir.join("artifacts"), Quotas::default())?;
        // The commands it leaves running stay for its next turn.
        let running = host
            .processes
            .lock()
            .expect("not poisoned")
            .entry(dir.clone())
            .or_default()
            .clone();
        let tools = CodingTools::new(Root::new(dir.clone()))
            .with_artifacts(artifacts)
            .with_processes(running);
        let tools = match run.services.get::<RunWake>() {
            Some(wake) => {
                let wake = wake.clone();
                tools.with_notifier(Arc::new(move |ended| {
                    wake.wake(ended_note(&ended));
                }))
            }
            None => tools,
        };
        let tools = match run.services.get::<RunCommands>() {
            Some(RunCommands::Through(launcher)) => {
                tools.with_launcher(launcher.clone())
            }
            Some(RunCommands::Off) => {
                tools.without(Tool::ExecCommand).without(Tool::WriteStdin)
            }
            Some(RunCommands::Direct) | None => tools,
        };
        // An agent's runs reach the network with `http`, its keys named
        // from its own secret store (ADR 0037); no other run does.
        let tools = match (run.services.get::<AgentRepo>(), &host.config) {
            (Some(AgentRepo), Some(config)) => {
                tools.with_http(SecretStore::of_agent(config, &run.repo.name))
            }
            _ => tools,
        };
        Ok(vec![Box::new(tools)])
    }

    /// Ends the commands left running in the workspace at `dir`.
    #[cfg(unix)]
    async fn workspace_closed(
        &self,
        host: &Tools,
        _repo: &RepoCtx,
        dir: &Path,
    ) {
        let ended = host.processes.lock().expect("not poisoned").remove(dir);
        if let Some(processes) = ended {
            processes.terminate_all();
        }
    }

    fn prompt(&self, _host: &Tools, _kind: RunKind, _settings: &()) -> Prompt {
        Prompt {
            instructions: Some(INSTRUCTIONS.to_owned()),
            context: None,
        }
    }

    async fn catalog(
        &self,
        _host: &Tools,
        _cx: &HostCx,
        _settings: &(),
    ) -> PluginInfo {
        PluginInfo {
            description:
                "read, exec_command, write_stdin, edit, write, grep, find \
                          and ls on the run's workspace"
                    .into(),
            page: None,
            ..Default::default()
        }
    }

    async fn act(
        &self,
        _host: &Tools,
        action: Value,
        cx: &HostCx,
    ) -> anyhow::Result<Option<Value>> {
        let Action::Read {
            run,
            id,
            offset,
            encoding,
        } = serde_json::from_value(action)?;
        let read = async {
            let encoding = match encoding.as_str() {
                "utf8" => Encoding::Utf8,
                "base64" => Encoding::Base64,
                _ => anyhow::bail!("unsupported encoding"),
            };
            let starts = cx
                .store
                .plugin_entries(&run.0, tau_ui_plugin::HOST_RECORD)
                .await?;
            let (_, start) = starts.first().context("run has no repository")?;
            if starts.len() != 1 {
                anyhow::bail!("ambiguous repository");
            }
            let start: tau_ui_plugin::HostRecord = serde_json::from_str(start)?;
            let repo =
                cx.repo(&start.repo).context("repository unavailable")?;
            let records: Vec<Value> = cx
                .store
                .records(&run.0, NAME)
                .await?
                .into_iter()
                .map(|body| serde_json::from_str(&body))
                .collect::<Result<_, _>>()?;
            let grants = fold_grants(&records).map_err(anyhow::Error::msg)?;
            let grant = grants
                .get(&id)
                .context("artifact is not granted to this run")?;
            let artifact: Artifact =
                serde_json::from_value(serde_json::to_value(&grant.artifact)?)?;
            // Files: read off the async workers (ADR 0028).
            let dir = repo.dir.join("artifacts");
            let range = tokio::task::spawn_blocking(move || {
                Bytes::new(dir, Quotas::default())?.read_range(
                    &artifact,
                    offset,
                    1024,
                    encoding,
                    &tokio_util::sync::CancellationToken::new(),
                )
            })
            .await??;
            Ok::<_, anyhow::Error>(serde_json::to_value(range)?)
        }
        .await;
        Ok(Some(serde_json::to_value(match read {
            Ok(range) => ActionReply {
                id,
                range: Some(range),
                error: None,
            },
            Err(error) => ActionReply {
                id,
                range: None,
                error: Some(error.to_string()),
            },
        })?))
    }
}

/// What a background command's end tells the run that asked to be told.
#[cfg(unix)]
fn ended_note(ended: &Ended) -> String {
    let how = if ended.code == 0 {
        "exited with code 0".to_owned()
    } else {
        format!("exited with code {}", ended.code)
    };
    format!(
        "Process {} (`{}`) {how}. write_stdin with process_id {} reads \
         its output.",
        ended.process, ended.command, ended.process
    )
}
