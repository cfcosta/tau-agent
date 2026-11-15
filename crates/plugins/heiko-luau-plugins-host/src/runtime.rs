//! Reading a plugin's folder, loading it, and calling its hooks, each in
//! a fresh codemode VM with codemode's limits.
//!
//! Every call is a short script: it requires the `heiko` module and the
//! plugin, builds `ctx` from what the host sends, calls the hook, and
//! returns the hook's answer with the state it left. The plugin's files
//! are codemode modules: `heiko`, `plugin`, and one per file beside its entry
//! under `src/`.
//!
//! An agent's repository (ADR 0037) is a package of the same layout whose
//! [`Root`] requires its entry as `agent` (ADR 0051): [`Files`], [`evaluate`]
//! and [`run_tests`] serve both.

use std::{collections::BTreeMap, path::Path, sync::Arc, time::Duration};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use heiko_ai::message::Usage;
use heiko_codemode::{
    modules::Definition,
    result::{Failure, Item},
};
use heiko_codemode_host::{
    Host,
    Options,
    Request,
    Source,
    ToolCall,
    ToolEntry,
    ToolReply,
};
use heiko_decide::Decider;
use heiko_luau_plugins::{HEIKO_TEST_MODULE, TestKind};
use tokio_util::sync::CancellationToken;

use crate::{Declaration, HEIKO_MODULE, TestResult};

/// What a package's entry is by default, the module it is required as, and
/// the field of `heiko.test` its tests find it in: how a plugin is told from
/// an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Root {
    pub entry: &'static str,
    pub module: &'static str,
    pub subject: &'static str,
}

/// A plugin's package: its entry is required as `plugin`.
pub const PLUGIN: Root = Root {
    entry: "src/plugin.luau",
    module: "plugin",
    subject: "_plugin",
};

impl Default for Root {
    fn default() -> Self {
        PLUGIN
    }
}

/// The file that says what a package is (ADR 0051).
pub const MANIFEST: &str = "heiko.toml";

/// A folder's modules, by name: what its scripts run against.
pub type Modules = BTreeMap<String, Definition>;

/// A package's `heiko.toml` (ADR 0051), checked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manifest {
    /// The folder's name, which is the plugin's or the agent's.
    pub name: String,
    /// For the person: a digest names a version, not this.
    pub version: String,
    pub description: String,
    /// The entry's path in the folder; empty for the [`Root`]'s default.
    pub entry: String,
}

/// `heiko.toml` as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WrittenManifest {
    name: String,
    version: String,
    description: String,
    entry: Option<String>,
}

impl Manifest {
    /// Reads `heiko.toml`'s text. A field it does not know, a missing one and
    /// a bad value are each an error, which says which.
    pub fn parse(text: &str) -> Result<Self, String> {
        let written: WrittenManifest = toml::from_str(text)
            .map_err(|error| format!("{MANIFEST}: {}", error.message()))?;
        let bad = |why: String| format!("{MANIFEST}: {why}");
        if written.name.is_empty()
            || written.name.contains(['/', '\\'])
            || written.name.starts_with('.')
        {
            return Err(bad(format!(
                "{:?} is not a folder's name",
                written.name
            )));
        }
        let numbers: Vec<&str> = written.version.split('.').collect();
        if numbers.len() != 3
            || numbers.iter().any(|number| {
                number.is_empty()
                    || !number.bytes().all(|byte| byte.is_ascii_digit())
            })
        {
            return Err(bad(format!(
                "`version` is {:?}, not three numbers: MAJOR.MINOR.PATCH",
                written.version
            )));
        }
        if written.description.trim().is_empty()
            || written.description.contains('\n')
        {
            return Err(bad("`description` is one line, not empty".to_owned()));
        }
        let entry = written.entry.unwrap_or_default();
        if !entry.is_empty() {
            let stem = entry
                .strip_prefix("src/")
                .and_then(|rest| rest.strip_suffix(".luau"));
            if !stem.is_some_and(|stem| {
                !stem.is_empty()
                    && stem != "heiko"
                    && !stem.contains(['/', '\\'])
                    && !stem.contains("..")
            }) {
                return Err(bad(format!(
                    "`entry` is {entry:?}, not `src/<name>.luau`"
                )));
            }
        }
        Ok(Self {
            name: written.name,
            version: written.version,
            description: written.description,
            entry,
        })
    }

    /// A `heiko.toml` saying `name` and `description`, at version 0.1.0 with
    /// the default entry: what a test or a new package starts from.
    pub fn text(name: &str, description: &str) -> String {
        format!(
            "name = {}\nversion = \"0.1.0\"\ndescription = {}\n",
            serde_json::Value::from(name),
            serde_json::Value::from(description),
        )
    }
}

/// What a plugin's folder holds, or an agent's: a package (ADR 0051).
#[derive(Debug, Clone, Default)]
pub struct Files {
    /// How its entry is required.
    pub root: Root,
    /// Its `heiko.toml`.
    pub manifest: Manifest,
    /// Its entry.
    pub source: String,
    /// `src/<name>.luau`, each file beside the entry, by name.
    pub libs: BTreeMap<String, String>,
    /// `tests/<name>.luau`, by name.
    pub tests: BTreeMap<String, String>,
    pub readme: Option<String>,
}

impl Files {
    /// A package named `name` that says nothing else: what a test builds
    /// by hand, in place of a folder and its `heiko.toml`.
    pub fn named(name: &str) -> Self {
        Self {
            manifest: Manifest {
                name: name.to_owned(),
                version: "0.1.0".to_owned(),
                ..Manifest::default()
            },
            ..Self::default()
        }
    }

    /// Its entry's path in the folder.
    pub fn entry(&self) -> &str {
        if self.manifest.entry.is_empty() {
            self.root.entry
        } else {
            &self.manifest.entry
        }
    }

    /// Reads a folder as a `root`. It reads what [`Self::from_paths`] would
    /// be given, so the two agree. It blocks: call it from `spawn_blocking`.
    pub fn read(dir: &Path, root: Root) -> Result<Self, String> {
        let mut paths = Vec::new();
        for name in [MANIFEST, "README.md"] {
            if let Ok(bytes) = std::fs::read(dir.join(name)) {
                paths.push((name.to_owned(), bytes));
            }
        }
        fn walk(
            dir: &Path,
            prefix: &str,
            paths: &mut Vec<(String, Vec<u8>)>,
        ) -> Result<(), String> {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return Ok(());
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(name) = entry.file_name().to_str().map(str::to_owned)
                else {
                    continue;
                };
                let inner = format!("{prefix}/{name}");
                if path.is_dir() {
                    walk(&path, &inner, paths)?;
                } else if name.ends_with(".luau") {
                    let bytes = std::fs::read(&path).map_err(|error| {
                        format!("{}: {error}", path.display())
                    })?;
                    paths.push((inner, bytes));
                }
            }
            Ok(())
        }
        if paths.iter().all(|(path, _)| path != MANIFEST) {
            return Err(format!("{}: it has no {MANIFEST}", dir.display()));
        }
        walk(&dir.join("src"), "src", &mut paths)?;
        walk(&dir.join("tests"), "tests", &mut paths)?;
        Self::from_paths(&paths, root)
    }

    /// A package from its files at a commit or in a folder, by their paths
    /// in it (`heiko.toml`, `src/x.luau`): what heiko reads of it. `fixtures/`
    /// and anything else is not read.
    pub fn from_paths(
        paths: &[(String, Vec<u8>)],
        root: Root,
    ) -> Result<Self, String> {
        let text = |path: &str, bytes: &[u8]| {
            String::from_utf8(bytes.to_vec())
                .map_err(|_| format!("{path} is not UTF-8"))
        };
        let Some((_, manifest)) =
            paths.iter().find(|(path, _)| path == MANIFEST)
        else {
            return Err(format!("the folder has no {MANIFEST}"));
        };
        let mut files = Self {
            root,
            manifest: Manifest::parse(&text(MANIFEST, manifest)?)?,
            ..Self::default()
        };
        let entry = files.entry().to_owned();
        let mut has_entry = false;
        for (path, bytes) in paths {
            let luau = |sub: &str| {
                path.strip_prefix(sub)
                    .and_then(|rest| rest.strip_suffix(".luau"))
            };
            if path == &entry {
                files.source = text(path, bytes)?;
                has_entry = true;
            } else if path == "README.md" {
                files.readme = Some(text(path, bytes)?);
            } else if let Some(stem) = luau("src/") {
                if stem.contains('/') {
                    return Err(format!(
                        "{path}: modules sit beside the entry, in `src/`"
                    ));
                }
                files.libs.insert(stem.to_owned(), text(path, bytes)?);
            } else if let Some(stem) = luau("tests/") {
                if stem.contains('/') {
                    return Err(format!(
                        "{path}: tests sit directly in `tests/`"
                    ));
                }
                files.tests.insert(stem.to_owned(), text(path, bytes)?);
            }
        }
        if !has_entry {
            return Err(format!("the folder has no {entry}"));
        }
        Ok(files)
    }

    /// The digest of what heiko reads of the folder: what names a version.
    pub fn digest(&self) -> String {
        let mut hasher = Sha256::new();
        let mut add = |path: &str, text: &str| {
            hasher.update(path.as_bytes());
            hasher.update([0]);
            hasher.update(text.as_bytes());
            hasher.update([0]);
        };
        let manifest = &self.manifest;
        add(
            MANIFEST,
            &format!(
                "{}\n{}\n{}\n{}",
                manifest.name,
                manifest.version,
                manifest.description,
                self.entry()
            ),
        );
        add(self.entry(), &self.source);
        for (name, text) in &self.libs {
            add(&format!("src/{name}.luau"), text);
        }
        for (name, text) in &self.tests {
            add(&format!("tests/{name}.luau"), text);
        }
        if let Some(readme) = &self.readme {
            add("README.md", readme);
        }
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// The codemode modules the folder makes: `heiko`, each file beside its
    /// entry under `src/` (which may require `heiko` and each other, but not
    /// in a cycle), and its entry, `plugin` or `agent` (which may require
    /// them all).
    pub fn modules(&self) -> Result<Modules, String> {
        let entry = self.entry().to_owned();
        let (file, module) = (entry.as_str(), self.root.module);
        let actor = Definition::new(
            "heiko_actor".into(),
            heiko_luau_plugins::HEIKO_ACTOR_MODULE.into(),
            json!({}),
            BTreeMap::new(),
        )?;
        // The standard part, which `heiko` installs into itself too.
        let std = std_parts()?
            .iter()
            .map(|(name, source)| {
                Definition::new(
                    name.clone(),
                    source.clone(),
                    json!({}),
                    BTreeMap::new(),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut heiko_deps = BTreeMap::from([(
            "heiko_actor".to_owned(),
            actor.version().to_owned(),
        )]);
        for part in &std {
            heiko_deps
                .insert(part.name().to_owned(), part.version().to_owned());
        }
        let heiko = Definition::new(
            "heiko".into(),
            HEIKO_MODULE.into(),
            json!({}),
            heiko_deps,
        )?;
        let mut modules = BTreeMap::new();
        let mut plugin_deps =
            BTreeMap::from([("heiko".to_owned(), heiko.version().to_owned())]);
        for name in self.libs.keys() {
            // `heiko`, and every part of it, are heiko's own.
            if name == "heiko" || name.starts_with("heiko_") || name == module
            {
                return Err(format!("src/{name}.luau: `{name}` is taken"));
            }
        }
        // Each lib once the libs it requires are: their versions pin it.
        let mut versions: BTreeMap<String, String> = BTreeMap::new();
        while versions.len() < self.libs.len() {
            let ready = self.libs.iter().find(|(name, source)| {
                !versions.contains_key(*name)
                    && required(source, &self.libs)
                        .iter()
                        .all(|dep| versions.contains_key(dep))
            });
            let Some((name, source)) = ready else {
                let left: Vec<String> = self
                    .libs
                    .keys()
                    .filter(|name| !versions.contains_key(*name))
                    .map(|name| format!("src/{name}.luau"))
                    .collect();
                return Err(format!(
                    "{} require each other: one of them cannot",
                    left.join(", ")
                ));
            };
            let mut deps =
                BTreeMap::from([("heiko".to_owned(), heiko.version().to_owned())]);
            for dep in required(source, &self.libs) {
                deps.insert(dep.clone(), versions[&dep].clone());
            }
            let lib =
                Definition::new(name.clone(), source.clone(), json!({}), deps)
                    .map_err(|error| format!("src/{name}.luau: {error}"))?;
            versions.insert(name.clone(), lib.version().to_owned());
            plugin_deps.insert(name.clone(), lib.version().to_owned());
            modules.insert(name.clone(), lib);
        }
        let root = Definition::new(
            module.into(),
            self.source.clone(),
            json!({}),
            plugin_deps,
        )
        .map_err(|error| format!("{file}: {error}"))?;
        modules.insert(module.into(), root);
        // The test harness, which only tests load.
        let harness = Definition::new(
            "heiko_test".into(),
            HEIKO_TEST_MODULE.into(),
            json!({}),
            BTreeMap::from([("heiko".to_owned(), heiko.version().to_owned())]),
        )?;
        modules.insert("heiko_test".into(), harness);
        modules.insert("heiko".into(), heiko);
        modules.insert("heiko_actor".into(), actor);
        for part in std {
            modules.insert(part.name().to_owned(), part);
        }
        Ok(modules)
    }
}

/// The standard part of `heiko` (ADR 0058): each part's module name and
/// its source, read once from heiko's packages folder, as its plugins
/// are, so that editing one compiles nothing. The first call blocks;
/// reading the built-ins makes it.
pub fn std_parts() -> Result<&'static [(String, String)], String> {
    static PARTS: std::sync::OnceLock<Result<Vec<(String, String)>, String>> =
        std::sync::OnceLock::new();
    PARTS
        .get_or_init(|| {
            let dir = heiko_luau_plugins::packages::find()?
                .join(heiko_luau_plugins::STD)
                .join("src");
            heiko_luau_plugins::STD_PARTS
                .iter()
                .map(|part| {
                    let path = dir.join(format!("{part}.luau"));
                    std::fs::read_to_string(&path)
                        .map(|source| (format!("heiko_{part}"), source))
                        .map_err(|error| format!("{}: {error}", path.display()))
                })
                .collect()
        })
        .as_ref()
        .map(Vec::as_slice)
        .map_err(Clone::clone)
}

/// The `libs` that `source` requires, by name: `require("name")`.
fn required(source: &str, libs: &BTreeMap<String, String>) -> Vec<String> {
    libs.keys()
        .filter(|name| {
            [format!("require(\"{name}\")"), format!("require('{name}')")]
                .iter()
                .any(|call| source.contains(call.as_str()))
        })
        .cloned()
        .collect()
}

/// A hook of a plugin, as the host calls it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hook {
    /// A tool's handler, with its arguments as input.
    Tool(String),
    /// A tool's card, with `{ call, result }` as input.
    Card(String),
    /// As the run starts, with `{ input, context, instructions }`.
    Start,
    BeforeTool,
    /// Before each turn's request, with `{ model, effort, turn, messages }`.
    BeforeRequest,
    /// After a call, with `{ id, name, args, ok, text, truncated }`.
    AfterToolResult,
    /// Offered the context, with `{ trigger, turn, tokens, window,
    /// messages }` ([`crate::rewrite::summary`]).
    RewriteContext,
    /// A rewrite replaced the context, with `{ plugin, before, after,
    /// drops_conversation }`.
    Rewritten,
    BeforeStop,
    TurnEnd,
    /// One of the run's events it names in `events`, as
    /// `{ kind, turn, ... }`.
    OnEvent,
    /// A run's title, for `heiko.title`, with `{ task, model }`.
    Title,
    /// One of heiko's notices, for `heiko.notify`, with `{ title, body }`.
    Notify,
    /// One of the forge's hooks, for `heiko.forge`, by name.
    Forge(String),
    /// One of its jobs, by name.
    Job(String),
    /// What it keeps of a repository (`repo_data`), with nothing as input.
    RepoData,
    /// What it keeps of the whole app (`app`, ADR 0050 gap K), with
    /// nothing as input.
    App,
    /// The page its `nav` opens, with what `app` kept as input.
    AppPage,
    /// Its commands, `commands.list` of what `app` kept.
    ListCommands,
    /// Its step of first-run setup, of what `app` kept (gap E).
    AppSetup,
    /// Its section of the Models screen, of what `app` kept.
    AppModels,
    /// One of its pages of a repository, by name, with the page's params.
    Page(String),
    /// heiko started, with the runs it found cut off as `{ runs }`.
    Recover,
    RunEnd,
    View,
    /// Its settings page, with the settings shown as input (ADR 0029).
    SettingsView,
    /// A button's action, with its arguments as input.
    Action(String),
}

impl Hook {
    /// Where it is in the plugin's table, as its actor finds it (ADR
    /// 0056).
    pub fn path(&self) -> Vec<String> {
        let keys: &[&str] = match self {
            Self::Tool(name) => {
                return vec!["tools".into(), name.clone(), "call".into()];
            }
            Self::Card(name) => {
                return vec!["tools".into(), name.clone(), "card".into()];
            }
            Self::Forge(name) => return vec!["forge".into(), name.clone()],
            Self::Job(name) => {
                return vec!["jobs".into(), name.clone(), "run".into()];
            }
            Self::Page(name) => return vec!["pages".into(), name.clone()],
            Self::Action(name) => return vec!["actions".into(), name.clone()],
            Self::Start => &["start"],
            Self::BeforeTool => &["before_tool"],
            Self::BeforeRequest => &["before_request"],
            Self::AfterToolResult => &["after_tool_result"],
            Self::RewriteContext => &["rewrite_context"],
            Self::Rewritten => &["rewritten"],
            Self::BeforeStop => &["before_stop"],
            Self::TurnEnd => &["turn_end"],
            Self::OnEvent => &["on_event"],
            Self::Title => &["title"],
            Self::Notify => &["notify"],
            Self::RepoData => &["repo_data"],
            Self::App => &["app"],
            Self::AppPage => &["page"],
            Self::ListCommands => &["listed", "_list"],
            Self::AppSetup => &["setup"],
            Self::AppModels => &["models"],
            Self::Recover => &["recover"],
            Self::RunEnd => &["run_end"],
            Self::View => &["view"],
            Self::SettingsView => &["settings", "view"],
        };
        keys.iter().map(|key| (*key).to_owned()).collect()
    }

    /// Whether it draws from a frozen copy of the model, waiting on
    /// nothing, rather than taking a step.
    pub fn draws(&self) -> bool {
        matches!(self, Self::View | Self::Card(_) | Self::SettingsView)
    }

    /// How long it may run (ADR 0027).
    pub fn limit(&self) -> Duration {
        Duration::from_millis(match self {
            Self::Tool(_) | Self::Job(_) => 60_000,
            Self::Forge(_) => 30_000,
            Self::Card(_)
            | Self::View
            | Self::SettingsView
            | Self::BeforeRequest
            | Self::OnEvent => 200,
            Self::Start
            | Self::RepoData
            | Self::App
            | Self::AppPage
            | Self::AppSetup
            | Self::AppModels
            | Self::ListCommands
            | Self::Page(_)
            | Self::Title
            | Self::BeforeTool
            | Self::AfterToolResult
            | Self::Rewritten
            | Self::TurnEnd
            | Self::Action(_) => 2_000,
            Self::BeforeStop
            | Self::RunEnd
            | Self::RewriteContext
            | Self::Recover
            | Self::Notify => 10_000,
        })
    }

    /// How long it may run for `declaration`: a run hook of a plugin
    /// whose `uses` names `infer` or `decide` may call them, and has a
    /// tool's time for it (ADR 0050), `rewrite_context` ten minutes;
    /// `on_event` never does.
    pub fn limit_for(&self, declaration: &Declaration) -> Duration {
        // An action may ask the decider too, through the host's.
        if matches!(self, Self::Action(_)) && declaration.uses.decide {
            return Self::Tool(String::new()).limit();
        }
        let run_hook = matches!(
            self,
            Self::Start
                | Self::BeforeTool
                | Self::BeforeRequest
                | Self::AfterToolResult
                | Self::RewriteContext
                | Self::Rewritten
                | Self::BeforeStop
                | Self::TurnEnd
                | Self::RunEnd
        );
        let uses = &declaration.uses;
        if !run_hook || !(uses.infer || uses.decide) {
            return self.limit();
        }
        // A compaction asks for many summaries, or a long one, while the
        // run waits for it as it waits for the loop's own.
        match self {
            Self::RewriteContext => REWRITE_LIMIT,
            _ => Self::Tool(String::new()).limit(),
        }
    }

    /// Whether `declaration` has this hook.
    pub fn declared_in(&self, declaration: &Declaration) -> bool {
        let tool =
            |name: &str| declaration.tools.iter().find(|t| t.name == name);
        match self {
            Self::Tool(name) => tool(name).is_some(),
            Self::Card(name) => tool(name).is_some_and(|t| t.card),
            Self::Start => declaration.hooks.start,
            Self::BeforeTool => declaration.hooks.before_tool,
            Self::BeforeRequest => declaration.hooks.before_request,
            Self::AfterToolResult => declaration.hooks.after_tool_result,
            Self::RewriteContext => declaration.hooks.rewrite_context,
            Self::Rewritten => declaration.hooks.rewritten,
            Self::BeforeStop => declaration.hooks.before_stop,
            Self::TurnEnd => declaration.hooks.turn_end,
            Self::OnEvent => declaration.hooks.on_event,
            Self::Title => declaration.hooks.title,
            Self::Notify => declaration.hooks.notify,
            Self::Job(name) => {
                declaration.jobs.iter().any(|job| &job.name == name)
            }
            Self::RepoData => declaration.hooks.repo_data,
            Self::App => declaration.hooks.app,
            Self::AppPage => declaration.hooks.page,
            Self::AppSetup => declaration.hooks.setup,
            Self::AppModels => declaration.hooks.models,
            Self::ListCommands => declaration.listed_commands,
            Self::Recover => declaration.hooks.recover,
            Self::Page(name) => declaration.pages.contains(name),
            Self::Forge(name) => declaration
                .forge
                .as_ref()
                .is_some_and(|forge| forge.hooks.contains(name)),
            Self::RunEnd => declaration.hooks.run_end,
            Self::View => declaration.hooks.view,
            Self::SettingsView => declaration.hooks.settings_view,
            Self::Action(name) => {
                declaration.actions.contains(name)
                    || (declaration.listed_commands && name.starts_with('/'))
            }
        }
    }
}

/// A plugin's store, as a hook's context names it.
#[derive(Debug, Clone)]
pub struct StoreRef(pub Arc<crate::store::Kv>);

impl PartialEq for StoreRef {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// A hook's session with its plugin's store.
type SessionRef = Arc<tokio::sync::Mutex<crate::store::Session>>;

/// What a job reaches beyond the hook's own (ADR 0050, gap D): with
/// `uses.chats`, its plugin's store in every repository, and the host's
/// chats to wake.
#[derive(Clone)]
pub struct JobRef {
    pub stores: Arc<crate::store::Stores>,
    pub conversations: Option<Arc<dyn heiko_ui_plugin::Conversations>>,
}

impl std::fmt::Debug for JobRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobRef").finish_non_exhaustive()
    }
}

impl PartialEq for JobRef {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.stores, &other.stores)
    }
}

/// What a hook's `ctx` holds, besides what it may reach.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Context {
    /// `{ id, kind, repo, model, turn }`.
    pub run: Value,
    /// `{ unix, iso, weekday, local, offset }`.
    pub now: Value,
    pub settings: Value,
    pub state: Value,
    /// The plugin's own secret store, which its HTTP requests name
    /// secrets from ([`secrets_of`]); none where the host keeps none.
    #[serde(skip)]
    pub secrets: Option<std::path::PathBuf>,
    /// The plugin's store in the repository the hook works in, when it
    /// has one and the plugin uses it (`uses.store`).
    #[serde(skip)]
    pub store: Option<StoreRef>,
    /// What a job reaches beyond that, when the hook is a job.
    #[serde(skip)]
    pub job: Option<JobRef>,
}

/// The file the plugin `plugin`'s secrets are kept in, in heiko's
/// configuration directory `config`: apart from every agent's, and from
/// every other plugin's.
pub fn secrets_of(
    config: &std::path::Path,
    plugin: &str,
) -> std::path::PathBuf {
    config
        .join(heiko_tools_host::secrets::AGENTS_DIR)
        .join("plugins")
        .join(format!("{plugin}.json"))
}

/// What a hook may reach beyond the VM: the tools its plugin's `uses`
/// names, the decider. A tool handler gets tools; other hooks, none.
#[async_trait]
pub trait Reach: Send + Sync {
    /// The tools there are to call; the host keeps those `uses` names.
    fn tools(&self) -> Vec<ToolEntry> {
        Vec::new()
    }

    async fn call(&self, call: ToolCall) -> Result<ToolReply, String> {
        Err(format!("`{}` cannot be called from this hook", call.name))
    }

    fn decide(&self) -> Option<Arc<dyn Decider>> {
        None
    }

    fn charge(&self, _usage: &Usage) {}
}

/// Nothing to reach: for every hook but a tool's handler, and tests.
pub struct NoReach;

impl Reach for NoReach {}

/// `reach`, with `ctx.files` too (ADR 0050, gap K): for the hooks past a
/// tool's call that read the folders `uses.files` names, `start` and
/// `app`.
pub struct WithFiles {
    pub reach: Arc<dyn Reach>,
    pub files: Arc<crate::files::Files>,
}

#[async_trait]
impl Reach for WithFiles {
    fn tools(&self) -> Vec<ToolEntry> {
        let mut tools = self.reach.tools();
        tools.extend(crate::files::Files::entries());
        tools
    }

    async fn call(&self, call: ToolCall) -> Result<ToolReply, String> {
        if crate::files::Files::handles(&call.name) {
            return self.files.call(&call.name, &call.args).await;
        }
        self.reach.call(call).await
    }

    fn decide(&self) -> Option<Arc<dyn Decider>> {
        self.reach.decide()
    }

    fn charge(&self, usage: &Usage) {
        self.reach.charge(usage);
    }
}

/// The folder of `plugin`'s package, when it is one of heiko's own: what
/// `uses.files`' `package` names. It blocks, once.
pub fn package_dir(plugin: &str) -> Option<std::path::PathBuf> {
    static FOUND: std::sync::OnceLock<Option<std::path::PathBuf>> =
        std::sync::OnceLock::new();
    let dir = FOUND
        .get_or_init(|| heiko_luau_plugins::packages::find().ok())
        .as_ref()?;
    let listed = crate::built_in::listed(dir).ok()?;
    listed
        .iter()
        .any(|name| name == plugin)
        .then(|| dir.join(plugin))
}

/// What an action reaches outside a run (ADR 0050): the host's decider, for
/// a plugin whose `uses` names it.
pub struct ActionReach(pub Option<Arc<dyn Decider>>);

impl Reach for ActionReach {
    fn decide(&self) -> Option<Arc<dyn Decider>> {
        self.0.clone()
    }
}

/// What a hook did.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// Its answer; `null` when it gave none, or failed.
    pub value: Value,
    /// The state it left; the state it got when it failed.
    pub state: Value,
    /// What it wrote with `ctx.log`.
    pub logs: Vec<String>,
    /// Why it failed, if it did: its error, a time out, a cancel.
    pub error: Option<String>,
    /// Whether `error` is one the hook raised itself (`error()`, or a
    /// Lua error), rather than a time out, a cancel or its store's.
    pub raised: bool,
    pub wall: Duration,
}

/// A plugin read from its folder and checked: what it declares, and the
/// modules its hooks run from.
#[derive(Clone)]
pub struct Loaded {
    pub files: Files,
    /// [`Files::digest`].
    pub digest: String,
    pub declaration: Declaration,
    pub(crate) modules: Arc<Modules>,
}

impl std::fmt::Debug for Loaded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Loaded")
            .field("digest", &self.digest)
            .field("declaration", &self.declaration)
            .finish()
    }
}

/// Reads what `files` declare, in a fresh VM, and checks it: the
/// plugin's name is its folder's, its tools have names a model can call.
pub async fn load(folder: &str, files: Files) -> Result<Loaded, String> {
    if files.manifest.name != folder {
        return Err(format!(
            "{MANIFEST}: the package is named `{}`, but its folder is `{folder}`",
            files.manifest.name
        ));
    }
    let modules = Arc::new(files.modules()?);
    let entry = files.entry().to_owned();
    let script = "local heiko = require(\"heiko\")\n\
                  local p = require(\"plugin\")\n\
                  return heiko._describe(p)\n";
    let value = evaluate(
        folder,
        modules.clone(),
        script.into(),
        Duration::from_secs(2),
        CancellationToken::new(),
    )
    .await?;
    let mut declaration: Declaration = serde_json::from_value(value)
        .map_err(|error| format!("{entry}: {error}"))?;
    declaration.name.clone_from(&files.manifest.name);
    declaration
        .description
        .clone_from(&files.manifest.description);
    // A nav's label is its name unless it says.
    if let Some(nav) = &mut declaration.nav
        && nav.label.is_empty()
    {
        nav.label.clone_from(&declaration.name);
    }
    if let Some(nav) = &mut declaration.repo_nav
        && nav.label.is_empty()
    {
        nav.label.clone_from(&declaration.name);
    }
    for slot in &declaration.fills {
        let missing = match slot.as_str() {
            s if s == heiko_ui_plugin::title::TITLE.name() => {
                (!declaration.hooks.title).then_some("a `title` hook")
            }
            s if s == heiko_ui_plugin::forge::FORGE.name() => match &declaration
                .forge
            {
                None => Some("a `forge` table"),
                Some(forge)
                    if [&forge.name, &forge.id, &forge.host, &forge.token]
                        .iter()
                        .any(|field| field.is_empty()) =>
                {
                    Some("its `forge` table's `name`, `id`, `host` and `token`")
                }
                Some(forge)
                    if !declaration.uses.secrets.contains(&forge.token) =>
                {
                    Some("its `token` named in `uses.secrets`")
                }
                Some(_) => None,
            },
            s if s == heiko_ui_plugin::notify::NOTIFY.name() => {
                (!declaration.hooks.notify).then_some("a `notify` hook")
            }
            s if s == heiko_ui_plugin::compaction::COMPACTION.name() => {
                declaration
                    .compaction
                    .is_none()
                    .then_some("a `compaction` table")
            }
            _ => {
                return Err(format!(
                    "{entry}: `fills` names `{slot}`; a plugin may fill {}",
                    heiko_luau_plugins::SLOTS.join(", ")
                ));
            }
        };
        if let Some(missing) = missing {
            return Err(format!(
                "{entry}: it fills `{slot}`, which needs {missing}"
            ));
        }
    }
    if let Some(nav) = &declaration.repo_nav
        && !declaration.pages.contains(&nav.page)
    {
        return Err(format!(
            "{entry}: `repo_nav` opens the page `{}`, which `pages` lacks",
            nav.page
        ));
    }
    if let Some(job) = declaration
        .jobs
        .iter()
        .find(|job| job.every < heiko_luau_plugins::SHORTEST_EVERY)
    {
        return Err(format!(
            "{entry}: job `{}` runs `every` {} seconds; the shortest is {}",
            job.name,
            job.every,
            heiko_luau_plugins::SHORTEST_EVERY
        ));
    }
    if let Some(unknown) = declaration
        .events
        .iter()
        .find(|event| !heiko_luau_plugins::EVENTS.contains(&event.as_str()))
    {
        return Err(format!(
            "{entry}: `events` names `{unknown}`, which is not one of {}",
            heiko_luau_plugins::EVENTS.join(", ")
        ));
    }
    for tool in &declaration.tools {
        let valid = tool
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
            && tool
                .name
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic())
            && tool.name.len() <= 64;
        if !valid {
            return Err(format!(
                "{entry}: `{}` is not a tool name: letters, digits and \
                 `_`, starting with a letter",
                tool.name
            ));
        }
    }
    Ok(Loaded {
        digest: files.digest(),
        files,
        declaration,
        modules,
    })
}

impl Loaded {
    /// Calls `hook` with `input` and `context`, reaching only what
    /// `reach` gives and the plugin's `uses` allows. A hook the plugin
    /// does not have answers `null` and leaves the state.
    pub async fn call(
        &self,
        hook: &Hook,
        input: Value,
        context: &Context,
        reach: Arc<dyn Reach>,
        cancel: CancellationToken,
    ) -> Outcome {
        if !hook.declared_in(&self.declaration) {
            return Outcome {
                value: Value::Null,
                state: context.state.clone(),
                logs: Vec::new(),
                error: None,
                raised: false,
                wall: Duration::ZERO,
            };
        }
        // A hook alone (ADR 0056): an actor made from the context's state,
        // taking the hook's steps, what it awaits carried out on `reach`,
        // and what it leaves of the model given back.
        let started = std::time::Instant::now();
        let failed = |error: String| Outcome {
            value: Value::Null,
            state: context.state.clone(),
            logs: Vec::new(),
            raised: !(error.contains("it ran past its")
                || error.contains("it was cancelled")),
            error: Some(error),
            wall: started.elapsed(),
        };
        let model = match &context.state {
            Value::Null => json!({}),
            state => state.clone(),
        };
        let actor = match crate::actor::Actor::new(
            &self.modules,
            self.files.root.module,
            &model,
        ) {
            Ok(actor) => actor,
            Err(error) => return failed(error),
        };
        let logs = Arc::new(crate::mailbox::KeepLogs::default());
        let mailbox = crate::mailbox::Mailbox::new(actor, logs.clone());
        let env = json!({
            "run": context.run,
            "now": context.now,
            "settings": context.settings,
            "models": plan_models(),
        });
        let limit = hook.limit_for(&self.declaration);
        if hook.draws() {
            let input = (!matches!(hook, Hook::View)).then_some(&input);
            return match mailbox.draw(&hook.path(), input, &env, limit).await {
                Ok(value) => Outcome {
                    value,
                    state: context.state.clone(),
                    logs: Vec::new(),
                    error: None,
                    raised: false,
                    wall: started.elapsed(),
                },
                Err(error) => failed(error),
            };
        }
        let executor = Executor::new(self, context, reach);
        let (ended, kept) = mailbox
            .call_kept(hook.path(), input, env, &executor, limit, &cancel, None)
            .await;
        let logs = logs.take();
        match ended {
            crate::actor::Ended::Value { value } => Outcome {
                value,
                state: match kept {
                    Some(state) => state,
                    None => match mailbox.model().await {
                        Ok(state) => state,
                        Err(error) => return failed(error),
                    },
                },
                logs,
                error: None,
                raised: false,
                wall: started.elapsed(),
            },
            crate::actor::Ended::Error { error } => Outcome {
                logs,
                ..failed(error)
            },
        }
    }
}

/// The models of the person's plan, newest of each family, as
/// `ctx.models` lists them: `{ id, family, efforts }`.
pub fn plan_models() -> Value {
    Value::Array(
        heiko_ai::model::plan_models()
            .into_iter()
            .map(|model| {
                json!({
                    "id": model.id,
                    "family": heiko_ai::model::family_version(&model.id)
                        .map(|(family, _)| family),
                    "efforts": model.efforts,
                })
            })
            .collect(),
    )
}

/// How long `rewrite_context` may run when it reaches the model or the decider.
pub const REWRITE_LIMIT: Duration = Duration::from_secs(600);

/// How long a test file may run.
const TEST_LIMIT: Duration = Duration::from_secs(10);

impl Loaded {
    /// Runs the plugin's tests, each file in a fresh VM against fake
    /// runs, reaching nothing. A file that fails outside its cases is one
    /// failed result, named after the file.
    pub async fn test(&self, cancel: CancellationToken) -> Vec<TestResult> {
        self.tested(cancel, &Progress::default()).await.results
    }

    /// [`Self::test`], with the fixtures its files made (ADR 0053),
    /// telling `progress` how it goes.
    pub async fn tested(
        &self,
        cancel: CancellationToken,
        progress: &Progress,
    ) -> Tested {
        run_all(
            &self.declaration.name,
            self.modules.clone(),
            &self.files,
            cancel,
            progress,
        )
        .await
    }
}

/// How a package's tests go as they run, for a runner to show.
#[derive(Debug, Clone, Copy)]
pub enum Step<'a> {
    /// A test began: a file's cases, named after the file, or a
    /// property, machine or loop; a property says the cases it asks for.
    Began {
        file: &'a str,
        name: &'a str,
        kind: TestKind,
        cases: Option<u64>,
    },
    /// The property `name` of `file` began a case: the cases begun, and
    /// the case its first failure was found in. Several go on at once.
    Case {
        file: &'a str,
        name: &'a str,
        ran: u64,
        failed_at: Option<u64>,
    },
    /// A test ended.
    Ended(&'a TestResult),
}

/// Where [`Step`]s go: nowhere, unless made [`Progress::to`] a runner.
#[derive(Clone, Default)]
pub struct Progress(Option<Arc<Tell>>);

/// What a runner does with a [`Step`].
type Tell = dyn Fn(Step<'_>) + Send + Sync;

impl Progress {
    pub fn to(tell: impl Fn(Step<'_>) + Send + Sync + 'static) -> Self {
        Self(Some(Arc::new(tell)))
    }

    pub fn tell(&self, step: Step<'_>) {
        if let Some(tell) = &self.0 {
            tell(step);
        }
    }

    /// Tells that `result` ended, and gives it back.
    fn ended(&self, result: TestResult) -> TestResult {
        self.tell(Step::Ended(&result));
        result
    }
}

/// What a package's tests came to: each case's and property's result,
/// a fixture that failed to be made among them, and the fixtures made.
#[derive(Debug, Clone, Default)]
pub struct Tested {
    pub results: Vec<TestResult>,
    pub fixtures: Vec<Made>,
}

/// A fixture a test file made with `t.fixture`.
#[derive(Debug, Clone, PartialEq)]
pub struct Made {
    /// The file under `tests/` that made it, without `.luau`.
    pub file: String,
    pub name: String,
    pub value: Value,
}

/// Runs `files`' tests against `modules`, each file in a fresh VM,
/// reaching nothing; `name` names the folder. A file that fails outside
/// its cases is one failed result, named after the file.
pub async fn run_tests(
    name: &str,
    modules: Arc<Modules>,
    files: &Files,
    cancel: CancellationToken,
) -> Vec<TestResult> {
    run_all(name, modules, files, cancel, &Progress::default())
        .await
        .results
}

/// [`run_tests`], with the fixtures the files made, telling `progress`
/// how it goes.
///
/// Its tests run in the [`crate::pool`], at once: each file's cases
/// first, which register what else runs, then each property and loop,
/// the costliest first. Results come in the order the files declare
/// them, as when they ran one by one, and a property's seed is its own,
/// so what they come to does not depend on the order they ran in.
pub async fn run_all(
    name: &str,
    modules: Arc<Modules>,
    files: &Files,
    cancel: CancellationToken,
    progress: &Progress,
) -> Tested {
    use futures_util::future::{BoxFuture, join_all};

    let Root {
        module, subject, ..
    } = files.root;
    let digest = files.digest();
    let shared = Arc::new(files.clone());
    // Each file's cases, all at once: a file runs as a function, from the
    // script's first line, so its errors keep their line numbers.
    let ran = join_all(files.tests.iter().map(|(file, source)| {
        let script = format!(
            "local __test = function() {source}\nend\n\
             require(\"heiko_test\")\n\
             require(\"heiko\").test.{subject} = require(\"{module}\")\n\
             __test()\n\
             return {{ cases = require(\"heiko\")._run_tests(), \
             properties = require(\"heiko\")._properties(), \
             fixtures = require(\"heiko\")._run_fixtures(), \
             loops = require(\"heiko\")._loops() }}\n"
        );
        let (modules, cancel, progress) =
            (modules.clone(), cancel.clone(), progress.clone());
        let (name, file) = (name.to_owned(), file.clone());
        // What every other test waits on goes first.
        crate::pool::run(u64::MAX, move || async move {
            let whole = format!("tests/{file}.luau");
            progress.tell(Step::Began {
                file: &file,
                name: &whole,
                kind: TestKind::Case,
                cases: None,
            });
            run(
                modules,
                Arc::new(NoReach),
                Reaches::none(&name),
                script,
                Vec::new(),
                TEST_LIMIT,
                cancel,
            )
            .await
        })
    }))
    .await;
    // What each file came to, in order: its cases' results, then the
    // properties and loops it registered, each a job of the pool.
    enum Slot {
        Done(TestResult),
        Pending(usize),
    }
    let mut slots = Vec::new();
    let mut pending: Vec<BoxFuture<'static, TestResult>> = Vec::new();
    let mut fixtures: Vec<Made> = Vec::new();
    for ((file, source), (value, error)) in files.tests.iter().zip(ran) {
        let whole = format!("tests/{file}.luau");
        if let Some(failed) = error {
            slots.push(Slot::Done(progress.ended(TestResult {
                file: file.clone(),
                name: whole,
                passed: false,
                error: Some(failed.why),
                kind: TestKind::File,
                ..TestResult::default()
            })));
            continue;
        }
        for case in value["cases"].as_array().into_iter().flatten() {
            slots.push(Slot::Done(progress.ended(TestResult {
                file: file.clone(),
                name: case["name"].as_str().unwrap_or_default().to_owned(),
                passed: case["passed"] == true,
                error: case["error"].as_str().map(str::to_owned),
                millis: case["millis"].as_u64(),
                kind: TestKind::Case,
                ..TestResult::default()
            })));
        }
        for fixture in value["fixtures"].as_array().into_iter().flatten() {
            let name = fixture["name"].as_str().unwrap_or_default().to_owned();
            let error = match fixture["error"].as_str() {
                Some(error) => Some(error.to_owned()),
                None if fixtures.iter().any(|made| made.name == name) => {
                    Some(format!("the fixture {name} is made twice"))
                }
                None => None,
            };
            match error {
                Some(error) => {
                    slots.push(Slot::Done(progress.ended(TestResult {
                        file: file.clone(),
                        name: format!("fixture {name}"),
                        passed: false,
                        error: Some(error),
                        kind: TestKind::Fixture,
                        ..TestResult::default()
                    })))
                }
                None => fixtures.push(Made {
                    file: file.clone(),
                    name,
                    value: fixture["value"].clone(),
                }),
            }
        }
        for property in value["properties"].as_array().into_iter().flatten() {
            let property = Property {
                name: property["name"].as_str().unwrap_or_default().to_owned(),
                cases: property["cases"].as_u64(),
                reproduce: property["reproduce"].as_str().map(str::to_owned),
                kind: if property["machine"] == true {
                    TestKind::Machine
                } else {
                    TestKind::Property
                },
            };
            // What it took the last time, or the cases it asks for, a
            // machine's each a run of steps.
            let asks = property.cases.unwrap_or(CASES)
                * if property.kind == TestKind::Machine {
                    10
                } else {
                    1
                };
            let cost =
                crate::pool::took(name, file, &property.name).unwrap_or(asks);
            let source = PropertySource {
                name: name.to_owned(),
                modules: modules.clone(),
                digest: digest.clone(),
                file: file.clone(),
                script: property_script(source, module, subject),
            };
            let (cancel, progress) = (cancel.clone(), progress.clone());
            slots.push(Slot::Pending(pending.len()));
            pending.push(Box::pin(async move {
                let telling = progress.clone();
                let result = crate::pool::run(cost, move || async move {
                    run_property(&source, property, &cancel, &telling).await
                })
                .await;
                progress.ended(result)
            }));
        }
        for each in value["loops"].as_array().into_iter().flatten() {
            let loop_name =
                each["name"].as_str().unwrap_or_default().to_owned();
            let cost = crate::pool::took(name, file, &loop_name).unwrap_or(300);
            let script = loop_script(source, module, subject);
            let (modules, shared, cancel, progress) = (
                modules.clone(),
                shared.clone(),
                cancel.clone(),
                progress.clone(),
            );
            let (name, file, spec) =
                (name.to_owned(), file.clone(), each["spec"].clone());
            slots.push(Slot::Pending(pending.len()));
            pending.push(Box::pin(async move {
                let telling = progress.clone();
                let result = crate::pool::run(cost, move || async move {
                    telling.tell(Step::Began {
                        file: &file,
                        name: &loop_name,
                        kind: TestKind::Loop,
                        cases: None,
                    });
                    let began = std::time::Instant::now();
                    let mut result = run_loop(
                        &name, &modules, &shared, &file, &loop_name, &spec,
                        script, &cancel,
                    )
                    .await;
                    result.millis = Some(millis(began.elapsed()));
                    result
                })
                .await;
                progress.ended(result)
            }));
        }
    }
    let mut done: Vec<Option<TestResult>> =
        join_all(pending).await.into_iter().map(Some).collect();
    let results = slots
        .into_iter()
        .map(|slot| match slot {
            Slot::Done(result) => result,
            Slot::Pending(at) => done[at].take().expect("each job ends once"),
        })
        .collect();
    Tested { results, fixtures }
}

/// The cases a property runs, unless it asks for others, and the most
/// it may ask for (ADR 0049).
const CASES: u64 = 100;
const MOST_CASES: u64 = 1000;

/// How long a property may go on finding and shrinking its failures.
const PROPERTY_LIMIT: Duration = Duration::from_secs(30);

/// A property a test file registered: `t.property(name, check, opts)`.
struct Property {
    name: String,
    cases: Option<u64>,
    reproduce: Option<String>,
    /// A property, or a machine registered as one.
    kind: TestKind,
}

/// `elapsed` in whole milliseconds.
fn millis(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
}

/// Where a property comes from: its plugin, the version's digest, and
/// the script each of its cases runs.
struct PropertySource {
    name: String,
    modules: Arc<Modules>,
    digest: String,
    file: String,
    script: String,
}

/// The script a loop's check runs: the file, registering its loops,
/// then the check of the loop `__loop` names on `__run`.
fn loop_script(source: &str, module: &str, subject: &str) -> String {
    format!(
        "local __test = function() {source}\nend\n\
         require(\"heiko_test\")\n\
         require(\"heiko\").test.{subject} = require(\"{module}\")\n\
         __test()\n\
         return require(\"heiko\")._check_loop(__loop, __run)\n"
    )
}

/// Runs the loop `loop_name` of `file`: its `spec` in a real agent loop,
/// then its check, in a fresh VM, on what the run came to.
#[allow(clippy::too_many_arguments, reason = "one loop of one file")]
async fn run_loop(
    name: &str,
    modules: &Arc<Modules>,
    files: &Files,
    file: &str,
    loop_name: &str,
    spec: &Value,
    script: String,
    cancel: &CancellationToken,
) -> TestResult {
    let result = |error: Option<String>| TestResult {
        file: file.to_owned(),
        name: loop_name.to_owned(),
        passed: error.is_none(),
        error,
        kind: TestKind::Loop,
        ..TestResult::default()
    };
    let ran = match crate::looping::run(files, spec).await {
        Ok(ran) => ran,
        Err(error) => {
            return result(Some(format!("the loop did not run: {error}")));
        }
    };
    let (value, failed) = run(
        modules.clone(),
        Arc::new(NoReach),
        Reaches::none(name),
        script,
        vec![
            ("__loop".to_owned(), Value::String(loop_name.to_owned())),
            ("__run".to_owned(), ran),
        ],
        TEST_LIMIT,
        cancel.clone(),
    )
    .await;
    if let Some(failed) = failed {
        return result(Some(failed.why));
    }
    result(match value["passed"].as_bool() {
        Some(true) => None,
        _ => Some(value["error"].as_str().unwrap_or("it failed").to_owned()),
    })
}

/// The script a property runs: the file, registering its cases and
/// properties, then every case of the property `__property` names.
fn property_script(source: &str, module: &str, subject: &str) -> String {
    format!(
        "local __test = function() {source}\nend\n\
         require(\"heiko_test\")\n\
         require(\"heiko\").test.{subject} = require(\"{module}\")\n\
         __test()\n\
         return require(\"heiko\")._run_cases(__property)\n"
    )
}

/// The seed a property runs from: its version's, file's and name's, so
/// a version gives the same result each time heiko tries it.
fn seed(digest: &str, file: &str, name: &str) -> u64 {
    let hash = Sha256::new()
        .chain_update(digest)
        .chain_update([0])
        .chain_update(file)
        .chain_update([0])
        .chain_update(name)
        .finalize();
    u64::from_le_bytes(hash[..8].try_into().expect("eight bytes"))
}

/// Runs one property on hegel's engine, every case it offers in one VM,
/// and gives its one result: passed, or its failure shrunk, with what
/// reproduces it.
async fn run_property(
    source: &PropertySource,
    property: Property,
    cancel: &CancellationToken,
    progress: &Progress,
) -> TestResult {
    use crate::hegel::{Driver, Run, Settings};
    let began = std::time::Instant::now();
    let result = |error: Option<String>| TestResult {
        file: source.file.clone(),
        name: property.name.clone(),
        passed: error.is_none(),
        error,
        kind: property.kind,
        millis: Some(millis(began.elapsed())),
        ..TestResult::default()
    };
    let settings = Settings {
        cases: property.cases.unwrap_or(CASES).clamp(1, MOST_CASES),
        seed: seed(&source.digest, &source.file, &property.name),
    };
    progress.tell(Step::Began {
        file: &source.file,
        name: &property.name,
        kind: property.kind,
        cases: Some(settings.cases),
    });
    let run = match Run::start(&settings, property.reproduce.as_deref()) {
        Ok(run) => run,
        Err(error) => {
            return result(Some(format!("hegel could not start: {error}")));
        }
    };
    let telling = progress.clone();
    let (file, name) = (source.file.clone(), property.name.clone());
    let on_case = Box::new(move |ran, failed_at| {
        telling.tell(Step::Case {
            file: &file,
            name: &name,
            ran,
            failed_at,
        });
    });
    let driver = Arc::new(std::sync::Mutex::new(Driver::telling(
        run,
        PROPERTY_LIMIT,
        on_case,
    )));
    let mut reaches = Reaches::none(&source.name);
    reaches.hegel = Some(driver.clone());
    let (_, failed) = self::run(
        source.modules.clone(),
        Arc::new(NoReach),
        reaches,
        source.script.clone(),
        vec![(
            "__property".to_owned(),
            Value::String(property.name.clone()),
        )],
        PROPERTY_LIMIT + TEST_LIMIT,
        cancel.clone(),
    )
    .await;
    let mut driver = driver.lock().expect("not poisoned");
    let (ran, failed_at) = driver.counts();
    TestResult {
        cases: Some(failed_at.unwrap_or(ran)),
        shrinks: failed_at.map(|at| ran - at),
        ..result(driver.failure(failed.map(|failed| failed.why)))
    }
}

/// `text` as a Luau long string, at a level `text` cannot close, nor
/// its last `]` with the closing bracket.
pub fn long_string(text: &str) -> String {
    let closed = format!("{text}]");
    let mut level = 0;
    while closed.contains(&format!("]{}]", "=".repeat(level))) {
        level += 1;
    }
    let equals = "=".repeat(level);
    format!("[{equals}[{text}]{equals}]")
}

/// Runs `script` against `modules`, reaching nothing, and gives back
/// the value it returned, or why it failed; `name` names the folder.
pub async fn evaluate(
    name: &str,
    modules: Arc<Modules>,
    script: String,
    limit: Duration,
    cancel: CancellationToken,
) -> Result<Value, String> {
    let (value, error) = run(
        modules,
        Arc::new(NoReach),
        Reaches::none(name),
        script,
        Vec::new(),
        limit,
        cancel,
    )
    .await;
    match error {
        Some(failed) => Err(failed.why),
        None => Ok(value),
    }
}

/// What a script may reach of what [`Reach`] gives: what a plugin's
/// `uses` names.
struct Reaches {
    /// The folder's name, for the call's id.
    name: String,
    tools: Vec<String>,
    infer: bool,
    decide: bool,
    network: Option<Network>,
    store: Option<SessionRef>,
    /// The run's conversation (`uses.transcript`).
    transcript: bool,
    /// Files of what it cuts (`uses.archive`).
    archive: bool,
    /// Folders it reads (`uses.files`).
    files: bool,
    /// The property case its draws come from, while a property runs
    /// (ADR 0049).
    hegel: Option<crate::hegel::Shared>,
    /// A job's every store, and the chats it wakes.
    job: Option<Arc<JobReaches>>,
}

/// The HTTP requests a plugin may send (`uses.network`): to these hosts,
/// naming these secrets from its own store.
#[derive(Clone)]
struct Network {
    hosts: Vec<String>,
    secrets: Vec<String>,
    store: Option<heiko_tools_host::secrets::SecretStore>,
}

impl Reaches {
    fn of(
        declaration: &Declaration,
        secrets: Option<&std::path::Path>,
    ) -> Self {
        let uses = &declaration.uses;
        Self {
            name: declaration.name.clone(),
            tools: uses.tools.clone(),
            infer: uses.infer,
            decide: uses.decide,
            network: (!uses.network.is_empty()).then(|| Network {
                hosts: uses.network.clone(),
                secrets: uses.secrets.clone(),
                store: secrets.map(heiko_tools_host::secrets::SecretStore::new),
            }),
            store: None,
            transcript: uses.transcript,
            archive: uses.archive,
            files: !uses.files.is_empty(),
            hegel: None,
            job: None,
        }
    }

    fn none(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            tools: Vec::new(),
            infer: false,
            decide: false,
            network: None,
            store: None,
            transcript: false,
            archive: false,
            files: false,
            hegel: None,
            job: None,
        }
    }
}

/// Why a script failed.
struct Failed {
    why: String,
}

/// Runs `script` (a test file, a property, a loop), with `inputs` as its
/// globals, and gives back the table it returned, or why it failed.
async fn run(
    modules: Arc<Modules>,
    reach: Arc<dyn Reach>,
    reaches: Reaches,
    script: String,
    inputs: Vec<(String, Value)>,
    limit: Duration,
    cancel: CancellationToken,
) -> (Value, Option<Failed>) {
    let host = Arc::new(PluginHost {
        modules,
        reach,
        tools: reaches.tools,
        infer: reaches.infer,
        decide: reaches.decide,
        network: reaches.network,
        store: reaches.store,
        transcript: reaches.transcript,
        archive: reaches.archive,
        files: reaches.files,
        hegel: reaches.hegel,
        job: reaches.job,
    });
    let request = Request {
        call_id: format!("plugin:{}", reaches.name),
        source: Source {
            options: Options {
                max_output_tokens: None,
                timeout_ms: Some(limit.as_millis() as u64),
            },
            code: script,
        },
        store: Default::default(),
        cancel,
        inputs,
    };
    let outcome = heiko_codemode_host::run(host, request).await;
    if let Some(failure) = outcome.failure {
        let why = failure_text(&failure);
        return (Value::Null, Some(Failed { why }));
    }
    let returned = outcome.items.iter().rev().find_map(|item| match item {
        Item::Json(text) => serde_json::from_str(text).ok(),
        _ => None,
    });
    match returned {
        Some(value) => (value, None),
        None => (
            Value::Null,
            Some(Failed {
                why: "the script returned nothing".into(),
            }),
        ),
    }
}

fn failure_text(failure: &Failure) -> String {
    match failure {
        Failure::Error(error) => error.clone(),
        Failure::TimedOut { timeout_ms } => {
            format!("it ran past its {timeout_ms} ms")
        }
        Failure::Cancelled => "it was cancelled".into(),
        Failure::Sandbox(error) => {
            format!("the sandbox could not start: {error}")
        }
    }
}

/// The codemode host a hook runs against: the plugin's modules, and what
/// it may reach.
struct PluginHost {
    modules: Arc<Modules>,
    reach: Arc<dyn Reach>,
    /// The tools `uses` names.
    tools: Vec<String>,
    infer: bool,
    decide: bool,
    network: Option<Network>,
    store: Option<SessionRef>,
    /// Whether it reads the run's conversation (`uses.transcript`).
    transcript: bool,
    /// Whether it keeps files of what it cuts (`uses.archive`).
    archive: bool,
    /// Whether its tools read folders it declares (`uses.files`).
    files: bool,
    /// The case a property's draws come from, while one runs.
    hegel: Option<crate::hegel::Shared>,
    job: Option<Arc<JobReaches>>,
}

impl PluginHost {
    /// Sends the HTTP request `args` names, when it goes to a host the
    /// plugin declares and names only the secrets it declares.
    async fn request(
        &self,
        network: &Network,
        args: &Value,
    ) -> Result<ToolReply, String> {
        use heiko_tools_host::http;
        // `keep` is the host's, not the request's (gap H).
        let mut sent = args.clone();
        let keep_spec = sent
            .as_object_mut()
            .and_then(|fields| fields.remove("keep"))
            .unwrap_or(Value::Null);
        let args = &sent;
        let request = http::Request::parse(args)?;
        let host = request.url.host_str().unwrap_or_default();
        if !network.hosts.iter().any(|allowed| allowed == host) {
            return Err(format!(
                "this plugin does not reach `{host}`: add it to `uses.network`"
            ));
        }
        let names = http::secret_names(args);
        if let Some(name) =
            names.iter().find(|name| !network.secrets.contains(name))
        {
            return Err(format!(
                "this plugin does not use the secret `{name}`: add it to `uses.secrets`"
            ));
        }
        let empty;
        let store = match &network.store {
            Some(store) => store,
            None if names.is_empty() => {
                empty = heiko_tools_host::secrets::SecretStore::new(
                    std::path::PathBuf::from("/nonexistent/heiko-secrets.json"),
                );
                &empty
            }
            None => return Err("this host keeps no secrets".into()),
        };
        // `keep`: a field of the answer saved as a secret, never seen
        // (ADR 0050, gap H).
        let keep = match &keep_spec {
            Value::Null => None,
            keep => {
                let secret = keep["secret"]
                    .as_str()
                    .ok_or("`keep` names its `secret`")?;
                let from = keep["from"]
                    .as_str()
                    .ok_or("`keep` names the field it is `from`")?;
                if !network.secrets.iter().any(|name| name == secret) {
                    return Err(format!(
                        "this plugin does not use the secret `{secret}`: add it to `uses.secrets`"
                    ));
                }
                Some((secret.to_owned(), from.to_owned()))
            }
        };
        if keep.is_some() && network.store.is_none() {
            return Err("this host keeps no secrets".into());
        }
        let mut fetched = http::fetch(
            &http::client(),
            store,
            args,
            &CancellationToken::new(),
        )
        .await?;
        let mut kept = false;
        if let Some((secret, from)) = keep {
            let value = fetched
                .json
                .as_mut()
                .and_then(Value::as_object_mut)
                .and_then(|fields| fields.remove(&from));
            if let Some(Value::String(value)) = value.filter(|value| {
                value.as_str().is_some_and(|text| !text.is_empty())
            }) {
                store.set(&secret, &value).await.map_err(|error| {
                    format!("cannot keep `{secret}`: {error}")
                })?;
                let hidden = format!("[secret {secret}]");
                fetched.body = fetched.body.replace(&value, &hidden);
                for header in fetched.headers.values_mut() {
                    *header = header.replace(&value, &hidden);
                }
                kept = true;
            }
        }
        Ok(ToolReply::success(json!({
            "status": fetched.status,
            "headers": fetched.headers,
            "body": fetched.body,
            "json": fetched.json,
            "truncated": fetched.cut,
            "kept": kept,
        })))
    }
}

impl PluginHost {
    fn allowed(&self, name: &str) -> bool {
        self.tools.iter().any(|tool| tool == name)
            || (self.infer && matches!(name, "infer" | "infer_turn"))
            || (self.transcript && name == "transcript")
            || (self.archive && matches!(name, "archive_path" | "archive_write"))
            || (self.files && crate::files::Files::handles(name))
            // Sizes only, where the hook holds the run's conversation.
            || name == "measure"
            // Only the notify slot's reach offers it (ADR 0050).
            || name == crate::desktop::NOTICE
    }
}

impl PluginHost {
    /// The command `op` with `args` (ADR 0056): the values it answers
    /// with. The decider answers with two, its answer and what it cost; every
    /// other op is a host call, answering with one.
    async fn command(
        &self,
        op: &str,
        args: &Value,
    ) -> Result<Vec<Value>, String> {
        if op == "decide" {
            return self.decide_command(args).await;
        }
        let call = ToolCall {
            id: format!("plugin:{op}"),
            name: op.to_owned(),
            args: args.clone(),
        };
        let reply = self.call_tool(call).await?;
        match reply.error {
            Some(error) => Err(error),
            None => Ok(vec![reply.value]),
        }
    }

    /// `decide` with `{ fn, request }`, `fn` one of `ask`, `predicate`, `choice`
    /// and `score`: the request shaped as the function asks, the usage
    /// charged, and the answer shaped back, with its cost.
    async fn decide_command(&self, args: &Value) -> Result<Vec<Value>, String> {
        let function = args["fn"].as_str().unwrap_or("ask");
        let Some(client) = self.reach.decide().filter(|_| self.decide) else {
            return Err("The decider is not available in this run".into());
        };
        let (request, shape) =
            heiko_codemode_host::decide::request(function, &args["request"])?;
        let response = client
            .ask(&request)
            .await
            .map_err(|error| error.to_string())?;
        let usage = response.usage();
        self.reach.charge(&usage);
        let answer = heiko_codemode_host::decide::answer(&shape, &response)
            .map_err(|error| error.to_string())?;
        Ok(vec![answer, json!(usage.cost.total)])
    }
}

#[async_trait]
impl Host for PluginHost {
    fn tools(&self) -> Vec<ToolEntry> {
        let mut tools: Vec<ToolEntry> = self
            .reach
            .tools()
            .into_iter()
            .filter(|entry| self.allowed(&entry.name))
            .collect();
        // What makes `http.request` there: the plugin's own requests.
        if self.store.is_some() {
            for (name, description) in [
                (
                    "store_get",
                    "The value of `key` in the plugin's store, or null.",
                ),
                ("store_put", "Sets `key` to `value` in the plugin's store."),
                ("store_delete", "Deletes `key` from the plugin's store."),
                (
                    "store_list",
                    "Every key starting with `prefix`, with its value.",
                ),
            ] {
                tools.push(ToolEntry {
                    name: name.into(),
                    description: description.into(),
                    input_schema: json!({ "type": "object" }),
                    output_schema: None,
                    namespace: None,
                    sequential: true,
                });
            }
        }
        let mut add = |name: &str, description: &str| {
            tools.push(ToolEntry {
                name: name.into(),
                description: description.into(),
                input_schema: json!({ "type": "object" }),
                output_schema: None,
                namespace: None,
                sequential: false,
            });
        };
        add(
            "time_local",
            "`at` as `YYYY-MM-DD HH:MM` in the host's local time.",
        );
        add(
            "time_parse",
            "A local `YYYY-MM-DD HH:MM`, in seconds since 1970.",
        );
        add("time_duration", "`90m`, `2h` or `3d`, in seconds.");
        if let Some(job) = &self.job {
            add(
                "repo_stores",
                "The repositories the plugin keeps a store in.",
            );
            for (name, description) in [
                (
                    "repo_store_get",
                    "The value of `key` in `repo`'s store, or null.",
                ),
                ("repo_store_put", "Sets `key` to `value` in `repo`'s store."),
                ("repo_store_delete", "Deletes `key` from `repo`'s store."),
                (
                    "repo_store_list",
                    "Every key of `repo`'s store starting with `prefix`.",
                ),
            ] {
                add(name, description);
            }
            if job.conversations.is_some() {
                add(
                    "chat_wake",
                    "Starts a turn of the chat `run` on heiko's `text`.",
                );
            }
        }
        if self.hegel.is_some() {
            tools.push(ToolEntry {
                name: "hegel".into(),
                description: "A draw from the property case going on.".into(),
                input_schema: json!({ "type": "object" }),
                output_schema: None,
                namespace: None,
                // Not codemode's sequential lock: a property's VM is one
                // thread, and the driver has its own. Under that lock a
                // long case stalled for good (ADR 0050, "As built").
                sequential: false,
            });
        }
        // Whether a secret is set, and emptying one (ADR 0050, gap H):
        // never its value.
        if self.network.as_ref().is_some_and(|network| {
            network.store.is_some() && !network.secrets.is_empty()
        }) {
            for (name, description) in [
                ("secret_has", "Whether the secret `name` is set."),
                ("secret_forget", "Empties the secret `name`."),
            ] {
                tools.push(ToolEntry {
                    name: name.into(),
                    description: description.into(),
                    input_schema: json!({ "type": "object" }),
                    output_schema: None,
                    namespace: None,
                    sequential: true,
                });
            }
        }
        if self.network.is_some() {
            tools.push(ToolEntry {
                name: "http".into(),
                description: "One HTTP request to a host `uses.network` names."
                    .into(),
                input_schema: json!({ "type": "object" }),
                output_schema: None,
                namespace: None,
                sequential: false,
            });
        }
        tools
    }

    async fn module(
        &self,
        name: &str,
        version: Option<&str>,
    ) -> Result<Option<Definition>, String> {
        Ok(self
            .modules
            .get(name)
            .filter(|module| {
                version.is_none_or(|version| module.version() == version)
            })
            .cloned())
    }

    async fn call_tool(&self, call: ToolCall) -> Result<ToolReply, String> {
        if let Some(store) = &self.store
            && call.name.starts_with("store_")
        {
            return store_call(store, &call).await;
        }
        match call.name.as_str() {
            "time_local" => {
                let at = call.args["at"].as_i64().ok_or("`at` is a number")?;
                return Ok(ToolReply::success(json!(heiko_clock::local(
                    &heiko_clock::SystemClock,
                    at
                ))));
            }
            "time_duration" => {
                let text = call.args["text"].as_str().unwrap_or_default();
                return Ok(ToolReply::success(json!(heiko_clock::duration(
                    text
                ))));
            }
            "time_parse" => {
                let text = call.args["text"].as_str().unwrap_or_default();
                return Ok(ToolReply::success(json!(heiko_clock::parse_local(
                    &heiko_clock::SystemClock,
                    text
                ))));
            }
            _ => {}
        }
        if let Some(job) = &self.job
            && (call.name.starts_with("repo_store") || call.name == "chat_wake")
        {
            return job.call(&call).await.map(ToolReply::success);
        }
        if let (Some(network), "http") = (&self.network, call.name.as_str()) {
            return self.request(network, &call.args).await;
        }
        if let (Some(network), "secret_has" | "secret_forget") =
            (&self.network, call.name.as_str())
        {
            let name = call.args["name"].as_str().unwrap_or_default();
            if !network.secrets.iter().any(|secret| secret == name) {
                return Err(format!(
                    "this plugin does not use the secret `{name}`: add it to `uses.secrets`"
                ));
            }
            let store =
                network.store.as_ref().ok_or("this host keeps no secrets")?;
            if call.name == "secret_forget" {
                store.remove(name).await.map_err(|error| {
                    format!("cannot forget `{name}`: {error}")
                })?;
                return Ok(ToolReply::success(Value::Null));
            }
            let set = store
                .read()
                .await
                .map_err(|error| error.to_string())?
                .get(name)
                .is_some_and(|value| !value.is_empty());
            return Ok(ToolReply::success(json!(set)));
        }
        if let (Some(driver), "hegel") = (&self.hegel, call.name.as_str()) {
            return crate::hegel::call(driver, &call.args)
                .await
                .map(ToolReply::success);
        }
        if !self.allowed(&call.name) {
            // The `uses` key that grants the command, not always a tool.
            let field = match call.name.as_str() {
                "infer" | "infer_turn" => "uses.infer",
                "transcript" => "uses.transcript",
                "archive_path" | "archive_write" => "uses.archive",
                "files_write" | "files_remove" | "files_rename"
                | "files_mkdir" => "uses.writes",
                name if crate::files::Files::handles(name) => "uses.files",
                _ => "uses.tools",
            };
            return Err(format!(
                "this plugin does not use `{}`: add it to `{field}`",
                call.name
            ));
        }
        self.reach.call(call).await
    }

    fn decide(&self) -> Option<Arc<dyn Decider>> {
        self.decide.then(|| self.reach.decide()).flatten()
    }

    fn charge(&self, usage: &Usage) {
        self.reach.charge(usage);
    }
}

/// A script's call to its store: `store_get { key }`, `store_put { key,
/// value }`, `store_delete { key }`, `store_list { prefix }`.
async fn store_call(
    store: &SessionRef,
    call: &ToolCall,
) -> Result<ToolReply, String> {
    let mut session = store.lock().await;
    let op = call.name.strip_prefix("store_").unwrap_or_default();
    store_op(&mut session, op, call)
        .await
        .map(ToolReply::success)
}

/// `op` (`get`, `put`, `delete` or `list`) on a session, as `call` asks.
async fn store_op(
    session: &mut crate::store::Session,
    op: &str,
    call: &ToolCall,
) -> Result<Value, String> {
    let text = |field: &str| -> Result<String, String> {
        call.args[field].as_str().map(str::to_owned).ok_or_else(|| {
            format!("`{}` needs its `{field}`, a string", call.name)
        })
    };
    let value = match op {
        "get" => session.get(&text("key")?).await?.unwrap_or(Value::Null),
        "put" => {
            session.put(&text("key")?, call.args["value"].clone());
            Value::Null
        }
        "delete" => {
            session.delete(&text("key")?);
            Value::Null
        }
        "list" => {
            let prefix = call.args["prefix"].as_str().unwrap_or_default();
            Value::Array(
                session
                    .list(prefix)
                    .await?
                    .into_iter()
                    .map(|(key, value)| json!({ "key": key, "value": value }))
                    .collect(),
            )
        }
        // Writes that go in together, only while every key it expects
        // still holds what it expects (ADR 0056): what a hook read, then
        // awaited, then decided on, is still so, or nothing is written.
        // A key it expects null is one the store does not have.
        "commit" => {
            let expected =
                call.args["expect"].as_object().cloned().unwrap_or_default();
            for (key, value) in &expected {
                let now = session.get(key).await?.unwrap_or(Value::Null);
                if now != *value {
                    return Ok(Value::Bool(false));
                }
            }
            for (key, value) in
                call.args["put"].as_object().into_iter().flatten()
            {
                session.put(key, value.clone());
            }
            for key in call.args["delete"].as_array().into_iter().flatten() {
                if let Some(key) = key.as_str() {
                    session.delete(key);
                }
            }
            Value::Bool(true)
        }
        other => return Err(format!("the store has no `{other}`")),
    };
    Ok(value)
}

/// A job's reach across repositories: its plugin's store in each, a
/// session opened as the job first names it, and the chats it wakes.
struct JobReaches {
    plugin: String,
    stores: Arc<crate::store::Stores>,
    conversations: Option<Arc<dyn heiko_ui_plugin::Conversations>>,
    sessions: tokio::sync::Mutex<
        std::collections::BTreeMap<String, crate::store::Session>,
    >,
}

impl JobReaches {
    async fn call(&self, call: &ToolCall) -> Result<Value, String> {
        match call.name.as_str() {
            "repo_stores" => {
                return Ok(json!(self.stores.repos_of(&self.plugin).await));
            }
            "chat_wake" => {
                let conversations = self
                    .conversations
                    .as_ref()
                    .ok_or("this host wakes no chats")?;
                let text = |field: &str| {
                    call.args[field].as_str().map(str::to_owned).ok_or_else(
                        || format!("`chat_wake` needs its `{field}`, a string"),
                    )
                };
                let (run, message) = (text("run")?, text("text")?);
                // What the job wrote is kept first: the turn it starts may
                // read the same stores.
                self.commit().await?;
                let waking = conversations
                    .wake(&run, message)
                    .await
                    .map_err(|error| format!("{error:#}"))?;
                return Ok(json!(match waking {
                    heiko_ui_plugin::Waking::Started => "started",
                    heiko_ui_plugin::Waking::Busy => "busy",
                    heiko_ui_plugin::Waking::Gone => "gone",
                }));
            }
            _ => {}
        }
        let repo = call.args["repo"]
            .as_str()
            .ok_or_else(|| format!("`{}` needs its `repo`", call.name))?
            .to_owned();
        let op = call.name.strip_prefix("repo_store_").unwrap_or_default();
        let mut sessions = self.sessions.lock().await;
        if !sessions.contains_key(&repo) {
            let kv = self.stores.of(&self.plugin, &repo).await?;
            sessions.insert(repo.clone(), kv.session().await);
        }
        let session = sessions.get_mut(&repo).expect("just opened");
        store_op(session, op, call).await
    }

    /// Keeps what the job wrote to each store, and lets go of them.
    async fn commit(&self) -> Result<(), String> {
        let sessions = std::mem::take(&mut *self.sessions.lock().await);
        for (_, session) in sessions {
            session.commit().await?;
        }
        Ok(())
    }
}

/// What a plugin's commands reach (ADR 0056): what its hooks reached
/// through `ctx`, now carried out by the host for one awaited command at
/// a time, or for the commands a step gave, in one turn with the store.
pub struct Executor {
    declaration: Declaration,
    modules: Arc<Modules>,
    reach: Arc<dyn Reach>,
    secrets: Option<std::path::PathBuf>,
    store: Option<StoreRef>,
    job: Option<JobRef>,
}

impl std::fmt::Debug for Executor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Executor")
            .field("plugin", &self.declaration.name)
            .finish_non_exhaustive()
    }
}

impl Executor {
    /// What `loaded`'s commands reach in `context`, through `reach`.
    pub fn new(
        loaded: &Loaded,
        context: &Context,
        reach: Arc<dyn Reach>,
    ) -> Self {
        Self {
            declaration: loaded.declaration.clone(),
            modules: loaded.modules.clone(),
            reach,
            secrets: context.secrets.clone(),
            store: context.store.clone(),
            job: context.job.clone(),
        }
    }

    /// A host for one turn, with the store's session for it.
    async fn host(
        &self,
    ) -> (Arc<PluginHost>, Option<SessionRef>, Option<Arc<JobReaches>>) {
        let session = match (&self.store, self.declaration.uses.store) {
            (Some(store), true) => {
                Some(Arc::new(tokio::sync::Mutex::new(store.0.session().await)))
            }
            _ => None,
        };
        let job = self
            .job
            .as_ref()
            .filter(|_| self.declaration.uses.chats)
            .map(|job| {
                Arc::new(JobReaches {
                    plugin: self.declaration.name.clone(),
                    stores: job.stores.clone(),
                    conversations: job.conversations.clone(),
                    sessions: Default::default(),
                })
            });
        let reaches = Reaches::of(&self.declaration, self.secrets.as_deref());
        let host = Arc::new(PluginHost {
            modules: self.modules.clone(),
            reach: self.reach.clone(),
            tools: reaches.tools,
            infer: reaches.infer,
            decide: reaches.decide,
            network: reaches.network,
            store: session.clone(),
            transcript: reaches.transcript,
            archive: reaches.archive,
            files: reaches.files,
            hegel: None,
            job: job.clone(),
        });
        (host, session, job)
    }

    /// What it writes to its stores goes in whole.
    async fn commit(
        session: Option<SessionRef>,
        job: Option<Arc<JobReaches>>,
    ) -> Result<(), String> {
        if let Some(job) = job {
            job.commit().await?;
        }
        if let Some(session) = session {
            match Arc::into_inner(session) {
                Some(session) => session.into_inner().commit().await?,
                None => return Err("the store was still in use".into()),
            }
        }
        Ok(())
    }

    /// Carries out `command` and gives its answer.
    pub async fn run(
        &self,
        command: &crate::actor::Command,
    ) -> crate::actor::Answer {
        if command.op == "all" {
            let commands: Vec<crate::actor::Command> =
                serde_json::from_value(command.args.clone())
                    .unwrap_or_default();
            let answers = futures_util::future::join_all(
                commands.iter().map(|command| Box::pin(self.run(command))),
            )
            .await;
            return crate::actor::Answer::of(json!(answers));
        }
        let (host, session, job) = self.host().await;
        let answer = execute(&host, command).await;
        drop(host);
        match (answer, Self::commit(session, job).await) {
            (Ok(values), Ok(())) => crate::actor::Answer::values(values),
            (Err(error), _) | (Ok(_), Err(error)) => {
                crate::actor::Answer::failed(error)
            }
        }
    }

    /// Carries out the commands a step gave, in order, in one turn with
    /// the store: they go in whole, or none does.
    pub async fn give(
        &self,
        commands: &[crate::actor::Command],
    ) -> Result<(), String> {
        if commands.is_empty() {
            return Ok(());
        }
        let (host, session, job) = self.host().await;
        for command in commands {
            execute(&host, command).await?;
        }
        drop(host);
        Self::commit(session, job).await
    }
}

/// One command on `host`: the values it answers with.
async fn execute(
    host: &Arc<PluginHost>,
    command: &crate::actor::Command,
) -> Result<Vec<Value>, String> {
    host.command(&command.op, &command.args).await
}
