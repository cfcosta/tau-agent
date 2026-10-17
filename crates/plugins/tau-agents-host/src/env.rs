//! What the agents reach of the host (ADR 0037): an agent's settings,
//! what its runs cost, and its conversation. [`HostEnv`] is the host's;
//! tests give their own.

use std::{path::Path, sync::Arc};

use serde_json::Value;
use tau_agents::{NAME, Settings};
use tau_tools_host::secrets::SecretStore;
use tau_ui_plugin::{
    Conversations,
    HOST_RECORD,
    HostCx,
    HostRecord,
    ResourceWatch,
    WatchError,
    registry::HostFuture,
};
use tau_vcs_host::Identity;

/// What the agents reach of the host.
pub trait Env: Send + Sync {
    /// Asks the interface to draw the agents again.
    fn refresh(&self);

    /// What the person set for `agent`.
    fn settings(&self, agent: &str) -> Settings;

    /// Saves `settings` as `agent`'s.
    fn save_settings(
        &self,
        agent: &str,
        settings: Settings,
    ) -> HostFuture<'_, anyhow::Result<()>>;

    /// What everything in `agent`'s repository cost from `since` (in
    /// seconds since the epoch) on, in dollars: its runs' turns, its
    /// sub-agents', its compactions and what its plugins charged.
    fn spent_since(
        &self,
        agent: &str,
        since: i64,
    ) -> HostFuture<'_, anyhow::Result<f64>>;

    /// The host's conversations, when it lets them be driven.
    fn conversations(&self) -> Option<Arc<dyn Conversations>>;

    /// `agent`'s own secret store, which its requests name keys from.
    fn secrets(&self, agent: &str) -> SecretStore;

    /// Calls the tool `name` with `args` outside any run, for the agent
    /// checked out at `checkout`: what its poll reads.
    fn call_tool<'a>(
        &'a self,
        checkout: &'a Path,
        name: &'a str,
        args: Value,
    ) -> HostFuture<'a, Result<Value, String>>;

    /// Makes `agent`'s HTTP request `spec`, its secrets from its own
    /// [`Self::secrets`]: what its poll reads ([`crate::poll::fetch`]).
    fn fetch<'a>(
        &'a self,
        agent: &'a str,
        spec: Value,
    ) -> HostFuture<'a, Result<Value, String>>;

    /// Watches the MCP resource `uri` of `server` for the agent checked
    /// out at `checkout`: what its `mcp` trigger follows.
    fn watch_resource<'a>(
        &'a self,
        _checkout: &'a Path,
        _server: &'a str,
        _uri: &'a str,
    ) -> HostFuture<'a, Result<ResourceWatch, WatchError>> {
        Box::pin(async {
            Err(WatchError::Failed(
                "this host watches no MCP resources".into(),
            ))
        })
    }

    /// Waits until the MCP server `server`'s connection for the agent
    /// checked out at `checkout` changes, so a watch it could not take is
    /// tried again. It never does by default.
    fn server_changed<'a>(
        &'a self,
        _checkout: &'a Path,
        _server: &'a str,
    ) -> HostFuture<'a, ()> {
        Box::pin(std::future::pending())
    }

    /// Stores `record`, a grant of tau-tools' (`artifact_read` reads its
    /// grants from them), with the run `run`: an agent's main chat, so
    /// its runs read the artifact an event's data was kept as.
    fn grant<'a>(
        &'a self,
        _run: &'a str,
        _record: Value,
    ) -> HostFuture<'a, anyhow::Result<()>> {
        Box::pin(async { anyhow::bail!("this host grants no artifacts") })
    }

    /// Reads the MCP resource `uri` of `server` for the agent checked
    /// out at `checkout`: its `ReadResourceResult` as JSON.
    fn read_resource<'a>(
        &'a self,
        _checkout: &'a Path,
        _server: &'a str,
        _uri: &'a str,
    ) -> HostFuture<'a, Result<Value, String>> {
        Box::pin(async { Err("this host reads no MCP resources".into()) })
    }

    /// Who the agents' repositories are opened as, and the jj config
    /// they follow. The default reads none.
    fn identity(&self) -> Identity {
        Identity::default()
    }
}

/// The host's [`Env`], over what it gives every plugin.
pub struct HostEnv(pub HostCx);

impl Env for HostEnv {
    fn refresh(&self) {
        self.0.refresh();
    }

    fn identity(&self) -> Identity {
        self.0
            .services
            .get::<Identity>()
            .cloned()
            .unwrap_or_default()
    }

    fn settings(&self, agent: &str) -> Settings {
        self.0.settings_in(NAME, agent)
    }

    fn save_settings(
        &self,
        agent: &str,
        settings: Settings,
    ) -> HostFuture<'_, anyhow::Result<()>> {
        let agent = agent.to_owned();
        Box::pin(async move {
            self.0.save_settings_in(NAME, &agent, &settings).await
        })
    }

    fn spent_since(
        &self,
        agent: &str,
        since: i64,
    ) -> HostFuture<'_, anyhow::Result<f64>> {
        let agent = agent.to_owned();
        Box::pin(async move {
            let mut runs: Vec<String> = self
                .0
                .records_everywhere(HOST_RECORD)
                .await?
                .into_iter()
                .filter(|(_, body)| {
                    serde_json::from_value::<HostRecord>(body.clone())
                        .is_ok_and(|record| record.repo == agent)
                })
                .map(|(run, _)| run.0.to_string())
                .collect();
            // A run's sub-agents, and theirs, are its repository's too.
            let mut seen = std::collections::BTreeSet::new();
            while let Some(run) = runs.pop() {
                if !seen.insert(run.clone()) {
                    continue;
                }
                runs.extend(
                    self.0
                        .store
                        .subagents(&run)
                        .await?
                        .into_iter()
                        .map(|child| child.id),
                );
            }
            let runs: Vec<String> = seen.into_iter().collect();
            Ok(self
                .0
                .store
                .spent_since(&runs, &crate::clock::stamp(since))
                .await?)
        })
    }

    fn conversations(&self) -> Option<Arc<dyn Conversations>> {
        self.0.conversations()
    }

    fn secrets(&self, agent: &str) -> SecretStore {
        let dir = self.0.config_dir().map_or_else(
            || self.0.dir.join("config"),
            std::path::Path::to_path_buf,
        );
        SecretStore::of_agent(&dir, agent)
    }

    fn call_tool<'a>(
        &'a self,
        checkout: &'a Path,
        name: &'a str,
        args: Value,
    ) -> HostFuture<'a, Result<Value, String>> {
        Box::pin(async move {
            match self.0.outside_tools() {
                Some(tools) => tools.call(checkout, name, args).await,
                None => Err("this host calls no tools outside a run".into()),
            }
        })
    }

    fn fetch<'a>(
        &'a self,
        agent: &'a str,
        spec: Value,
    ) -> HostFuture<'a, Result<Value, String>> {
        Box::pin(async move {
            crate::poll::fetch(&self.secrets(agent), &spec).await
        })
    }

    fn server_changed<'a>(
        &'a self,
        checkout: &'a Path,
        server: &'a str,
    ) -> HostFuture<'a, ()> {
        Box::pin(async move {
            match self.0.outside_resources() {
                Some(resources) => resources.changed(checkout, server).await,
                None => std::future::pending().await,
            }
        })
    }

    fn grant<'a>(
        &'a self,
        run: &'a str,
        record: Value,
    ) -> HostFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            let run = tau_agent::tool::RunId(run.into());
            self.0.publish(&run, tau_tools::ui::NAME, &record).await
        })
    }

    fn watch_resource<'a>(
        &'a self,
        checkout: &'a Path,
        server: &'a str,
        uri: &'a str,
    ) -> HostFuture<'a, Result<ResourceWatch, WatchError>> {
        Box::pin(async move {
            match self.0.outside_resources() {
                Some(resources) => resources.watch(checkout, server, uri).await,
                None => Err(WatchError::Failed(
                    "this host watches no MCP resources".into(),
                )),
            }
        })
    }

    fn read_resource<'a>(
        &'a self,
        checkout: &'a Path,
        server: &'a str,
        uri: &'a str,
    ) -> HostFuture<'a, Result<Value, String>> {
        Box::pin(async move {
            match self.0.outside_resources() {
                Some(resources) => resources.read(checkout, server, uri).await,
                None => Err("this host reads no MCP resources".into()),
            }
        })
    }
}
