//! `t.loop` (ADR 0053): a package's plugin in a real agent loop, from its
//! tests. The test gives the run as data, its `spec`; the host runs it
//! with a scripted model, the fake tools and Jev the spec names, the
//! run's workspace and blobs in a folder of its own, an in-memory store
//! and real time, and gives back what the run came to for the test's
//! check, with the workspace as written and the blobs its state names.
//!
//! The spec:
//!
//! - `prompt`: the task; `steer`: a message, or a list, steered in as it
//!   starts; `model`: the turns, over the run and its steps, each a part
//!   or a list of them, a part `{ text }`, `{ thinking }`,
//!   `{ tool, args }`, `{ usage = { input, output } }`, `{ cost = usd }`,
//!   `{ delay = ms }`, `{ error = { code, message } }`,
//!   `{ failure = { message, class } }` (`retryable`, `context_overflow`
//!   or `fatal`) or `{ stop = "length" }`; `name`: the run's model, `m`
//!   unless given.
//! - `settings`: over the plugin's defaults; `kind` and `repo` of the
//!   run; `reasoning` (`"high"` …), `clock` (the time every message is
//!   stamped with) and `retry = { max_attempts }` of the agent.
//! - `plugins`: others of tau's own after this one, each
//!   `{ name, settings }`.
//! - `files`: the workspace, a path to its text, `{ base64 }` or
//!   `{ png = { width, height } }`; `skills`: the person's skills folder,
//!   as the host has it, written the same way.
//! - `tools`: fake tools beside the plugin's, a name to what it answers:
//!   a text, or a list taken in order, the last kept; `{ error }` fails.
//!   In a text, `{name}` is the call's argument `name`, and `{x*name}` is
//!   `x` that many times.
//! - `jev`: a fake Jev, `{ error = 503 }` failing every request, or rules
//!   answering each question by the first that matches: `id` a prefix of
//!   the question's id, `has` a text in what the request's state holds
//!   of it (the object there whose `id` is the question's), then `noul`,
//!   `choice` (with `probabilities`) or `score`.
//! - `data` and `repository`: what tau's data folder and its folder for
//!   the run's repository hold as the run starts (`uses.files`' `data`
//!   and `repository`), written as `files` is; `commits`: the commit a
//!   turn leaves, `{ turn, change, paths }` each, which `turn_end` is
//!   given (ADR 0050, gap L).
//! - `then`: steps after the run, each with its own `settings` and `jev`:
//!   a fork from where the run before it ended, with its `prompt` and
//!   `steer`; `{ resume = true, prompt }`, the same run going on; or
//!   `{ idle = true }`, the finished run compacted as it waits, from its
//!   last request.
//!
//! What a run came to: `results` (each call's `text` and `error`),
//! `outcome` (`run`, `text`, `stop`, `usage`, `request`: the last
//! request's settings), `error`, `events`, `state` (the plugin's last),
//! `rewrites` (the context's, by plugin), `logs`, `charged` (what the
//! plugins were charged), `record` (the store's: `status`, `turns`,
//! tokens, `cost_usd`), `transcript` (each entry: a `message` with its
//! `role`, a `context` rewrite with its `plugin`, `body` and `trigger`,
//! or a `plugin` record) and `archive` (each archived file's `text` and
//! `mode`). An idle step gives `compacted` (`plugin`, `tokens_before`,
//! `tokens_after`), `before` and `record`, `transcript`, `rewrites`,
//! `state` and `logs`. The first run's also has `forks` (each step's),
//! `requests` (each request's `settings`, its fields, and its
//! `messages`), `most_at_once` (the most responses going on at once),
//! `jev` (what Jev was asked), `workspace` and `blobs`, and `data` and
//! `repository`: each file of those folders as the loop left them, its
//! path to its text.

use std::{
    collections::BTreeMap,
    io::Cursor,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use base64::Engine as _;
use futures_util::StreamExt;
use serde_json::{Value, json};
use tau_agent::{
    agent::Agent,
    error::ToolError,
    event::RunEvent,
    tool::{AgentTool, ToolCtx, ToolOutput},
};
use tau_ai::message::StopReason;
use tau_jev::{Answer, JevError, fake::FakeJev};
use tau_scripted::{ScriptedModel, TurnBuilder};

use crate::{
    agent::{Active, LuauPlugins},
    runtime::{Files, load},
};

/// The most a loop runs: real time, model turns and every hook.
pub const LOOP_LIMIT: Duration = Duration::from_secs(60);

/// Runs `spec` with the package `files`, loaded anew: what the run came to,
/// for the test's check.
pub async fn run(files: &Files, spec: &Value) -> Result<Value, String> {
    let name = files.manifest.name.clone();
    let loaded = load(&name, files.clone())
        .await
        .map_err(|error| format!("{name} does not load: {error}"))?;
    let mut others = Vec::new();
    for other in spec["plugins"].as_array().into_iter().flatten() {
        let other_name = other["name"]
            .as_str()
            .ok_or("a plugin of `plugins` names one of tau's own")?;
        let files = crate::built_in::files(other_name)?;
        let loaded = load(other_name, files)
            .await
            .map_err(|error| format!("{other_name} does not load: {error}"))?;
        others.push((loaded, other["settings"].clone()));
    }
    let dir = tempfile::tempdir().map_err(|error| error.to_string())?;
    let written = workspace(&dir.path().join("work"), &spec["files"])?;
    if spec["skills"].is_object() {
        workspace(&dir.path().join("skills"), &spec["skills"])?;
    }
    workspace(&dir.path().join("data"), &spec["data"])?;
    workspace(&dir.path().join("repository"), &spec["repository"])?;
    let llm = model(&spec["model"])?;
    let looped = Looped {
        name: name.clone(),
        loaded,
        others,
        dir: dir.path().to_owned(),
        counted: Counted::new(llm.clone()),
        llm,
        jev: spec.get("jev").map(Rules::new).transpose()?,
        repo: spec["repo"].as_str().unwrap_or("test").to_owned(),
        spec: spec.clone(),
    };
    let ran = tokio::spawn(async move {
        let store = tau_store_sqlite::memory()
            .await
            .map_err(|error| error.to_string())?;
        looped.all(&store).await
    });
    let mut came = tokio::time::timeout(LOOP_LIMIT, ran)
        .await
        .map_err(|_| {
            format!("the loop ran past its {} s", LOOP_LIMIT.as_secs())
        })?
        .map_err(|error| error.to_string())??;
    came["workspace"] = written;
    let repo = came["repo"].as_str().unwrap_or("test").to_owned();
    let blobs =
        crate::files::blobs_dir(&dir.path().join("blobs"), &name, &repo);
    came["blobs"] = read_back(&blobs, &came["state"]).await;
    came["data"] = tree(&dir.path().join("data"));
    came["repository"] = tree(&dir.path().join("repository"));
    Ok(came)
}

/// One loop's run and the steps that follow it, on one scripted model,
/// one fake Jev and one folder.
struct Looped {
    name: String,
    loaded: crate::runtime::Loaded,
    /// Other plugins of tau's own, after this one, with their settings.
    others: Vec<(crate::runtime::Loaded, Value)>,
    dir: std::path::PathBuf,
    llm: ScriptedModel,
    /// `llm`, counting the responses going on at once.
    counted: Counted,
    jev: Option<Rules>,
    repo: String,
    spec: Value,
}

/// Where the last run stands, for the step after it.
struct Last {
    run: Option<tau_agent::tool::RunId>,
    checkpoint: Option<tau_agent::agent::Checkpoint>,
    request: Option<tau_ai::responses::request::Settings>,
}

impl Looped {
    /// The run, then each step of `then`: a fork from where the one before
    /// it ended (`fork`, the default), the same run going on (`resume`),
    /// or the finished run compacted as it waits (`idle`). The first's
    /// account, with the steps' under `forks`, and what the model and Jev
    /// were asked over all of them.
    async fn all(self, store: &tau_store::Store) -> Result<Value, String> {
        let agent = self.agent(&self.spec["settings"]);
        let run = agent.start(self.prompt(&self.spec), store);
        steer(&run, &self.spec["steer"]);
        let (mut came, mut last) = self.account(run, store).await;
        let mut forks = Vec::new();
        for step in self.spec["then"].as_array().into_iter().flatten() {
            if let (Some(jev), Some(rules)) = (&self.jev, step.get("jev")) {
                jev.replace(rules)?;
            }
            let settings = if step["settings"].is_object() {
                &step["settings"]
            } else {
                &self.spec["settings"]
            };
            let agent = self.agent(settings);
            let Some(id) = last.run.clone() else {
                return Err("a step follows a run that did not finish".into());
            };
            if step["idle"] == true {
                let request = last
                    .request
                    .clone()
                    .ok_or("an idle step follows a run that did not finish")?;
                let before = record(store, &id.0).await;
                let compacted = agent
                    .resume(&id)
                    .compact_idle(
                        request,
                        store,
                        None,
                        tokio_util::sync::CancellationToken::new(),
                    )
                    .await;
                let mut idle = json!({
                    "compacted": match &compacted {
                        Ok(Some(done)) => json!({
                            "plugin": &*done.plugin,
                            "tokens_before": done.tokens_before,
                            "tokens_after": done.tokens_after,
                        }),
                        _ => Value::Null,
                    },
                    "error": compacted.err().map(|error| error.to_string()),
                    "before": before,
                    "record": record(store, &id.0).await,
                    "transcript": transcript(store, &id.0).await,
                });
                stored(&mut idle, &self.name, store, &id.0).await;
                forks.push(idle);
                continue;
            }
            let run = if step["resume"] == true {
                agent.resume(&id).start(self.prompt(step), store)
            } else {
                let from = last
                    .checkpoint
                    .clone()
                    .ok_or("a fork follows a run that did not finish")?;
                agent.fork(&from).start(self.prompt(step), store)
            };
            steer(&run, &step["steer"]);
            let (fork, next) = self.account(run, store).await;
            forks.push(fork);
            last = next;
        }
        came["forks"] = Value::Array(forks);
        came["requests"] = requests(&self.llm);
        came["most_at_once"] = json!(self.counted.most());
        came["jev"] = self.jev.as_ref().map_or(json!([]), Rules::asked);
        came["repo"] = json!(self.repo);
        Ok(came)
    }

    fn prompt(&self, spec: &Value) -> String {
        spec["prompt"].as_str().unwrap_or("Go.").to_owned()
    }

    /// The agent: the plugin with `settings` over its defaults, and the
    /// other plugins after it; the fake tools, Jev, the spec's reasoning,
    /// clock and retries, each a millisecond apart.
    fn agent(&self, given: &Value) -> Agent {
        let over = |loaded: &crate::runtime::Loaded, given: &Value| {
            let mut settings = loaded.declaration.default_settings();
            for (key, value) in given.as_object().into_iter().flatten() {
                settings[key] = value.clone();
            }
            Active {
                loaded: loaded.clone(),
                settings,
            }
        };
        let mut active = vec![over(&self.loaded, given)];
        active.extend(
            self.others
                .iter()
                .map(|(loaded, given)| over(loaded, given)),
        );
        let run = json!({
            "kind": self.spec["kind"].as_str().unwrap_or("main"),
            "repo": self.repo,
            "model": self.spec["name"].as_str().unwrap_or("m"),
        });
        let plugins = LuauPlugins::new(
            active,
            run,
            self.jev
                .as_ref()
                .map(|jev| Arc::new(jev.fake.clone()) as Arc<dyn tau_jev::Jev>),
        )
        .with_files(Some(self.dir.join("work")), self.dir.join("blobs"))
        .with_skills(
            self.spec["skills"]
                .is_object()
                .then(|| self.dir.join("skills")),
        )
        .with_archive(self.dir.join("archive"))
        .with_folders(
            Some(self.dir.join("repository")),
            Some(self.dir.join("data")),
        );
        let retry = &self.spec["retry"];
        // The turns' commits, told before the plugins hear a turn end, as
        // the run's workspace tells them.
        let commits = Commits {
            plugins: plugins.clone(),
            commits: self.spec["commits"].clone(),
        };
        let agent = Agent::new(self.counted.clone()).plugin(commits);
        let mut agent =
            agent.plugin(plugins).retry(tau_ai::retry::RetryPolicy {
                base: Duration::from_millis(1),
                max_attempts: retry["max_attempts"].as_u64().map_or(
                    tau_ai::retry::RetryPolicy::default().max_attempts,
                    |n| n as u32,
                ),
                ..tau_ai::retry::RetryPolicy::default()
            });
        if let Ok(effort) =
            serde_json::from_value(self.spec["reasoning"].clone())
        {
            agent = agent.reasoning(effort);
        }
        if let Some(at) = self.spec["clock"].as_u64() {
            agent = agent.clock(Arc::new(move || at));
        }
        for (tool, answers) in
            self.spec["tools"].as_object().into_iter().flatten()
        {
            agent = agent.tool(Fake::new(tool, answers));
        }
        agent
    }

    /// What `run` came to, read from its events and the store, and where
    /// the step after it starts.
    async fn account(
        &self,
        mut run: tau_agent::agent::Run,
        store: &tau_store::Store,
    ) -> (Value, Last) {
        let events: Vec<RunEvent> = run.events().collect().await;
        let (outcome, last) = match run.outcome().await {
            Ok(outcome) => (
                json!({
                    "run": outcome.run.0.to_string(),
                    "text": outcome.text,
                    "stop": outcome.stop,
                    "usage": outcome.usage,
                    "request": settings(&outcome.request),
                }),
                Last {
                    run: Some(outcome.run.clone()),
                    checkpoint: Some(outcome.checkpoint()),
                    request: Some(outcome.request.clone()),
                },
            ),
            Err(error) => (
                json!({ "error": error.to_string() }),
                Last {
                    run: None,
                    checkpoint: None,
                    request: None,
                },
            ),
        };
        let mut came = came_to(&self.name, &events);
        if let Some(id) = outcome["run"].as_str() {
            stored(&mut came, &self.name, store, id).await;
            came["record"] = record(store, id).await;
            came["transcript"] = transcript(store, id).await;
        }
        came["error"] = outcome.get("error").cloned().unwrap_or(Value::Null);
        came["outcome"] = outcome;
        came["archive"] = archive(&self.dir.join("archive"));
        (came, last)
    }
}

/// Steers `steer` into `run`: a message, or each of a list in order.
fn steer(run: &tau_agent::agent::Run, steer: &Value) {
    match steer {
        Value::String(text) => run.steer(text.as_str()),
        Value::Array(texts) => {
            for text in texts.iter().filter_map(Value::as_str) {
                run.steer(text);
            }
        }
        _ => {}
    }
}

/// The store's record of the run `id`: its status, turns, tokens and cost.
async fn record(store: &tau_store::Store, id: &str) -> Value {
    match store.run(id).await {
        Ok(Some(record)) => json!({
            "status": format!("{:?}", record.status).to_lowercase(),
            "turns": record.turns,
            "input_tokens": record.input_tokens,
            "output_tokens": record.output_tokens,
            "cost_usd": record.cost_usd,
        }),
        _ => Value::Null,
    }
}

/// Every entry the store keeps of the run `id`'s transcript, in order: a
/// message, or a rewrite with the plugin that stored it and its stats.
async fn transcript(store: &tau_store::Store, id: &str) -> Value {
    store
        .transcript(id)
        .await
        .unwrap_or_default()
        .iter()
        .map(|entry| match entry {
            tau_store::Entry::Context {
                plugin, body, stats, ..
            } => json!({
                "kind": "context",
                "plugin": plugin,
                "body": serde_json::from_str::<Value>(body).unwrap_or(Value::Null),
                "trigger": stats.as_ref().map(|stats| stats.trigger.clone()),
            }),
            tau_store::Entry::Plugin { plugin, body } => json!({
                "kind": "plugin",
                "plugin": plugin,
                "body": serde_json::from_str::<Value>(body).unwrap_or(Value::Null),
            }),
            tau_store::Entry::Message { role, body } => json!({
                "kind": "message",
                "role": role,
                "message": serde_json::from_str::<Value>(body).unwrap_or(Value::Null),
            }),
        })
        .collect()
}

/// A request's settings, as JSON: what a test compares.
fn settings(settings: &tau_ai::responses::request::Settings) -> Value {
    json!({
        "model": settings.model,
        "instructions": settings.instructions,
        "reasoning": settings.reasoning,
        "text_format": settings.text_format,
        "max_output_tokens": settings.max_output_tokens,
        "tools": settings
            .tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<Vec<_>>(),
    })
}

/// What the store keeps of the run `id`: the context's rewrites, by
/// plugin, and the plugin's last state and its log lines.
async fn stored(
    came: &mut Value,
    plugin: &str,
    store: &tau_store::Store,
    id: &str,
) {
    let rewrites: Vec<Value> = store
        .transcript(id)
        .await
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| match entry {
            tau_store::Entry::Context { body, .. } => {
                let body: Value = serde_json::from_str(body).ok()?;
                Some(json!({ "plugin": body["plugin"], "details": body["details"] }))
            }
            _ => None,
        })
        .collect();
    let records: Vec<tau_luau_plugins::Record> = store
        .records(id, tau_luau_plugins::NAME)
        .await
        .unwrap_or_default()
        .iter()
        .filter_map(|body| serde_json::from_str(body).ok())
        .collect();
    let state = records.iter().rev().find_map(|record| match record {
        tau_luau_plugins::Record::State { plugin: of, state }
            if of == plugin =>
        {
            Some(state.clone())
        }
        _ => None,
    });
    let logs: Vec<String> = records
        .iter()
        .filter_map(|record| match record {
            tau_luau_plugins::Record::Log { plugin: of, lines }
                if of == plugin =>
            {
                Some(lines.clone())
            }
            _ => None,
        })
        .flatten()
        .collect();
    came["rewrites"] = json!(rewrites);
    if let Some(state) = state {
        came["state"] = state;
    }
    came["logs"] = json!(logs);
}

/// The files the plugin archived under `dir`: each one's path to its text
/// and its permission bits.
fn archive(dir: &Path) -> Value {
    let mut out = serde_json::Map::new();
    let mut dirs = vec![dir.to_owned()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            use std::os::unix::fs::PermissionsExt as _;
            let mode = entry
                .metadata()
                .map(|meta| meta.permissions().mode() & 0o777)
                .unwrap_or_default();
            let text = std::fs::read(&path)
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .unwrap_or_default();
            out.insert(
                path.display().to_string(),
                json!({ "text": text, "mode": mode }),
            );
        }
    }
    Value::Object(out)
}

/// What the model was sent, each request in order: its settings, and
/// its messages.
fn requests(llm: &ScriptedModel) -> Value {
    llm.requests()
        .into_iter()
        .map(|request| {
            let mut sent = settings(&request.settings);
            sent["settings"] = settings(&request.settings);
            sent["messages"] = json!(request.transcript);
            sent
        })
        .collect()
}

/// Each blob `state` refers to, `{ id, digest, size_bytes }`, read back
/// whole from `dir`: its id to its bytes in base64.
async fn read_back(dir: &Path, state: &Value) -> Value {
    let mut refs = Vec::new();
    blob_refs(state, &mut refs);
    let mut out = serde_json::Map::new();
    for blob in refs {
        let id = blob["id"].as_str().unwrap_or_default().to_owned();
        if let Ok(data) = crate::files::blob(dir.to_owned(), &blob).await {
            out.insert(id, Value::String(data));
        }
    }
    Value::Object(out)
}

fn blob_refs(value: &Value, found: &mut Vec<Value>) {
    match value {
        Value::Object(map)
            if map.get("id").is_some_and(Value::is_string)
                && map.get("digest").is_some_and(Value::is_string)
                && map.get("size_bytes").is_some_and(Value::is_u64) =>
        {
            found.push(value.clone());
        }
        Value::Object(map) => {
            map.values().for_each(|value| blob_refs(value, found));
        }
        Value::Array(items) => {
            items.iter().for_each(|value| blob_refs(value, found));
        }
        _ => {}
    }
}

/// What a run's events say: each call's result, what the plugins were
/// charged, the events, and the plugin's state at the end.
fn came_to(plugin: &str, events: &[RunEvent]) -> Value {
    let results: Vec<Value> = events
        .iter()
        .filter_map(|event| match event {
            RunEvent::ToolEnd {
                output, is_error, ..
            } => Some(json!({
                "text": output.text_content(),
                "error": is_error,
            })),
            _ => None,
        })
        .collect();
    let state = events
        .iter()
        .rev()
        .find_map(|event| match event {
            RunEvent::PluginReport { body, .. }
                if body["plugin"] == plugin && body.get("state").is_some() =>
            {
                Some(body["state"].clone())
            }
            _ => None,
        })
        .unwrap_or(Value::Null);
    let charged: f64 = events
        .iter()
        .filter_map(|event| match event {
            RunEvent::PluginCharged { usage, .. } => Some(usage.cost.total),
            _ => None,
        })
        .sum();
    json!({
        "results": results,
        "charged": charged,
        "events": events,
        "state": state,
    })
}

/// Writes the workspace `files` names under `dir`: each file's bytes as
/// written, in base64.
/// Each file under `dir`, its path there to its text.
fn tree(dir: &Path) -> Value {
    fn walk(dir: &Path, at: &Path, found: &mut serde_json::Map<String, Value>) {
        let Ok(entries) = std::fs::read_dir(at) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(dir, &path, found);
            } else if let (Ok(text), Ok(name)) =
                (std::fs::read_to_string(&path), path.strip_prefix(dir))
            {
                found.insert(name.display().to_string(), Value::String(text));
            }
        }
    }
    let mut found = serde_json::Map::new();
    walk(dir, dir, &mut found);
    Value::Object(found)
}

/// What tells the loop's plugins the commit each turn leaves, from the
/// spec's `commits`.
struct Commits {
    plugins: LuauPlugins,
    commits: Value,
}

#[async_trait]
impl tau_agent::plugin::Plugin for Commits {
    fn name(&self) -> &str {
        "t.loop commits"
    }

    async fn start(
        &self,
        _plan: &mut tau_agent::plugin::RunPlan,
        _ctx: &tau_agent::plugin::PluginCtx,
    ) -> Result<
        Box<dyn tau_agent::plugin::PluginRun>,
        tau_agent::plugin::PluginError,
    > {
        Ok(Box::new(Commits {
            plugins: self.plugins.clone(),
            commits: self.commits.clone(),
        }))
    }
}

#[async_trait]
impl tau_agent::plugin::PluginRun for Commits {
    async fn on_event(
        &mut self,
        event: &RunEvent,
        _ctx: &tau_agent::plugin::PluginCtx,
    ) {
        let RunEvent::TurnEnd { turn, .. } = event else {
            return;
        };
        let commit = self
            .commits
            .as_array()
            .into_iter()
            .flatten()
            .find(|commit| commit["turn"].as_u64() == Some(u64::from(*turn)));
        if let Some(commit) = commit {
            self.plugins.heard(&tau_ui_plugin::TurnCommit {
                change_id: commit["change"].as_str().unwrap_or_default().into(),
                paths: commit["paths"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|path| path.as_str().map(str::to_owned))
                    .collect(),
            });
        }
    }
}

fn workspace(dir: &Path, files: &Value) -> Result<Value, String> {
    let mut written = serde_json::Map::new();
    std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    for (path, content) in files.as_object().into_iter().flatten() {
        let bytes = match content {
            Value::String(text) => text.clone().into_bytes(),
            _ if content["base64"].is_string() => {
                base64::engine::general_purpose::STANDARD
                    .decode(content["base64"].as_str().unwrap_or_default())
                    .map_err(|error| format!("files[{path}]: {error}"))?
            }
            _ if content["png"].is_object() => png(
                content["png"]["width"].as_u64().unwrap_or(1) as u32,
                content["png"]["height"].as_u64().unwrap_or(1) as u32,
            )?,
            other => {
                return Err(format!(
                    "files[{path}] is a text, {{ base64 }} or {{ png }}, not {other}"
                ));
            }
        };
        let at = dir.join(path);
        if !at.starts_with(dir) || path.contains("..") {
            return Err(format!("files[{path}] is not in the workspace"));
        }
        if let Some(parent) = at.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| error.to_string())?;
        }
        written.insert(
            path.clone(),
            Value::String(
                base64::engine::general_purpose::STANDARD.encode(&bytes),
            ),
        );
        std::fs::write(&at, bytes).map_err(|error| error.to_string())?;
    }
    Ok(Value::Object(written))
}

fn png(width: u32, height: u32) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    image::RgbImage::new(width.max(1), height.max(1))
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
        .map_err(|error| error.to_string())?;
    Ok(bytes)
}

/// The scripted model `turns` gives.
fn model(turns: &Value) -> Result<ScriptedModel, String> {
    let mut llm = ScriptedModel::new();
    for (at, turn) in turns.as_array().into_iter().flatten().enumerate() {
        let parts: Vec<Value> = match turn {
            Value::Array(parts) => parts.clone(),
            part => vec![part.clone()],
        };
        for part in &parts {
            if !part.is_object() {
                return Err(format!("model[{}]: a part is a table", at + 1));
            }
            if part.get("tool").is_some()
                && !(part["args"].is_object() || part["args"].is_null())
            {
                return Err(format!(
                    "model[{}]: a tool's `args` is a table",
                    at + 1
                ));
            }
        }
        llm = llm.turn(move |mut turn: TurnBuilder| {
            for part in parts {
                if let Some(text) = part["thinking"].as_str() {
                    turn = turn.thinking(text);
                }
                if let Some(text) = part["text"].as_str() {
                    turn = turn.text(text);
                }
                if let Some(tool) = part["tool"].as_str() {
                    let args = if part["args"].is_object() {
                        part["args"].clone()
                    } else {
                        json!({})
                    };
                    turn = turn.tool_call(tool, args);
                }
                if part["usage"].is_object() {
                    turn = turn.usage(
                        part["usage"]["input"].as_u64().unwrap_or(0),
                        part["usage"]["output"].as_u64().unwrap_or(0),
                    );
                }
                if let Some(usd) = part["cost"].as_f64() {
                    turn = turn.cost(usd);
                }
                if let Some(ms) = part["delay"].as_u64() {
                    turn = turn.delay(Duration::from_millis(ms));
                }
                if part["failure"].is_object() {
                    let class = match part["failure"]["class"].as_str() {
                        Some("retryable") => tau_ai::retry::Class::Retryable,
                        Some("context_overflow") => {
                            tau_ai::retry::Class::ContextOverflow
                        }
                        _ => tau_ai::retry::Class::Fatal,
                    };
                    turn = turn.failure(
                        part["failure"]["message"].as_str().unwrap_or(""),
                        class,
                    );
                }
                if part["error"].is_object() {
                    turn = turn.error(
                        part["error"]["code"].as_str().unwrap_or("error"),
                        part["error"]["message"].as_str().unwrap_or(""),
                    );
                }
                match part["stop"].as_str() {
                    Some("length") => turn = turn.stop(StopReason::Length),
                    Some("error") => turn = turn.stop(StopReason::Error),
                    Some("aborted") => turn = turn.stop(StopReason::Aborted),
                    _ => {}
                }
            }
            turn
        });
    }
    Ok(llm)
}

/// The fake Jev a loop's `jev` gives: its rules, which a step may
/// replace, and every request it was asked.
struct Rules {
    fake: FakeJev,
    rules: Arc<Mutex<Vec<Value>>>,
    failing: Arc<Mutex<Option<u16>>>,
}

impl Rules {
    fn new(spec: &Value) -> Result<Self, String> {
        let rules: Arc<Mutex<Vec<Value>>> = Arc::default();
        let failing: Arc<Mutex<Option<u16>>> = Arc::default();
        let (answering, status) = (rules.clone(), failing.clone());
        let fake = FakeJev::new(move |request| {
            if let Some(status) = *status.lock().expect("not poisoned") {
                return Err(JevError::Status(status));
            }
            let rules = answering.lock().expect("not poisoned").clone();
            let mut answers = BTreeMap::new();
            for id in request.questions.keys() {
                let held = held(&request.state, id);
                let rule = rules.iter().find(|rule| {
                    rule["id"]
                        .as_str()
                        .is_none_or(|prefix| id.starts_with(prefix))
                        && rule["has"]
                            .as_str()
                            .is_none_or(|text| held.contains(text))
                });
                let Some(rule) = rule else {
                    return Err(JevError::MissingAnswer(id.clone()));
                };
                answers.insert(id.clone(), answer(rule));
            }
            Ok(tau_jev::fake::response(answers, request))
        });
        let made = Self {
            fake,
            rules,
            failing,
        };
        made.replace(spec)?;
        Ok(made)
    }

    /// Answers from `spec` from now on: `{ error = 503 }`, or rules.
    fn replace(&self, spec: &Value) -> Result<(), String> {
        let status = spec["error"]
            .as_u64()
            .map(u16::try_from)
            .transpose()
            .map_err(|error| error.to_string())?;
        let rules = match spec {
            Value::Array(rules) => rules.clone(),
            _ => spec["rules"].as_array().cloned().unwrap_or_default(),
        };
        *self.failing.lock().expect("not poisoned") = status;
        *self.rules.lock().expect("not poisoned") = rules;
        Ok(())
    }

    /// Each request asked: its question ids and its state.
    fn asked(&self) -> Value {
        self.fake
            .requests()
            .iter()
            .map(|request| {
                json!({
                    "questions": request.questions.keys().collect::<Vec<_>>(),
                    "state": request.state,
                })
            })
            .collect()
    }
}

/// What `state` holds of the question `id`: the object whose `id` it is,
/// as JSON; empty when there is none.
fn held(state: &Value, id: &str) -> String {
    match state {
        Value::Object(map)
            if map.get("id").and_then(Value::as_str) == Some(id) =>
        {
            state.to_string()
        }
        Value::Object(map) => map
            .values()
            .map(|value| held(value, id))
            .find(|found| !found.is_empty())
            .unwrap_or_default(),
        Value::Array(items) => items
            .iter()
            .map(|value| held(value, id))
            .find(|found| !found.is_empty())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn answer(rule: &Value) -> Answer {
    let probabilities = || -> BTreeMap<String, f64> {
        rule["probabilities"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(name, p)| (name.clone(), p.as_f64().unwrap_or(0.0)))
            .collect()
    };
    if let Some(choice) = rule["choice"].as_str() {
        return Answer::Choice {
            choice: choice.to_owned(),
            probabilities: probabilities(),
            confidence: rule["confidence"].as_f64().unwrap_or(1.0),
        };
    }
    if let Some(score) = rule["score"].as_f64() {
        return Answer::Score {
            score,
            probabilities: probabilities(),
            confidence: rule["confidence"].as_f64().unwrap_or(1.0),
        };
    }
    Answer::Noul {
        noul: rule["noul"].as_f64().unwrap_or(0.5),
    }
}

/// A fake tool: its answers in order, the last kept.
struct Fake {
    name: String,
    answers: Vec<Value>,
    next: Mutex<usize>,
}

impl Fake {
    fn new(name: &str, answers: &Value) -> Self {
        let answers = match answers {
            Value::Array(answers) => answers.clone(),
            answer => vec![answer.clone()],
        };
        Self {
            name: name.to_owned(),
            answers,
            next: Mutex::new(0),
        }
    }
}

static PARAMETERS: std::sync::LazyLock<Value> =
    std::sync::LazyLock::new(|| json!({ "type": "object" }));

#[async_trait]
impl AgentTool for Fake {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        "A fake tool of the test's."
    }

    fn parameters(&self) -> &Value {
        &PARAMETERS
    }

    async fn call(
        &self,
        args: Value,
        _ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let answer = {
            let mut next = self.next.lock().expect("not poisoned");
            let at = (*next).min(self.answers.len().saturating_sub(1));
            *next += 1;
            self.answers.get(at).cloned().unwrap_or(Value::Null)
        };
        match answer {
            Value::String(text) => Ok(ToolOutput::text(filled(&text, &args))),
            _ if answer["error"].is_string() => {
                Err(filled(answer["error"].as_str().unwrap_or_default(), &args)
                    .into())
            }
            other => Ok(ToolOutput::text(other.to_string())),
        }
    }
}

/// `text` with the call's arguments in it: `{name}` is the argument
/// `name`, `{text*name}` is `text` that many times. Anything else in braces
/// stays as it is.
fn filled(text: &str, args: &Value) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            out.push_str(&rest[open..]);
            return out;
        };
        let inside = &after[..close];
        let made = match inside.rsplit_once('*') {
            Some((repeated, name)) => args[name]
                .as_u64()
                .map(|times| repeated.repeat(times as usize)),
            None => match &args[inside] {
                Value::String(value) => Some(value.clone()),
                Value::Null => None,
                value => Some(value.to_string()),
            },
        };
        match made {
            Some(made) => out.push_str(&made),
            None => {
                out.push('{');
                out.push_str(inside);
                out.push('}');
            }
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// A model that counts how many of its responses are going on at once,
/// the most it saw: what a plugin's limit on its own requests shows.
#[derive(Clone)]
struct Counted {
    inner: ScriptedModel,
    now: Arc<std::sync::atomic::AtomicUsize>,
    most: Arc<std::sync::atomic::AtomicUsize>,
}

impl Counted {
    fn new(inner: ScriptedModel) -> Self {
        Self {
            inner,
            now: Arc::default(),
            most: Arc::default(),
        }
    }

    fn most(&self) -> usize {
        self.most.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl tau_ai::llm::Llm for Counted {
    fn supports_output_token_limit(&self) -> bool {
        self.inner.supports_output_token_limit()
    }

    fn open(
        &self,
        settings: tau_ai::responses::request::Settings,
    ) -> futures_util::future::BoxFuture<
        'static,
        Result<Box<dyn tau_ai::llm::LlmSession>, tau_ai::llm::LlmError>,
    > {
        use futures_util::FutureExt as _;
        let opening = self.inner.open(settings);
        let (now, most) = (self.now.clone(), self.most.clone());
        async move {
            let session = opening.await?;
            Ok(Box::new(CountedSession { session, now, most })
                as Box<dyn tau_ai::llm::LlmSession>)
        }
        .boxed()
    }
}

struct CountedSession {
    session: Box<dyn tau_ai::llm::LlmSession>,
    now: Arc<std::sync::atomic::AtomicUsize>,
    most: Arc<std::sync::atomic::AtomicUsize>,
}

/// One response going on, until its stream is dropped.
struct Going(Arc<std::sync::atomic::AtomicUsize>);

impl Drop for Going {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

impl tau_ai::llm::LlmSession for CountedSession {
    fn settings(&self) -> &tau_ai::responses::request::Settings {
        self.session.settings()
    }

    fn set_reasoning(
        &mut self,
        effort: Option<tau_ai::responses::request::ReasoningEffort>,
    ) {
        self.session.set_reasoning(effort);
    }

    fn respond(
        &mut self,
        transcript: &[tau_ai::message::Message],
        timestamp: tau_ai::message::Timestamp,
    ) -> tau_ai::llm::EventStream {
        use std::sync::atomic::Ordering;
        let going = self.now.fetch_add(1, Ordering::SeqCst) + 1;
        self.most.fetch_max(going, Ordering::SeqCst);
        let guard = Going(self.now.clone());
        self.session
            .respond(transcript, timestamp)
            .map(move |event| {
                let _ = &guard;
                event
            })
            .boxed()
    }

    fn warm_up(
        &mut self,
        timestamp: tau_ai::message::Timestamp,
    ) -> futures_util::future::BoxFuture<
        'static,
        Result<tau_ai::message::Usage, tau_ai::llm::LlmError>,
    > {
        self.session.warm_up(timestamp)
    }
}
