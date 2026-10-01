//! The host's half of the plugins with their UI (ADR 0017): each one's
//! state on this host, the context it gets, and what it is asked.

use std::collections::BTreeMap;

use serde_json::Value;
use tau_ui_plugin::{
    ErasedPlugin,
    HostCx,
    Push,
    RepoCtx,
    RunCtx,
    RunKind,
    Services,
    registry::HostState,
};

use super::*;
use crate::plugins::registry;

/// A plugin with its UI, and its state on this host.
#[derive(Clone)]
pub(super) struct Hosted {
    pub plugin: Arc<dyn ErasedPlugin>,
    pub state: Arc<HostState>,
}

impl Host {
    /// Makes each plugin's host state; a plugin whose state cannot be
    /// made is left out, and says why.
    pub(super) fn host_plugins(&mut self) {
        let cx = self.host_cx();
        self.hosted = registry()
            .plugins()
            .filter_map(|plugin| match plugin.host(&cx) {
                Ok(state) => Some(Hosted {
                    plugin: plugin.clone(),
                    state: Arc::new(state),
                }),
                Err(error) => {
                    eprintln!("tau-ui: {} is off: {error:#}", plugin.name());
                    None
                }
            })
            .collect();
    }

    /// What every plugin reaches of this host now.
    pub(super) fn host_cx(&self) -> HostCx {
        let mut services = Services::default();
        if let Some(jev) = self.jev() {
            services = services.with(jev);
        }
        let repos = self
            .repos
            .lock()
            .expect("not poisoned")
            .iter()
            .map(|slot| self.repo_ctx(slot))
            .collect();
        let pushes = self.pushes.clone();
        HostCx::new(
            self.store.clone(),
            self.runtime.handle().clone(),
            services,
            self.config
                .repos
                .parent()
                .unwrap_or(&self.config.repos)
                .to_owned(),
            repos,
            Arc::new(move |push| {
                let _ = pushes.send(push);
            }),
        )
    }

    pub(super) fn repo_ctx(&self, slot: &RepoSlot) -> RepoCtx {
        RepoCtx {
            name: slot.name.clone(),
            checkout: slot.path.clone(),
            dir: self.config.project_dir_of(&slot.path),
        }
    }

    /// The run (or sub-agent) `kind` in `repo` on `choice`, as plugins
    /// see it.
    pub(super) fn run_ctx(
        &self,
        kind: RunKind,
        repo: &RepoSlot,
        choice: &ModelChoice,
    ) -> RunCtx {
        let mut services = Services::default();
        if let Some(jev) = self.jev() {
            services = services.with(jev);
        }
        RunCtx {
            kind,
            repo: self.repo_ctx(repo),
            model: choice.model.clone(),
            effort: choice
                .effort
                .reasoning()
                .map(|effort| effort.as_str().to_owned()),
            services,
        }
    }

    /// The saved settings of `plugin`, or its defaults.
    pub(super) fn plugin_settings(&self, plugin: &dyn ErasedPlugin) -> Value {
        self.settings
            .lock()
            .expect("not poisoned")
            .plugins
            .get(plugin.name())
            .cloned()
            .unwrap_or_else(|| plugin.default_settings())
    }

    /// What adds each plugin's agent plugins, in the registry's order, to
    /// an agent in `repo`: the run's, or a sub-agent's, on its model. A
    /// plugin that cannot build its own fails the run.
    pub(super) fn registered(
        &self,
        repo: &RepoSlot,
    ) -> impl Fn(Agent, RunKind, &ModelChoice) -> anyhow::Result<Agent>
    + Clone
    + Send
    + Sync
    + 'static {
        let hosted: Vec<(Hosted, Value)> = self
            .hosted
            .iter()
            .map(|hosted| {
                (hosted.clone(), self.plugin_settings(hosted.plugin.as_ref()))
            })
            .collect();
        let jev = self.jev();
        let repo = self.repo_ctx(repo);
        move |agent: Agent, kind: RunKind, choice: &ModelChoice| {
            let mut services = Services::default();
            if let Some(jev) = &jev {
                services = services.with(jev.clone());
            }
            let run = RunCtx {
                kind,
                repo: repo.clone(),
                model: choice.model.clone(),
                effort: choice
                    .effort
                    .reasoning()
                    .map(|effort| effort.as_str().to_owned()),
                services,
            };
            hosted.iter().try_fold(agent, |agent, (hosted, settings)| {
                Ok(hosted
                    .plugin
                    .agent_plugins(&hosted.state, &run, settings)?
                    .into_iter()
                    .fold(agent, Agent::boxed_plugin))
            })
        }
    }

    /// What plugins say as `run` starts or goes on, by plugin, to fold
    /// into its view.
    pub(super) fn starting(&self, run: &RunCtx) -> Vec<(String, Value)> {
        self.hosted
            .iter()
            .flat_map(|hosted| {
                let settings = self.plugin_settings(hosted.plugin.as_ref());
                hosted
                    .plugin
                    .starting(&hosted.state, run, &settings)
                    .into_iter()
                    .map(|body| (hosted.plugin.name().to_owned(), body))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Each plugin's catalog entry, its data, and its settings.
    pub(super) fn registered_catalog(
        &self,
    ) -> (
        Vec<PluginInfo>,
        BTreeMap<String, Value>,
        BTreeMap<String, Value>,
    ) {
        let cx = self.host_cx();
        let mut plugins = Vec::new();
        let mut data = BTreeMap::new();
        let mut settings = BTreeMap::new();
        for hosted in &self.hosted {
            let name = hosted.plugin.name().to_owned();
            let saved = self.plugin_settings(hosted.plugin.as_ref());
            let info = hosted.plugin.catalog(&hosted.state, &cx, &saved);
            plugins.push(PluginInfo {
                name: info.name,
                description: info.description,
                seams: info.seams,
                spend: info.spend,
                screen: None,
                page: info.page,
            });
            data.insert(name.clone(), hosted.plugin.data(&hosted.state, &cx));
            settings.insert(name, saved);
        }
        (plugins, data, settings)
    }

    /// Each plugin's data for the repository in `slot`.
    pub(super) fn registered_repo_data(
        &self,
        slot: &RepoSlot,
    ) -> BTreeMap<String, Value> {
        let cx = self.host_cx();
        let repo = self.repo_ctx(slot);
        self.hosted
            .iter()
            .map(|hosted| {
                (
                    hosted.plugin.name().to_owned(),
                    hosted.plugin.repo_data(&hosted.state, &repo, &cx),
                )
            })
            .collect()
    }

    /// Carries out what `plugin`'s UI asked; its answer, if any, goes
    /// back to the UI.
    pub fn plugin_act(
        &self,
        plugin: &str,
        action: Value,
    ) -> anyhow::Result<Option<Value>> {
        let hosted = self
            .hosted
            .iter()
            .find(|hosted| hosted.plugin.name() == plugin)
            .ok_or_else(|| anyhow::anyhow!("No plugin {plugin} here"))?;
        hosted.plugin.act(&hosted.state, action, &self.host_cx())
    }

    /// Saves `plugin`'s settings with the model settings; runs started
    /// from now on take them.
    pub fn save_plugin_settings(
        &self,
        plugin: &str,
        value: Value,
    ) -> anyhow::Result<()> {
        let mut settings = self.settings.lock().expect("not poisoned").clone();
        settings.plugins.insert(plugin.to_owned(), value);
        self.save_settings(settings)
    }

    /// Stores `body` as `plugin`'s record with `run`: a change the
    /// interface made and folded already.
    pub fn store_plugin_record(
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
            tau_store::TurnUsage::default(),
        ))?;
        Ok(())
    }

    /// What plugins say as `run` goes on, on `choice`.
    pub fn starting_of(
        &self,
        run: &RunId,
        choice: &ModelChoice,
    ) -> Vec<(String, Value)> {
        let Ok(slot) = self.slot_of_run(run) else {
            return Vec::new();
        };
        let kind = if self.is_main(run) {
            RunKind::Main
        } else {
            RunKind::Chat
        };
        self.starting(&self.run_ctx(kind, &slot, choice))
    }
}

/// What a push from a plugin's host half does to the interface.
pub(super) fn apply_push(
    host: &Host,
    push: Push,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    workspace.update(cx, |ws, cx| match push {
        Push::Record { run, plugin, body } => {
            ws.apply(HostUpdate::PluginRecord { run, plugin, body }, cx)
        }
        Push::Catalog => ws.apply(HostUpdate::catalog(host.catalog()), cx),
        Push::Alert { title, message } => {
            ws.apply(HostUpdate::alert(title, message), cx)
        }
    });
}
