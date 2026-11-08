//! The demo's host half of the plugins: each plugin's real host state,
//! over a store and directories of its own that go away with it. What a
//! plugin's page asks is carried out the way a host would. The Luau
//! plugins tau comes with, which the demo cannot run, draw their pages
//! from their fixtures.

#![allow(
    clippy::disallowed_methods,
    reason = "the demo answers each scripted step at once, from an in-memory store, so its screens are the same every time (ADR 0028)"
)]

use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use serde_json::Value;
use tau_jev::{Jev, fake::FakeJev};
use tau_ui::hosted::{self, Hosted};
use tau_ui_plugin::{
    ConfigDir,
    ErasedPlugin,
    HostCx,
    PluginValue,
    Push,
    RepoCtx,
    SavedSettings,
    Services,
};
use tau_ui_remote::catalog::{Catalog, PluginInfo};
use tokio::{runtime::Runtime, sync::mpsc};

/// What the demo's runs cost each plugin over the last 30 days, as the
/// store would add it up.
const SPEND: [(&str, f64); 7] = [
    (tau_reasoning::NAME, 0.004),
    (tau_memory_host::NAME, 0.212),
    (tau_constitution::NAME, 0.031),
    (tau_fast_compaction::NAME, 0.046),
    (tau_compaction::NAME, 0.061),
    (tau_goal::NAME, 0.009),
    (tau_codemode::PLUGIN, 0.002),
];

/// Jev's scores as the demo scripts them: one per question, in the
/// order they come.
const SCORES: [f64; 6] = [0.94, 0.41, 0.06, 0.12, 0.33, 0.71];

/// The plugins' host halves for the demo.
pub struct DemoHost {
    cx: HostCx,
    hosted: Vec<Hosted>,
    settings: Arc<Mutex<BTreeMap<String, Value>>>,
    /// The repositories' own copies, by repository, then plugin.
    repo_settings: Mutex<BTreeMap<String, BTreeMap<String, Value>>>,
    pushed: Mutex<Option<mpsc::UnboundedReceiver<Push>>>,
    /// Plugins whose entry and data a screen set, whatever their host
    /// half says.
    pinned: Mutex<Vec<(PluginInfo, PluginValue)>>,
    // Dropped last: the plugins' files, and the runtime their work runs
    // on.
    runtime: Runtime,
    _dir: tempfile::TempDir,
}

impl DemoHost {
    /// The host halves for the repositories `catalog` lists, with the
    /// demo's notes and rules in them.
    pub fn new(catalog: &Catalog) -> anyhow::Result<Self> {
        let dir = tempfile::tempdir()?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()?;
        let store = runtime.block_on(tau_ui::hosted::open_store(
            &dir.path().join("runs.db"),
            &tau_ui_plugin::UiState::default(),
        ))?;
        let repos = catalog
            .repos
            .iter()
            .map(|repo| {
                let checkout = dir.path().join("checkouts").join(&repo.name);
                std::fs::create_dir_all(&checkout)?;
                Ok(RepoCtx {
                    id: repo.id.clone(),
                    remote: None,
                    folder: None,
                    name: repo.name.clone(),
                    checkout,
                    dir: dir.path().join("projects").join(&repo.name),
                    workspaces: dir.path().join("projects").join(&repo.name),
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let settings: Arc<Mutex<BTreeMap<String, Value>>> = Arc::default();
        let (read, save) = (settings.clone(), settings.clone());
        // The demo keeps no repository's own copy: one is the plugin's.
        let saved = SavedSettings::new(
            move |plugin, _repo| {
                read.lock().expect("not poisoned").get(plugin).cloned()
            },
            move |plugin, _repo, value| {
                save.lock()
                    .expect("not poisoned")
                    .insert(plugin.to_owned(), value);
                Box::pin(std::future::ready(Ok(())))
            },
        );
        let jev: Arc<dyn Jev> = Arc::new(scripted_jev());
        let skills = dir.path().join("skills");
        tau_skills::demo::seed(&skills)?;
        let services = Services::default()
            .with(tau_memory_host::Search::Keywords)
            .with(saved)
            .with(ConfigDir(dir.path().join("config")))
            .with(tau_skills::SkillsDir(skills))
            .with(tau_jev::Upstream(jev.clone()))
            .with(jev);
        let (pushes, pushed) = mpsc::unbounded_channel();
        let cx = HostCx::new(
            store,
            runtime.handle().clone(),
            services,
            dir.path().to_owned(),
            repos,
            Arc::new(move |push| {
                let _ = pushes.send(push);
            }),
        );
        tau_memory_host::demo::seed(&cx)?;
        let hosted = runtime.block_on(hosted::host_all(&cx));
        Ok(Self {
            cx,
            hosted,
            settings,
            repo_settings: Mutex::default(),
            pushed: Mutex::new(Some(pushed)),
            pinned: Mutex::default(),
            runtime,
            _dir: dir,
        })
    }

    /// Shows `info` and `data` for its plugin from now on, in place of
    /// what its host half has: a screen's plugin in a state the demo's
    /// host never reaches.
    pub fn pin(&self, info: PluginInfo, data: PluginValue) {
        self.pinned.lock().expect("not poisoned").push((info, data));
    }

    /// `catalog` with what the plugins have now: their entries, data and
    /// settings, and each repository's plugin data.
    pub fn catalog(&self, mut catalog: Catalog) -> Catalog {
        let (mut plugins, data, settings) = self.runtime.block_on(
            hosted::catalog(&self.hosted, &self.cx, |plugin| {
                self.settings_of(plugin)
            }),
        );
        for plugin in &mut plugins {
            plugin.spend = SPEND
                .iter()
                .find(|(name, _)| *name == plugin.name)
                .map_or(0.0, |(_, usd)| *usd);
        }
        catalog.plugins = plugins;
        catalog.plugin_data = data;
        // tau's Luau plugins, which the demo cannot run: what they
        // declare, from their fixtures (ADR 0050).
        let (info, data) = luau_built_ins();
        catalog.plugins.push(info);
        catalog
            .plugin_data
            .insert(tau_luau_plugins::NAME.into(), data);
        for (info, data) in self.pinned.lock().expect("not poisoned").iter() {
            catalog.plugins.retain(|plugin| plugin.name != info.name);
            catalog.plugins.push(info.clone());
            catalog.plugin_data.insert(info.name.clone(), data.clone());
        }
        catalog.plugin_settings = settings;
        catalog.repo_plugin_settings = self
            .repo_settings
            .lock()
            .expect("not poisoned")
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
            .collect();
        for repo in &mut catalog.repos {
            if let Some(ctx) = self.cx.repo(&repo.name) {
                repo.plugins = self.runtime.block_on(hosted::repo_data(
                    &self.hosted,
                    ctx,
                    &self.cx,
                ));
            }
            // No server answers in a demo: tau-agent's show as a host
            // that started them would see them.
            if repo.name == "tau-agent" {
                repo.plugins.insert(
                    tau_mcp_host::NAME.to_owned(),
                    PluginValue::typed(tau_mcp::demo::servers()),
                );
                // It has an `.envrc`, as the real one does, and direnv
                // is there.
                repo.plugins.insert(
                    tau_direnv::NAME.to_owned(),
                    PluginValue::typed(tau_direnv::RepoData {
                        envrc: true,
                        direnv: true,
                    }),
                );
            }
        }
        catalog
    }

    /// The repository listed as `name`, as its host halves see it.
    pub fn repo(&self, name: &str) -> Option<&RepoCtx> {
        self.cx.repo(name)
    }

    /// Runs `future` on the demo's runtime: a screen putting in place
    /// what a host half would have, such as an image the canvas kept.
    pub fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(future)
    }

    /// Carries out what `plugin`'s UI asked, as a host would.
    pub fn act(
        &self,
        plugin: &str,
        action: Value,
    ) -> anyhow::Result<Option<Value>> {
        if let Some(page) = luau_page(plugin, &action) {
            return Ok(Some(page));
        }
        self.runtime.block_on(hosted::act(
            &self.hosted,
            plugin,
            action,
            &self.cx,
        ))
    }

    /// Saves `plugin`'s settings everywhere, or as `repo`'s own copy;
    /// no value drops the copy.
    pub fn save_settings(
        &self,
        plugin: &str,
        repo: Option<&str>,
        value: Option<Value>,
    ) {
        match (repo, value) {
            (None, Some(value)) => {
                self.settings
                    .lock()
                    .expect("not poisoned")
                    .insert(plugin.to_owned(), value);
            }
            (Some(repo), Some(value)) => {
                self.repo_settings
                    .lock()
                    .expect("not poisoned")
                    .entry(repo.to_owned())
                    .or_default()
                    .insert(plugin.to_owned(), value);
            }
            (Some(repo), None) => {
                if let Some(plugins) = self
                    .repo_settings
                    .lock()
                    .expect("not poisoned")
                    .get_mut(repo)
                {
                    plugins.remove(plugin);
                }
            }
            (None, None) => {}
        }
    }

    /// What the plugins push as their work finishes, once.
    pub fn pushed(&self) -> Option<mpsc::UnboundedReceiver<Push>> {
        self.pushed.lock().expect("not poisoned").take()
    }

    fn settings_of(&self, plugin: &dyn ErasedPlugin) -> PluginValue {
        self.settings
            .lock()
            .expect("not poisoned")
            .get(plugin.name())
            .cloned()
            .map(PluginValue::from_json)
            .unwrap_or_else(|| plugin.default_settings())
    }
}

/// Jev answering with [`SCORES`], in turn.
fn scripted_jev() -> FakeJev {
    let next = AtomicUsize::new(0);
    FakeJev::nouls(move |_| {
        SCORES[next.fetch_add(1, Ordering::Relaxed) % SCORES.len()]
    })
}

/// tau-luau-plugins' entry, with the Luau plugins tau comes with that the
/// demo draws: each as it declares itself, from its fixture.
fn luau_built_ins() -> (PluginInfo, PluginValue) {
    use tau_luau_plugins::{Entry, Overview, Standing};
    let built_in = |name: &str, declared: &str| Entry {
        name: name.into(),
        description: String::new(),
        standing: Standing::Active,
        declaration: Some(
            serde_json::from_str(declared)
                .expect("a declaration's fixture is one"),
        ),
        tests: Vec::new(),
        keeps_earlier: false,
        built_in: true,
    };
    let overview = Overview {
        commit: None,
        plugins: vec![
            built_in(
                tau_constitution::NAME,
                tau_constitution::DEMO_DECLARATION,
            ),
            built_in(tau_goal::NAME, tau_goal::DEMO_DECLARATION),
            built_in(tau_ask::NAME, tau_ask::DEMO_DECLARATION),
            built_in(
                tau_fast_compaction::NAME,
                tau_fast_compaction::DEMO_DECLARATION,
            ),
            built_in(tau_compaction::NAME, tau_compaction::DEMO_DECLARATION),
            built_in(
                tau_tree_compaction::NAME,
                tau_tree_compaction::DEMO_DECLARATION,
            ),
            built_in(tau_reasoning::NAME, tau_reasoning::DEMO_DECLARATION),
        ],
        error: None,
    };
    let entries = overview
        .plugins
        .iter()
        .map(|entry| tau_ui_plugin::CatalogEntry {
            name: entry.name.clone(),
            description: entry.description.clone(),
            group: tau_ui_plugin::Group::Yours,
            seams: entry
                .declaration
                .as_ref()
                .map(tau_luau_plugins::Declaration::seams)
                .unwrap_or_default(),
            note: None,
        })
        .collect();
    let info = PluginInfo {
        name: tau_luau_plugins::NAME.into(),
        description: "Plugins written in Luau: 3 active".into(),
        group: tau_ui_plugin::Group::Yours,
        page: Some(tau_ui_plugin::Link::page(tau_luau_plugins::ui::PAGE)),
        reach: Default::default(),
        spend: 0.0,
        note: None,
        entries,
        settings: true,
        ui: true,
        rewrites_keep_transcript: false,
    };
    (info, PluginValue::typed(overview))
}

/// tau-constitution's rules page of tau-agent, which the demo cannot draw
/// with Luau: the tree its fixture keeps for the screen its `demo`
/// parameter names (the rules when it names none), as tau-luau-plugins'
/// host half answers a page. `None` for any other action.
fn luau_page(plugin: &str, action: &Value) -> Option<Value> {
    let Ok(tau_luau_plugins::Act::Page {
        plugin: luau,
        params,
        ..
    }) = serde_json::from_value(action.clone())
    else {
        return None;
    };
    if plugin != tau_luau_plugins::NAME || luau != tau_constitution::NAME {
        return None;
    }
    let pages: Value = serde_json::from_str(tau_constitution::DEMO_PAGES)
        .expect("the pages' fixture is JSON");
    let screen = params.get("demo").map_or("constitution", String::as_str);
    let repo = params.get("repo").map_or("", String::as_str);
    let tree = tau_luau_plugins::ui::embed_repo(&pages[screen], &luau, repo);
    Some(serde_json::json!({ "tree": tree.value() }))
}
