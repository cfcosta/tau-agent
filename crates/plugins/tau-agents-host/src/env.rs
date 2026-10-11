//! What the agents reach of the host (ADR 0037): an agent's settings,
//! what its runs cost, and its conversation. [`HostEnv`] is the host's;
//! tests give their own.

use std::{collections::BTreeMap, path::Path, sync::Arc};

use serde_json::Value;
use tau_agents::{NAME, Settings};
use tau_tools_host::secrets::SecretStore;
use tau_ui_plugin::{
    Conversations,
    HOST_RECORD,
    HostCx,
    HostRecord,
    registry::HostFuture,
};

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

    /// What each run in `agent`'s repository has cost so far, in
    /// dollars, by run: its turns, its sub-agents', what its plugins
    /// charged.
    fn costs(
        &self,
        agent: &str,
    ) -> HostFuture<'_, anyhow::Result<BTreeMap<String, f64>>>;

    /// The host's conversations, when it lets them be driven.
    fn conversations(&self) -> Option<Arc<dyn Conversations>>;

    /// tau's secret store, which agents' requests name keys from.
    fn secrets(&self) -> SecretStore;

    /// Calls the tool `name` with `args` outside any run, for the agent
    /// checked out at `checkout`: what its poll reads.
    fn call_tool<'a>(
        &'a self,
        checkout: &'a Path,
        name: &'a str,
        args: Value,
    ) -> HostFuture<'a, Result<Value, String>>;

    /// Makes the HTTP request `spec`, its secrets from [`Self::secrets`]:
    /// what its poll reads ([`crate::poll::fetch`]).
    fn fetch(&self, spec: Value) -> HostFuture<'_, Result<Value, String>>;
}

/// The host's [`Env`], over what it gives every plugin.
pub struct HostEnv(pub HostCx);

impl Env for HostEnv {
    fn refresh(&self) {
        self.0.refresh();
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

    fn costs(
        &self,
        agent: &str,
    ) -> HostFuture<'_, anyhow::Result<BTreeMap<String, f64>>> {
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
            runs.sort();
            runs.dedup();
            let mut costs = BTreeMap::new();
            // A run's sub-agents, and theirs, are its repository's too.
            while let Some(run) = runs.pop() {
                if costs.contains_key(&run) {
                    continue;
                }
                if let Some(record) = self.0.store.run(&run).await? {
                    costs.insert(run.clone(), record.cost_usd);
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
            Ok(costs)
        })
    }

    fn conversations(&self) -> Option<Arc<dyn Conversations>> {
        self.0.conversations()
    }

    fn secrets(&self) -> SecretStore {
        let dir = self.0.config_dir().map_or_else(
            || self.0.dir.join("config"),
            std::path::Path::to_path_buf,
        );
        SecretStore::in_dir(&dir)
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

    fn fetch(&self, spec: Value) -> HostFuture<'_, Result<Value, String>> {
        Box::pin(
            async move { crate::poll::fetch(&self.secrets(), &spec).await },
        )
    }
}
