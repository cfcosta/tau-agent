//! tau-luau-plugins' host half (ADR 0030).

use serde_json::Value;
use tau_agent::plugin::Plugin;
use tau_luau_plugins::{
    Act,
    LuauPluginsUi,
    LuauSettings,
    Overview,
    Standing,
    ui::PAGE,
};
use tau_ui_kit::theme::Tone;
use tau_ui_plugin::{
    HostCx,
    HostHalf,
    Link,
    PluginInfo,
    RepoCtx,
    RunCtx,
    Seam,
};

/// tau-luau-plugins on the host.
#[derive(Debug, Clone, Copy, Default)]
pub struct LuauPluginsHost;

/// The versions one run has active, with the person's settings: trunk's,
/// or for a run in the plugins repository, its workspace's where they
/// may be.
struct RunVersions {
    registry: crate::registry::Registry,
    settings: LuauSettings,
    workspace: Option<std::path::PathBuf>,
}

#[async_trait::async_trait]
impl crate::agent::Versions for RunVersions {
    async fn now(&self) -> Vec<crate::agent::Active> {
        let active = match &self.workspace {
            Some(dir) => self.registry.in_workspace(dir).await,
            None => self.registry.active().await,
        };
        active
            .into_iter()
            .map(|mut active| {
                let name = &active.loaded.declaration.name;
                active.settings = crate::settings::effective(
                    &active.loaded.declaration,
                    self.settings.plugins.get(name),
                )
                .0;
                active
            })
            .collect()
    }
}

impl HostHalf for LuauPluginsHost {
    type Plugin = LuauPluginsUi;
    type Host = crate::registry::Registry;

    async fn agent_plugins(
        &self,
        host: &Self::Host,
        run: &RunCtx,
        settings: &LuauSettings,
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        let mut plugins: Vec<Box<dyn Plugin>> = Vec::new();
        // A run in the plugins repository tests plugins as its workspace
        // has them, and runs them from there as soon as they pass.
        let workspace = (run.repo.name == crate::REPO)
            .then(|| run.services.get::<tau_ui_plugin::WorkspaceDir>())
            .flatten()
            .map(|dir| dir.0.clone());
        if let Some(dir) = &workspace {
            plugins.push(Box::new(crate::testing::PluginTesting::new(
                dir.clone(),
            )));
        }
        let kind = match run.kind {
            tau_ui_plugin::RunKind::Main => "main",
            tau_ui_plugin::RunKind::Chat => "chat",
            tau_ui_plugin::RunKind::SubAgent => "sub_agent",
        };
        let info = serde_json::json!({
            "kind": kind,
            "repo": run.repo.name,
            "model": run.model,
        });
        let jev = run
            .services
            .get::<std::sync::Arc<dyn tau_jev::Jev>>()
            .cloned();
        // Always there, even with no plugin active yet: one that becomes
        // active while the run goes on works in it from its next request.
        let versions = RunVersions {
            registry: host.clone(),
            settings: settings.clone(),
            workspace,
        };
        plugins.push(Box::new(
            crate::agent::LuauPlugins::live(
                std::sync::Arc::new(versions),
                info,
                jev,
            )
            .await,
        ));
        Ok(plugins)
    }

    /// A chat in the plugins repository lands by itself once its
    /// plugins pass and want nothing new (ADR 0034).
    async fn lands_itself(
        &self,
        host: &Self::Host,
        repo: &RepoCtx,
        workspace: &std::path::Path,
    ) -> bool {
        repo.name == crate::REPO && host.ready(workspace).await
    }

    async fn catalog(
        &self,
        host: &Self::Host,
        _cx: &HostCx,
        _settings: &LuauSettings,
    ) -> PluginInfo {
        let description = {
            let overview = host.overview().await;
            let waiting = overview
                .plugins
                .iter()
                .filter(|entry| !matches!(entry.standing, Standing::Active))
                .count();
            match (overview.plugins.len(), waiting) {
                (0, _) => "Plugins written in Luau: none yet".to_owned(),
                (all, 0) => format!("Plugins written in Luau: {all} active"),
                (all, waiting) => {
                    format!(
                        "Plugins written in Luau: {all}, {waiting} need a look"
                    )
                }
            }
        };
        // Each plugin of the repository is a row of its own.
        let entries: Vec<tau_ui_plugin::CatalogEntry> = host
            .overview()
            .await
            .plugins
            .iter()
            .map(entry_row)
            .collect();
        PluginInfo {
            group: tau_ui_plugin::Group::Yours,
            description,
            seams: vec![
                Seam::Tools,
                Seam::BeforeTool,
                Seam::BeforeStop,
                Seam::Finish,
            ],
            page: Some(Link::page(PAGE)),
            entries,
            ..Default::default()
        }
    }

    async fn data(&self, host: &Self::Host, _cx: &HostCx) -> Overview {
        host.overview().await
    }

    async fn act(
        &self,
        host: &Self::Host,
        action: Value,
        _cx: &HostCx,
    ) -> anyhow::Result<Option<Value>> {
        let action: Act = serde_json::from_value(action)?;
        match action {
            Act::Allow { plugin } => host.allow(&plugin).await?,
            Act::SettingsView { plugin, settings } => {
                let page = host.settings_page(&plugin, settings).await;
                return Ok(Some(serde_json::to_value(page)?));
            }
        }
        Ok(None)
    }
}

/// A plugin of the repository as a row of the Plugins screen: where it
/// steps in, and a word when it needs the person.
fn entry_row(entry: &crate::Entry) -> tau_ui_plugin::CatalogEntry {
    use tau_ui_plugin::{CatalogEntry, Note};

    let mut seams = Vec::new();
    if let Some(declaration) = &entry.declaration {
        if !declaration.tools.is_empty() {
            seams.push(Seam::Tools);
        }
        if declaration.hooks.before_tool {
            seams.push(Seam::BeforeTool);
        }
        if declaration.hooks.before_stop {
            seams.push(Seam::BeforeStop);
        }
        if declaration.hooks.run_end {
            seams.push(Seam::Finish);
        }
    }
    let note = match &entry.standing {
        Standing::Active => None,
        Standing::Waiting { .. } => {
            Some(Note::new("waits for you", Tone::Warn))
        }
        Standing::Failing => Some(Note::new("tests fail", Tone::Danger)),
        Standing::Broken { .. } => {
            Some(Note::new("does not load", Tone::Danger))
        }
    };
    CatalogEntry {
        name: entry.name.clone(),
        description: entry.description.clone(),
        group: tau_ui_plugin::Group::Yours,
        seams,
        note,
    }
}
