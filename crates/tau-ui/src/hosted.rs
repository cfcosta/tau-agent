//! The host's half of the plugins (ADR 0017): each one's state on a
//! host, and what the host asks of it. The host and the demo both hold
//! them.

use std::{collections::BTreeMap, sync::Arc};

use serde_json::Value;
use tau_ui_plugin::{
    ErasedPlugin,
    HostCx,
    PluginValue,
    RepoCtx,
    registry::HostState,
};

use tau_ui_remote::{catalog::PluginInfo, plugins::registry};

/// A plugin with its UI, and its state on this host.
#[derive(Clone)]
pub struct Hosted {
    pub plugin: Arc<dyn ErasedPlugin>,
    pub state: Arc<HostState>,
}

/// Each plugin's host state; a plugin whose state cannot be made is left
/// out, and says why.
pub fn host_all(cx: &HostCx) -> Vec<Hosted> {
    registry()
        .plugins()
        .filter_map(|plugin| match plugin.host(cx) {
            Ok(state) => Some(Hosted {
                plugin: plugin.clone(),
                state: Arc::new(state),
            }),
            Err(error) => {
                eprintln!("tau-ui: {} is off: {error:#}", plugin.name());
                None
            }
        })
        .collect()
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
pub fn catalog(
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
        let info = hosted.plugin.catalog(&hosted.state, cx, &settings);
        plugins.push(PluginInfo {
            name: info.name,
            description: info.description,
            seams: info.seams,
            spend: info.spend,
            page: info.page,
        });
        data.insert(name.clone(), hosted.plugin.data(&hosted.state, cx));
        saved.insert(name, settings);
    }
    (plugins, data, saved)
}

/// Each plugin's data for the repository `repo`.
pub fn repo_data(
    hosted: &[Hosted],
    repo: &RepoCtx,
    cx: &HostCx,
) -> BTreeMap<String, PluginValue> {
    hosted
        .iter()
        .map(|hosted| {
            (
                hosted.plugin.name().to_owned(),
                hosted.plugin.repo_data(&hosted.state, repo, cx),
            )
        })
        .collect()
}

/// Carries out what `plugin`'s UI asked; its answer, if any, goes back
/// to the UI.
pub fn act(
    hosted: &[Hosted],
    plugin: &str,
    action: Value,
    cx: &HostCx,
) -> anyhow::Result<Option<Value>> {
    let hosted = hosted
        .iter()
        .find(|hosted| hosted.plugin.name() == plugin)
        .ok_or_else(|| anyhow::anyhow!("No plugin {plugin} here"))?;
    hosted.plugin.act(&hosted.state, action, cx)
}
