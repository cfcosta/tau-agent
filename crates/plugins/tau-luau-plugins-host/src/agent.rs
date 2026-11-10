//! The tau-agent plugin that runs Luau plugins in a run (ADR 0027): their
//! tools are the run's tools, and their hooks run at tau-agent's seams.
//!
//! Every hook's answer is applied here: a block stops the call, a
//! continue keeps the run going, and the state, log lines, errors and
//! the view drawn from the new state are published as [`Record`]s. A
//! hook that fails counts as allowing or stopping; after
//! [`MAX_FAILURES`] in a run, its plugin is off for the rest of it.
//!
//! The plugins follow their versions live ([`Versions`]): before each
//! model request the run takes the versions active then, so a plugin
//! that changed, or a new one, works from that request on. Each hook and
//! tool call goes to the plugin's version of the moment, its state kept
//! by name across versions. The tools the run started with are declared
//! to the model; one that came later is reached from code mode, through
//! the plugin's tool source, so the request's tools, and the prompt
//! cache they key, stay as they were.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use serde_json::{Value, json};
use tau_agent::{
    error::{PluginError, ToolError},
    event::RunEvent,
    plugin::{
        ContextView,
        Decision,
        FinishedRun,
        Plugin,
        PluginCtx,
        PluginRun,
        RequestView,
        Rewrite,
        RunPlan,
        StopDecision,
        ToolCall,
        ToolResultView,
    },
    tool::{AgentTool, RunId, ToolCtx, ToolOutput, ToolSource},
};
use tau_ai::{
    message::{
        AssistantBlock,
        AssistantMessage,
        InputBlock,
        Message,
        TextContent,
        Usage,
    },
    responses::request::ReasoningEffort,
};
use tau_codemode_host::{ToolCall as ScriptCall, ToolEntry, ToolReply};
use tau_jev::Jev;

use crate::{
    NAME,
    Record,
    ToolSpec,
    infer::Refused,
    runtime::{Context, Hook, Loaded, NoReach, Outcome, Reach},
};

/// Failed hooks a plugin may have in a run before it is turned off.
pub const MAX_FAILURES: u32 = 3;

/// Times `before_stop` may keep a run going, each plugin, unless it
/// declares its own (`continues`).
pub const MAX_CONTINUATIONS: u32 = 3;

/// The most a plugin may declare `continues` to be.
pub const MOST_CONTINUATIONS: u32 = 20;

/// A plugin as a run gets it: loaded, with the person's settings.
#[derive(Debug, Clone)]
pub struct Active {
    pub loaded: Loaded,
    pub settings: Value,
}

impl Active {
    /// The plugin tau comes with by `name` (`plugins/built-in.toml`), loaded
    /// as tau loads it, with its default settings: for an agent of its
    /// own, as an evaluation's.
    pub async fn built_in(name: &str) -> Result<Self, String> {
        let loaded = crate::built_in::loaded()
            .await?
            .into_iter()
            .find(|loaded| loaded.declaration.name == name)
            .ok_or_else(|| format!("tau comes with no plugin {name}"))?;
        let settings = loaded.declaration.default_settings();
        Ok(Self { loaded, settings })
    }
}

/// Where a run's plugins come from: the versions active for it now.
#[async_trait]
pub trait Versions: Send + Sync + 'static {
    async fn now(&self) -> Vec<Active>;
}

/// The same versions, always.
struct Fixed(Vec<Active>);

#[async_trait]
impl Versions for Fixed {
    async fn now(&self) -> Vec<Active> {
        self.0.clone()
    }
}

/// The run's Luau plugins, as one tau-agent plugin. Build one per run.
#[derive(Clone)]
pub struct LuauPlugins(Arc<Inner>);

struct Inner {
    versions: Arc<dyn Versions>,
    /// The versions as last taken, in the order they are active.
    active: Mutex<Arc<Vec<Active>>>,
    /// `{ kind, repo, model }`: what `ctx.run` says besides the run's id
    /// and turn.
    run: Value,
    jev: Option<Arc<dyn Jev>>,
    /// tau's configuration directory, where each plugin's secrets are.
    config: Option<std::path::PathBuf>,
    /// Each plugin's store in each repository (`uses.store`).
    stores: Option<Arc<crate::store::Stores>>,
    /// Where `ctx.archive` keeps its files: tau's archive of the run's
    /// repository, else the system's temporary directory.
    archive: std::path::PathBuf,
    /// The run's workspace, which `ctx.files` reads relative paths in,
    /// and where each plugin's blobs are kept (`uses.files`).
    workspace: Option<std::path::PathBuf>,
    blobs: Option<std::path::PathBuf>,
    /// The person's skills folder, as the host has it (`uses.files`'
    /// `skills`).
    skills: Option<std::path::PathBuf>,
    /// tau's directory for the run's repository, and tau's data folder
    /// when it is not the app's (`uses.files`' `repository` and `data`,
    /// ADR 0050, gap L).
    repository: Option<std::path::PathBuf>,
    data: Option<std::path::PathBuf>,
    /// The commit the turn ending left, as the run's workspace told it,
    /// for `turn_end` (ADR 0050, gap L).
    commit: Arc<Mutex<Option<tau_ui_plugin::TurnCommit>>>,
    /// What carries out what a tool asked once the person allows it.
    dispatch: Option<Dispatch>,
    /// The state of each run going on, by run.
    runs: Mutex<HashMap<RunId, Arc<tokio::sync::Mutex<RunState>>>>,
}

/// One run's state for every plugin, by name.
#[derive(Debug, Default)]
struct RunState {
    states: HashMap<String, Value>,
    /// Each plugin's state as last stored with the run, by this run or
    /// as it started: a stored state that differs came from elsewhere.
    stored: HashMap<String, Value>,
    failures: HashMap<String, u32>,
    /// The digest of each plugin's version the run last drew.
    drawn: HashMap<String, String>,
    /// Times each plugin's `before_stop` kept the run going.
    continued: HashMap<String, u32>,
    turn: u32,
    /// The current turn's text and its calls, by id.
    text: String,
    names: HashMap<String, String>,
    calls: Vec<Value>,
}

/// What a run's tool reaches once the person allows what it asked: the
/// asks waiting for them, the registry whose actions carry it out, as
/// the person's own, and the chats those actions ask of.
#[derive(Clone)]
pub struct Dispatch {
    pub allows: crate::chats::Allows,
    /// The forms its tools ask (`tau.ask`), waiting for the answers.
    pub forms: crate::forms::Forms,
    pub registry: crate::registry::Registry,
    pub chats: Option<Arc<dyn tau_ui_plugin::Conversations>>,
}

impl LuauPlugins {
    /// The plugins `active`, always. `run` is `{ kind, repo, model }`;
    /// `jev` is what plugins that use Jev ask.
    pub fn new(
        active: Vec<Active>,
        run: Value,
        jev: Option<Arc<dyn Jev>>,
    ) -> Self {
        Self(Arc::new(Inner {
            active: Mutex::new(Arc::new(active.clone())),
            versions: Arc::new(Fixed(active)),
            run,
            jev,
            config: None,
            stores: None,
            archive: std::env::temp_dir(),
            workspace: None,
            blobs: None,
            skills: None,
            repository: None,
            data: None,
            commit: Default::default(),
            dispatch: None,
            runs: Mutex::default(),
        }))
    }

    /// The plugins, each with its store in the run's repository.
    pub fn with_stores(mut self, stores: Arc<crate::store::Stores>) -> Self {
        Arc::get_mut(&mut self.0)
            .expect("the stores are given before the plugins are shared")
            .stores = Some(stores);
        self
    }

    /// The plugins, `ctx.archive` keeping its files in `dir`.
    pub fn with_archive(mut self, dir: std::path::PathBuf) -> Self {
        Arc::get_mut(&mut self.0)
            .expect("the archive is given before the plugins are shared")
            .archive = dir;
        self
    }

    /// The plugins, `ctx.files` reading relative paths in the run's
    /// `workspace` and keeping blobs under `blobs`.
    pub fn with_files(
        mut self,
        workspace: Option<std::path::PathBuf>,
        blobs: std::path::PathBuf,
    ) -> Self {
        let inner = Arc::get_mut(&mut self.0)
            .expect("the files are given before the plugins are shared");
        inner.workspace = workspace;
        inner.blobs = Some(blobs);
        self
    }

    /// The person's skills folder, as the host has it, for the plugins
    /// whose `uses.files` names `skills`.
    pub fn with_skills(mut self, dir: Option<std::path::PathBuf>) -> Self {
        Arc::get_mut(&mut self.0)
            .expect("the skills folder is given before the plugins are shared")
            .skills = dir;
        self
    }

    /// The plugins, `uses.files`' `repository` being tau's directory for
    /// the run's repository, `repository`, and its `data` tau's data
    /// folder at `data` when given, else the app's (ADR 0050, gap L).
    pub fn with_folders(
        mut self,
        repository: Option<std::path::PathBuf>,
        data: Option<std::path::PathBuf>,
    ) -> Self {
        let inner = Arc::get_mut(&mut self.0)
            .expect("the folders are given before the plugins are shared");
        inner.repository = repository;
        inner.data = data;
        self
    }

    /// Hears the commit the turn ending left, which `turn_end` is given
    /// (ADR 0050, gap L): what the run's [`tau_ui_plugin::TurnHooks`]
    /// call, before the plugins hear the turn end.
    pub fn heard(&self, commit: &tau_ui_plugin::TurnCommit) {
        *self.0.commit.lock().expect("not poisoned") = Some(commit.clone());
    }

    /// The plugins, their tools asking the person through `dispatch`
    /// (`tau.allow`).
    pub fn with_dispatch(mut self, dispatch: Dispatch) -> Self {
        Arc::get_mut(&mut self.0)
            .expect("the dispatch is given before the plugins are shared")
            .dispatch = Some(dispatch);
        self
    }

    /// The plugins, their HTTP requests naming secrets kept in tau's
    /// configuration directory `config`.
    pub fn with_config(mut self, config: std::path::PathBuf) -> Self {
        Arc::get_mut(&mut self.0)
            .expect("the directory is given before the plugins are shared")
            .config = Some(config);
        self
    }

    /// The plugins `versions` has active, as they change: taken now, and
    /// again before each model request.
    pub async fn live(
        versions: Arc<dyn Versions>,
        run: Value,
        jev: Option<Arc<dyn Jev>>,
    ) -> Self {
        let active = versions.now().await;
        Self(Arc::new(Inner {
            active: Mutex::new(Arc::new(active)),
            versions,
            run,
            jev,
            config: None,
            stores: None,
            archive: std::env::temp_dir(),
            workspace: None,
            blobs: None,
            skills: None,
            repository: None,
            data: None,
            commit: Default::default(),
            dispatch: None,
            runs: Mutex::default(),
        }))
    }
}

impl Inner {
    fn run_state(&self, run: &RunId) -> Arc<tokio::sync::Mutex<RunState>> {
        self.runs
            .lock()
            .expect("not poisoned")
            .entry(run.clone())
            .or_default()
            .clone()
    }

    /// The versions as last taken.
    fn active(&self) -> Arc<Vec<Active>> {
        self.active.lock().expect("not poisoned").clone()
    }

    /// Takes the versions active now.
    async fn refresh(&self) {
        let now = Arc::new(self.versions.now().await);
        *self.active.lock().expect("not poisoned") = now;
    }

    /// Plugin `name`'s version now, if it is active.
    fn plugin(&self, name: &str) -> Option<Active> {
        self.active()
            .iter()
            .find(|active| active.loaded.declaration.name == name)
            .cloned()
    }

    /// What `active`'s run hooks reach (ADR 0050): the model and Jev,
    /// when its `uses` names them, charged to the run, and the run's
    /// conversation, `transcript`, where the hook has it and `uses`
    /// names it; else nothing.
    /// What `ctx.files` and `ctx.blobs` reach for `active`, with
    /// `uses.files`: its folders, the run's workspace, its package when it
    /// is one of tau's own, and its blobs in the run's repository.
    fn files_of(&self, active: &Active) -> Option<Arc<crate::files::Files>> {
        let uses = &active.loaded.declaration.uses;
        if uses.files.is_empty() {
            return None;
        }
        let name = &active.loaded.declaration.name;
        let blobs = self
            .blobs
            .as_ref()
            .zip(self.run["repo"].as_str())
            .map(|(root, repo)| crate::files::blobs_dir(root, name, repo));
        let files = crate::files::Files::new(
            &uses.files,
            self.workspace.clone(),
            blobs,
        )
        .writing(&uses.writes)
        .with_package(crate::runtime::package_dir(name))
        .with_skills(self.skills.clone())
        .with_repository(self.repository.clone());
        Some(Arc::new(match &self.data {
            Some(data) => files.with_data(data.clone()),
            None => files,
        }))
    }

    fn reach(
        &self,
        active: &Active,
        ctx: &PluginCtx,
        transcript: Option<&[Message]>,
    ) -> Arc<dyn Reach> {
        self.reach_asking(active, ctx, transcript, None, None)
    }

    /// [`Self::reach`], with `ctx.files` too when `uses.files` names
    /// folders: for `start`, `after_tool_result`, `rewritten`, `turn_end`
    /// and `run_end`.
    fn reach_with_files(
        &self,
        active: &Active,
        ctx: &PluginCtx,
    ) -> Arc<dyn Reach> {
        self.reach_with_files_and(active, ctx, None)
    }

    /// [`Self::reach_with_files`], `ctx.transcript()` giving `transcript`
    /// with `uses.transcript`: what a rewrite replaced, in `rewritten`,
    /// and the run's conversation, in `run_end`.
    fn reach_with_files_and(
        &self,
        active: &Active,
        ctx: &PluginCtx,
        transcript: Option<&[Message]>,
    ) -> Arc<dyn Reach> {
        let reach = self.reach(active, ctx, transcript);
        match self.files_of(active) {
            Some(files) => Arc::new(crate::runtime::WithFiles { reach, files }),
            None => reach,
        }
    }

    /// [`Self::reach`], `infer` asking in the run's own conversation
    /// through `conversation` too, and `ctx.measure` sizing rewrites of
    /// `rewriting` (`rewrite_context`, which holds them).
    fn reach_asking(
        &self,
        active: &Active,
        ctx: &PluginCtx,
        transcript: Option<&[Message]>,
        conversation: Option<Asks>,
        rewriting: Option<&[Message]>,
    ) -> Arc<dyn Reach> {
        let uses = &active.loaded.declaration.uses;
        let transcript = transcript
            .filter(|_| uses.transcript)
            .map(|transcript| Arc::from(transcript.to_vec()));
        if !uses.infer
            && !uses.jev
            && !uses.archive
            && transcript.is_none()
            && rewriting.is_none()
        {
            return Arc::new(NoReach);
        }
        Arc::new(RunReach {
            plugin: ctx.clone(),
            jev: self.jev.clone(),
            named: uses.model.clone(),
            run_model: self.run["model"].as_str().unwrap_or_default().into(),
            transcript,
            conversation: conversation.filter(|_| uses.infer),
            rewriting: rewriting.map(|messages| Arc::from(messages.to_vec())),
            archive: self.archive.clone(),
        })
    }

    fn context(
        &self,
        run: &RunId,
        state: &RunState,
        active: &Active,
    ) -> Context {
        let mut info = self.run.clone();
        if let Some(info) = info.as_object_mut() {
            info.insert("id".into(), json!(run.0.as_ref()));
            info.insert("turn".into(), json!(state.turn));
        }
        let name = &active.loaded.declaration.name;
        Context {
            run: info,
            now: now(),
            settings: active.settings.clone(),
            state: state.states.get(name).cloned().unwrap_or_else(|| json!({})),
            secrets: self
                .config
                .as_deref()
                .map(|config| crate::runtime::secrets_of(config, name)),
            store: None,
            job: None,
        }
    }

    /// `active`'s store in the run's repository, when it uses one.
    async fn store_of(
        &self,
        active: &Active,
    ) -> Option<crate::runtime::StoreRef> {
        let declaration = &active.loaded.declaration;
        if !declaration.uses.store {
            return None;
        }
        let repo = self.run["repo"].as_str()?;
        let stores = self.stores.as_ref()?;
        match stores.of(&declaration.name, repo).await {
            Ok(store) => Some(crate::runtime::StoreRef(store)),
            Err(error) => {
                eprintln!("{}'s store in {repo}: {error}", declaration.name);
                None
            }
        }
    }

    fn off(&self, state: &RunState, name: &str) -> bool {
        state.failures.get(name).copied().unwrap_or(0) >= MAX_FAILURES
    }

    /// Calls `hook` of `active` and applies what it did: state, log
    /// lines, the view, or the error.
    async fn call(
        &self,
        active: &Active,
        hook: Hook,
        input: Value,
        state: &mut RunState,
        ctx: &PluginCtx,
        reach: Arc<dyn Reach>,
    ) -> Option<Value> {
        let outcome = self
            .run_hook(active, &hook, input, state, ctx, reach)
            .await?;
        self.apply(active, &hook, outcome, state, ctx).await
    }

    /// Calls `hook` of `active`, applying nothing: `None` when its plugin
    /// is off or has no such hook.
    async fn run_hook(
        &self,
        active: &Active,
        hook: &Hook,
        input: Value,
        state: &mut RunState,
        ctx: &PluginCtx,
        reach: Arc<dyn Reach>,
    ) -> Option<Outcome> {
        self.run_hook_of(active, hook, input, state, ctx, reach, None)
            .await
    }

    /// [`Self::run_hook`], for a tool's `call` the call's id as
    /// `ctx.run.call`.
    #[allow(clippy::too_many_arguments)]
    async fn run_hook_of(
        &self,
        active: &Active,
        hook: &Hook,
        input: Value,
        state: &mut RunState,
        ctx: &PluginCtx,
        reach: Arc<dyn Reach>,
        call: Option<&str>,
    ) -> Option<Outcome> {
        let name = &active.loaded.declaration.name;
        if self.off(state, name)
            || !hook.declared_in(&active.loaded.declaration)
        {
            return None;
        }
        // Views and cards only draw the state; anything else may change
        // it, so it starts from the state as stored.
        if !matches!(hook, Hook::View | Hook::Card(_)) {
            self.catch_up(state, ctx).await;
        }
        let mut context = self.context(&ctx.run, state, active);
        context.store = self.store_of(active).await;
        if let (Some(call), Some(run)) = (call, context.run.as_object_mut()) {
            run.insert("call".into(), json!(call));
        }
        // `run_end` runs however the run ended, stopped by the person
        // too: what it lets go of goes, the run's cancel or not; an
        // action called in the run carries out what the person did.
        let cancel = match hook {
            Hook::RunEnd | Hook::Action(_) => {
                tokio_util::sync::CancellationToken::new()
            }
            _ => ctx.cancel.child_token(),
        };
        Some(
            active
                .loaded
                .call(hook, input, &context, reach, cancel)
                .await,
        )
    }

    /// Takes each plugin's state stored with the run since this run last
    /// knew it: what an interface recorded meanwhile, such as an action
    /// a button carried out (ADR 0045).
    async fn catch_up(&self, state: &mut RunState, ctx: &PluginCtx) {
        let Ok(records) = ctx.records().await else {
            return;
        };
        for (plugin, stored) in tau_luau_plugins::last_states(&records) {
            if state.stored.get(&plugin) != Some(&stored) {
                state.states.insert(plugin.clone(), stored.clone());
                state.stored.insert(plugin, stored);
            }
        }
    }

    async fn apply(
        &self,
        active: &Active,
        hook: &Hook,
        outcome: Outcome,
        state: &mut RunState,
        ctx: &PluginCtx,
    ) -> Option<Value> {
        let plugin = active.loaded.declaration.name.clone();
        if !outcome.logs.is_empty() {
            ctx.publish(&Record::Log {
                plugin: plugin.clone(),
                lines: outcome.logs,
            })
            .await;
        }
        if let Some(error) = outcome.error {
            *state.failures.entry(plugin.clone()).or_default() += 1;
            ctx.publish(&Record::Error {
                plugin: plugin.clone(),
                hook: hook_name(hook),
                error,
            })
            .await;
            if self.off(state, &plugin) {
                ctx.publish(&Record::Off {
                    plugin,
                    why: format!("{MAX_FAILURES} hooks failed in this run"),
                })
                .await;
            }
            return None;
        }
        if state.states.get(&plugin) != Some(&outcome.state) {
            state.states.insert(plugin.clone(), outcome.state.clone());
            state.stored.insert(plugin.clone(), outcome.state.clone());
            // What `start` left shows after the message the run started
            // on, in history as live.
            let place = matches!(hook, Hook::Start)
                .then_some(tau_ui_plugin::PLACE_MESSAGE);
            publish(
                ctx,
                &Record::State {
                    plugin: plugin.clone(),
                    state: outcome.state,
                },
                place,
            )
            .await;
            if !matches!(hook, Hook::View) {
                self.draw(active, state, ctx, place).await;
            }
        }
        Some(outcome.value)
    }

    /// Draws `active`'s view from its state and publishes it, at
    /// `place` in history ([`tau_ui_plugin::PLACE`]).
    async fn draw(
        &self,
        active: &Active,
        state: &mut RunState,
        ctx: &PluginCtx,
        place: Option<&str>,
    ) {
        let name = active.loaded.declaration.name.clone();
        state
            .drawn
            .insert(name.clone(), active.loaded.digest.clone());
        if self.off(state, &name) || !active.loaded.declaration.hooks.view {
            return;
        }
        let context = self.context(&ctx.run, state, active);
        let outcome = active
            .loaded
            .call(
                &Hook::View,
                Value::Null,
                &context,
                Arc::new(NoReach),
                ctx.cancel.child_token(),
            )
            .await;
        let plugin = name;
        match outcome.error {
            Some(error) => {
                *state.failures.entry(plugin.clone()).or_default() += 1;
                ctx.publish(&Record::Error {
                    plugin,
                    hook: "view".into(),
                    error,
                })
                .await;
            }
            None => {
                let view = Record::View {
                    plugin,
                    view: outcome.value,
                };
                publish(ctx, &view, place).await;
            }
        }
    }
}

/// Publishes `record`, at `place` in history when it says.
async fn publish(ctx: &PluginCtx, record: &Record, place: Option<&str>) {
    match place {
        Some(place) => ctx.publish(&tau_ui_plugin::placed(record, place)).await,
        None => ctx.publish(record).await,
    }
}

/// What a hook asks in the run's own conversation: how many of its
/// messages go first, the message after them, and where the answer goes.
type Ask = (
    usize,
    Message,
    tokio::sync::oneshot::Sender<Result<AssistantMessage, String>>,
);

/// Where a hook sends what it asks in the run's conversation.
type Asks = tokio::sync::mpsc::Sender<Ask>;

/// Runs `hook`, asking in `view`'s conversation what it asks there
/// meanwhile, one at a time: the conversation is the run's, which only
/// `rewrite_context` holds.
async fn in_conversation<F: std::future::Future>(
    hook: F,
    asked: &mut tokio::sync::mpsc::Receiver<Ask>,
    view: &ContextView<'_>,
    ctx: &PluginCtx,
) -> F::Output {
    tokio::pin!(hook);
    loop {
        tokio::select! {
            biased;
            output = &mut hook => return output,
            Some((first, message, reply)) = asked.recv() => {
                let first = first.min(view.transcript.len());
                let mut input = view.transcript[..first].to_vec();
                input.push(message);
                let answer = view
                    .conversation
                    .ask(&input, ctx)
                    .await
                    .map_err(|error| error.to_string());
                let _ = reply.send(answer);
            }
        }
    }
}

/// Adds what a plugin's `start` answered to `plan`: its texts after the
/// context's, its instructions after the run's, and the message the
/// model gets in place of the run's, unless a plugin before it
/// (`replaced`) gave one.
fn apply_start(plan: &mut RunPlan, answer: &Value, replaced: &mut bool) {
    if !*replaced && let Some(input) = answer["input"].as_str() {
        plan.input = input.to_owned();
        *replaced = true;
    }
    plan.context.extend(
        answer["context"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned),
    );
    if let Some(added) = answer["instructions"].as_str() {
        plan.instructions = Some(match plan.instructions.take() {
            Some(own) if !own.is_empty() => format!("{own}\n\n{added}"),
            _ => added.to_owned(),
        });
    }
    // The effort, before the session opens: on auto only, and the first
    // plugin that sets one sets it.
    if plan.reasoning.is_none()
        && let Some(effort) =
            answer["effort"].as_str().and_then(ReasoningEffort::parse)
    {
        plan.reasoning = Some(effort);
    }
}

/// The most of a result's text `after_tool_result` is given, well inside
/// what a hook's input may hold.
const RESULT_TEXT: usize = 256 * 1024;

/// `output`'s text, cut at [`RESULT_TEXT`], and whether it was.
fn result_text(output: &ToolOutput) -> (String, bool) {
    let mut text = String::new();
    for block in &output.content {
        if let InputBlock::Text(content) = block {
            text.push_str(&content.text);
        }
    }
    if text.len() <= RESULT_TEXT {
        return (text, false);
    }
    let mut end = RESULT_TEXT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    (text, true)
}

/// `output` with `text` in place of its text; its other blocks, such as
/// images, stay.
fn replace_text(output: &mut ToolOutput, text: &str) {
    output
        .content
        .retain(|block| !matches!(block, InputBlock::Text(_)));
    output.content.insert(
        0,
        InputBlock::Text(TextContent {
            text: text.to_owned(),
            text_signature: None,
        }),
    );
}

/// `event` as `on_event` hears it, by the kind `events` names: the
/// run's own events, not its sub-agents'; what streams in pieces and
/// tools' output stay out.
fn luau_event(
    event: &RunEvent,
    state: &RunState,
) -> Option<(&'static str, Value)> {
    let turn = state.turn;
    Some(match event {
        RunEvent::TurnStart { turn, .. } => {
            ("turn_start", json!({ "kind": "turn_start", "turn": turn }))
        }
        RunEvent::ToolStart {
            call_id,
            tool,
            args,
            parent: None,
            ..
        } => (
            "tool_start",
            json!({ "kind": "tool_start", "turn": turn, "id": call_id,
                    "name": tool.as_ref(), "args": args }),
        ),
        RunEvent::ToolEnd {
            call_id,
            is_error,
            parent: None,
            ..
        } => (
            "tool_end",
            json!({ "kind": "tool_end", "turn": turn, "id": call_id,
                    "name": state.names.get(call_id), "ok": !is_error }),
        ),
        RunEvent::Retry { attempt, error, .. } => (
            "retry",
            json!({ "kind": "retry", "turn": turn, "attempt": attempt,
                    "error": error }),
        ),
        RunEvent::Steered { text, .. } => (
            "steered",
            json!({ "kind": "steered", "turn": turn, "text": text }),
        ),
        RunEvent::Continued {
            plugin, message, ..
        } => (
            "continued",
            json!({ "kind": "continued", "turn": turn,
                    "plugin": plugin.as_ref(), "message": message }),
        ),
        RunEvent::ContextRewritten {
            plugin,
            tokens_before,
            tokens_after,
            ..
        } => (
            "context_rewritten",
            json!({ "kind": "context_rewritten", "turn": turn,
                    "plugin": plugin.as_ref(), "tokens_before": tokens_before,
                    "tokens_after": tokens_after }),
        ),
        RunEvent::PluginError {
            plugin, message, ..
        } => (
            "plugin_error",
            json!({ "kind": "plugin_error", "turn": turn,
                    "plugin": plugin.as_ref(), "message": message }),
        ),
        _ => return None,
    })
}

fn hook_name(hook: &Hook) -> String {
    match hook {
        Hook::Tool(name) => format!("tool {name}"),
        Hook::Card(name) => format!("card of {name}"),
        Hook::Start => "start".into(),
        Hook::BeforeTool => "before_tool".into(),
        Hook::BeforeRequest => "before_request".into(),
        Hook::AfterToolResult => "after_tool_result".into(),
        Hook::RewriteContext => "rewrite_context".into(),
        Hook::Rewritten => "rewritten".into(),
        Hook::BeforeStop => "before_stop".into(),
        Hook::TurnEnd => "turn_end".into(),
        Hook::OnEvent => "on_event".into(),
        Hook::Title => "title".into(),
        Hook::Notify => "notify".into(),
        Hook::Forge(name) => format!("forge.{name}"),
        Hook::Job(name) => format!("jobs.{name}"),
        Hook::RepoData => "repo_data".into(),
        Hook::App => "app".into(),
        Hook::AppPage => "page".into(),
        Hook::AppSetup => "setup".into(),
        Hook::AppModels => "models".into(),
        Hook::ListCommands => "commands.list".into(),
        Hook::Recover => "recover".into(),
        Hook::Page(name) => format!("pages.{name}"),
        Hook::RunEnd => "run_end".into(),
        Hook::View => "view".into(),
        Hook::SettingsView => "settings.view".into(),
        Hook::Action(name) => format!("action {name}"),
    }
}

#[async_trait]
impl Plugin for LuauPlugins {
    fn name(&self) -> &str {
        NAME
    }

    /// The tools of the versions active as the run starts, declared to
    /// the model.
    fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        self.tools_now()
    }

    /// The tools of the versions active now: those that came after the
    /// run started, for code mode.
    fn tool_source(&self) -> Option<Arc<dyn ToolSource>> {
        Some(Arc::new(LiveTools(self.clone())))
    }

    async fn start(
        &self,
        plan: &mut RunPlan,
        ctx: &PluginCtx,
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        let shared = self.0.run_state(&ctx.run);
        {
            let mut state = shared.lock().await;
            inherit(plan, &mut state);
            for active in self.0.active().iter() {
                self.0.draw(active, &mut state, ctx, None).await;
            }
            // What each plugin adds to what the run starts with; the
            // ones after it see it.
            let mut replaced = false;
            for active in self.0.active().iter() {
                // The prefix a change of effort resends uncached: what
                // the run inherits, and its instructions.
                let prefix = plan.inherited_tokens()
                    + tau_agent::output::estimate_tokens(
                        plan.instructions.as_deref().unwrap_or_default(),
                    );
                let mut input = json!({
                    "input": plan.input,
                    "context": plan.context,
                    "instructions": plan.instructions,
                    "model": plan.model(),
                    // The efforts the run's model takes, lowest first.
                    "efforts": tau_ai::model::efforts(plan.model())
                        .into_iter()
                        .map(ReasoningEffort::as_str)
                        .collect::<Vec<_>>(),
                    "prefix_tokens": prefix,
                });
                // Left out on auto, so it is `nil` in Luau, not `json.null`.
                if let Some(effort) = plan.reasoning {
                    input["effort"] = json!(effort.as_str());
                }
                let Some(answer) = self
                    .0
                    .call(
                        active,
                        Hook::Start,
                        input,
                        &mut state,
                        ctx,
                        self.0.reach_with_files(active, ctx),
                    )
                    .await
                else {
                    continue;
                };
                // A plugin that cannot check the run ends it.
                if let Some(why) = answer["fail"].as_str() {
                    let name = &active.loaded.declaration.name;
                    return Err(PluginError::Message(format!("{name}: {why}")));
                }
                apply_start(plan, &answer, &mut replaced);
            }
        }
        Ok(Box::new(LuauRun {
            plugins: self.clone(),
            state: shared,
        }))
    }

    /// An idle run's compaction takes the plugins in when one of them
    /// rewrites the context.
    async fn start_idle(
        &self,
        plan: &RunPlan,
        ctx: &PluginCtx,
    ) -> Result<Option<Box<dyn PluginRun>>, PluginError> {
        if !self
            .0
            .active()
            .iter()
            .any(|active| active.loaded.declaration.hooks.rewrite_context)
        {
            return Ok(None);
        }
        let shared = self.0.run_state(&ctx.run);
        inherit(plan, &mut *shared.lock().await);
        Ok(Some(Box::new(LuauRun {
            plugins: self.clone(),
            state: shared,
        })))
    }
}

/// The state a fork, a resumed or an idle run inherits: each plugin's
/// last.
fn inherit(plan: &RunPlan, state: &mut RunState) {
    for (plugin, kept) in tau_luau_plugins::last_states(plan.records()) {
        state.stored.insert(plugin.clone(), kept.clone());
        state.states.insert(plugin, kept);
    }
}

/// The plugins' part in one run.
struct LuauRun {
    plugins: LuauPlugins,
    state: Arc<tokio::sync::Mutex<RunState>>,
}

#[async_trait]
impl PluginRun for LuauRun {
    async fn before_tool(
        &mut self,
        call: &mut ToolCall,
        ctx: &PluginCtx,
    ) -> Result<Decision, PluginError> {
        let inner = &self.plugins.0;
        let mut state = self.state.lock().await;
        let mut input =
            json!({ "id": call.id, "name": call.name, "args": call.args });
        for active in inner.active().iter() {
            let Some(answer) = inner
                .call(
                    active,
                    Hook::BeforeTool,
                    input.clone(),
                    &mut state,
                    ctx,
                    inner.reach(active, ctx, None),
                )
                .await
            else {
                continue;
            };
            let reason =
                answer["reason"].as_str().unwrap_or_default().to_owned();
            let plugin = &active.loaded.declaration;
            match answer["decision"].as_str() {
                Some("block") => return Ok(Decision::Block(reason)),
                // The plugins after it see the call as it changed it.
                Some("change") if plugin.uses.change_args => {
                    call.args = answer["args"].clone();
                    input["args"] = call.args.clone();
                    ctx.publish(&Record::Flag {
                        plugin: plugin.name.clone(),
                        call_id: call.id.clone(),
                        reason: "changed its arguments".into(),
                    })
                    .await;
                }
                Some("change") => {
                    ctx.publish(&Record::Error {
                        plugin: plugin.name.clone(),
                        hook: hook_name(&Hook::BeforeTool),
                        error: "changing a call's arguments needs \
                                `uses.change_args`"
                            .into(),
                    })
                    .await;
                }
                Some("flag") => {
                    ctx.publish(&Record::Flag {
                        plugin: active.loaded.declaration.name.clone(),
                        call_id: call.id.clone(),
                        reason,
                    })
                    .await;
                }
                _ => {}
            }
        }
        Ok(Decision::Allow)
    }

    async fn rewrite_context(
        &mut self,
        view: &ContextView<'_>,
        ctx: &PluginCtx,
    ) -> Result<Option<Rewrite>, PluginError> {
        let inner = &self.plugins.0;
        if !inner
            .active()
            .iter()
            .any(|active| active.loaded.declaration.hooks.rewrite_context)
        {
            return Ok(None);
        }
        let mut input = json!({
            "trigger": view.trigger,
            "turn": view.turn,
            "tokens": view.tokens,
            "messages": crate::rewrite::summary(view.transcript),
        });
        // Left out when the model's is not known, so it is `nil` in Luau.
        if let Some(window) = view.window {
            input["window"] = json!(window);
        }
        let mut state = self.state.lock().await;
        // What a hook asks in the run's conversation (`infer{
        // conversation }`), asked here, which holds it.
        let (asks, mut asked) = tokio::sync::mpsc::channel(8);
        // Why the plugins that failed did: the run hears it when none
        // rewrites, as a failed overflow's error.
        let mut failed = Vec::new();
        // The first plugin that rewrites wins.
        for active in inner.active().iter() {
            let hook = Hook::RewriteContext;
            let reach = inner.reach_asking(
                active,
                ctx,
                Some(view.transcript),
                Some(asks.clone()),
                Some(view.transcript),
            );
            let run = inner.run_hook(
                active,
                &hook,
                input.clone(),
                &mut state,
                ctx,
                reach,
            );
            let Some(outcome) =
                in_conversation(run, &mut asked, view, ctx).await
            else {
                continue;
            };
            let name = &active.loaded.declaration.name;
            if let Some(error) = &outcome.error {
                failed.push(format!("{name}: {error}"));
            }
            let Some(answer) =
                inner.apply(active, &hook, outcome, &mut state, ctx).await
            else {
                continue;
            };
            if answer["rewrite"].is_null() {
                continue;
            }
            match crate::rewrite::rebuild(
                view.transcript,
                &answer["rewrite"],
                name,
                ctx.now(),
            ) {
                Ok(Some(rewrite)) => return Ok(Some(rewrite)),
                Ok(None) => {}
                Err(error) => {
                    failed.push(format!("{name}: {error}"));
                    ctx.publish(&Record::Error {
                        plugin: name.clone(),
                        hook: hook_name(&hook),
                        error,
                    })
                    .await;
                }
            }
        }
        if failed.is_empty() {
            return Ok(None);
        }
        Err(PluginError::Message(failed.join("; ")))
    }

    async fn rewritten(
        &mut self,
        replaced: &[tau_ai::message::Message],
        rewrite: &Rewrite,
        ctx: &PluginCtx,
    ) -> Result<(), PluginError> {
        let inner = &self.plugins.0;
        let input = json!({
            "plugin": rewrite.details["plugin"],
            "before": replaced.len(),
            "after": rewrite.messages.len(),
            "drops_conversation": rewrite.drops_conversation,
        });
        let mut state = self.state.lock().await;
        for active in inner.active().iter() {
            inner
                .call(
                    active,
                    Hook::Rewritten,
                    input.clone(),
                    &mut state,
                    ctx,
                    inner.reach_with_files_and(active, ctx, Some(replaced)),
                )
                .await;
        }
        Ok(())
    }

    async fn after_tool_result(
        &mut self,
        view: &ToolResultView<'_>,
        output: &mut ToolOutput,
        ctx: &PluginCtx,
    ) -> Result<(), PluginError> {
        let inner = &self.plugins.0;
        if !inner
            .active()
            .iter()
            .any(|active| active.loaded.declaration.hooks.after_tool_result)
        {
            return Ok(());
        }
        let mut state = self.state.lock().await;
        // The conversation as the call found it: what came before its
        // turn, and the message that made it.
        let mut seen = view.transcript.to_vec();
        seen.push(Message::Assistant(view.message.clone()));
        for active in inner.active().iter() {
            let (text, truncated) = result_text(output);
            let mut input = json!({
                "id": view.call.id,
                "name": view.call.name,
                "args": view.call.args,
                "ok": !view.is_error,
                "text": text,
                "truncated": truncated,
            });
            let images = output
                .content
                .iter()
                .filter(|block| matches!(block, InputBlock::Image(_)))
                .count();
            if images > 0 {
                input["images"] = json!(images);
            }
            // A call a tool made goes back to that tool, never to the
            // model.
            if view.call.parent.is_some() {
                input["nested"] = json!(true);
            }
            let Some(answer) = inner
                .call(
                    active,
                    Hook::AfterToolResult,
                    input,
                    &mut state,
                    ctx,
                    inner.reach_with_files_and(active, ctx, Some(&seen)),
                )
                .await
            else {
                continue;
            };
            // The plugins after it read the result as it left it.
            if let Some(text) = answer["text"].as_str() {
                replace_text(output, text);
                if answer["flag"] != Value::Bool(false) {
                    ctx.publish(&Record::Flag {
                        plugin: active.loaded.declaration.name.clone(),
                        call_id: view.call.id.clone(),
                        reason: "changed what the model reads of it".into(),
                    })
                    .await;
                }
            }
        }
        Ok(())
    }

    async fn before_stop(
        &mut self,
        message: &AssistantMessage,
        ctx: &PluginCtx,
    ) -> Result<StopDecision, PluginError> {
        let inner = &self.plugins.0;
        let mut state = self.state.lock().await;
        let text: String = message
            .content
            .iter()
            .filter_map(|block| match block {
                AssistantBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect();
        let input =
            json!({ "text": text, "turn": state.turn, "reason": "stop" });
        for active in inner.active().iter() {
            let name = &active.loaded.declaration.name;
            let most = active
                .loaded
                .declaration
                .continues
                .unwrap_or(MAX_CONTINUATIONS)
                .min(MOST_CONTINUATIONS);
            if state.continued.get(name).copied().unwrap_or(0) >= most {
                continue;
            }
            let Some(answer) = inner
                .call(
                    active,
                    Hook::BeforeStop,
                    input.clone(),
                    &mut state,
                    ctx,
                    inner.reach(active, ctx, None),
                )
                .await
            else {
                continue;
            };
            if answer["decision"] == "continue" {
                *state.continued.entry(name.clone()).or_default() += 1;
                let message =
                    answer["message"].as_str().unwrap_or_default().to_owned();
                return Ok(StopDecision::Continue(message));
            }
        }
        Ok(StopDecision::Stop)
    }

    /// Takes the versions active now, before each request, and draws
    /// the view of each plugin whose version changed or that is new.
    async fn before_request(
        &mut self,
        view: &RequestView<'_>,
        ctx: &PluginCtx,
    ) -> Result<Option<ReasoningEffort>, PluginError> {
        let inner = &self.plugins.0;
        inner.refresh().await;
        let mut state = self.state.lock().await;
        for active in inner.active().iter() {
            let name = &active.loaded.declaration.name;
            if state.drawn.get(name) != Some(&active.loaded.digest) {
                inner.draw(active, &mut state, ctx, None).await;
            }
        }
        // The first plugin that picks an effort sets it.
        let mut input = json!({
            "model": view.model,
            "turn": view.turn,
            "messages": view.transcript.len(),
            "replies": view
                .transcript
                .iter()
                .filter(|message| matches!(message, Message::Assistant(_)))
                .count(),
        });
        // Left out at the model's default, so it is `nil` in Luau.
        if let Some(effort) = view.effort {
            input["effort"] = json!(effort);
        }
        for active in inner.active().iter() {
            let Some(answer) = inner
                .call(
                    active,
                    Hook::BeforeRequest,
                    input.clone(),
                    &mut state,
                    ctx,
                    inner.reach(active, ctx, Some(view.transcript)),
                )
                .await
            else {
                continue;
            };
            if let Ok(effort) = serde_json::from_value::<ReasoningEffort>(
                answer["effort"].clone(),
            ) {
                return Ok(Some(effort));
            }
        }
        Ok(None)
    }

    async fn on_event(&mut self, event: &RunEvent, ctx: &PluginCtx) {
        let inner = &self.plugins.0;
        let mut state = self.state.lock().await;
        if let Some((kind, input)) = luau_event(event, &state) {
            for active in inner.active().iter() {
                if !active.loaded.declaration.events.iter().any(|e| e == kind) {
                    continue;
                }
                inner
                    .call(
                        active,
                        Hook::OnEvent,
                        input.clone(),
                        &mut state,
                        ctx,
                        Arc::new(NoReach),
                    )
                    .await;
            }
        }
        match event {
            RunEvent::TurnStart { turn, .. } => {
                state.turn = *turn;
                state.text.clear();
                state.calls.clear();
            }
            RunEvent::TextDelta {
                parent: None,
                delta,
                ..
            } => state.text.push_str(delta),
            RunEvent::ToolStart {
                call_id,
                tool,
                parent: None,
                ..
            } => {
                state.names.insert(call_id.clone(), tool.to_string());
            }
            RunEvent::ToolEnd {
                call_id,
                is_error,
                parent: None,
                ..
            } => {
                let name = state.names.remove(call_id).unwrap_or_default();
                state.calls.push(json!({ "name": name, "ok": !is_error }));
            }
            RunEvent::TurnEnd { turn, usage, .. } => {
                let mut input = json!({
                    "turn": turn, "text": state.text, "calls": state.calls,
                    "cost": usage.cost.total,
                });
                // The turn's commit, which the run's workspace made as it
                // heard the turn end, before the plugins hear it.
                let commit = inner.commit.lock().expect("not poisoned").take();
                if let Some(commit) = commit {
                    input["commit"] = json!({
                        "change": commit.change_id,
                        "paths": commit.paths,
                    });
                }
                for active in inner.active().iter() {
                    inner
                        .call(
                            active,
                            Hook::TurnEnd,
                            input.clone(),
                            &mut state,
                            ctx,
                            inner.reach_with_files(active, ctx),
                        )
                        .await;
                }
            }
            _ => {}
        }
    }

    async fn finish(&mut self, run: &FinishedRun<'_>, ctx: &PluginCtx) {
        let inner = &self.plugins.0;
        {
            let mut state = self.state.lock().await;
            let input = json!({
                "stop": format!("{:?}", run.stop),
                "turns": state.turn,
                "cost": run.usage.cost.total,
                "text": run.text,
            });
            for active in inner.active().iter() {
                inner
                    .call(
                        active,
                        Hook::RunEnd,
                        input.clone(),
                        &mut state,
                        ctx,
                        inner.reach_with_files_and(
                            active,
                            ctx,
                            Some(run.transcript),
                        ),
                    )
                    .await;
            }
        }
        inner.runs.lock().expect("not poisoned").remove(&ctx.run);
    }
}

impl LuauPlugins {
    /// The tools of the versions active now.
    fn tools_now(&self) -> Vec<Arc<dyn AgentTool>> {
        self.0
            .active()
            .iter()
            .flat_map(|active| {
                active
                    .loaded
                    .declaration
                    .tools
                    .iter()
                    .filter(|spec| spec.offered(&active.settings))
                    .map(|spec| {
                        Arc::new(LuauTool {
                            plugins: self.clone(),
                            plugin: active.loaded.declaration.name.clone(),
                            spec: spec.clone(),
                        }) as Arc<dyn AgentTool>
                    })
            })
            .collect()
    }
}

/// The plugins' tools as they are now, for code mode: a run's own tools
/// hide those of the same name, so this offers the ones that came after
/// it started.
struct LiveTools(LuauPlugins);

#[async_trait]
impl ToolSource for LiveTools {
    fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        self.0.tools_now()
    }
}

/// A tool a Luau plugin adds. A call goes to the plugin's version of the
/// moment.
struct LuauTool {
    plugins: LuauPlugins,
    plugin: String,
    spec: ToolSpec,
}

#[async_trait]
impl AgentTool for LuauTool {
    fn name(&self) -> &str {
        &self.spec.name
    }

    fn description(&self) -> &str {
        &self.spec.description
    }

    fn parameters(&self) -> &Value {
        &self.spec.parameters
    }

    /// A tool that waits for the person is the model's alone: a script's
    /// call would end with the script, the person still answering.
    fn exposure(&self) -> tau_agent::tool::Exposure {
        if self.spec.model_only {
            tau_agent::tool::Exposure::ModelOnly
        } else {
            tau_agent::tool::Exposure::default()
        }
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let inner = &self.plugins.0;
        let Some(plugin) = ctx.plugin().cloned() else {
            return Err("a Luau plugin's tool runs only in a run".into());
        };
        let shared = inner.run_state(&ctx.run);
        let mut state = shared.lock().await;
        let tool = Hook::Tool(self.spec.name.clone());
        let Some(active) = inner.plugin(&self.plugin).filter(|active| {
            active.loaded.declaration.tools.iter().any(|spec| {
                spec.name == self.spec.name && spec.offered(&active.settings)
            })
        }) else {
            return Err(format!(
                "{} has no tool {} any more",
                self.plugin, self.spec.name
            )
            .into());
        };
        let files = inner.files_of(&active);
        let reach: Arc<dyn Reach> = Arc::new(ToolReach {
            ctx: ctx.clone(),
            plugin: plugin.clone(),
            jev: inner.jev.clone(),
            files,
        });
        let failures = state.failures.get(&self.plugin).copied().unwrap_or(0);
        let outcome = inner
            .run_hook_of(
                &active,
                &tool,
                args.clone(),
                &mut state,
                &plugin,
                reach,
                Some(ctx.call_id()),
            )
            .await;
        let value = match outcome {
            // An error the tool raised is its answer, as a value is: the
            // model reads it, and its plugin is not at fault (ADR 0027).
            Some(Outcome {
                error: Some(error),
                raised: true,
                ..
            }) => {
                plugin
                    .publish(&Record::Log {
                        plugin: self.plugin.clone(),
                        lines: vec![format!("{}: {error}", self.spec.name)],
                    })
                    .await;
                return Err(error.into());
            }
            Some(outcome) => {
                inner
                    .apply(&active, &tool, outcome, &mut state, &plugin)
                    .await
            }
            None => None,
        };
        let Some(value) = value else {
            if inner.off(&state, &self.plugin) && failures >= MAX_FAILURES {
                return Err(format!(
                    "{} is off for this run: {MAX_FAILURES} of its hooks failed",
                    self.plugin
                )
                .into());
            }
            return Err(format!(
                "{} failed; its plugin's page says why",
                self.spec.name
            )
            .into());
        };
        if let Some(asked) = crate::chats::ask(&value) {
            drop(state);
            return self.ask_first(asked?, &ctx, &plugin).await;
        }
        if let Some(form) = crate::forms::asked(&value) {
            drop(state);
            return self.ask_form(form?, &ctx, &plugin).await;
        }
        let card = if self.spec.card {
            let input = json!({ "call": args, "result": value });
            inner
                .call(
                    &active,
                    Hook::Card(self.spec.name.clone()),
                    input,
                    &mut state,
                    &plugin,
                    Arc::new(NoReach),
                )
                .await
        } else {
            None
        };
        let text = match &value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        Ok(ToolOutput {
            details: Some(json!({
                "plugin": self.plugin,
                "value": value,
                "card": card,
            })),
            ..ToolOutput::text(text)
        })
    }
}

impl LuauTool {
    /// Asks the person to allow what the call asked (`tau.allow`), with
    /// no hook going meanwhile: a card in the run and on phones, which
    /// the first answer takes. Allowed, the plugin's action carries it
    /// out as the person's own, and the model reads what it answered;
    /// denied, the model reads why; the run ending first withdraws it.
    async fn ask_first(
        &self,
        ask: crate::chats::Ask,
        ctx: &ToolCtx,
        plugin: &PluginCtx,
    ) -> Result<ToolOutput, ToolError> {
        use tau_luau_plugins::Outcome;
        let inner = &self.plugins.0;
        let dispatch = inner
            .dispatch
            .as_ref()
            .ok_or("this host asks the person nothing")?;
        let repo = inner.run["repo"]
            .as_str()
            .ok_or("this run works in no repository")?
            .to_owned();
        let call = ctx.call_id().to_owned();
        let waiting = dispatch.allows.wait(&ctx.run.0, &call, ());
        plugin
            .publish(&Record::Asked {
                plugin: self.plugin.clone(),
                call_id: call.clone(),
                title: ask.title.clone(),
                text: ask.text.clone(),
            })
            .await;
        // A plugin's records reach the interface with the run's next
        // event, and none comes while the call waits: this update is one,
        // so the Allow card shows now.
        ctx.updates.send(ToolOutput::text(format!(
            "Waiting for the person: {}",
            ask.title
        )));
        // A run that ended takes no answer, even one that came as it did.
        let answer = tokio::select! {
            biased;
            () = ctx.cancel.cancelled() => None,
            answer = waiting.answer() => answer,
        }
        .filter(|_| !ctx.cancel.is_cancelled());
        let outcome = match &answer {
            Some(answer) if answer.allowed => Outcome::Allowed,
            Some(_) => Outcome::Denied,
            None => Outcome::Withdrawn,
        };
        plugin
            .publish(&Record::Answered {
                plugin: self.plugin.clone(),
                call_id: call,
                outcome,
            })
            .await;
        let answer =
            answer.ok_or("the run ended before the person answered")?;
        if !answer.allowed {
            return Ok(ToolOutput::text(if answer.reason.is_empty() {
                "The person did not allow it.".to_owned()
            } else {
                format!("The person did not allow it: {}", answer.reason)
            }));
        }
        let value = dispatch
            .registry
            .repo_action(
                &self.plugin,
                &repo,
                &ask.action,
                ask.args,
                dispatch.chats.as_deref(),
                // What a call the person allowed does is the tool's,
                // with no Jev of its own.
                None,
            )
            .await
            .map_err(|error| format!("{error:#}"))?;
        let text = match &value {
            Value::String(text) => text.clone(),
            Value::Null => "The person allowed it, and it is done.".to_owned(),
            other => other.to_string(),
        };
        Ok(ToolOutput {
            details: Some(json!({ "plugin": self.plugin, "value": value })),
            ..ToolOutput::text(text)
        })
    }
}

impl LuauTool {
    /// Asks the person the form the call asked (`tau.ask`), with no hook
    /// going meanwhile: the plugin's view draws it, in the run and on
    /// phones, and the first answers that fit, from one of its actions,
    /// take it. Then its `action` is called in the run with them, and
    /// the model reads what it answers; a run cancelled first ends the
    /// call with an error.
    async fn ask_form(
        &self,
        form: crate::forms::Form,
        ctx: &ToolCtx,
        plugin: &PluginCtx,
    ) -> Result<ToolOutput, ToolError> {
        let inner = &self.plugins.0;
        let dispatch = inner
            .dispatch
            .as_ref()
            .ok_or("this host asks the person nothing")?;
        let call = ctx.call_id().to_owned();
        let action = form.action.clone();
        let waiting = dispatch.forms.wait(
            &ctx.run.0,
            &call,
            crate::forms::Asked {
                plugin: self.plugin.clone(),
                form,
            },
        );
        // A plugin's records reach the interface with the run's next
        // event, and none comes while the call waits: this update is one,
        // so the form shows now.
        ctx.updates
            .send(ToolOutput::text("Waiting for the person to answer."));
        // A run that ended takes no answer, even one that came as it did.
        let reply = tokio::select! {
            biased;
            () = ctx.cancel.cancelled() => None,
            reply = waiting.answer() => reply,
        }
        .filter(|_| !ctx.cancel.is_cancelled());
        let reply =
            reply.ok_or("The run was cancelled before the person answered.")?;
        let mut given = serde_json::to_value(&reply)?;
        let value = match action {
            Some(action) => {
                given["call"] = json!(call);
                let shared = inner.run_state(&ctx.run);
                let mut state = shared.lock().await;
                let Some(active) = inner.plugin(&self.plugin) else {
                    return Err(format!(
                        "{} is not active any more",
                        self.plugin
                    )
                    .into());
                };
                inner
                    .call(
                        &active,
                        Hook::Action(action),
                        given.clone(),
                        &mut state,
                        plugin,
                        Arc::new(NoReach),
                    )
                    .await
                    .ok_or_else(|| {
                        format!(
                            "{} could not take the answers; its plugin's page \
                             says why",
                            self.spec.name
                        )
                    })?
            }
            None => given.clone(),
        };
        let text = match &value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        let details = json!({ "plugin": self.plugin, "value": given });
        Ok(ToolOutput {
            details: Some(details),
            structured: Some(given),
            ..ToolOutput::text(text)
        })
    }
}

/// What a plugin's tool handler reaches: the run's tools, through the
/// loop, and Jev.
struct ToolReach {
    ctx: ToolCtx,
    plugin: PluginCtx,
    jev: Option<Arc<dyn Jev>>,
    /// What `ctx.files` and `ctx.blobs` reach, with `uses.files`.
    files: Option<Arc<crate::files::Files>>,
}

#[async_trait]
impl Reach for ToolReach {
    fn tools(&self) -> Vec<ToolEntry> {
        let mut tools: Vec<ToolEntry> = self
            .ctx
            .catalog()
            .tools()
            .iter()
            .map(|tool| ToolEntry {
                name: tool.name().to_owned(),
                description: tool.description().to_owned(),
                input_schema: tool.parameters().clone(),
                output_schema: tool.output_schema().cloned(),
                namespace: None,
                sequential: false,
            })
            .collect();
        if self.files.is_some() {
            tools.extend(crate::files::Files::entries());
        }
        tools
    }

    async fn call(&self, call: ScriptCall) -> Result<ToolReply, String> {
        if let Some(files) = &self.files
            && crate::files::Files::handles(&call.name)
        {
            return files.call(&call.name, &call.args).await;
        }
        match self.ctx.call(&call.name, call.args).await {
            Ok(output) => Ok(ToolReply::success(
                output
                    .structured
                    .clone()
                    .unwrap_or_else(|| json!(output.text_content())),
            )),
            Err(error) => Err(error.to_string()),
        }
    }

    fn jev(&self) -> Option<Arc<dyn Jev>> {
        self.jev.clone()
    }

    fn charge(&self, usage: &Usage) {
        self.plugin.charge(usage);
    }
}

/// What a run hook reaches besides a tool's call (ADR 0050): the model,
/// as `infer`, and Jev, both charged to the run and cancelled with it.
struct RunReach {
    plugin: PluginCtx,
    jev: Option<Arc<dyn Jev>>,
    /// The model `uses.model` names, if it names one.
    named: Option<String>,
    /// The run's model: what `infer` asks when neither the call nor
    /// `uses.model` names one.
    run_model: String,
    /// The run's conversation, when the hook has it and the plugin uses
    /// it.
    transcript: Option<Arc<[Message]>>,
    /// Where `infer{ conversation }` asks, in the hook that holds the
    /// run's conversation.
    conversation: Option<Asks>,
    /// The transcript `ctx.measure` sizes rewrites of, in
    /// `rewrite_context`.
    rewriting: Option<Arc<[Message]>>,
    /// Where `ctx.archive` keeps its files.
    archive: std::path::PathBuf,
}

#[async_trait]
impl Reach for RunReach {
    fn tools(&self) -> Vec<ToolEntry> {
        let mut tools = vec![
            ToolEntry {
                name: "infer".into(),
                description: "Asks the model one task.".into(),
                input_schema: crate::infer::parameters(),
                output_schema: None,
                namespace: None,
                sequential: false,
            },
            ToolEntry {
                name: "infer_turn".into(),
                description: "Asks the model one round of a task, offering \
                              the plugin's own tools."
                    .into(),
                input_schema: json!({ "type": "object" }),
                output_schema: None,
                namespace: None,
                sequential: false,
            },
        ];
        let entry = |name: &str, description: &str| ToolEntry {
            name: name.into(),
            description: description.into(),
            input_schema: json!({ "type": "object" }),
            output_schema: None,
            namespace: None,
            sequential: false,
        };
        if self.transcript.is_some() {
            tools.push(entry("transcript", "The run's conversation, whole."));
        }
        if self.rewriting.is_some() {
            tools.push(entry(
                "measure",
                "The transcript's JSON bytes, and a rewrite's.",
            ));
        }
        tools.push(entry("archive_path", "A new archive file's path."));
        tools.push(entry("archive_write", "Writes an archive file."));
        tools
    }

    async fn call(&self, call: ScriptCall) -> Result<ToolReply, String> {
        if let ("transcript", Some(transcript)) =
            (call.name.as_str(), &self.transcript)
        {
            return Ok(ToolReply::success(crate::rewrite::transcript(
                transcript,
            )));
        }
        if let ("measure", Some(transcript)) =
            (call.name.as_str(), &self.rewriting)
        {
            return crate::rewrite::measure(transcript, &call.args)
                .map(ToolReply::success);
        }
        match call.name.as_str() {
            "archive_path" => {
                let kind = call.args["kind"].as_str().unwrap_or("file");
                return Ok(ToolReply::success(json!(
                    crate::archive::new_path(&self.archive, kind)
                        .display()
                        .to_string()
                )));
            }
            "archive_write" => {
                let (Some(path), Some(text)) =
                    (call.args["path"].as_str(), call.args["text"].as_str())
                else {
                    return Err("archive_write needs `path` and `text`".into());
                };
                crate::archive::write(&self.archive, path, text)?;
                return Ok(ToolReply::success(Value::Null));
            }
            _ => {}
        }
        if call.name != "infer" && call.name != "infer_turn" {
            return Err(format!(
                "`{}` cannot be called from this hook",
                call.name
            ));
        }
        let answered = if call.name == "infer_turn" {
            self.infer_turn(call.args).await
        } else {
            self.infer(call.args).await
        };
        // As `infer` answers in a tool: `{ ok, value }`, or `{ ok, error }`
        // with the `reason` an answer was refused for.
        let reply = match answered {
            Ok(value) => json!({ "ok": true, "value": value }),
            Err(crate::infer::Refused { error, reason }) => {
                let mut reply = json!({ "ok": false, "error": error });
                if let Some(reason) = reason {
                    reply["reason"] = json!(reason);
                }
                reply
            }
        };
        Ok(ToolReply::success(reply))
    }

    fn jev(&self) -> Option<Arc<dyn Jev>> {
        self.jev.clone()
    }

    fn charge(&self, usage: &Usage) {
        self.plugin.charge(usage);
    }
}

impl RunReach {
    async fn infer(&self, args: Value) -> Result<Value, Refused> {
        use tau_ai::message::{UserContent, UserMessage};
        use tau_codemode_host::inference::{INSTRUCTIONS, InferRequest};

        use crate::infer::{Own, answer_text};
        if args.get("conversation").is_some()
            || args.get("instructions").is_some()
        {
            return self.infer_own(Own::parse(args)?).await;
        }
        let request = InferRequest::parse(args)?;
        let named = request.model.as_deref().or(self.named.as_deref());
        let model = crate::slots::model_named(named, Some(&self.run_model))
            .unwrap_or_default();
        let settings = tau_ai::responses::request::Settings {
            reasoning: crate::slots::effort_for(&model, request.effort),
            model,
            instructions: Some(INSTRUCTIONS.into()),
            text_format: request.text_format(),
            ..Default::default()
        };
        let input = [Message::User(UserMessage {
            content: UserContent::Text(request.input_text()),
            timestamp: self.plugin.now(),
        })];
        let answer = self
            .plugin
            .ask(settings, &input)
            .await
            .map_err(|error| error.to_string())?;
        Ok(request.decode_answer(&answer_text(&answer)?)?)
    }

    /// One round of `infer{ tools }` (ADR 0050, gap L): the task and the
    /// rounds before it, offering the plugin's own tools, with its
    /// instructions or [`crate::infer::TOOL_INSTRUCTIONS`].
    async fn infer_turn(&self, args: Value) -> Result<Value, Refused> {
        let round = crate::infer::Round::parse(args)?;
        let named = round.model.as_deref().or(self.named.as_deref());
        let model = crate::slots::model_named(named, Some(&self.run_model))
            .unwrap_or_default();
        let settings = tau_ai::responses::request::Settings {
            reasoning: crate::slots::effort_for(&model, round.effort),
            model,
            instructions: Some(round.instructions()),
            tools: round.tools.clone(),
            ..Default::default()
        };
        let answer = self
            .plugin
            .ask(settings, &round.messages(self.plugin.now()))
            .await
            .map_err(|error| error.to_string())?;
        crate::infer::round_answer(&answer)
    }

    /// A request the plugin writes itself: in the run's own conversation
    /// (ADR 0050, gap B), its first messages, then the message, on the
    /// run's connection with its instructions, tools and effort, so the
    /// part the conversation starts with reads the provider's cache; or
    /// with `instructions` of its own, on its own. The answer is its
    /// text.
    async fn infer_own(
        &self,
        own: crate::infer::Own,
    ) -> Result<Value, Refused> {
        use crate::infer::answer_text;
        let now = self.plugin.now();
        let message = crate::infer::said(true, own.blocks, now);
        let answer = match own.conversation {
            Some(first) => {
                let asks = self.conversation.as_ref().ok_or(
                    "`conversation` is for rewrite_context, which holds the \
                     run's conversation",
                )?;
                let (reply, answer) = tokio::sync::oneshot::channel();
                asks.send((first, message, reply))
                    .await
                    .map_err(|_| "the run's conversation is gone")?;
                answer
                    .await
                    .map_err(|_| "the run's conversation is gone")??
            }
            None => {
                let named = own.model.as_deref().or(self.named.as_deref());
                let model =
                    crate::slots::model_named(named, Some(&self.run_model))
                        .unwrap_or_default();
                let settings = tau_ai::responses::request::Settings {
                    reasoning: crate::slots::effort_for(&model, own.effort),
                    model,
                    instructions: own.instructions,
                    ..Default::default()
                };
                let mut input: Vec<Message> = own
                    .earlier
                    .into_iter()
                    .map(|(person, blocks)| {
                        crate::infer::said(person, blocks, now)
                    })
                    .collect();
                input.push(message);
                self.plugin
                    .ask(settings, &input)
                    .await
                    .map_err(|error| error.to_string())?
            }
        };
        Ok(Value::String(answer_text(&answer)?))
    }
}

/// `{ unix, iso, weekday }` for now, in UTC, with the host's `local`
/// time (`YYYY-MM-DD HH:MM`) and its `offset` from UTC, in seconds
/// (ADR 0050, gap D).
pub fn now() -> Value {
    use tau_clock::Clock as _;
    let clock = tau_clock::SystemClock;
    let unix = clock.now();
    let mut now = at(unix);
    now["local"] = json!(tau_clock::local(&clock, unix));
    now["offset"] = json!(clock.offset(unix));
    // Milliseconds too, for what keeps times finer than a second.
    now["ms"] = json!(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(unix.saturating_mul(1000), |since| since.as_millis()
                as i64)
    );
    now
}

/// `{ unix, iso, weekday }` for `unix`, in UTC.
pub fn at(unix: i64) -> Value {
    const DAYS: [&str; 7] = [
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
    ];
    let days = unix.div_euclid(86_400);
    let seconds = unix.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    json!({
        "unix": unix,
        "iso": format!(
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
            seconds / 3_600,
            seconds % 3_600 / 60,
            seconds % 60
        ),
        "weekday": DAYS[days.rem_euclid(7) as usize],
    })
}
