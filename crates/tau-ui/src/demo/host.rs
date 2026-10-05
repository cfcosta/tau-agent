//! The demo's host half of the plugins: each plugin's real host state,
//! over a store and directories of its own that go away with it. What a
//! plugin's page asks is carried out the way a host would, so a rule
//! added in the demo is kept by tau-constitution, not by the demo.

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
use tau_store::Store;
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
use tokio::{runtime::Runtime, sync::mpsc};

use crate::{
    catalog::Catalog,
    hosted::{self, Hosted},
};

/// What the demo's runs cost each plugin over the last 30 days, as the
/// store would add it up.
const SPEND: [(&str, f64); 7] = [
    (tau_reasoning::NAME, 0.004),
    (tau_memory::plugin::NAME, 0.212),
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
    pushed: Mutex<Option<mpsc::UnboundedReceiver<Push>>>,
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
        let store =
            runtime.block_on(Store::open(dir.path().join("runs.db")))?;
        let repos = catalog
            .repos
            .iter()
            .map(|repo| {
                let checkout = dir.path().join("checkouts").join(&repo.name);
                std::fs::create_dir_all(&checkout)?;
                Ok(RepoCtx {
                    name: repo.name.clone(),
                    checkout,
                    dir: dir.path().join("projects").join(&repo.name),
                    workspaces: dir.path().join("projects").join(&repo.name),
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let settings: Arc<Mutex<BTreeMap<String, Value>>> = Arc::default();
        let (read, save) = (settings.clone(), settings.clone());
        let saved = SavedSettings::new(
            move |plugin| {
                read.lock().expect("not poisoned").get(plugin).cloned()
            },
            move |plugin, value| {
                save.lock()
                    .expect("not poisoned")
                    .insert(plugin.to_owned(), value);
                Ok(())
            },
        );
        let jev: Arc<dyn Jev> = Arc::new(scripted_jev());
        let skills = dir.path().join("skills");
        tau_skills::demo::seed(&skills)?;
        let services = Services::default()
            .with(tau_memory::ui::Search::Keywords)
            .with(saved)
            .with(ConfigDir(dir.path().join("config")))
            .with(tau_skills::SkillsDir(skills))
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
        tau_memory::demo::seed(&cx)?;
        let hosted = runtime.block_on(hosted::host_all(&cx));
        for act in tau_constitution::demo::acts() {
            runtime.block_on(hosted::act(
                &hosted,
                tau_constitution::NAME,
                serde_json::to_value(act)?,
                &cx,
            ))?;
        }
        Ok(Self {
            cx,
            hosted,
            settings,
            pushed: Mutex::new(Some(pushed)),
            runtime,
            _dir: dir,
        })
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
        catalog.plugin_settings = settings;
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
                    tau_mcp::NAME.to_owned(),
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

    /// Carries out what `plugin`'s UI asked, as a host would.
    pub fn act(
        &self,
        plugin: &str,
        action: Value,
    ) -> anyhow::Result<Option<Value>> {
        self.runtime.block_on(hosted::act(
            &self.hosted,
            plugin,
            action,
            &self.cx,
        ))
    }

    /// Saves `plugin`'s settings.
    pub fn save_settings(&self, plugin: &str, value: Value) {
        self.settings
            .lock()
            .expect("not poisoned")
            .insert(plugin.to_owned(), value);
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
