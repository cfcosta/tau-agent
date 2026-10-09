//! The host's half of the plugins with their UI (ADR 0017): each one's
//! state on this host, the context it gets, and what it is asked.

use std::collections::BTreeMap;

use serde_json::Value;
use tau_ui_plugin::{
    ConfigDir,
    ErasedPlugin,
    HostCx,
    PluginValue,
    Push,
    RepoCtx,
    RunCtx,
    RunKind,
    SavedSettings,
    Services,
};

use super::*;
use crate::hosted;
pub(super) use crate::hosted::Hosted;

impl Host {
    /// Makes each plugin's host state.
    pub(super) fn host_plugins(&mut self) {
        let cx = self.host_cx();
        self.hosted = self.setting_up(hosted::host_all(&cx));
    }

    /// What every plugin reaches of this host now.
    pub(super) fn host_cx(&self) -> HostCx {
        let (settings, path, saving) = (
            self.settings.clone(),
            self.config.settings.clone(),
            self.settings_saving.clone(),
        );
        let read = settings.clone();
        let saved = SavedSettings::new(
            move |plugin| {
                read.lock()
                    .expect("not poisoned")
                    .plugins
                    .get(plugin)
                    .cloned()
            },
            move |plugin, value| {
                settings
                    .lock()
                    .expect("not poisoned")
                    .plugins
                    .insert(plugin.to_owned(), value);
                let (settings, path, saving) =
                    (settings.clone(), path.clone(), saving.clone());
                Box::pin(async move {
                    saving
                        .save(
                            || settings.lock().expect("not poisoned").clone(),
                            async move |settings| {
                                write_settings(&path, &settings).await
                            },
                        )
                        .await
                })
            },
        );
        let mut services = Services::default()
            .with(self.memory_search)
            .with(saved)
            .with(ConfigDir(self.config.credentials.dir.clone()))
            .with(tau_skills::SkillsDir(self.config.skills.clone()));
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
            // The project's root once imported; until then, where it
            // will be.
            workspaces: match slot.project.peek() {
                ProjectState::Ready(project) => project.root().to_owned(),
                _ => self.config.project_dir_of(&slot.path),
            },
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

    /// Where `plugin` keeps its own files on this host.
    pub fn plugin_dir(&self, plugin: &str) -> PathBuf {
        self.host_cx().plugin_dir(plugin)
    }

    /// The saved settings of `plugin`, or its defaults.
    pub(super) fn plugin_settings(
        &self,
        plugin: &dyn ErasedPlugin,
        repo: Option<&str>,
    ) -> PluginValue {
        let settings = self.settings.lock().expect("not poisoned");
        repo.and_then(|repo| {
            settings.repo_plugins.get(repo)?.get(plugin.name())
        })
        .or_else(|| settings.plugins.get(plugin.name()))
        .cloned()
        .map(PluginValue::from_json)
        .unwrap_or_else(|| plugin.default_settings())
    }

    /// The repositories' own copies of plugins' settings, by repository,
    /// then plugin.
    pub(super) fn repo_plugin_settings(
        &self,
    ) -> BTreeMap<String, BTreeMap<String, PluginValue>> {
        self.settings
            .lock()
            .expect("not poisoned")
            .repo_plugins
            .iter()
            .map(|(repo, plugins)| {
                let plugins = plugins
                    .iter()
                    .map(|(plugin, value)| {
                        (plugin.clone(), PluginValue::from_json(value.clone()))
                    })
                    .collect();
                (repo.clone(), plugins)
            })
            .collect()
    }

    /// What commands in `repo` start through: every plugin's launcher,
    /// joined in the registry's order; none when no plugin gives one.
    pub(super) async fn launcher_of(
        &self,
        repo: &RepoSlot,
    ) -> Option<Arc<dyn tau_agent::launch::Launcher>> {
        let ctx = self.repo_ctx(repo);
        let mut launchers: Vec<Arc<dyn tau_agent::launch::Launcher>> =
            Vec::new();
        for hosted in &self.hosted {
            let settings =
                self.plugin_settings(hosted.plugin.as_ref(), Some(&ctx.name));
            launchers.extend(
                hosted.plugin.launcher(&hosted.state, &ctx, &settings).await,
            );
        }
        match launchers.len() {
            0 => None,
            1 => launchers.pop(),
            _ => Some(Arc::new(tau_agent::launch::Launchers(launchers))),
        }
    }

    /// Whether a plugin lets a chat in `repo`, working in `workspace`,
    /// land by itself (ADR 0034).
    pub(super) async fn lands_itself(
        &self,
        repo: &RepoSlot,
        workspace: &Path,
    ) -> bool {
        let ctx = self.repo_ctx(repo);
        for hosted in &self.hosted {
            if hosted
                .plugin
                .lands_itself(&hosted.state, &ctx, workspace)
                .await
            {
                return true;
            }
        }
        false
    }

    /// What adds each plugin's agent plugins, in the registry's order, to
    /// an agent in `repo`: the run's, or a sub-agent's, on its model, with
    /// what its workspace offers (`services`). A plugin that cannot build
    /// its own fails the run.
    pub(super) async fn registered(
        &self,
        repo: &RepoSlot,
    ) -> impl Fn(
        Agent,
        RunKind,
        &ModelChoice,
        Services,
    ) -> tau_ui_plugin::registry::HostFuture<
        'static,
        anyhow::Result<Agent>,
    > + Clone
    + Send
    + Sync
    + 'static {
        let hosted: Vec<(Hosted, PluginValue)> = self
            .hosted
            .iter()
            .map(|hosted| {
                (
                    hosted.clone(),
                    self.plugin_settings(
                        hosted.plugin.as_ref(),
                        Some(&repo.name),
                    ),
                )
            })
            .collect();
        let jev = self.jev();
        // The repository's own commands (its MCP servers) start through
        // what the plugins give it, in its main workspace.
        let repo_launcher = self.launcher_of(repo).await.and_then(|launcher| {
            let project = repo.project.ready()?;
            Some(tau_ui_plugin::RepoLauncher {
                launcher,
                dir: project.workspace_dir(DEFAULT_WORKSPACE),
            })
        });
        let hosted = Arc::new(hosted);
        let repo = self.repo_ctx(repo);
        move |agent: Agent,
              kind: RunKind,
              choice: &ModelChoice,
              mut services: Services| {
            if let Some(jev) = &jev {
                services = services.with(jev.clone());
            }
            if let Some(launcher) = &repo_launcher {
                services = services.with(launcher.clone());
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
            let hosted = hosted.clone();
            Box::pin(async move {
                let mut agent = agent;
                for (hosted, settings) in hosted.iter() {
                    agent = hosted
                        .plugin
                        .agent_plugins(&hosted.state, &run, settings)
                        .await?
                        .into_iter()
                        .fold(agent, Agent::boxed_plugin);
                }
                Ok(agent)
            })
        }
    }

    /// What plugins say as `run` starts or goes on, by plugin, to fold
    /// into its view.
    pub(super) async fn starting(&self, run: &RunCtx) -> Vec<(String, Value)> {
        let mut said = Vec::new();
        for hosted in &self.hosted {
            let settings = self
                .plugin_settings(hosted.plugin.as_ref(), Some(&run.repo.name));
            said.extend(
                hosted
                    .plugin
                    .starting(&hosted.state, run, &settings)
                    .await
                    .into_iter()
                    .map(|body| (hosted.plugin.name().to_owned(), body)),
            );
        }
        said
    }

    /// Each plugin's catalog entry, its data, and its settings.
    pub(super) async fn registered_catalog(&self) -> hosted::Catalogued {
        hosted::catalog(&self.hosted, &self.host_cx(), |plugin| {
            self.plugin_settings(plugin, None)
        })
        .await
    }

    /// Each plugin's data for the repository in `slot`.
    pub(super) async fn registered_repo_data(
        &self,
        slot: &RepoSlot,
    ) -> BTreeMap<String, PluginValue> {
        hosted::repo_data(&self.hosted, &self.repo_ctx(slot), &self.host_cx())
            .await
    }

    /// Carries out what `plugin`'s UI asked; its answer, if any, goes
    /// back to the UI.
    pub async fn plugin_act(
        &self,
        plugin: &str,
        action: Value,
    ) -> anyhow::Result<Option<Value>> {
        hosted::act(&self.hosted, plugin, action, &self.host_cx()).await
    }

    /// Starts with `value` as `plugin`'s settings everywhere, unsaved:
    /// for a host that must not take a plugin's defaults, as in tests
    /// whose scripted model answers only the requests they script.
    pub fn with_plugin_settings(self, plugin: &str, value: Value) -> Self {
        self.settings
            .lock()
            .expect("not poisoned")
            .plugins
            .insert(plugin.to_owned(), value);
        self
    }

    /// Saves `plugin`'s settings with the model settings; runs started
    /// from now on take them.
    pub async fn save_plugin_settings(
        &self,
        plugin: &str,
        repo: Option<String>,
        value: Option<Value>,
    ) -> anyhow::Result<()> {
        let plugin = plugin.to_owned();
        self.change_settings(move |settings| match (repo, value) {
            (None, Some(value)) => {
                settings.plugins.insert(plugin, value);
            }
            (Some(repo), Some(value)) => {
                settings
                    .repo_plugins
                    .entry(repo)
                    .or_default()
                    .insert(plugin, value);
            }
            (Some(repo), None) => {
                if let Some(plugins) = settings.repo_plugins.get_mut(&repo) {
                    plugins.remove(&plugin);
                    if plugins.is_empty() {
                        settings.repo_plugins.remove(&repo);
                    }
                }
            }
            (None, None) => {}
        })
        .await
    }

    /// Stores `body` as `plugin`'s record with `run`: a change the
    /// interface made and folded already.
    pub async fn store_plugin_record(
        &self,
        run: &RunId,
        plugin: &str,
        body: &Value,
    ) -> anyhow::Result<()> {
        let entry = Entry::Plugin {
            plugin: plugin.to_owned(),
            body: body.to_string(),
        };
        self.store
            .append_turn(&run.0, &[entry], tau_store::TurnUsage::default())
            .await?;
        Ok(())
    }

    /// What plugins say as `run` goes on, on `choice`.
    pub async fn starting_of(
        &self,
        run: &RunId,
        choice: &ModelChoice,
    ) -> Vec<(String, Value)> {
        let Ok(slot) = self.slot_of_run(run).await else {
            return Vec::new();
        };
        let kind = if self.is_main(run) {
            RunKind::Main
        } else {
            RunKind::Chat
        };
        self.starting(&self.run_ctx(kind, &slot, choice)).await
    }
}

/// What a push from a plugin's host half does to the interface.
pub(super) fn apply_push(
    host: &Arc<Host>,
    push: Push,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    match push {
        Push::Record { run, plugin, body } => workspace.update(cx, |ws, cx| {
            ws.apply(HostUpdate::PluginRecord { run, plugin, body }, cx)
        }),
        Push::Catalog => host.catalog_changed(),
        Push::Alert { title, message } => workspace.update(cx, |ws, cx| {
            ws.apply(HostUpdate::alert(title, message), cx)
        }),
    }
}
