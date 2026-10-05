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
use tau_ui_plugin::{HostCx, HostHalf, Link, PluginInfo, RunCtx, Seam};

/// tau-luau-plugins on the host.
#[derive(Debug, Clone, Copy, Default)]
pub struct LuauPluginsHost;

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
        // A run in the plugins repository tests the plugins it writes.
        if run.repo.name == crate::REPO
            && let Some(dir) = run.services.get::<tau_ui_plugin::WorkspaceDir>()
        {
            plugins.push(Box::new(crate::testing::PluginTesting::new(
                dir.0.clone(),
            )));
        }
        // Each plugin takes what the person set, over its defaults.
        let active: Vec<crate::agent::Active> = host
            .active()
            .await
            .into_iter()
            .map(|mut active| {
                let name = &active.loaded.declaration.name;
                active.settings = crate::settings::effective(
                    &active.loaded.declaration,
                    settings.plugins.get(name),
                )
                .0;
                active
            })
            .collect();
        if active.is_empty() {
            return Ok(plugins);
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
        plugins
            .push(Box::new(crate::agent::LuauPlugins::new(active, info, jev)));
        Ok(plugins)
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
