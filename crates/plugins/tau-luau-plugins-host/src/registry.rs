//! The plugins the host has active, read from the plugins repository's
//! trunk (ADR 0027).
//!
//! The registry watches trunk. When it moves, each plugin's folder at
//! the new commit is loaded and tested, and activates by itself unless
//! its tests fail or it reaches further than the version the person
//! allowed; then the version before stays active, and the Plugins
//! screen says why. A run takes the plugins active before each of its
//! model requests, so a change reaches runs already going.
//!
//! A run in the plugins repository has its own workspace's versions
//! too ([`Registry::in_workspace`]): a plugin it wrote or changed works
//! for it as soon as that version's tests pass, before it lands, and for
//! every other run once it lands on trunk.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use serde::{Deserialize, Serialize};
pub use tau_luau_plugins::{
    REPO,
    repository::{ROOT, first_files},
};
use tau_ui_plugin::{HostCx, PluginHost};
use tau_vcs_host::{Identity, Project};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::{
    Declaration,
    Entry,
    Overview,
    Standing,
    agent::Active,
    runtime::{Files, Loaded, load},
};

/// How often trunk is read for a change.
const POLL: Duration = Duration::from_secs(2);

/// The version of a plugin the person allowed: what it declared.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Allowed {
    digest: String,
    declaration: Declaration,
}

/// The host's Luau plugins. Clones share them.
#[derive(Clone)]
pub struct Registry(Arc<Shared>);

struct Shared {
    root: PathBuf,
    /// What the repository is opened with.
    identity: Identity,
    /// Where what the person allowed is kept.
    allowed_path: PathBuf,
    /// Held while trunk's plugins are read again or one is allowed: each
    /// reads what was allowed and writes it, so one never loses the
    /// other's.
    allowing: tokio::sync::Mutex<()>,
    state: RwLock<State>,
    /// Versions read from runs' workspaces, by their files' digest: how
    /// loading and testing them went, which their files alone decide.
    /// Each is loaded and tested once; what it may reach is weighed
    /// against what is allowed as it is each time.
    tried: tokio::sync::Mutex<BTreeMap<String, Tried>>,
    /// Asks the interface to draw the catalog again.
    refresh: Box<dyn Fn() + Send + Sync>,
    /// The person's settings, as they are now.
    settings: Box<dyn Fn() -> tau_luau_plugins::LuauSettings + Send + Sync>,
    /// The Luau plugins tau comes with, loaded.
    built_in: Vec<Loaded>,
    /// tau's configuration directory, where each plugin's secrets are.
    config: Option<PathBuf>,
    /// The host's runtime, for hooks called from outside it.
    runtime: Option<tokio::runtime::Handle>,
    /// The person's model, for `infer` in hooks outside any run.
    model: Option<tau_ui_plugin::Model>,
    /// Each plugin's store in each repository.
    stores: Arc<crate::store::Stores>,
    /// Where each plugin's blobs are kept (`ctx.blobs`).
    blobs: PathBuf,
    /// The person's skills folder, as the host has it (`uses.files`'
    /// `skills`, ADR 0050 gap K).
    skills: Option<PathBuf>,
    /// tau's directory for each repository, by name, as the host last
    /// listed them (`uses.files`' `repository`, ADR 0050, gap L).
    repositories: std::sync::RwLock<Vec<(String, PathBuf)>>,
    /// The repositories tau lists, as it last said them.
    listed: std::sync::RwLock<Vec<tau_ui_plugin::RepoCtx>>,
    /// What runs' tools ask the person to allow, waiting for an answer.
    allows: crate::chats::Allows,
    /// The forms runs' tools ask the person, waiting for the answers.
    forms: crate::forms::Forms,
    /// The host's chats, which what the person allows starts or steers.
    chats: Option<Arc<dyn tau_ui_plugin::Conversations>>,
}

/// How loading and testing a workspace's version went.
#[derive(Clone)]
enum Tried {
    /// It does not load, or its tests fail.
    Broken,
    Passed(Box<Loaded>),
}

/// A version of trunk's that is not active.
#[derive(Clone)]
enum Held {
    /// It reaches further than the person allowed: they may allow it.
    Waiting(Loaded),
    /// Its tests fail: nothing allows it.
    Failing(Loaded),
}

impl Held {
    fn loaded(&self) -> &Loaded {
        match self {
            Self::Waiting(loaded) | Self::Failing(loaded) => loaded,
        }
    }
}

#[derive(Default)]
struct State {
    overview: Overview,
    active: Vec<Active>,
    /// Versions not active, by plugin: waiting for the person, or
    /// failing their tests.
    waiting: BTreeMap<String, Held>,
    /// Each plugin's files on trunk, by their digest, whatever its
    /// standing: what a workspace's plugins are told apart from.
    trunk: BTreeMap<String, String>,
    /// The Luau plugins tau comes with, loaded: always active.
    built_in: Vec<(Entry, Active)>,
    /// What each active plugin's `app` kept, by plugin, and when it was
    /// read (ADR 0050, gap K).
    app: Option<(
        std::time::Instant,
        BTreeMap<String, tau_luau_plugins::AppView>,
    )>,
}

/// How long what `app` kept is drawn before it is read again.
const APP_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

/// One of a plugin's actions to carry out ([`Registry::action`]): on the
/// `state` the plugin has in `run`, which works in `repo`, under the
/// person's `settings` for it.
#[derive(Clone)]
pub struct ActionCall<'a> {
    pub plugin: &'a str,
    pub action: &'a str,
    pub args: &'a serde_json::Value,
    pub run: &'a str,
    pub repo: Option<&'a str>,
    pub state: &'a serde_json::Value,
    pub settings: Option<&'a serde_json::Value>,
    /// What it asks of the chats goes to.
    pub chats: Option<&'a dyn tau_ui_plugin::Conversations>,
    /// The host's Jev, for a plugin that uses it.
    pub jev: Option<Arc<dyn tau_jev::Jev>>,
    /// Whether the run is going on, and a Jev key saved, as the
    /// interface that asked sees them: `ctx.run.live` and `ctx.run.jev`.
    pub live: bool,
    pub keyed: bool,
}

impl PluginHost for Registry {
    async fn new(cx: &HostCx) -> anyhow::Result<Self> {
        // The skill runs read to write a plugin, as this build has it.
        let skills = cx.dir.join(tau_ui_plugin::SHIPPED_SKILLS);
        tokio::task::spawn_blocking(move || crate::skill::install(&skills))
            .await??;
        let refresher = cx.clone();
        let reader = cx.clone();
        let registry = Self::at(
            cx.dir.join(ROOT),
            cx.plugin_dir(crate::NAME).join("allowed.json"),
            cx.services.get::<Identity>().cloned().unwrap_or_default(),
            move || refresher.refresh(),
        )
        .with_settings(move || reader.settings(crate::NAME))
        .with_built_in(
            crate::built_in::loaded()
                .await
                .map_err(anyhow::Error::msg)?,
        )
        .with_chats(cx.conversations());
        let registry = match cx.config_dir() {
            Some(config) => registry.with_config(config.to_owned()),
            None => registry,
        }
        .with_runtime(cx.runtime.clone());
        let registry = match cx.services.get::<tau_ui_plugin::Model>() {
            Some(model) => registry.with_model(model.clone()),
            None => registry,
        };
        // What draws from a store draws again once a hook wrote it.
        let changer = cx.clone();
        let registry = registry.with_stores(Arc::new(
            crate::store::Stores::at(cx.plugin_dir(crate::NAME).join("stores"))
                .with_changed(Arc::new(move || changer.refresh())),
        ));
        let registry =
            registry.with_blobs(cx.plugin_dir(crate::NAME).join("blobs"));
        let registry = registry.with_skills(
            cx.services
                .get::<tau_ui_plugin::SkillsDir>()
                .map(|dir| dir.0.clone()),
        );
        registry.load_built_in().await;
        let watcher = registry.clone();
        cx.runtime.spawn(async move { watcher.watch().await });
        Ok(registry)
    }
}

impl Registry {
    /// A registry of the plugins repository at `root`, opened as
    /// `identity`, keeping what the person allowed at `allowed`;
    /// `refresh` redraws the catalog. Empty until it reads trunk.
    pub fn at(
        root: PathBuf,
        allowed: PathBuf,
        identity: Identity,
        refresh: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self(Arc::new(Shared {
            root,
            identity,
            allowed_path: allowed,
            allowing: tokio::sync::Mutex::new(()),
            state: RwLock::default(),
            tried: tokio::sync::Mutex::default(),
            built_in: Vec::new(),
            refresh: Box::new(refresh),
            settings: Box::new(Default::default),
            config: None,
            runtime: None,
            model: None,
            stores: Arc::new(crate::store::Stores::memory()),
            blobs: std::env::temp_dir().join("tau-luau-blobs"),
            skills: None,
            repositories: Default::default(),
            listed: Default::default(),
            allows: crate::chats::Allows::default(),
            forms: crate::forms::Forms::default(),
            chats: None,
        }))
    }

    /// The registry, starting and steering the host's `chats`.
    pub fn with_chats(
        self,
        chats: Option<Arc<dyn tau_ui_plugin::Conversations>>,
    ) -> Self {
        let shared = Arc::into_inner(self.0)
            .expect("the chats are given before the registry is shared");
        Self(Arc::new(Shared { chats, ..shared }))
    }

    /// Takes tau's directory for each of `repos`, by name: what hooks
    /// outside a run reach of them (`ctx.files.repository`), and a repo's
    /// hooks as its `repository`.
    pub fn knows_repositories(&self, repos: &[tau_ui_plugin::RepoCtx]) {
        let mut known = self.0.repositories.write().expect("not poisoned");
        for repo in repos {
            match known.iter_mut().find(|(name, _)| name == &repo.name) {
                Some(found) => found.1 = repo.dir.clone(),
                None => known.push((repo.name.clone(), repo.dir.clone())),
            }
        }
    }

    /// Takes `repos` as the repositories tau lists, which
    /// `ctx.files.repositories` gives, and tau's directory for each.
    pub fn lists_repositories(&self, repos: &[tau_ui_plugin::RepoCtx]) {
        self.knows_repositories(repos);
        *self.0.listed.write().expect("not poisoned") = repos.to_vec();
    }

    /// What `ctx.files` reaches of `declaration`'s folders in a hook
    /// outside a run, in the repository `repo` when it is in one: none
    /// when `uses.files` names none.
    pub fn files_for(
        &self,
        declaration: &tau_luau_plugins::Declaration,
        repo: Option<&str>,
    ) -> Option<crate::files::Files> {
        if declaration.uses.files.is_empty() {
            return None;
        }
        let known = self.0.repositories.read().expect("not poisoned").clone();
        let repository = repo.and_then(|repo| {
            known
                .iter()
                .find(|(name, _)| name == repo)
                .map(|(_, dir)| dir.clone())
        });
        Some(
            crate::files::Files::new(&declaration.uses.files, None, None)
                .writing(&declaration.uses.writes)
                .with_package(crate::runtime::package_dir(&declaration.name))
                .with_skills(self.0.skills.clone())
                .with_repository(repository)
                .with_repositories(known)
                .with_listed(&self.0.listed.read().expect("not poisoned")),
        )
    }

    /// `reach`, with `ctx.files` too when `declaration`'s `uses.files`
    /// names folders, in the repository `repo` when the hook is in one.
    pub fn with_files(
        &self,
        declaration: &tau_luau_plugins::Declaration,
        repo: Option<&str>,
        reach: Arc<dyn crate::runtime::Reach>,
    ) -> Arc<dyn crate::runtime::Reach> {
        match self.files_for(declaration, repo) {
            Some(files) => Arc::new(crate::runtime::WithFiles {
                reach,
                files: Arc::new(files),
            }),
            None => reach,
        }
    }

    /// The host's chats, when it has them.
    pub fn chats(&self) -> Option<Arc<dyn tau_ui_plugin::Conversations>> {
        self.0.chats.clone()
    }

    /// The registry, keeping each plugin's stores in `stores`.
    pub fn with_stores(self, stores: Arc<crate::store::Stores>) -> Self {
        let shared = Arc::into_inner(self.0)
            .expect("the stores are given before the registry is shared");
        Self(Arc::new(Shared { stores, ..shared }))
    }

    /// Each plugin's store in each repository.
    pub fn stores(&self) -> Arc<crate::store::Stores> {
        self.0.stores.clone()
    }

    /// The registry, keeping each plugin's blobs under `dir`
    /// (`ctx.blobs`).
    pub fn with_skills(self, dir: Option<PathBuf>) -> Self {
        let shared = Arc::into_inner(self.0)
            .expect("the skills folder is given before the registry is shared");
        Self(Arc::new(Shared {
            skills: dir,
            ..shared
        }))
    }

    pub fn with_blobs(self, dir: PathBuf) -> Self {
        let shared = Arc::into_inner(self.0)
            .expect("the blobs are given before the registry is shared");
        Self(Arc::new(Shared {
            blobs: dir,
            ..shared
        }))
    }

    /// Where each plugin's blobs are, by plugin and repository.
    pub fn blobs(&self) -> PathBuf {
        self.0.blobs.clone()
    }

    /// What runs' tools ask the person to allow, waiting for an answer.
    pub fn allows(&self) -> crate::chats::Allows {
        self.0.allows.clone()
    }

    /// The forms runs' tools ask the person, waiting for the answers.
    pub fn forms(&self) -> crate::forms::Forms {
        self.0.forms.clone()
    }

    /// `plugin`'s store in `repo`, when its declaration uses one.
    pub async fn store_of(
        &self,
        declaration: &crate::Declaration,
        repo: Option<&str>,
    ) -> Option<crate::runtime::StoreRef> {
        if !declaration.uses.store {
            return None;
        }
        let repo = repo.unwrap_or(crate::store::APP);
        let store = self.0.stores.of(&declaration.name, repo).await.ok()?;
        Some(crate::runtime::StoreRef(store))
    }

    /// The registry, asking `model` for `infer` in hooks outside any run.
    pub fn with_model(self, model: tau_ui_plugin::Model) -> Self {
        let shared = Arc::into_inner(self.0)
            .expect("the model is given before the registry is shared");
        Self(Arc::new(Shared {
            model: Some(model),
            ..shared
        }))
    }

    /// The person's model as the Luau plugin `plugin` asks it outside
    /// any run, booked to it.
    pub fn model_of(&self, plugin: &str) -> Option<tau_ui_plugin::PluginModel> {
        let model = self.0.model.as_ref()?;
        model.signed_in().then(|| model.of(plugin))
    }

    /// The registry, calling hooks asked from outside the host's runtime
    /// on `runtime`.
    pub fn with_runtime(self, runtime: tokio::runtime::Handle) -> Self {
        let shared = Arc::into_inner(self.0)
            .expect("the runtime is given before the registry is shared");
        Self(Arc::new(Shared {
            runtime: Some(runtime),
            ..shared
        }))
    }

    /// The host's runtime, once given, else the one this runs on.
    pub fn runtime(&self) -> Option<tokio::runtime::Handle> {
        self.0
            .runtime
            .clone()
            .or_else(|| tokio::runtime::Handle::try_current().ok())
    }

    /// The registry, with the Luau plugins tau comes with, `built_in`:
    /// active once [`Self::load_built_in`] took them.
    pub fn with_built_in(self, built_in: Vec<Loaded>) -> Self {
        let shared = Arc::into_inner(self.0)
            .expect("built-in plugins are given before the registry is shared");
        Self(Arc::new(Shared { built_in, ..shared }))
    }

    /// Makes the Luau plugins tau comes with active.
    pub async fn load_built_in(&self) {
        let loaded = self
            .0
            .built_in
            .iter()
            .map(|plugin| {
                let declaration = plugin.declaration.clone();
                let settings = declaration.default_settings();
                let entry = Entry {
                    name: declaration.name.clone(),
                    description: declaration.description.clone(),
                    version: plugin.files.manifest.version.clone(),
                    standing: Standing::Active,
                    declaration: Some(declaration),
                    tests: Vec::new(),
                    keeps_earlier: false,
                    built_in: true,
                    app: None,
                };
                (
                    entry,
                    Active {
                        loaded: plugin.clone(),
                        settings,
                    },
                )
            })
            .collect();
        let mut state = self.0.state.write().await;
        state.built_in = loaded;
        merge_built_in(&mut state);
        drop(state);
        (self.0.refresh)();
    }

    /// The registry, keeping each plugin's secrets in tau's configuration
    /// directory `config`.
    pub fn with_config(self, config: PathBuf) -> Self {
        let shared = Arc::into_inner(self.0)
            .expect("the directory is given before the registry is shared");
        Self(Arc::new(Shared {
            config: Some(config),
            ..shared
        }))
    }

    /// Where `plugin`'s secrets are kept, when the host keeps any.
    pub fn secrets_of(&self, plugin: &str) -> Option<PathBuf> {
        let config = self.0.config.as_deref()?;
        Some(crate::runtime::secrets_of(config, plugin))
    }

    /// Keeps `value` as `plugin`'s secret `name`, one its active version
    /// declares.
    pub async fn set_secret(
        &self,
        plugin: &str,
        name: &str,
        value: &str,
    ) -> anyhow::Result<()> {
        let declared = self.active().await.iter().any(|active| {
            active.loaded.declaration.name == plugin
                && active
                    .loaded
                    .declaration
                    .uses
                    .secrets
                    .iter()
                    .any(|s| s == name)
        });
        anyhow::ensure!(declared, "{plugin} does not use the secret `{name}`");
        let path = self
            .secrets_of(plugin)
            .ok_or_else(|| anyhow::anyhow!("this host keeps no secrets"))?;
        tau_tools_host::secrets::SecretStore::new(path)
            .set(name, value)
            .await?;
        // What `app` keeps may say who is signed in: read it again.
        self.0.state.write().await.app = None;
        (self.0.refresh)();
        Ok(())
    }

    /// The registry, reading the person's settings with `settings`.
    pub fn with_settings(
        self,
        settings: impl Fn() -> tau_luau_plugins::LuauSettings
        + Send
        + Sync
        + 'static,
    ) -> Self {
        let shared = Arc::into_inner(self.0)
            .expect("settings are given before the registry is shared");
        Self(Arc::new(Shared {
            settings: Box::new(settings),
            ..shared
        }))
    }

    /// The active plugin that fills `slot`, with the person's settings
    /// for it: the first by name.
    pub async fn filler(&self, slot: &str) -> Option<Active> {
        let active = self.active().await;
        self.filler_of(slot, active)
    }

    /// [`Self::filler`] without waiting: none while trunk's plugins are
    /// being read again.
    pub fn filler_now(&self, slot: &str) -> Option<Active> {
        let active = self.0.state.try_read().ok()?.active.clone();
        self.filler_of(slot, active)
    }

    /// A hook's context in the repository `repo`, outside any run: the
    /// person's settings for `active`, and its store there.
    async fn repo_context(
        &self,
        active: &Active,
        repo: &str,
    ) -> crate::runtime::Context {
        crate::runtime::Context {
            run: serde_json::json!({ "repo": repo }),
            now: crate::agent::now(),
            settings: active.settings.clone(),
            state: serde_json::json!({}),
            secrets: self.secrets_of(&active.loaded.declaration.name),
            store: self.store_of(&active.loaded.declaration, Some(repo)).await,
            job: None,
        }
    }

    /// The overview, with what each active plugin with an `app` keeps of
    /// the whole app, the commands it lists and the page it draws of it
    /// (ADR 0050, gap K): read again at most once a second.
    pub async fn app_overview(&self) -> Overview {
        let mut overview = self.overview().await;
        let views = self.app_views().await;
        for entry in &mut overview.plugins {
            entry.app = views.get(&entry.name).cloned();
        }
        overview
    }

    async fn app_views(&self) -> BTreeMap<String, tau_luau_plugins::AppView> {
        if let Some((at, views)) = &self.0.state.read().await.app
            && at.elapsed() < APP_EVERY
        {
            return views.clone();
        }
        let mut views = BTreeMap::new();
        for active in self.active().await {
            if !active.loaded.declaration.hooks.app {
                continue;
            }
            let name = active.loaded.declaration.name.clone();
            let Some(active) = self.plugin(&name).await else {
                continue;
            };
            views.insert(name, self.app_view(&active).await);
        }
        self.0.state.write().await.app =
            Some((std::time::Instant::now(), views.clone()));
        views
    }

    /// What `active`'s `app` keeps, with its tools' folders read, and what
    /// it lists and draws of it.
    async fn app_view(&self, active: &Active) -> tau_luau_plugins::AppView {
        let declaration = &active.loaded.declaration;
        let files = self.files_for(declaration, None).unwrap_or_else(|| {
            crate::files::Files::new(&declaration.uses.files, None, None)
        });
        let context = self.app_context(active).await;
        app_view(active, files, &context).await
    }

    /// What an `app` hook of `active`'s runs in: no run, no repository.
    async fn app_context(&self, active: &Active) -> crate::runtime::Context {
        let declaration = &active.loaded.declaration;
        crate::runtime::Context {
            run: serde_json::json!({ "kind": "app" }),
            now: crate::agent::now(),
            settings: active.settings.clone(),
            state: serde_json::json!({}),
            secrets: self.secrets_of(&declaration.name),
            store: self.store_of(declaration, None).await,
            job: None,
        }
    }

    /// Carries out `plugin`'s `actions[action]` with `args` from its page of
    /// the app (ADR 0050, gap K), following what it asks of the interface;
    /// the page is drawn again after.
    pub async fn app_action(
        &self,
        plugin: &str,
        action: &str,
        args: serde_json::Value,
        chats: Option<&dyn tau_ui_plugin::Conversations>,
        jev: Option<Arc<dyn tau_jev::Jev>>,
    ) -> anyhow::Result<serde_json::Value> {
        let hook = |action: String, args: serde_json::Value| {
            let jev = jev.clone();
            async move {
                let active = self
                    .plugin(plugin)
                    .await
                    .ok_or_else(|| format!("{plugin} is not active"))?;
                let context = self.app_context(&active).await;
                let outcome = active
                    .loaded
                    .call(
                        &crate::runtime::Hook::Action(action.clone()),
                        args,
                        &context,
                        self.with_files(
                            &active.loaded.declaration,
                            None,
                            Arc::new(crate::runtime::ActionReach(jev)),
                        ),
                        CancellationToken::new(),
                    )
                    .await;
                match outcome.error {
                    Some(error) => Err(format!("{plugin}'s {action}: {error}")),
                    None => Ok(outcome.value),
                }
            }
        };
        let value = hook(action.to_owned(), args)
            .await
            .map_err(anyhow::Error::msg)?;
        let followed = crate::chats::follow(value, chats, None, hook)
            .await
            .map_err(|error| anyhow::anyhow!("{plugin}'s {action}: {error}"));
        self.0.state.write().await.app = None;
        followed
    }

    /// What each active plugin keeps of `repo` (`repo_data`), by plugin.
    /// One whose hook fails gives nothing, and says why.
    pub async fn repo_data(&self, repo: &str) -> tau_luau_plugins::RepoValues {
        let mut values = BTreeMap::new();
        for active in self.active().await {
            if !active.loaded.declaration.hooks.repo_data {
                continue;
            }
            let name = active.loaded.declaration.name.clone();
            let Some(active) = self.plugin(&name).await else {
                continue;
            };
            let context = self.repo_context(&active, repo).await;
            // What it reads of its folders there, as its pages do.
            let reach = self.with_files(
                &active.loaded.declaration,
                Some(repo),
                Arc::new(crate::runtime::NoReach),
            );
            let outcome = active
                .loaded
                .call(
                    &crate::runtime::Hook::RepoData,
                    serde_json::Value::Null,
                    &context,
                    reach,
                    CancellationToken::new(),
                )
                .await;
            match outcome.error {
                None => {
                    values.insert(name, outcome.value);
                }
                Some(error) => {
                    eprintln!("{name}'s repo_data in {repo}: {error}")
                }
            }
        }
        tau_luau_plugins::RepoValues(values)
    }

    /// tau started, having found `runs` cut off: each active plugin's
    /// `recover` lets go of what it held for them, in each repository of
    /// `repos`, on its store there.
    pub async fn recover(&self, repos: &[String], runs: &[String]) {
        for active in self.active().await {
            if !active.loaded.declaration.hooks.recover {
                continue;
            }
            let name = active.loaded.declaration.name.clone();
            let Some(active) = self.plugin(&name).await else {
                continue;
            };
            // Every repository it keeps a store in, listed now or not
            // yet, as tau starts before it lists them.
            let mut all: Vec<String> = repos.to_vec();
            for kept in self.0.stores.repos_of(&name).await {
                if !all.contains(&kept) {
                    all.push(kept);
                }
            }
            for repo in &all {
                let context = self.repo_context(&active, repo).await;
                let outcome = active
                    .loaded
                    .call(
                        &crate::runtime::Hook::Recover,
                        serde_json::json!({ "runs": runs }),
                        &context,
                        Arc::new(crate::runtime::NoReach),
                        CancellationToken::new(),
                    )
                    .await;
                if let Some(error) = outcome.error {
                    eprintln!("{name}'s recover in {repo}: {error}");
                }
            }
        }
    }

    /// Draws `plugin`'s page `page` of `params` as a fetched tree: of the
    /// repository `repo` among them, its actions carried out there, or
    /// of the app without one (ADR 0050, gap L). `jev` says whether a Jev
    /// key is saved, as `ctx.run.jev`; `drafts`, what the inputs its
    /// `drafts` names hold, as `params.drafts`.
    pub async fn page(
        &self,
        plugin: &str,
        page: &str,
        params: &BTreeMap<String, String>,
        jev: bool,
        drafts: &BTreeMap<String, String>,
    ) -> serde_json::Value {
        let failed = |why: String| {
            serde_json::json!({ "tree": {
                "piece": "status", "text": format!("Its page failed: {why}"), "tone": "bad",
            } })
        };
        let Some(active) = self.plugin(plugin).await else {
            return failed(format!("{plugin} is not active"));
        };
        let repo = params.get("repo").filter(|repo| !repo.is_empty());
        let mut context = match repo {
            Some(repo) => self.repo_context(&active, repo).await,
            None => self.app_context(&active).await,
        };
        context.run["jev"] = serde_json::json!(jev);
        let mut input = serde_json::json!(params);
        if !drafts.is_empty() {
            input["drafts"] = serde_json::json!(drafts);
        }
        let reach = self.with_files(
            &active.loaded.declaration,
            repo.map(String::as_str),
            Arc::new(crate::runtime::NoReach),
        );
        let outcome = active
            .loaded
            .call(
                &crate::runtime::Hook::Page(page.to_owned()),
                input,
                &context,
                reach,
                CancellationToken::new(),
            )
            .await;
        match outcome.error {
            Some(error) => failed(error),
            None => {
                let tree = match repo {
                    Some(repo) => tau_luau_plugins::ui::embed_repo(
                        &outcome.value,
                        plugin,
                        repo,
                    ),
                    None => {
                        tau_luau_plugins::ui::embed_app(&outcome.value, plugin)
                    }
                };
                serde_json::json!({ "tree": tree.value() })
            }
        }
    }

    /// `plugin`'s step of first-run setup with the repositories `picked`
    /// so far, as `ctx.run.picked`: its body, as a fetched tree (ADR 0050,
    /// gap L).
    pub async fn setup(
        &self,
        plugin: &str,
        picked: &[serde_json::Value],
    ) -> serde_json::Value {
        let failed = |why: String| {
            serde_json::json!({ "tree": {
                "piece": "status", "text": format!("Its step failed: {why}"), "tone": "bad",
            } })
        };
        let Some(active) = self.plugin(plugin).await else {
            return failed(format!("{plugin} is not active"));
        };
        let declaration = &active.loaded.declaration;
        if !declaration.hooks.setup {
            return failed(format!("{plugin} has no setup"));
        }
        let app = match self.app_views().await.get(plugin) {
            Some(view) => view.value.clone(),
            None => serde_json::Value::Null,
        };
        let mut context = self.app_context(&active).await;
        context.run["picked"] = serde_json::json!(picked);
        let reach = self.with_files(
            declaration,
            None,
            Arc::new(crate::runtime::NoReach),
        );
        let outcome = active
            .loaded
            .call(
                &crate::runtime::Hook::AppSetup,
                app,
                &context,
                reach,
                CancellationToken::new(),
            )
            .await;
        match outcome.error {
            Some(error) => failed(error),
            None => {
                let tree = tau_luau_plugins::ui::embed_app(
                    &outcome.value["body"],
                    plugin,
                );
                serde_json::json!({ "tree": tree.value() })
            }
        }
    }

    /// Carries out `plugin`'s `actions[action]` with `args` from one of
    /// its pages of `repo`: on its store there, with no run's state; then
    /// what it asks of the chats, through `chats` ([`crate::chats::follow`]).
    pub async fn repo_action(
        &self,
        plugin: &str,
        repo: &str,
        action: &str,
        args: serde_json::Value,
        chats: Option<&dyn tau_ui_plugin::Conversations>,
        jev: Option<Arc<dyn tau_jev::Jev>>,
    ) -> anyhow::Result<serde_json::Value> {
        let value = self
            .repo_hook(plugin, repo, action, args, jev.clone())
            .await?;
        crate::chats::follow(value, chats, Some(repo), |next, args| {
            let jev = jev.clone();
            async move {
                self.repo_hook(plugin, repo, &next, args, jev)
                    .await
                    .map_err(|error| format!("{error:#}"))
            }
        })
        .await
        .map_err(|error| anyhow::anyhow!("{plugin}'s {action}: {error}"))
    }

    /// `plugin`'s `actions[action]` with `args` in `repo`: what it
    /// answered.
    async fn repo_hook(
        &self,
        plugin: &str,
        repo: &str,
        action: &str,
        args: serde_json::Value,
        jev: Option<Arc<dyn tau_jev::Jev>>,
    ) -> anyhow::Result<serde_json::Value> {
        let active = self
            .plugin(plugin)
            .await
            .ok_or_else(|| anyhow::anyhow!("{plugin} is not active"))?;
        let context = self.repo_context(&active, repo).await;
        let outcome = active
            .loaded
            .call(
                &crate::runtime::Hook::Action(action.to_owned()),
                args,
                &context,
                self.with_files(
                    &active.loaded.declaration,
                    Some(repo),
                    Arc::new(crate::runtime::ActionReach(jev)),
                ),
                CancellationToken::new(),
            )
            .await;
        match outcome.error {
            Some(error) => anyhow::bail!("{plugin}'s {action}: {error}"),
            None => Ok(outcome.value),
        }
    }

    /// The active version of `plugin`, with the person's settings for it.
    pub async fn plugin(&self, plugin: &str) -> Option<Active> {
        let mut active = self
            .active()
            .await
            .into_iter()
            .find(|active| active.loaded.declaration.name == plugin)?;
        let settings = (self.0.settings)();
        active.settings = tau_luau_plugins::settings::effective(
            &active.loaded.declaration,
            settings.plugins.get(plugin),
        )
        .0;
        Some(active)
    }

    fn filler_of(&self, slot: &str, active: Vec<Active>) -> Option<Active> {
        let mut active = active.into_iter().find(|active| {
            active.loaded.declaration.fills.iter().any(|s| s == slot)
        })?;
        let settings = (self.0.settings)();
        active.settings = tau_luau_plugins::settings::effective(
            &active.loaded.declaration,
            settings.plugins.get(&active.loaded.declaration.name),
        )
        .0;
        Some(active)
    }

    /// The plugins every new run gets.
    pub async fn active(&self) -> Vec<Active> {
        self.0.state.read().await.active.clone()
    }

    /// The plugins a run whose workspace is `dir`, in the plugins
    /// repository, has active: trunk's, each in its version in `dir`
    /// instead when that version loads, passes its tests and reaches no
    /// further than the person allowed; else trunk's stays. A plugin
    /// `dir` lacks keeps trunk's version: removing one takes effect when
    /// the removal lands.
    pub async fn in_workspace(&self, dir: &Path) -> Vec<Active> {
        let mut active = self.active().await;
        let dir = dir.to_owned();
        let folders = tokio::task::spawn_blocking(move || plugin_folders(&dir))
            .await
            .unwrap_or_default();
        if folders.is_empty() {
            return active;
        }
        let allowed = read_allowed(&self.0.allowed_path).await;
        for (name, files) in folders {
            let digest = files.digest();
            let at = active
                .iter()
                .position(|active| active.loaded.declaration.name == name);
            if at.is_some_and(|at| active[at].loaded.digest == digest) {
                continue;
            }
            let Some(loaded) = self.try_version(&name, files, &allowed).await
            else {
                continue;
            };
            let settings = loaded.declaration.default_settings();
            let version = Active { loaded, settings };
            match at {
                Some(at) => active[at] = version,
                None => active.push(version),
            }
        }
        active
    }

    /// Whether what `dir`, a workspace of the plugins repository,
    /// changes in its plugins may land on trunk as it is: each plugin
    /// whose files differ from trunk's loads, passes its tests and
    /// reaches no further than the person allowed (ADR 0034). One that
    /// is the same as trunk's does not count, even when trunk's fails.
    pub async fn ready(&self, dir: &Path) -> bool {
        let trunk = self.0.state.read().await.trunk.clone();
        let dir = dir.to_owned();
        let folders = tokio::task::spawn_blocking(move || plugin_folders(&dir))
            .await
            .unwrap_or_default();
        let allowed = read_allowed(&self.0.allowed_path).await;
        for (name, files) in folders {
            if trunk.get(&name) == Some(&files.digest()) {
                continue;
            }
            if self.try_version(&name, files, &allowed).await.is_none() {
                return false;
            }
        }
        true
    }

    /// `name` in `files`, when it may be active: it loads, its tests
    /// pass, and it reaches no further than `allowed` lets it.
    async fn try_version(
        &self,
        name: &str,
        files: Files,
        allowed: &BTreeMap<String, Allowed>,
    ) -> Option<Loaded> {
        let digest = files.digest();
        let tried = {
            let mut tried = self.0.tried.lock().await;
            match tried.get(&digest) {
                Some(known) => known.clone(),
                None => {
                    let known = match load(name, files).await {
                        Ok(loaded) => {
                            let tests =
                                loaded.test(CancellationToken::new()).await;
                            if tests.iter().all(|test| test.passed) {
                                Tried::Passed(Box::new(loaded))
                            } else {
                                Tried::Broken
                            }
                        }
                        Err(_) => Tried::Broken,
                    };
                    tried.insert(digest, known.clone());
                    known
                }
            }
        };
        let Tried::Passed(loaded) = tried else {
            return None;
        };
        let grown = match allowed.get(name) {
            Some(allowed) if allowed.digest == loaded.digest => Vec::new(),
            Some(allowed) => {
                loaded.declaration.grown_from(&allowed.declaration)
            }
            None => loaded.declaration.grown_from(&Declaration::default()),
        };
        grown.is_empty().then_some(*loaded)
    }

    /// The repository as last read, for the Plugins screen.
    pub async fn overview(&self) -> Overview {
        self.0.state.read().await.overview.clone()
    }

    /// Reads trunk every [`POLL`], and reloads when it moved. Runs for
    /// the host's life.
    async fn watch(&self) {
        let mut project: Option<Project> = None;
        loop {
            if project.is_none() {
                project =
                    Project::open(self.0.root.clone(), self.0.identity.clone())
                        .await
                        .ok();
            }
            if let Some(open) = &project {
                let read = open
                    .run(|repo| repo.trunk())
                    .await
                    .map_err(|error| error.to_string());
                let (moved, shown) = {
                    let overview = &mut self.0.state.write().await.overview;
                    let before = overview.error.clone();
                    let moved = overview
                        .trunk_read(read.as_deref().map_err(Clone::clone));
                    (moved, overview.error != before)
                };
                if moved && let Ok(trunk) = &read {
                    self.reload(open, trunk).await;
                } else if shown {
                    (self.0.refresh)();
                }
            }
            tokio::time::sleep(POLL).await;
        }
    }

    /// Loads and tests every plugin at `trunk`, activates what may
    /// activate, and redraws the catalog.
    pub async fn reload(&self, project: &Project, trunk: &str) {
        let _allowing = self.0.allowing.lock().await;
        let at = trunk.to_owned();
        let read = project.run(move |repo| repo.files_under(&at, "")).await;
        let files = match read {
            Ok(files) => files,
            Err(error) => {
                self.0.state.write().await.overview.error =
                    Some(error.to_string());
                (self.0.refresh)();
                return;
            }
        };
        // Each top-level folder with files is a plugin.
        let mut folders: BTreeMap<String, Vec<(String, Vec<u8>)>> =
            BTreeMap::new();
        for (path, bytes) in files {
            if let Some((folder, rest)) = path.split_once('/') {
                folders
                    .entry(folder.to_owned())
                    .or_default()
                    .push((rest.to_owned(), bytes));
            }
        }
        let path = self.0.allowed_path.clone();
        let mut allowed = read_allowed(&path).await;
        let before: BTreeMap<String, Active> = self
            .0
            .state
            .read()
            .await
            .active
            .iter()
            .map(|active| {
                (active.loaded.declaration.name.clone(), active.clone())
            })
            .collect();
        let mut state = State {
            overview: Overview {
                commit: Some(trunk.to_owned()),
                plugins: Vec::new(),
                error: None,
            },
            ..State::default()
        };
        let built_in: Vec<&str> = self
            .0
            .built_in
            .iter()
            .map(|built_in| built_in.declaration.name.as_str())
            .collect();
        for (name, paths) in folders {
            // A name tau's own plugins have is theirs.
            if built_in.contains(&name.as_str()) {
                state.overview.plugins.push(Entry {
                    name: name.clone(),
                    description: String::new(),
                    version: String::new(),
                    standing: Standing::Broken {
                        error: format!(
                            "`{name}` is the name of a plugin tau comes with"
                        ),
                    },
                    declaration: None,
                    tests: Vec::new(),
                    keeps_earlier: false,
                    built_in: false,
                    app: None,
                });
                continue;
            }
            let earlier = before.get(&name).cloned();
            let loaded = match Files::from_paths(&paths, crate::runtime::PLUGIN)
            {
                Ok(files) => {
                    state.trunk.insert(name.clone(), files.digest());
                    load(&name, files).await
                }
                Err(error) => Err(error),
            };
            let loaded = match loaded {
                Ok(loaded) => loaded,
                Err(error) => {
                    state.overview.plugins.push(Entry {
                        name: name.clone(),
                        description: String::new(),
                        version: String::new(),
                        standing: Standing::Broken { error },
                        declaration: None,
                        tests: Vec::new(),
                        keeps_earlier: earlier.is_some(),
                        built_in: false,
                        app: None,
                    });
                    state.active.extend(earlier);
                    continue;
                }
            };
            let tests = loaded.test(CancellationToken::new()).await;
            let declaration = loaded.declaration.clone();
            let grown = match allowed.get(&name) {
                Some(allowed) if allowed.digest == loaded.digest => Vec::new(),
                Some(allowed) => declaration.grown_from(&allowed.declaration),
                None => declaration.grown_from(&Declaration::default()),
            };
            let standing = if tests.iter().any(|test| !test.passed) {
                Standing::Failing
            } else if !grown.is_empty() {
                Standing::Waiting { grown }
            } else {
                Standing::Active
            };
            let active = matches!(standing, Standing::Active);
            let failing = matches!(standing, Standing::Failing);
            state.overview.plugins.push(Entry {
                name: name.clone(),
                description: declaration.description.clone(),
                version: loaded.files.manifest.version.clone(),
                standing,
                declaration: Some(declaration.clone()),
                tests,
                keeps_earlier: !active && earlier.is_some(),
                built_in: false,
                app: None,
            });
            if active {
                allowed.insert(
                    name,
                    Allowed {
                        digest: loaded.digest.clone(),
                        declaration,
                    },
                );
                let settings = loaded.declaration.default_settings();
                state.active.push(Active { loaded, settings });
            } else {
                let held = if failing {
                    Held::Failing(loaded)
                } else {
                    Held::Waiting(loaded)
                };
                state.waiting.insert(name, held);
                state.active.extend(earlier);
            }
        }
        write_allowed(&path, &allowed).await;
        let mut held = self.0.state.write().await;
        state.built_in = std::mem::take(&mut held.built_in);
        merge_built_in(&mut state);
        *held = state;
        drop(held);
        (self.0.refresh)();
    }

    /// Draws `plugin`'s settings page of `settings`: the version waiting
    /// for the person, else the active one.
    pub async fn settings_page(
        &self,
        plugin: &str,
        settings: serde_json::Value,
    ) -> crate::SettingsPage {
        let loaded = {
            let state = self.0.state.read().await;
            state
                .waiting
                .get(plugin)
                .map(|held| held.loaded().clone())
                .or_else(|| {
                    state
                        .active
                        .iter()
                        .find(|active| active.loaded.declaration.name == plugin)
                        .map(|active| active.loaded.clone())
                })
        };
        let page = match loaded {
            None => Err(format!("{plugin} is not loaded")),
            Some(loaded) => {
                let context = crate::runtime::Context {
                    settings: settings.clone(),
                    secrets: self.secrets_of(plugin),
                    ..Default::default()
                };
                let outcome = loaded
                    .call(
                        &crate::runtime::Hook::SettingsView,
                        settings.clone(),
                        &context,
                        std::sync::Arc::new(crate::runtime::NoReach),
                        CancellationToken::new(),
                    )
                    .await;
                match outcome.error {
                    Some(error) => Err(error),
                    None => Ok(outcome.value),
                }
            }
        };
        crate::SettingsPage {
            plugin: plugin.to_owned(),
            settings,
            page,
        }
    }

    /// Carries out `action` of `plugin`'s active version with `args`, on
    /// the `state` it has in `run`, under the person's `settings`, then
    /// draws its view from the state it left (ADR 0045). An action that
    /// fails leaves the state as it was, and says why.
    pub async fn action(&self, call: ActionCall<'_>) -> crate::ActionDone {
        use crate::runtime::{Context, Hook, NoReach};
        let ActionCall {
            plugin,
            action,
            args,
            run,
            repo,
            state,
            settings,
            chats,
            jev,
            live,
            keyed,
        } = call;

        let hook = Hook::Action(action.to_owned());
        let mut done = crate::ActionDone {
            plugin: plugin.to_owned(),
            run: run.to_owned(),
            on: state.clone(),
            state: state.clone(),
            changed: false,
            view: None,
            logs: Vec::new(),
            error: None,
            interface: None,
        };
        let loaded = {
            let state = self.0.state.read().await;
            state
                .active
                .iter()
                .find(|active| active.loaded.declaration.name == plugin)
                .map(|active| active.loaded.clone())
        };
        let Some(loaded) = loaded else {
            done.error = Some((
                format!("actions.{action}"),
                format!("{plugin} is not active"),
            ));
            return done;
        };
        if !hook.declared_in(&loaded.declaration) {
            done.error = Some((
                format!("actions.{action}"),
                format!("{plugin} has no action {action}"),
            ));
            return done;
        }
        let mut context = Context {
            run: serde_json::json!({ "id": run, "live": live, "jev": keyed }),
            now: crate::agent::now(),
            settings: tau_luau_plugins::settings::effective(
                &loaded.declaration,
                settings,
            )
            .0,
            state: state.clone(),
            secrets: self.secrets_of(plugin),
            store: self.store_of(&loaded.declaration, repo).await,
            job: None,
        };
        let outcome = loaded
            .call(
                &hook,
                args.clone(),
                &context,
                self.with_files(
                    &loaded.declaration,
                    repo,
                    Arc::new(crate::runtime::ActionReach(jev.clone())),
                ),
                CancellationToken::new(),
            )
            .await;
        done.logs = outcome.logs;
        if let Some(error) = outcome.error {
            done.error = Some((format!("actions.{action}"), error));
            return done;
        }
        done.changed = outcome.state != *state;
        done.state = outcome.state;
        // The answers to a form a call waits on, given to it: one that
        // does not fit, or comes once it is taken, is the action's error.
        if let Some(answered) = crate::forms::answered(&outcome.value) {
            let given = answered.and_then(|(call, reply)| {
                crate::forms::answer(&self.forms(), plugin, run, &call, reply)
                    .map_err(|error| format!("{error:#}"))
            });
            if let Err(error) = given {
                done.error = Some((format!("actions.{action}"), error));
            }
        }
        let followed = match repo {
            Some(repo) => {
                crate::chats::follow(
                    outcome.value,
                    chats,
                    Some(repo),
                    |next, args| {
                        let jev = jev.clone();
                        async move {
                            self.repo_hook(plugin, repo, &next, args, jev)
                                .await
                                .map_err(|error| format!("{error:#}"))
                        }
                    },
                )
                .await
            }
            None => {
                crate::chats::follow(
                    outcome.value,
                    chats,
                    None,
                    |next, _| async move {
                        Err(format!("`{next}` needs the run's repository"))
                    },
                )
                .await
            }
        };
        match followed {
            Ok(value) => done.interface = value.get("tau_interface").cloned(),
            Err(error) => {
                done.error = Some((format!("actions.{action}"), error));
            }
        }
        if loaded.declaration.hooks.view {
            context.state = done.state.clone();
            let drawn = loaded
                .call(
                    &Hook::View,
                    serde_json::Value::Null,
                    &context,
                    Arc::new(NoReach),
                    CancellationToken::new(),
                )
                .await;
            match drawn.error {
                Some(error) => done.error = Some(("view".into(), error)),
                None => done.view = Some(drawn.value),
            }
        }
        done
    }

    /// The person allows `plugin`'s waiting version, which they saw ask
    /// for `shown`: it activates, and what it reaches becomes what that
    /// plugin may reach. A version that fails its tests, or that asks for
    /// something else now (trunk moved since the page was drawn), is not
    /// allowed.
    pub async fn allow(
        &self,
        plugin: &str,
        shown: &[String],
    ) -> anyhow::Result<()> {
        let _allowing = self.0.allowing.lock().await;
        let mut state = self.0.state.write().await;
        let asks = state.overview.plugins.iter().find_map(|entry| match &entry
            .standing
        {
            Standing::Waiting { grown } if entry.name == plugin => {
                Some(grown.clone())
            }
            _ => None,
        });
        if asks.as_deref().is_some_and(|asks| asks != shown) {
            anyhow::bail!(
                "{plugin}'s waiting version asks for more than was shown: \
                 look at it again"
            );
        }
        let loaded = match state.waiting.remove(plugin) {
            Some(Held::Waiting(loaded)) => loaded,
            Some(failing @ Held::Failing(_)) => {
                state.waiting.insert(plugin.to_owned(), failing);
                anyhow::bail!("{plugin}'s waiting version fails its tests");
            }
            None => anyhow::bail!("{plugin} has no version waiting"),
        };
        let path = self.0.allowed_path.clone();
        let mut allowed = read_allowed(&path).await;
        allowed.insert(
            plugin.to_owned(),
            Allowed {
                digest: loaded.digest.clone(),
                declaration: loaded.declaration.clone(),
            },
        );
        write_allowed(&path, &allowed).await;
        state
            .active
            .retain(|active| active.loaded.declaration.name != plugin);
        let settings = loaded.declaration.default_settings();
        state.active.push(Active { loaded, settings });
        for entry in &mut state.overview.plugins {
            if entry.name == plugin {
                entry.standing = Standing::Active;
                entry.keeps_earlier = false;
            }
        }
        drop(state);
        (self.0.refresh)();
        Ok(())
    }
}

/// The plugin folders in a workspace `dir`: each top-level folder with a
/// `tau.toml`, as its files.
fn plugin_folders(dir: &Path) -> Vec<(String, Files)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut folders: Vec<(String, Files)> = entries
        .flatten()
        .filter(|entry| entry.path().join(crate::runtime::MANIFEST).is_file())
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_owned();
            if name.starts_with('.') {
                return None;
            }
            Some((
                name,
                Files::read(&entry.path(), crate::runtime::PLUGIN).ok()?,
            ))
        })
        .collect();
    folders.sort_by(|a, b| a.0.cmp(&b.0));
    folders
}

async fn read_allowed(path: &Path) -> BTreeMap<String, Allowed> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    })
    .await
    .unwrap_or_default()
}

async fn write_allowed(path: &Path, allowed: &BTreeMap<String, Allowed>) {
    let (path, text) = (
        path.to_owned(),
        serde_json::to_string_pretty(allowed).unwrap_or_default(),
    );
    let written = tokio::task::spawn_blocking(move || {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, text)
    })
    .await;
    if let Ok(Err(error)) = written {
        eprintln!("{}: cannot keep what was allowed: {error}", crate::NAME);
    }
}

/// Puts the built-in plugins first among `state`'s, once each.
fn merge_built_in(state: &mut State) {
    let names: Vec<String> = state
        .built_in
        .iter()
        .map(|(entry, _)| entry.name.clone())
        .collect();
    state
        .overview
        .plugins
        .retain(|entry| !(entry.built_in && names.contains(&entry.name)));
    state
        .active
        .retain(|active| !names.contains(&active.loaded.declaration.name));
    for (entry, active) in state.built_in.iter().rev() {
        state.overview.plugins.insert(0, entry.clone());
        state.active.insert(0, active.clone());
    }
}

/// What `active`'s `app` keeps in `context`, its tools' folders read
/// through `files`, and the commands it lists and the page it draws of it
/// (ADR 0050, gap K): what the registry draws, and the demo.
pub async fn app_view(
    active: &Active,
    files: crate::files::Files,
    context: &crate::runtime::Context,
) -> tau_luau_plugins::AppView {
    use crate::runtime::{Hook, NoReach, Reach, WithFiles};
    let declaration = &active.loaded.declaration;
    let roots = files.root_paths();
    let reach: Arc<dyn Reach> = if declaration.uses.files.is_empty() {
        Arc::new(NoReach)
    } else {
        Arc::new(WithFiles {
            reach: Arc::new(NoReach),
            files: Arc::new(files),
        })
    };
    let call = |hook: Hook, input: serde_json::Value| {
        let reach = reach.clone();
        async move {
            active
                .loaded
                .call(&hook, input, context, reach, CancellationToken::new())
                .await
        }
    };
    let kept = call(Hook::App, serde_json::Value::Null).await;
    let mut view = tau_luau_plugins::AppView {
        value: kept.value,
        roots,
        error: kept.error,
        ..Default::default()
    };
    if view.error.is_some() {
        return view;
    }
    if declaration.listed_commands {
        let listed = call(Hook::ListCommands, view.value.clone()).await;
        match listed.error {
            None => {
                view.commands =
                    serde_json::from_value(listed.value).unwrap_or_default();
            }
            Some(error) => view.error = Some(format!("commands.list: {error}")),
        }
    }
    if declaration.hooks.page {
        let page = call(Hook::AppPage, view.value.clone()).await;
        match page.error {
            None => view.page = Some(page.value),
            Some(error) => view.error = Some(format!("page: {error}")),
        }
    }
    for (on, hook) in [
        (declaration.hooks.setup, Hook::AppSetup),
        (declaration.hooks.models, Hook::AppModels),
    ] {
        if !on {
            continue;
        }
        let setup = matches!(hook, Hook::AppSetup);
        let drawn = call(hook, view.value.clone()).await;
        match drawn.error {
            None if setup => view.setup = Some(drawn.value),
            None => view.models = Some(drawn.value),
            Some(error) => {
                let place = if setup { "setup" } else { "models" };
                view.error = Some(format!("{place}: {error}"));
            }
        }
    }
    view
}
