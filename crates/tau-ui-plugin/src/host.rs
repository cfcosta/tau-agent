//! What a plugin reaches on the machine that runs agents: the run it is
//! built for ([`RunCtx`]), and the host's services ([`HostCx`]).

use std::{path::PathBuf, sync::Arc};

use serde_json::Value;
use tau_agent::tool::RunId;
use tau_store::{Entry, Store, TurnUsage};

use crate::services::Services;

/// Where a run sits (ADR 0016): a repository's main chat, a chat under
/// it, or a sub-agent of either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunKind {
    Main,
    Chat,
    SubAgent,
}

/// A repository runs work in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoCtx {
    /// The name the sidebar lists it under.
    pub name: String,
    /// Its checkout.
    pub checkout: PathBuf,
    /// tau's own directory for it, outside the repository: archives,
    /// memory notes.
    pub dir: PathBuf,
}

/// The run a plugin is built for.
#[derive(Debug, Clone)]
pub struct RunCtx {
    pub kind: RunKind,
    pub repo: RepoCtx,
    /// The model the run (or sub-agent) runs on.
    pub model: String,
    /// The reasoning effort picked by hand; `None` is auto.
    pub effort: Option<String>,
    /// What the host has for this run: the metered Jev when there is a
    /// key, the run's workspace, and so on.
    pub services: Services,
}

/// What the host tells the interface after a plugin acted.
#[derive(Debug, Clone, PartialEq)]
pub enum Push {
    /// A record stored with a run: the interface folds it like a
    /// published one.
    Record {
        run: RunId,
        plugin: String,
        body: Value,
    },
    /// The catalog changed (rules, notes, settings): draw it again.
    Catalog,
    Alert {
        title: String,
        message: String,
    },
}

/// The host's services, the only part of the host a plugin reaches.
#[derive(Clone)]
pub struct HostCx {
    pub store: Store,
    pub runtime: tokio::runtime::Handle,
    /// Host-wide services, by type.
    pub services: Services,
    /// tau's directory for data shared by every repository.
    pub dir: PathBuf,
    /// The repositories the host lists.
    pub repos: Vec<RepoCtx>,
    push: Arc<dyn Fn(Push) + Send + Sync>,
}

impl HostCx {
    pub fn new(
        store: Store,
        runtime: tokio::runtime::Handle,
        services: Services,
        dir: PathBuf,
        repos: Vec<RepoCtx>,
        push: Arc<dyn Fn(Push) + Send + Sync>,
    ) -> Self {
        Self {
            store,
            runtime,
            services,
            dir,
            repos,
            push,
        }
    }

    /// The repository listed as `name`.
    pub fn repo(&self, name: &str) -> Option<&RepoCtx> {
        self.repos.iter().find(|repo| repo.name == name)
    }

    /// `plugin`'s records along `run`'s fork chain, oldest first.
    pub fn records(
        &self,
        run: &RunId,
        plugin: &str,
    ) -> anyhow::Result<Vec<Value>> {
        let bodies =
            self.runtime.block_on(self.store.records(&run.0, plugin))?;
        Ok(bodies
            .iter()
            .filter_map(|body| serde_json::from_str(body).ok())
            .collect())
    }

    /// `plugin`'s records in every run, each with the run that stored it.
    pub fn records_everywhere(
        &self,
        plugin: &str,
    ) -> anyhow::Result<Vec<(RunId, Value)>> {
        let rows = self
            .runtime
            .block_on(self.store.plugin_entries_everywhere(plugin))?;
        Ok(rows
            .into_iter()
            .filter_map(|(run, body)| {
                Some((RunId(run.into()), serde_json::from_str(&body).ok()?))
            })
            .collect())
    }

    /// Stores `body` as `plugin`'s record with `run`, and hands it to the
    /// interface, which folds it as if the run had published it: an
    /// interface's own change to a plugin's state (pause a goal).
    pub fn publish(
        &self,
        run: &RunId,
        plugin: &str,
        body: &Value,
    ) -> anyhow::Result<()> {
        let entry = Entry::Plugin {
            plugin: plugin.to_owned(),
            body: body.to_string(),
        };
        self.runtime.block_on(self.store.append_turn(
            &run.0,
            &[entry],
            TurnUsage::default(),
        ))?;
        (self.push)(Push::Record {
            run: run.clone(),
            plugin: plugin.to_owned(),
            body: body.clone(),
        });
        Ok(())
    }

    /// Asks the interface to draw the catalog again.
    pub fn refresh(&self) {
        (self.push)(Push::Catalog);
    }

    pub fn alert(&self, title: impl Into<String>, message: impl Into<String>) {
        (self.push)(Push::Alert {
            title: title.into(),
            message: message.into(),
        });
    }
}
