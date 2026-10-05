//! The host's half of the plugins (ADR 0017): each one's state on a
//! host, and what the host asks of it. The host and the demo both hold
//! them.

use std::{
    collections::BTreeMap,
    sync::{Arc, Once},
};

use serde_json::Value;
use tau_ui_plugin::{
    ErasedPlugin,
    HostCx,
    PluginValue,
    Registry,
    RepoCtx,
    registry::HostState,
};
use tau_ui_remote::{
    catalog::PluginInfo,
    plugins::{self, registry},
};

/// The plugins tau-ui-remote lists, each given its host half (ADR 0030).
pub fn halves(plugins: Registry) -> Registry {
    plugins
        .host(tau_tools_host::ToolsHost)
        .host(tau_vcs_host::VcsHost)
        .host(tau_reasoning::ReasoningHost)
        .host(tau_fast_compaction::ui::FastCompactionHost)
        .host(tau_compaction::ui::CompactionHost)
        .host(tau_memory_host::MemoryHost)
        .host(tau_constitution_host::ConstitutionHost)
        .host(tau_goal::GoalHost)
        .host(tau_luau_plugins_host::LuauPluginsHost)
        .host(tau_ask::AskHost)
        .host(tau_direnv::DirenvHost)
        .host(tau_mcp_host::McpHost)
        .host(tau_skills::SkillsHost)
        .host(tau_codemode_host::CodemodeHost)
}

/// Gives the plugins their host halves, once: what a host does before
/// anything reads them (`tau_ui_remote::plugins::install`).
pub fn install() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| plugins::install(halves(plugins::plugins())));
}

/// A plugin with its UI, and its state on this host.
#[derive(Clone)]
pub struct Hosted {
    pub plugin: Arc<dyn ErasedPlugin>,
    pub state: Arc<HostState>,
}

/// Each plugin's host state; a plugin whose state cannot be made is left
/// out, and says why.
pub async fn host_all(cx: &HostCx) -> Vec<Hosted> {
    install();
    let mut hosted = Vec::new();
    for plugin in registry().plugins() {
        match plugin.host(cx).await {
            Ok(state) => hosted.push(Hosted {
                plugin: plugin.clone(),
                state: Arc::new(state),
            }),
            Err(error) => {
                eprintln!("tau-ui: {} is off: {error:#}", plugin.name());
            }
        }
    }
    hosted
}

/// What the catalog lists of the plugins: each one's entry, by name its
/// data, and by name its settings.
pub type Catalogued = (
    Vec<PluginInfo>,
    BTreeMap<String, PluginValue>,
    BTreeMap<String, PluginValue>,
);

/// Each plugin's catalog entry, its data, and its settings, which
/// `settings` reads.
pub async fn catalog(
    hosted: &[Hosted],
    cx: &HostCx,
    settings: impl Fn(&dyn ErasedPlugin) -> PluginValue,
) -> Catalogued {
    let mut plugins = Vec::new();
    let mut data = BTreeMap::new();
    let mut saved = BTreeMap::new();
    for hosted in hosted {
        let name = hosted.plugin.name().to_owned();
        let settings = settings(hosted.plugin.as_ref());
        let info = hosted.plugin.catalog(&hosted.state, cx, &settings).await;
        plugins.push(PluginInfo {
            name: info.name,
            description: info.description,
            seams: info.seams,
            spend: info.spend,
            page: info.page,
            group: info.group,
            note: info.note,
            entries: info.entries,
            settings: info.settings,
        });
        data.insert(name.clone(), hosted.plugin.data(&hosted.state, cx).await);
        saved.insert(name, settings);
    }
    (plugins, data, saved)
}

/// Each plugin's data for the repository `repo`.
pub async fn repo_data(
    hosted: &[Hosted],
    repo: &RepoCtx,
    cx: &HostCx,
) -> BTreeMap<String, PluginValue> {
    let mut data = BTreeMap::new();
    for hosted in hosted {
        data.insert(
            hosted.plugin.name().to_owned(),
            hosted.plugin.repo_data(&hosted.state, repo, cx).await,
        );
    }
    data
}

/// Carries out what `plugin`'s UI asked; its answer, if any, goes back
/// to the UI.
pub async fn act(
    hosted: &[Hosted],
    plugin: &str,
    action: Value,
    cx: &HostCx,
) -> anyhow::Result<Option<Value>> {
    let hosted = hosted
        .iter()
        .find(|hosted| hosted.plugin.name() == plugin)
        .ok_or_else(|| anyhow::anyhow!("No plugin {plugin} here"))?;
    hosted.plugin.act(&hosted.state, action, cx).await
}
