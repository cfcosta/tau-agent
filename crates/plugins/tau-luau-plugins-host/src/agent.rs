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
    time::{SystemTime, UNIX_EPOCH},
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
    runtime::{Context, Hook, Loaded, NoReach, Outcome, Reach},
};

/// Failed hooks a plugin may have in a run before it is turned off.
pub const MAX_FAILURES: u32 = 3;

/// Times `before_stop` may keep a run going, all plugins together.
pub const MAX_CONTINUATIONS: u32 = 3;

/// A plugin as a run gets it: loaded, with the person's settings.
#[derive(Debug, Clone)]
pub struct Active {
    pub loaded: Loaded,
    pub settings: Value,
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
    continued: u32,
    turn: u32,
    /// The current turn's text and its calls, by id.
    text: String,
    names: HashMap<String, String>,
    calls: Vec<Value>,
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
            runs: Mutex::default(),
        }))
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
        let context = self.context(&ctx.run, state, active);
        let outcome = active
            .loaded
            .call(&hook, input, &context, reach, ctx.cancel.child_token())
            .await;
        self.apply(active, &hook, outcome, state, ctx).await
    }

    /// Takes each plugin's state stored with the run since this run last
    /// knew it: what an interface recorded meanwhile, such as an action
    /// a button carried out (ADR 0045).
    async fn catch_up(&self, state: &mut RunState, ctx: &PluginCtx) {
        let Ok(records) = ctx.records().await else {
            return;
        };
        let mut last: HashMap<String, Value> = HashMap::new();
        for record in records {
            if let Ok(Record::State {
                plugin,
                state: stored,
            }) = serde_json::from_value::<Record>(record)
            {
                last.insert(plugin, stored);
            }
        }
        for (plugin, stored) in last {
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
            ctx.publish(&Record::State {
                plugin: plugin.clone(),
                state: outcome.state,
            })
            .await;
            if !matches!(hook, Hook::View) {
                self.draw(active, state, ctx).await;
            }
        }
        Some(outcome.value)
    }

    /// Draws `active`'s view from its state and publishes it.
    async fn draw(
        &self,
        active: &Active,
        state: &mut RunState,
        ctx: &PluginCtx,
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
                ctx.publish(&Record::View {
                    plugin,
                    view: outcome.value,
                })
                .await;
            }
        }
    }
}

/// Adds what a plugin's `start` answered to `plan`: its texts after the
/// context's, its instructions after the run's.
fn apply_start(plan: &mut RunPlan, answer: &Value) {
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
            Some(own) => format!("{own}\n\n{added}"),
            None => added.to_owned(),
        });
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
                self.0.draw(active, &mut state, ctx).await;
            }
            // What each plugin adds to what the run starts with; the
            // ones after it see it.
            for active in self.0.active().iter() {
                let input = json!({
                    "input": plan.input,
                    "context": plan.context,
                    "instructions": plan.instructions,
                });
                let Some(answer) = self
                    .0
                    .call(
                        active,
                        Hook::Start,
                        input,
                        &mut state,
                        ctx,
                        Arc::new(NoReach),
                    )
                    .await
                else {
                    continue;
                };
                apply_start(plan, &answer);
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
    for record in plan.records() {
        let Ok(Record::State {
            plugin,
            state: kept,
        }) = serde_json::from_value::<Record>(record.clone())
        else {
            continue;
        };
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
                    Arc::new(NoReach),
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
        let input = json!({
            "trigger": view.trigger,
            "turn": view.turn,
            "tokens": view.tokens,
            "window": view.window,
            "messages": crate::rewrite::summary(view.transcript),
        });
        let mut state = self.state.lock().await;
        // The first plugin that rewrites wins.
        for active in inner.active().iter() {
            let Some(answer) = inner
                .call(
                    active,
                    Hook::RewriteContext,
                    input.clone(),
                    &mut state,
                    ctx,
                    Arc::new(NoReach),
                )
                .await
            else {
                continue;
            };
            if answer["rewrite"].is_null() {
                continue;
            }
            let name = &active.loaded.declaration.name;
            match crate::rewrite::rebuild(
                view.transcript,
                &answer["rewrite"],
                name,
                ctx.now(),
            ) {
                Ok(Some(rewrite)) => return Ok(Some(rewrite)),
                Ok(None) => {}
                Err(error) => {
                    ctx.publish(&Record::Error {
                        plugin: name.clone(),
                        hook: hook_name(&Hook::RewriteContext),
                        error,
                    })
                    .await;
                }
            }
        }
        Ok(None)
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
                    Arc::new(NoReach),
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
        for active in inner.active().iter() {
            let (text, truncated) = result_text(output);
            let input = json!({
                "id": view.call.id,
                "name": view.call.name,
                "args": view.call.args,
                "ok": !view.is_error,
                "text": text,
                "truncated": truncated,
            });
            let Some(answer) = inner
                .call(
                    active,
                    Hook::AfterToolResult,
                    input,
                    &mut state,
                    ctx,
                    Arc::new(NoReach),
                )
                .await
            else {
                continue;
            };
            // The plugins after it read the result as it left it.
            if let Some(text) = answer["text"].as_str() {
                replace_text(output, text);
                ctx.publish(&Record::Flag {
                    plugin: active.loaded.declaration.name.clone(),
                    call_id: view.call.id.clone(),
                    reason: "changed what the model reads of it".into(),
                })
                .await;
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
        if state.continued >= MAX_CONTINUATIONS {
            return Ok(StopDecision::Stop);
        }
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
            let Some(answer) = inner
                .call(
                    active,
                    Hook::BeforeStop,
                    input.clone(),
                    &mut state,
                    ctx,
                    Arc::new(NoReach),
                )
                .await
            else {
                continue;
            };
            if answer["decision"] == "continue" {
                state.continued += 1;
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
                inner.draw(active, &mut state, ctx).await;
            }
        }
        // The first plugin that picks an effort sets it.
        let input = json!({
            "model": view.model,
            "effort": view.effort,
            "turn": view.turn,
            "messages": view.transcript.len(),
        });
        for active in inner.active().iter() {
            let Some(answer) = inner
                .call(
                    active,
                    Hook::BeforeRequest,
                    input.clone(),
                    &mut state,
                    ctx,
                    Arc::new(NoReach),
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
            RunEvent::TurnEnd { turn, .. } => {
                let input = json!({ "turn": turn, "text": state.text, "calls": state.calls });
                for active in inner.active().iter() {
                    inner
                        .call(
                            active,
                            Hook::TurnEnd,
                            input.clone(),
                            &mut state,
                            ctx,
                            Arc::new(NoReach),
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
            });
            for active in inner.active().iter() {
                inner
                    .call(
                        active,
                        Hook::RunEnd,
                        input.clone(),
                        &mut state,
                        ctx,
                        Arc::new(NoReach),
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
                active.loaded.declaration.tools.iter().map(|spec| {
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
        let reach: Arc<dyn Reach> = Arc::new(ToolReach {
            ctx: ctx.clone(),
            plugin: plugin.clone(),
            jev: inner.jev.clone(),
        });
        let tool = Hook::Tool(self.spec.name.clone());
        let Some(active) = inner.plugin(&self.plugin).filter(|active| {
            active
                .loaded
                .declaration
                .tools
                .iter()
                .any(|spec| spec.name == self.spec.name)
        }) else {
            return Err(format!(
                "{} has no tool {} any more",
                self.plugin, self.spec.name
            )
            .into());
        };
        let failures = state.failures.get(&self.plugin).copied().unwrap_or(0);
        let Some(value) = inner
            .call(&active, tool, args.clone(), &mut state, &plugin, reach)
            .await
        else {
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

/// What a plugin's tool handler reaches: the run's tools, through the
/// loop, and Jev.
struct ToolReach {
    ctx: ToolCtx,
    plugin: PluginCtx,
    jev: Option<Arc<dyn Jev>>,
}

#[async_trait]
impl Reach for ToolReach {
    fn tools(&self) -> Vec<ToolEntry> {
        self.ctx
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
            .collect()
    }

    async fn call(&self, call: ScriptCall) -> Result<ToolReply, String> {
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

/// `{ unix, iso, weekday }` for now, in UTC.
pub fn now() -> Value {
    let unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64);
    at(unix)
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
