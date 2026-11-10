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
    Services,
    registry::HostState,
    slots::Fills,
};
use tau_ui_remote::{
    catalog::PluginInfo,
    plugins::{self, registry},
};

/// The plugins tau-plugins lists, each given the host half tau-ui has
/// for it: every one but the Luau ones (tau-codemode, tau-luau-plugins),
/// which only the `tau` app links, so tau-ui builds while Luau's C++
/// does (ADR 0030).
pub fn halves(plugins: Registry) -> Registry {
    plugins
        .host(tau_tools_host::ToolsHost)
        .host(tau_vcs_host::VcsHost)
        .host(tau_github_host::GitHubHost)
        .host(tau_memory_host::MemoryHost)
        .host(tau_canvas::CanvasHost)
        .host(tau_direnv::DirenvHost)
        .host(tau_mcp_host::McpHost)
        .host(tau_skills::SkillsHost)
        .host(tau_notify::NotifyHost)
        .host(tau_phones::PhonesHost)
        .host(tau_jev::JevHost)
        .host(tau_sqlite::SqliteHost)
        .host(tau_chatgpt::ChatGptHost)
}

/// What the app gives its plugins beside the host's own services
/// (`HostConfig::services`): the person's identity for commits, under
/// their jj config; their skills in `home`'s `~/.agents/skills`.
pub fn app_services(home: Option<&std::path::Path>) -> tau_ui_plugin::Services {
    use tau_ui_plugin::services::names;
    let services = tau_ui_plugin::Services::default()
        .offer(names::IDENTITY, tau_vcs_host::Identity::from_environment());
    match home {
        Some(home) => services.offer(
            tau_skills::DIR_SERVICE,
            tau_skills::SkillsDir(home.join(tau_skills::SKILLS_DIR)),
        ),
        None => services,
    }
}

/// Gives the plugins the host halves tau-ui has ([`halves`]), unless the
/// process installed its own: what a host does before it makes any
/// plugin's host state. The `tau` app installs every half first.
pub fn install() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        if !plugins::installed() {
            plugins::install(halves(tau_plugins::plugins()));
        }
    });
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

/// What each plugin can fill of the slots (`HostHalf::fill`), with each
/// slot's declared order: the same in whatever order `hosted` is.
pub fn fills(hosted: &[Hosted]) -> Fills {
    let mut fills = registry().fills();
    for hosted in hosted {
        hosted.plugin.fill(&hosted.state, &mut fills);
    }
    fills
}

/// The kinds of tau's own repository the plugins keep, each with the
/// plugin that keeps it (`UiPlugin::repo_kinds`).
pub fn repo_kinds() -> Vec<(&'static str, tau_ui_plugin::kinds::KindHost)> {
    install();
    registry()
        .plugins()
        .flat_map(|plugin| {
            let name = plugin.name();
            plugin
                .repo_kinds()
                .into_iter()
                .map(move |kind| (name, kind))
        })
        .collect()
}

/// What each plugin can fill of the slots needed before any host state
/// is made (`HostHalf::fill_early`), with each slot's declared order and
/// the person's picks in `ui`.
pub fn early_fills(ui: &tau_ui_plugin::UiState) -> Fills {
    let mut fills = registry().fills();
    for plugin in registry().plugins() {
        plugin.fill_early(&mut fills);
    }
    fills.picks_from(ui);
    fills
}

/// Opens the store at `path` with what fills `tau.store`
/// (`tau_ui_plugin::store::STORE`), picked as `ui` says.
pub async fn open_store(
    path: &std::path::Path,
    ui: &tau_ui_plugin::UiState,
) -> anyhow::Result<tau_store::Store> {
    install();
    let opener = early_fills(ui)
        .get(&tau_ui_plugin::store::STORE)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "No plugin fills {}: tau has nowhere to keep its runs",
                tau_ui_plugin::store::STORE.name()
            )
        })?;
    opener.open(path).await
}

/// `services` and those each plugin provides now
/// (`HostHalf::provide`), in the registry's order.
pub fn provided(hosted: &[Hosted], services: Services) -> Services {
    hosted.iter().fold(services, |services, hosted| {
        hosted.plugin.provide(&hosted.state, services)
    })
}

/// Asks each plugin to start again the jobs a restart cut off, in the
/// registry's order (`HostHalf::resume`).
pub async fn resume(hosted: &[Hosted], cx: &HostCx) {
    for hosted in hosted {
        hosted.plugin.resume(&hosted.state, cx).await;
    }
}

/// Tells each plugin that first-run setup ended with `repos` picked, in
/// the registry's order (`HostHalf::setup_finished`).
pub async fn setup_finished(
    hosted: &[Hosted],
    repos: &[tau_ui_plugin::points::SetupRepo],
    cx: &HostCx,
) {
    for hosted in hosted {
        hosted.plugin.setup_finished(&hosted.state, repos, cx).await;
    }
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
            reach: info.reach,
            spend: info.spend,
            page: info.page,
            group: info.group,
            note: info.note,
            entries: info.entries,
            settings: info.settings,
            ui: true,
            rewrites_keep_transcript: info.rewrites_keep_transcript,
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

/// The model and effort `repo`'s conversations go to by default, as the
/// first plugin that sets them says.
pub async fn conversation_model(
    hosted: &[Hosted],
    repo: &RepoCtx,
) -> Option<tau_ui_plugin::ModelPick> {
    for hosted in hosted {
        if let Some(pick) =
            hosted.plugin.conversation_model(&hosted.state, repo).await
        {
            return Some(pick);
        }
    }
    None
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
