//! Reading a plugin's folder, loading it, and calling its hooks, each in
//! a fresh codemode VM with codemode's limits.
//!
//! Every call is a short script: it requires the `tau` module and the
//! plugin, builds `ctx` from what the host sends, calls the hook, and
//! returns the hook's answer with the state it left. The plugin's files
//! are codemode modules: `tau`, `plugin`, and one per file under `lib/`.

use std::{collections::BTreeMap, path::Path, sync::Arc, time::Duration};

use async_trait::async_trait;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tau_ai::message::Usage;
use tau_codemode::{
    Host,
    Options,
    Request,
    Source,
    ToolCall,
    ToolEntry,
    ToolReply,
    modules::Definition,
    result::{Failure, Item},
};
use tau_jev::Jev;
use tokio_util::sync::CancellationToken;

use crate::{Declaration, TAU_MODULE};

/// The file a plugin's folder starts from.
pub const PLUGIN_FILE: &str = "plugin.luau";

/// Module names a plugin's `lib/` files cannot take.
const RESERVED: [&str; 2] = ["tau", "plugin"];

/// What a plugin's folder holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Files {
    /// `plugin.luau`.
    pub plugin: String,
    /// `lib/<name>.luau`, by name.
    pub libs: BTreeMap<String, String>,
    /// `tests/<name>.luau`, by name.
    pub tests: BTreeMap<String, String>,
    pub readme: Option<String>,
}

impl Files {
    /// Reads a plugin's folder. It blocks: call it from
    /// `spawn_blocking`.
    pub fn read(dir: &Path) -> Result<Self, String> {
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
            plugin: read(&dir.join(PLUGIN_FILE))?,
            libs: luau_files("lib")?,
            tests: luau_files("tests")?,
            readme: std::fs::read_to_string(dir.join("README.md")).ok(),
        })
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
        add(PLUGIN_FILE, &self.plugin);
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
    /// (which may require `tau`), and `plugin` (which may require them
    /// all).
    fn modules(&self) -> Result<BTreeMap<String, Definition>, String> {
        let tau = Definition::new(
            "tau".into(),
            TAU_MODULE.into(),
            json!({}),
            BTreeMap::new(),
        )?;
        let mut modules = BTreeMap::new();
        let mut plugin_deps =
            BTreeMap::from([("tau".to_owned(), tau.version().to_owned())]);
        for (name, source) in &self.libs {
            if RESERVED.contains(&name.as_str()) {
                return Err(format!("lib/{name}.luau: `{name}` is taken"));
            }
            let lib = Definition::new(
                name.clone(),
                source.clone(),
                json!({}),
                BTreeMap::from([("tau".to_owned(), tau.version().to_owned())]),
            )
            .map_err(|error| format!("lib/{name}.luau: {error}"))?;
            plugin_deps.insert(name.clone(), lib.version().to_owned());
            modules.insert(name.clone(), lib);
        }
        let plugin = Definition::new(
            "plugin".into(),
            self.plugin.clone(),
            json!({}),
            plugin_deps,
        )
        .map_err(|error| format!("{PLUGIN_FILE}: {error}"))?;
        modules.insert("plugin".into(), plugin);
        modules.insert("tau".into(), tau);
        Ok(modules)
    }
}

/// A hook of a plugin, as the host calls it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hook {
    /// A tool's handler, with its arguments as input.
    Tool(String),
    /// A tool's card, with `{ call, result }` as input.
    Card(String),
    BeforeTool,
    BeforeStop,
    TurnEnd,
    RunEnd,
    View,
    /// A button's action, with its arguments as input.
    Action(String),
}

impl Hook {
    /// How long it may run (ADR 0027).
    pub fn limit(&self) -> Duration {
        Duration::from_millis(match self {
            Self::Tool(_) => 60_000,
            Self::Card(_) | Self::View => 200,
            Self::BeforeTool | Self::TurnEnd | Self::Action(_) => 2_000,
            Self::BeforeStop | Self::RunEnd => 10_000,
        })
    }

    /// The Luau expression that calls it, with `p`, `input` and `ctx`.
    fn expression(&self) -> String {
        match self {
            Self::Tool(name) => format!("p.tools[\"{name}\"].call(input, ctx)"),
            Self::Card(name) => format!(
                "p.tools[\"{name}\"].card(input.call, input.result, ctx)"
            ),
            Self::BeforeTool => "p.before_tool(input, ctx)".into(),
            Self::BeforeStop => "p.before_stop(input, ctx)".into(),
            Self::TurnEnd => "p.turn_end(input, ctx)".into(),
            Self::RunEnd => "p.run_end(input, ctx)".into(),
            Self::View => "p.view(ctx.state, ctx)".into(),
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
            Self::BeforeTool => declaration.hooks.before_tool,
            Self::BeforeStop => declaration.hooks.before_stop,
            Self::TurnEnd => declaration.hooks.turn_end,
            Self::RunEnd => declaration.hooks.run_end,
            Self::View => declaration.hooks.view,
            Self::Action(name) => declaration.actions.contains(name),
        }
    }
}

/// What a hook's `ctx` holds, besides what it may reach.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Context {
    /// `{ id, kind, repo, model, turn }`.
    pub run: Value,
    /// `{ unix, iso, weekday }`.
    pub now: Value,
    pub settings: Value,
    pub state: Value,
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
    modules: Arc<BTreeMap<String, Definition>>,
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
    let (value, error) = run(
        modules.clone(),
        Arc::new(NoReach),
        &Declaration::empty(folder),
        script.into(),
        Duration::from_secs(2),
        CancellationToken::new(),
    )
    .await;
    if let Some(error) = error {
        return Err(error);
    }
    let declaration: Declaration = serde_json::from_value(value)
        .map_err(|error| format!("{PLUGIN_FILE}: {error}"))?;
    if declaration.name != folder {
        return Err(format!(
            "{PLUGIN_FILE}: the plugin is named `{}`, but its folder is `{folder}`",
            declaration.name
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

impl Declaration {
    /// A declaration that reaches nothing, for reading one.
    fn empty(name: &str) -> Self {
        Self {
            name: name.into(),
            description: String::new(),
            uses: Default::default(),
            settings: None,
            tools: Vec::new(),
            hooks: Default::default(),
            actions: Vec::new(),
        }
    }
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
                wall: Duration::ZERO,
            };
        }
        let script = format!(
            "local tau = require(\"tau\")\n\
             local p = require(\"plugin\")\n\
             local input = json.decode({input})\n\
             local ctx = tau._ctx(json.decode({context}))\n\
             local value = {call}\n\
             if value == nil then value = json.null end\n\
             return {{ value = value, state = ctx.state, logs = ctx._logs }}\n",
            input = long_string(&input.to_string()),
            context = long_string(&json!(context).to_string()),
            call = hook.expression(),
        );
        let started = std::time::Instant::now();
        let (value, error) = run(
            self.modules.clone(),
            reach,
            &self.declaration,
            script,
            hook.limit(),
            cancel,
        )
        .await;
        let wall = started.elapsed();
        if let Some(error) = error {
            return Outcome {
                value: Value::Null,
                state: context.state.clone(),
                logs: Vec::new(),
                error: Some(error),
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
            wall,
        }
    }
}

/// How long a test file may run.
const TEST_LIMIT: Duration = Duration::from_secs(10);

/// One case of a plugin's tests, as it came out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct TestResult {
    /// Its file under `tests/`, without `.luau`.
    pub file: String,
    pub name: String,
    pub passed: bool,
    #[serde(default)]
    pub error: Option<String>,
}

impl Loaded {
    /// Runs the plugin's tests, each file in a fresh VM against fake
    /// runs, reaching nothing. A file that fails outside its cases is one
    /// failed result, named after the file.
    pub async fn test(&self, cancel: CancellationToken) -> Vec<TestResult> {
        let mut results = Vec::new();
        for (file, source) in &self.files.tests {
            // The file runs as a function, from the script's first line,
            // so its errors keep their line numbers.
            let script = format!(
                "local __test = function() {source}\nend\n\
                 require(\"tau\").test._plugin = require(\"plugin\")\n\
                 __test()\n\
                 return require(\"tau\")._run_tests()\n"
            );
            let (value, error) = run(
                self.modules.clone(),
                Arc::new(NoReach),
                &self.declaration,
                script,
                TEST_LIMIT,
                cancel.clone(),
            )
            .await;
            if let Some(error) = error {
                results.push(TestResult {
                    file: file.clone(),
                    name: format!("tests/{file}.luau"),
                    passed: false,
                    error: Some(error),
                });
                continue;
            }
            let cases: Vec<TestResult> = value
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
        }
        results
    }
}

/// `text` as a Luau long string, at a level `text` cannot close.
fn long_string(text: &str) -> String {
    let mut level = 0;
    while text.contains(&format!("]{}]", "=".repeat(level))) {
        level += 1;
    }
    let equals = "=".repeat(level);
    format!("[{equals}[{text}]{equals}]")
}

/// Runs `script` and gives back the table it returned, or why it
/// failed.
async fn run(
    modules: Arc<BTreeMap<String, Definition>>,
    reach: Arc<dyn Reach>,
    declaration: &Declaration,
    script: String,
    limit: Duration,
    cancel: CancellationToken,
) -> (Value, Option<String>) {
    let host = Arc::new(PluginHost {
        modules,
        reach,
        tools: declaration.uses.tools.clone(),
        infer: declaration.uses.infer,
        jev: declaration.uses.jev,
    });
    let request = Request {
        call_id: format!("plugin:{}", declaration.name),
        source: Source {
            options: Options {
                max_output_tokens: None,
                timeout_ms: Some(limit.as_millis() as u64),
            },
            code: script,
        },
        store: Default::default(),
        cancel,
    };
    let outcome = tau_codemode::run(host, request).await;
    if let Some(failure) = outcome.failure {
        return (Value::Null, Some(failure_text(&failure)));
    }
    let returned = outcome.items.iter().rev().find_map(|item| match item {
        Item::Json(text) => serde_json::from_str(text).ok(),
        _ => None,
    });
    match returned {
        Some(value) => (value, None),
        None => (Value::Null, Some("the hook returned nothing".into())),
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
    modules: Arc<BTreeMap<String, Definition>>,
    reach: Arc<dyn Reach>,
    /// The tools `uses` names.
    tools: Vec<String>,
    infer: bool,
    jev: bool,
}

impl PluginHost {
    fn allowed(&self, name: &str) -> bool {
        self.tools.iter().any(|tool| tool == name)
            || (self.infer && name == "infer")
    }
}

#[async_trait]
impl Host for PluginHost {
    fn tools(&self) -> Vec<ToolEntry> {
        self.reach
            .tools()
            .into_iter()
            .filter(|entry| self.allowed(&entry.name))
            .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_string_cannot_be_closed_by_its_text() {
        assert_eq!(long_string("a"), "[[a]]");
        assert_eq!(long_string("x]]y"), "[=[x]]y]=]");
        assert_eq!(long_string("]=]]]"), "[==[]=]]]]==]");
    }
}
