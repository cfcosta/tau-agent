//! Reading a plugin's folder, loading it, and calling its hooks, each in
//! a fresh codemode VM with codemode's limits.
//!
//! Every call is a short script: it requires the `tau` module and the
//! plugin, builds `ctx` from what the host sends, calls the hook, and
//! returns the hook's answer with the state it left. The plugin's files
//! are codemode modules: `tau`, `plugin`, and one per file under `lib/`.
//!
//! An agent's repository (ADR 0037) is a folder of the same shape whose
//! [`Root`] is `agent.luau`: [`Files`], [`evaluate`] and [`run_tests`]
//! serve both.

use std::{collections::BTreeMap, path::Path, sync::Arc, time::Duration};

use async_trait::async_trait;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tau_ai::message::Usage;
use tau_codemode::{
    modules::Definition,
    result::{Failure, Item},
};
use tau_codemode_host::{
    Host,
    Options,
    Request,
    Source,
    ToolCall,
    ToolEntry,
    ToolReply,
};
use tau_jev::Jev;
use tokio_util::sync::CancellationToken;

use crate::{Declaration, TAU_MODULE, TestResult};

/// The file a folder starts from, the module it is required as, and
/// the field of `tau.test` its tests find it in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Root {
    pub file: &'static str,
    pub module: &'static str,
    pub subject: &'static str,
}

/// A plugin's folder: `plugin.luau`, required as `plugin`.
pub const PLUGIN: Root = Root {
    file: "plugin.luau",
    module: "plugin",
    subject: "_plugin",
};

impl Default for Root {
    fn default() -> Self {
        PLUGIN
    }
}

/// The file a plugin's folder starts from.
pub const PLUGIN_FILE: &str = PLUGIN.file;

/// A folder's modules, by name: what its scripts run against.
pub type Modules = BTreeMap<String, Definition>;

/// What a plugin's folder holds, or an agent's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Files {
    /// Which file it starts from.
    pub root: Root,
    /// That file: `plugin.luau`.
    pub source: String,
    /// `lib/<name>.luau`, by name.
    pub libs: BTreeMap<String, String>,
    /// `tests/<name>.luau`, by name.
    pub tests: BTreeMap<String, String>,
    pub readme: Option<String>,
}

impl Files {
    /// Reads a folder starting from `root`. It blocks: call it from
    /// `spawn_blocking`.
    pub fn read(dir: &Path, root: Root) -> Result<Self, String> {
        let read = |path: &Path| {
            std::fs::read_to_string(path)
                .map_err(|error| format!("{}: {error}", path.display()))
        };
        let luau_files =
            |sub: &str| -> Result<BTreeMap<String, String>, String> {
                let dir = dir.join(sub);
                let mut found = BTreeMap::new();
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    return Ok(found);
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) != Some("luau")
                    {
                        continue;
                    }
                    let Some(stem) = path.file_stem().and_then(|s| s.to_str())
                    else {
                        continue;
                    };
                    found.insert(stem.to_owned(), read(&path)?);
                }
                Ok(found)
            };
        Ok(Self {
            root,
            source: read(&dir.join(root.file))?,
            libs: luau_files("lib")?,
            tests: luau_files("tests")?,
            readme: std::fs::read_to_string(dir.join("README.md")).ok(),
        })
    }

    /// A folder starting from `root` from its files at a commit, by
    /// their paths in the folder (`plugin.luau`, `lib/x.luau`): what
    /// trunk holds.
    pub fn from_paths(
        paths: &[(String, Vec<u8>)],
        root: Root,
    ) -> Result<Self, String> {
        let text = |path: &str, bytes: &[u8]| {
            String::from_utf8(bytes.to_vec())
                .map_err(|_| format!("{path} is not UTF-8"))
        };
        let mut files = Self {
            root,
            ..Self::default()
        };
        let mut has_root = false;
        for (path, bytes) in paths {
            let luau = |sub: &str| {
                path.strip_prefix(sub)
                    .and_then(|rest| rest.strip_suffix(".luau"))
                    .filter(|stem| !stem.contains('/'))
                    .map(str::to_owned)
            };
            if path == root.file {
                files.source = text(path, bytes)?;
                has_root = true;
            } else if path == "README.md" {
                files.readme = Some(text(path, bytes)?);
            } else if let Some(stem) = luau("lib/") {
                files.libs.insert(stem, text(path, bytes)?);
            } else if let Some(stem) = luau("tests/") {
                files.tests.insert(stem, text(path, bytes)?);
            }
        }
        if !has_root {
            return Err(format!("the folder has no {}", root.file));
        }
        Ok(files)
    }

    /// The digest of everything in the folder: what names a version.
    pub fn digest(&self) -> String {
        let mut hasher = Sha256::new();
        let mut add = |path: &str, text: &str| {
            hasher.update(path.as_bytes());
            hasher.update([0]);
            hasher.update(text.as_bytes());
            hasher.update([0]);
        };
        add(self.root.file, &self.source);
        for (name, text) in &self.libs {
            add(&format!("lib/{name}.luau"), text);
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

    /// The codemode modules the folder makes: `tau`, each `lib/` file
    /// (which may require `tau` and the other `lib/` files, but not in a
    /// cycle), and its root, `plugin` (which may require them all).
    pub fn modules(&self) -> Result<Modules, String> {
        let (file, module) = (self.root.file, self.root.module);
        let tau = Definition::new(
            "tau".into(),
            TAU_MODULE.into(),
            json!({}),
            BTreeMap::new(),
        )?;
        let mut modules = BTreeMap::new();
        let mut plugin_deps =
            BTreeMap::from([("tau".to_owned(), tau.version().to_owned())]);
        for name in self.libs.keys() {
            if name == "tau" || name == module {
                return Err(format!("lib/{name}.luau: `{name}` is taken"));
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
                    .map(|name| format!("lib/{name}.luau"))
                    .collect();
                return Err(format!(
                    "{} require each other: one of them cannot",
                    left.join(", ")
                ));
            };
            let mut deps =
                BTreeMap::from([("tau".to_owned(), tau.version().to_owned())]);
            for dep in required(source, &self.libs) {
                deps.insert(dep.clone(), versions[&dep].clone());
            }
            let lib =
                Definition::new(name.clone(), source.clone(), json!({}), deps)
                    .map_err(|error| format!("lib/{name}.luau: {error}"))?;
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
        modules.insert("tau".into(), tau);
        Ok(modules)
    }
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
    /// A run's title, for `tau.title`, with `{ task, model }`.
    Title,
    /// One of tau's notices, for `tau.notify`, with `{ title, body }`.
    Notify,
    /// One of the forge's hooks, for `tau.forge`, by name.
    Forge(String),
    /// One of its jobs, by name.
    Job(String),
    /// What it keeps of a repository (`repo_data`), with nothing as input.
    RepoData,
    /// One of its pages of a repository, by name, with the page's params.
    Page(String),
    /// tau started, with the runs it found cut off as `{ runs }`.
    Recover,
    RunEnd,
    View,
    /// Its settings page, with the settings shown as input (ADR 0029).
    SettingsView,
    /// A button's action, with its arguments as input.
    Action(String),
}

impl Hook {
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
    /// whose `uses` names `infer` or `jev` may call them, and has a
    /// tool's time for it (ADR 0050), `rewrite_context` ten minutes;
    /// `on_event` never does.
    pub fn limit_for(&self, declaration: &Declaration) -> Duration {
        // An action may ask Jev too, through the host's.
        if matches!(self, Self::Action(_)) && declaration.uses.jev {
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
        if !run_hook || !(uses.infer || uses.jev) {
            return self.limit();
        }
        // A compaction asks for many summaries, or a long one, while the
        // run waits for it as it waits for the loop's own.
        match self {
            Self::RewriteContext => REWRITE_LIMIT,
            _ => Self::Tool(String::new()).limit(),
        }
    }

    /// The Luau expression that calls it, with `p`, `input` and `ctx`.
    fn expression(&self) -> String {
        match self {
            Self::Tool(name) => format!("p.tools[\"{name}\"].call(input, ctx)"),
            Self::Card(name) => format!(
                "p.tools[\"{name}\"].card(input.call, input.result, ctx)"
            ),
            Self::Start => "p.start(input, ctx)".into(),
            Self::BeforeTool => "p.before_tool(input, ctx)".into(),
            Self::BeforeRequest => "p.before_request(input, ctx)".into(),
            Self::AfterToolResult => "p.after_tool_result(input, ctx)".into(),
            Self::RewriteContext => "p.rewrite_context(input, ctx)".into(),
            Self::Rewritten => "p.rewritten(input, ctx)".into(),
            Self::BeforeStop => "p.before_stop(input, ctx)".into(),
            Self::TurnEnd => "p.turn_end(input, ctx)".into(),
            Self::OnEvent => "p.on_event(input, ctx)".into(),
            Self::Title => "p.title(input, ctx)".into(),
            Self::Notify => "p.notify(input, ctx)".into(),
            Self::Job(name) => format!("p.jobs[\"{name}\"].run(ctx)"),
            Self::RepoData => "p.repo_data(ctx)".into(),
            Self::Recover => "p.recover(input, ctx)".into(),
            Self::Page(name) => format!("p.pages[\"{name}\"](input, ctx)"),
            Self::Forge(name) => format!("p.forge[\"{name}\"](input, ctx)"),
            Self::RunEnd => "p.run_end(input, ctx)".into(),
            Self::View => "p.view(ctx.state, ctx)".into(),
            Self::SettingsView => "p.settings.view(input, ctx)".into(),
            Self::Action(name) => format!("p.actions[\"{name}\"](input, ctx)"),
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
            Self::Recover => declaration.hooks.recover,
            Self::Page(name) => declaration.pages.contains(name),
            Self::Forge(name) => declaration
                .forge
                .as_ref()
                .is_some_and(|forge| forge.hooks.contains(name)),
            Self::RunEnd => declaration.hooks.run_end,
            Self::View => declaration.hooks.view,
            Self::SettingsView => declaration.hooks.settings_view,
            Self::Action(name) => declaration.actions.contains(name),
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

/// What a hook's `ctx` holds, besides what it may reach.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Context {
    /// `{ id, kind, repo, model, turn }`.
    pub run: Value,
    /// `{ unix, iso, weekday }`.
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
}

/// The file the plugin `plugin`'s secrets are kept in, in tau's
/// configuration directory `config`: apart from every agent's, and from
/// every other plugin's.
pub fn secrets_of(
    config: &std::path::Path,
    plugin: &str,
) -> std::path::PathBuf {
    config
        .join(tau_tools_host::secrets::AGENTS_DIR)
        .join("plugins")
        .join(format!("{plugin}.json"))
}

/// What a hook may reach beyond the VM: the tools its plugin's `uses`
/// names, Jev. A tool handler gets tools; other hooks, none.
#[async_trait]
pub trait Reach: Send + Sync {
    /// The tools there are to call; the host keeps those `uses` names.
    fn tools(&self) -> Vec<ToolEntry> {
        Vec::new()
    }

    async fn call(&self, call: ToolCall) -> Result<ToolReply, String> {
        Err(format!("`{}` cannot be called from this hook", call.name))
    }

    fn jev(&self) -> Option<Arc<dyn Jev>> {
        None
    }

    fn charge(&self, _usage: &Usage) {}
}

/// Nothing to reach: for every hook but a tool's handler, and tests.
pub struct NoReach;

impl Reach for NoReach {}

/// What an action reaches outside a run (ADR 0050): the host's Jev, for
/// a plugin whose `uses` names it.
pub struct ActionReach(pub Option<Arc<dyn Jev>>);

impl Reach for ActionReach {
    fn jev(&self) -> Option<Arc<dyn Jev>> {
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
    modules: Arc<Modules>,
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
    let modules = Arc::new(files.modules()?);
    let script = "local tau = require(\"tau\")\n\
                  local p = require(\"plugin\")\n\
                  return tau._describe(p)\n";
    let value = evaluate(
        folder,
        modules.clone(),
        script.into(),
        Duration::from_secs(2),
        CancellationToken::new(),
    )
    .await?;
    let declaration: Declaration = serde_json::from_value(value)
        .map_err(|error| format!("{PLUGIN_FILE}: {error}"))?;
    if declaration.name != folder {
        return Err(format!(
            "{PLUGIN_FILE}: the plugin is named `{}`, but its folder is `{folder}`",
            declaration.name
        ));
    }
    for slot in &declaration.fills {
        let missing = match slot.as_str() {
            s if s == tau_ui_plugin::title::TITLE.name() => {
                (!declaration.hooks.title).then_some("a `title` hook")
            }
            s if s == tau_ui_plugin::forge::FORGE.name() => match &declaration
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
            s if s == tau_ui_plugin::notify::NOTIFY.name() => {
                (!declaration.hooks.notify).then_some("a `notify` hook")
            }
            s if s == tau_ui_plugin::compaction::COMPACTION.name() => {
                declaration
                    .compaction
                    .is_none()
                    .then_some("a `compaction` table")
            }
            _ => {
                return Err(format!(
                    "{PLUGIN_FILE}: `fills` names `{slot}`; a plugin may fill {}",
                    tau_luau_plugins::SLOTS.join(", ")
                ));
            }
        };
        if let Some(missing) = missing {
            return Err(format!(
                "{PLUGIN_FILE}: it fills `{slot}`, which needs {missing}"
            ));
        }
    }
    if let Some(nav) = &declaration.repo_nav
        && !declaration.pages.contains(&nav.page)
    {
        return Err(format!(
            "{PLUGIN_FILE}: `repo_nav` opens the page `{}`, which `pages` lacks",
            nav.page
        ));
    }
    if let Some(job) = declaration
        .jobs
        .iter()
        .find(|job| job.every < tau_luau_plugins::SHORTEST_EVERY)
    {
        return Err(format!(
            "{PLUGIN_FILE}: job `{}` runs `every` {} seconds; the shortest is {}",
            job.name,
            job.every,
            tau_luau_plugins::SHORTEST_EVERY
        ));
    }
    if let Some(unknown) = declaration
        .events
        .iter()
        .find(|event| !tau_luau_plugins::EVENTS.contains(&event.as_str()))
    {
        return Err(format!(
            "{PLUGIN_FILE}: `events` names `{unknown}`, which is not one of {}",
            tau_luau_plugins::EVENTS.join(", ")
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
                "{PLUGIN_FILE}: `{}` is not a tool name: letters, digits and \
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
        // The input and the context are globals, given as values: never
        // source for the compiler, nor a text `json.decode` would bound,
        // however large a transcript is (ADR 0050, gap B).
        let script = format!(
            "local tau = require(\"tau\")\n\
             local p = require(\"plugin\")\n\
             local input = __input\n\
             local ctx = tau._ctx(__context)\n\
             local value = {call}\n\
             if value == nil then value = json.null end\n\
             return {{ value = value, state = ctx.state, logs = ctx._logs }}\n",
            call = hook.expression(),
        );
        let inputs = vec![
            ("__input".to_owned(), input),
            ("__context".to_owned(), {
                let mut context = json!(context);
                context["models"] = plan_models();
                context
            }),
        ];
        let started = std::time::Instant::now();
        // Its turn with the store, from here to the end of the hook.
        let session = match (&context.store, self.declaration.uses.store) {
            (Some(store), true) => {
                Some(Arc::new(tokio::sync::Mutex::new(store.0.session().await)))
            }
            _ => None,
        };
        let mut reaches =
            Reaches::of(&self.declaration, context.secrets.as_deref());
        reaches.store = session.clone();
        let (value, failed) = run(
            self.modules.clone(),
            reach,
            reaches,
            script,
            inputs,
            hook.limit_for(&self.declaration),
            cancel,
        )
        .await;
        let raised = failed.as_ref().is_some_and(|failed| failed.raised);
        let mut error = failed.map(|failed| without_caller(failed.why));
        // What it wrote goes in whole once it ends well, and not at all
        // when it fails.
        if let (Some(session), None) = (session, &error) {
            let committed = match Arc::into_inner(session) {
                Some(session) => session.into_inner().commit().await,
                None => Err("the hook's store was still in use".into()),
            };
            if let Err(why) = committed {
                error = Some(format!("its store: {why}"));
            }
        }
        let wall = started.elapsed();
        if let Some(error) = error {
            return Outcome {
                value: Value::Null,
                state: context.state.clone(),
                logs: Vec::new(),
                error: Some(error),
                raised,
                wall,
            };
        }
        let logs = value["logs"]
            .as_array()
            .map(|logs| {
                logs.iter()
                    .filter_map(|log| log.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        Outcome {
            value: value["value"].clone(),
            state: match &value["state"] {
                Value::Array(items) if items.is_empty() => json!({}),
                state => state.clone(),
            },
            logs,
            error: None,
            raised: false,
            wall,
        }
    }
}

/// The models of the person's plan, newest of each family, as
/// `ctx.models` lists them: `{ id, family, efforts }`.
pub fn plan_models() -> Value {
    Value::Array(
        tau_ai::model::plan_models()
            .into_iter()
            .map(|model| {
                json!({
                    "id": model.id,
                    "family": tau_ai::model::family_version(&model.id)
                        .map(|(family, _)| family),
                    "efforts": model.efforts,
                })
            })
            .collect(),
    )
}

/// How long `rewrite_context` may run when it reaches the model or Jev.
pub const REWRITE_LIMIT: Duration = Duration::from_secs(600);

/// How long a test file may run.
const TEST_LIMIT: Duration = Duration::from_secs(10);

impl Loaded {
    /// Runs the plugin's tests, each file in a fresh VM against fake
    /// runs, reaching nothing. A file that fails outside its cases is one
    /// failed result, named after the file.
    pub async fn test(&self, cancel: CancellationToken) -> Vec<TestResult> {
        run_tests(
            &self.declaration.name,
            self.modules.clone(),
            &self.files,
            cancel,
        )
        .await
    }
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
    let Root {
        module, subject, ..
    } = files.root;
    let digest = files.digest();
    let mut results = Vec::new();
    for (file, source) in &files.tests {
        // The file runs as a function, from the script's first line, so
        // its errors keep their line numbers.
        let script = format!(
            "local __test = function() {source}\nend\n\
             require(\"tau\").test.{subject} = require(\"{module}\")\n\
             __test()\n\
             return {{ cases = require(\"tau\")._run_tests(), \
             properties = require(\"tau\")._properties() }}\n"
        );
        let (value, error) = run(
            modules.clone(),
            Arc::new(NoReach),
            Reaches::none(name),
            script,
            Vec::new(),
            TEST_LIMIT,
            cancel.clone(),
        )
        .await;
        if let Some(failed) = error {
            results.push(TestResult {
                file: file.clone(),
                name: format!("tests/{file}.luau"),
                passed: false,
                error: Some(failed.why),
            });
            continue;
        }
        let cases: Vec<TestResult> = value["cases"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|case| TestResult {
                file: file.clone(),
                name: case["name"].as_str().unwrap_or_default().to_owned(),
                passed: case["passed"] == true,
                error: case["error"].as_str().map(str::to_owned),
            })
            .collect();
        results.extend(cases);
        for property in value["properties"].as_array().into_iter().flatten() {
            let property = Property {
                name: property["name"].as_str().unwrap_or_default().to_owned(),
                cases: property["cases"].as_u64(),
                reproduce: property["reproduce"].as_str().map(str::to_owned),
            };
            let source = PropertySource {
                name,
                modules: &modules,
                digest: &digest,
                file,
                script: property_script(source, module, subject),
            };
            results.push(run_property(&source, property, &cancel).await);
        }
    }
    results
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
}

/// Where a property comes from: its plugin, the version's digest, and
/// the script each of its cases runs.
struct PropertySource<'a> {
    name: &'a str,
    modules: &'a Arc<Modules>,
    digest: &'a str,
    file: &'a str,
    script: String,
}

/// The script a property runs: the file, registering its cases and
/// properties, then every case of the property `__property` names.
fn property_script(source: &str, module: &str, subject: &str) -> String {
    format!(
        "local __test = function() {source}\nend\n\
         require(\"tau\").test.{subject} = require(\"{module}\")\n\
         __test()\n\
         return require(\"tau\")._run_cases(__property)\n"
    )
}

/// The seed a property runs from: its version's, file's and name's, so
/// a version gives the same result each time tau tries it.
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
    source: &PropertySource<'_>,
    property: Property,
    cancel: &CancellationToken,
) -> TestResult {
    use crate::hegel::{Driver, Run, Settings};
    let result = |error: Option<String>| TestResult {
        file: source.file.to_owned(),
        name: property.name.clone(),
        passed: error.is_none(),
        error,
    };
    let settings = Settings {
        cases: property.cases.unwrap_or(CASES).clamp(1, MOST_CASES),
        seed: seed(source.digest, source.file, &property.name),
    };
    let run = match Run::start(&settings, property.reproduce.as_deref()) {
        Ok(run) => run,
        Err(error) => {
            return result(Some(format!("hegel could not start: {error}")));
        }
    };
    let driver =
        Arc::new(std::sync::Mutex::new(Driver::new(run, PROPERTY_LIMIT)));
    let mut reaches = Reaches::none(source.name);
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
    result(driver.failure(failed.map(|failed| failed.why)))
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
    jev: bool,
    network: Option<Network>,
    store: Option<SessionRef>,
    /// The run's conversation (`uses.transcript`).
    transcript: bool,
    /// The property case its draws come from, while a property runs
    /// (ADR 0049).
    hegel: Option<crate::hegel::Shared>,
}

/// The HTTP requests a plugin may send (`uses.network`): to these hosts,
/// naming these secrets from its own store.
#[derive(Clone)]
struct Network {
    hosts: Vec<String>,
    secrets: Vec<String>,
    store: Option<tau_tools_host::secrets::SecretStore>,
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
            jev: uses.jev,
            network: (!uses.network.is_empty()).then(|| Network {
                hosts: uses.network.clone(),
                secrets: uses.secrets.clone(),
                store: secrets.map(tau_tools_host::secrets::SecretStore::new),
            }),
            store: None,
            transcript: uses.transcript,
            hegel: None,
        }
    }

    fn none(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            tools: Vec::new(),
            infer: false,
            jev: false,
            network: None,
            store: None,
            transcript: false,
            hegel: None,
        }
    }
}

/// `why` without the line of the script that called the hook, which is
/// tau's and not the plugin's: `codemode:5: x is taken` is `x is taken`.
fn without_caller(why: String) -> String {
    let Some(rest) = why.strip_prefix("codemode:") else {
        return why;
    };
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    match rest[digits..].strip_prefix(": ") {
        Some(message) if digits > 0 => message.to_owned(),
        _ => why,
    }
}

/// Why a script failed, and whether it raised that itself: a Lua error,
/// not a time out, a cancel or a sandbox that did not start.
struct Failed {
    why: String,
    raised: bool,
}

/// Runs `script`, with `inputs` as its globals, and gives back
/// the table it returned, or why it failed.
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
        jev: reaches.jev,
        network: reaches.network,
        store: reaches.store,
        transcript: reaches.transcript,
        hegel: reaches.hegel,
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
    let outcome = tau_codemode_host::run(host, request).await;
    if let Some(failure) = outcome.failure {
        let raised = matches!(failure, Failure::Error(_));
        let why = failure_text(&failure);
        return (Value::Null, Some(Failed { why, raised }));
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
                why: "the hook returned nothing".into(),
                raised: false,
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
    jev: bool,
    network: Option<Network>,
    store: Option<SessionRef>,
    /// Whether it reads the run's conversation (`uses.transcript`).
    transcript: bool,
    /// The case a property's draws come from, while one runs.
    hegel: Option<crate::hegel::Shared>,
}

impl PluginHost {
    /// Sends the HTTP request `args` names, when it goes to a host the
    /// plugin declares and names only the secrets it declares.
    async fn request(
        &self,
        network: &Network,
        args: &Value,
    ) -> Result<ToolReply, String> {
        use tau_tools_host::http;
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
                empty = tau_tools_host::secrets::SecretStore::new(
                    std::path::PathBuf::from("/nonexistent/tau-secrets.json"),
                );
                &empty
            }
            None => return Err("this host keeps no secrets".into()),
        };
        let fetched = http::fetch(
            &http::client(),
            store,
            args,
            &CancellationToken::new(),
        )
        .await?;
        Ok(ToolReply::success(json!({
            "status": fetched.status,
            "headers": fetched.headers,
            "body": fetched.body,
            "json": fetched.json,
            "truncated": fetched.cut,
        })))
    }
}

impl PluginHost {
    fn allowed(&self, name: &str) -> bool {
        self.tools.iter().any(|tool| tool == name)
            || (self.infer && name == "infer")
            || (self.transcript && name == "transcript")
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
        if let (Some(network), "http") = (&self.network, call.name.as_str()) {
            return self.request(network, &call.args).await;
        }
        if let (Some(driver), "hegel") = (&self.hegel, call.name.as_str()) {
            return crate::hegel::call(driver, &call.args)
                .await
                .map(ToolReply::success);
        }
        if !self.allowed(&call.name) {
            return Err(format!(
                "this plugin does not use `{}`: add it to `uses.tools`",
                call.name
            ));
        }
        self.reach.call(call).await
    }

    fn jev(&self) -> Option<Arc<dyn Jev>> {
        self.jev.then(|| self.reach.jev()).flatten()
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
    let text = |field: &str| -> Result<String, String> {
        call.args[field].as_str().map(str::to_owned).ok_or_else(|| {
            format!("`{}` needs its `{field}`, a string", call.name)
        })
    };
    let mut session = store.lock().await;
    let value = match call.name.as_str() {
        "store_get" => session.get(&text("key")?).await?.unwrap_or(Value::Null),
        "store_put" => {
            session.put(&text("key")?, call.args["value"].clone());
            Value::Null
        }
        "store_delete" => {
            session.delete(&text("key")?);
            Value::Null
        }
        "store_list" => {
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
        other => return Err(format!("the store has no `{other}`")),
    };
    Ok(ToolReply::success(value))
}
