//! The tau-agent plugin that runs Luau plugins in a run (ADR 0027): their
//! tools are the run's tools, and their hooks run at tau-agent's seams.
//!
//! Every hook's answer is applied here: a block stops the call, a
//! continue keeps the run going, and the state, log lines, errors and
//! the view drawn from the new state are published as [`Record`]s. A
//! hook that fails counts as allowing or stopping; after
//! [`MAX_FAILURES`] in a run, its plugin is off for the rest of it.

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
        Decision,
        FinishedRun,
        Plugin,
        PluginCtx,
        PluginRun,
        RunPlan,
        StopDecision,
        ToolCall,
    },
    tool::{AgentTool, RunId, ToolCtx, ToolOutput},
};
use tau_ai::message::{AssistantBlock, AssistantMessage, Usage};
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

/// The run's Luau plugins, as one tau-agent plugin. Build one per run.
#[derive(Clone)]
pub struct LuauPlugins(Arc<Inner>);

struct Inner {
    active: Vec<Active>,
    /// `{ kind, repo, model }`: what `ctx.run` says besides the run's id
    /// and turn.
    run: Value,
    jev: Option<Arc<dyn Jev>>,
    /// The state of each run going on, by run.
    runs: Mutex<HashMap<RunId, Arc<tokio::sync::Mutex<RunState>>>>,
}

/// One run's state for every plugin, in the order they are active.
#[derive(Debug, Default)]
struct RunState {
    states: Vec<Value>,
    failures: Vec<u32>,
    continued: u32,
    turn: u32,
    /// The current turn's text and its calls, by id.
    text: String,
    names: HashMap<String, String>,
    calls: Vec<Value>,
}

impl LuauPlugins {
    /// `run` is `{ kind, repo, model }`; `jev` is what plugins that use
    /// Jev ask.
    pub fn new(
        active: Vec<Active>,
        run: Value,
        jev: Option<Arc<dyn Jev>>,
    ) -> Self {
        Self(Arc::new(Inner {
            active,
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
            .or_insert_with(|| {
                Arc::new(tokio::sync::Mutex::new(RunState {
                    states: vec![json!({}); self.active.len()],
                    failures: vec![0; self.active.len()],
                    ..RunState::default()
                }))
            })
            .clone()
    }

    fn context(&self, run: &RunId, state: &RunState, index: usize) -> Context {
        let mut info = self.run.clone();
        if let Some(info) = info.as_object_mut() {
            info.insert("id".into(), json!(run.0.as_ref()));
            info.insert("turn".into(), json!(state.turn));
        }
        Context {
            run: info,
            now: now(),
            settings: self.active[index].settings.clone(),
            state: state.states[index].clone(),
        }
    }

    fn off(&self, state: &RunState, index: usize) -> bool {
        state.failures[index] >= MAX_FAILURES
    }

    /// Calls `hook` of plugin `index` and applies what it did: state,
    /// log lines, the view, or the error.
    async fn call(
        &self,
        index: usize,
        hook: Hook,
        input: Value,
        state: &mut RunState,
        ctx: &PluginCtx,
        reach: Arc<dyn Reach>,
    ) -> Option<Value> {
        if self.off(state, index)
            || !hook.declared_in(&self.active[index].loaded.declaration)
        {
            return None;
        }
        let context = self.context(&ctx.run, state, index);
        let outcome = self.active[index]
            .loaded
            .call(&hook, input, &context, reach, ctx.cancel.child_token())
            .await;
        self.apply(index, &hook, outcome, state, ctx).await
    }

    async fn apply(
        &self,
        index: usize,
        hook: &Hook,
        outcome: Outcome,
        state: &mut RunState,
        ctx: &PluginCtx,
    ) -> Option<Value> {
        let plugin = self.active[index].loaded.declaration.name.clone();
        if !outcome.logs.is_empty() {
            ctx.publish(&Record::Log {
                plugin: plugin.clone(),
                lines: outcome.logs,
            })
            .await;
        }
        if let Some(error) = outcome.error {
            state.failures[index] += 1;
            ctx.publish(&Record::Error {
                plugin: plugin.clone(),
                hook: hook_name(hook),
                error,
            })
            .await;
            if self.off(state, index) {
                ctx.publish(&Record::Off {
                    plugin,
                    why: format!("{MAX_FAILURES} hooks failed in this run"),
                })
                .await;
            }
            return None;
        }
        if outcome.state != state.states[index] {
            state.states[index] = outcome.state.clone();
            ctx.publish(&Record::State {
                plugin: plugin.clone(),
                state: outcome.state,
            })
            .await;
            if !matches!(hook, Hook::View) {
                self.draw(index, state, ctx).await;
            }
        }
        Some(outcome.value)
    }

    /// Draws plugin `index`'s view from its state and publishes it.
    async fn draw(&self, index: usize, state: &mut RunState, ctx: &PluginCtx) {
        if self.off(state, index)
            || !self.active[index].loaded.declaration.hooks.view
        {
            return;
        }
        let context = self.context(&ctx.run, state, index);
        let outcome = self.active[index]
            .loaded
            .call(
                &Hook::View,
                Value::Null,
                &context,
                Arc::new(NoReach),
                ctx.cancel.child_token(),
            )
            .await;
        let plugin = self.active[index].loaded.declaration.name.clone();
        match outcome.error {
            Some(error) => {
                state.failures[index] += 1;
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

fn hook_name(hook: &Hook) -> String {
    match hook {
        Hook::Tool(name) => format!("tool {name}"),
        Hook::Card(name) => format!("card of {name}"),
        Hook::BeforeTool => "before_tool".into(),
        Hook::BeforeStop => "before_stop".into(),
        Hook::TurnEnd => "turn_end".into(),
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

    fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        self.0
            .active
            .iter()
            .enumerate()
            .flat_map(|(index, active)| {
                active.loaded.declaration.tools.iter().map(move |spec| {
                    Arc::new(LuauTool {
                        plugins: self.clone(),
                        index,
                        spec: spec.clone(),
                    }) as Arc<dyn AgentTool>
                })
            })
            .collect()
    }

    async fn start(
        &self,
        plan: &mut RunPlan,
        ctx: &PluginCtx,
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        let shared = self.0.run_state(&ctx.run);
        {
            let mut state = shared.lock().await;
            // The state a fork or a resumed run inherits: each plugin's
            // last.
            for record in plan.records() {
                let Ok(Record::State {
                    plugin,
                    state: kept,
                }) = serde_json::from_value::<Record>(record.clone())
                else {
                    continue;
                };
                if let Some(index) =
                    self.0.active.iter().position(|active| {
                        active.loaded.declaration.name == plugin
                    })
                {
                    state.states[index] = kept;
                }
            }
            for index in 0..self.0.active.len() {
                self.0.draw(index, &mut state, ctx).await;
            }
        }
        Ok(Box::new(LuauRun {
            plugins: self.clone(),
            state: shared,
        }))
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
        let input =
            json!({ "id": call.id, "name": call.name, "args": call.args });
        for index in 0..inner.active.len() {
            let Some(answer) = inner
                .call(
                    index,
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
            match answer["decision"].as_str() {
                Some("block") => return Ok(Decision::Block(reason)),
                Some("flag") => {
                    ctx.publish(&Record::Flag {
                        plugin: inner.active[index]
                            .loaded
                            .declaration
                            .name
                            .clone(),
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
        for index in 0..inner.active.len() {
            let Some(answer) = inner
                .call(
                    index,
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

    async fn on_event(&mut self, event: &RunEvent, ctx: &PluginCtx) {
        let inner = &self.plugins.0;
        let mut state = self.state.lock().await;
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
                for index in 0..inner.active.len() {
                    inner
                        .call(
                            index,
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
            for index in 0..inner.active.len() {
                inner
                    .call(
                        index,
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

/// A tool a Luau plugin adds.
struct LuauTool {
    plugins: LuauPlugins,
    index: usize,
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
        let failures = state.failures[self.index];
        let Some(value) = inner
            .call(self.index, tool, args.clone(), &mut state, &plugin, reach)
            .await
        else {
            if inner.off(&state, self.index) && failures >= MAX_FAILURES {
                return Err(format!(
                    "{} is off for this run: {MAX_FAILURES} of its hooks failed",
                    inner.active[self.index].loaded.declaration.name
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
                    self.index,
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
                "plugin": inner.active[self.index].loaded.declaration.name,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_names_the_day() {
        assert_eq!(
            at(0),
            json!({ "unix": 0, "iso": "1970-01-01T00:00:00Z", "weekday": "Thursday" })
        );
        // A Friday, mid-morning.
        assert_eq!(at(1_791_540_000)["iso"], "2026-10-09T10:00:00Z");
        assert_eq!(at(1_791_540_000)["weekday"], "Friday");
        assert_eq!(at(951_782_400)["iso"], "2000-02-29T00:00:00Z");
    }
}
