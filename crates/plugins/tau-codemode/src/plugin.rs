//! The plugin: the `codemode` tool, wired to the run through
//! `ToolCtx::call`, `ToolCtx::catalog` and `ToolCtx::plugin`.
//!
//! - [`Codemode`] is the plugin. Its `start` puts the Luau signatures
//!   of the run's `Direct` tools in the run's context.
//! - [`CodemodeTool`] is the tool. Each call parses the options line,
//!   waits for the tool sources the script needs, folds the store from
//!   the plugin's records, runs the script against a [`Host`] over the
//!   call's `ToolCtx`, and keeps a successful script's store writes.
//!
//! Sequential tools: the loop runs a caller's nested calls so that a
//! `Sequential` tool's call waits for that caller's other calls and runs
//! alone (`docs/reference/plugins.md`, "Scheduling"). A script is one
//! caller, so among one script's calls the rule holds; the engine also
//! queues a script's sequential calls behind one another, so they reach
//! the loop in the order the script made them. Across scripts nothing
//! is serialized: two codemode calls in one batch are two callers, and a
//! `Sequential` tool called by both may run alongside the other
//! script's calls, as two tools that call tools would.

use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tau_agent::{
    error::{PluginError, ToolError},
    plugin::{
        AskAttemptEnd,
        AskObserver,
        AskPermit,
        Plugin,
        PluginCtx,
        PluginRun,
        RunPlan,
    },
    tool::{AgentTool, Catalog, ExecutionMode, Exposure, ToolCtx, ToolOutput},
};
use tau_ai::{
    message::{
        ImageContent,
        InputBlock,
        Message,
        StopReason,
        TextContent,
        Usage,
        UserContent,
        UserMessage,
    },
    responses::request::{ReasoningEffort, Settings},
};
use tau_jev::Jev;
use tokio_util::sync::CancellationToken;

use crate::{
    Host,
    Item,
    Namespace,
    PLUGIN,
    Request,
    ToolCall,
    ToolEntry,
    ToolReply,
    description::{self, NAME},
    inference::InferRequest,
    inference_budget::{Budget, Limits, Permit},
    inference_trace::{self, Attempt, AttemptOutcome, UsageProvenance},
    modules,
    options,
    repository_modules::RepositoryModules,
    run,
    signature::{self, CATALOG_BUDGET_TOKENS},
    store,
};

/// Why a codemode call that no plugin added cannot run.
pub const NOT_A_PLUGIN: &str = "codemode runs only as its plugin's tool: \
add it with `Agent::plugin(Codemode::new(..))`";

/// The heading of the signatures `start` puts in the run's context.
pub const CONTEXT_HEADING: &str = "Tools a `codemode` script can call, as \
Luau signatures. Others may be callable too: find them with \
`search_tools(query)`.";

/// The Codemode plugin: adds the `codemode` tool, and lists the run's
/// direct tools' signatures in its context.
pub struct Codemode {
    tool: Arc<CodemodeTool>,
    module_writes: Arc<tokio::sync::Mutex<()>>,
    inference_limits: Limits,
    inference_model: Option<String>,
    module_test_slots: Arc<tokio::sync::Semaphore>,
    repository: Option<RepositoryModules>,
}

impl Codemode {
    /// A plugin whose scripts ask `jev`, when given one; without one,
    /// `jev` is nil and the description says so.
    pub fn new(jev: Option<Arc<dyn Jev>>) -> Self {
        Self {
            tool: Arc::new(CodemodeTool::new(jev, false)),
            module_writes: Arc::new(tokio::sync::Mutex::new(())),
            inference_limits: Limits::default(),
            inference_model: None,
            module_test_slots: Arc::new(tokio::sync::Semaphore::new(2)),
            repository: None,
        }
    }

    /// Set the limits shared by inference calls in each agent run.
    pub fn with_inference_limits(mut self, limits: Limits) -> Self {
        self.inference_limits = limits;
        self
    }

    /// Override the host model for inference calls.
    pub fn with_inference_model(mut self, model: impl Into<String>) -> Self {
        self.inference_model = Some(model.into());
        self
    }

    /// Use tau's private per-repository module directory, outside the checkout.
    pub fn with_repository(mut self, path: impl Into<PathBuf>) -> Self {
        self.repository = Some(RepositoryModules::new(path));
        self.tool = Arc::new(CodemodeTool::new(self.tool.jev.clone(), true));
        self
    }

    /// The tool the plugin adds.
    pub fn tool(&self) -> Arc<CodemodeTool> {
        self.tool.clone()
    }
}

impl Default for Codemode {
    fn default() -> Self {
        Self::new(None)
    }
}

fn infer_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "ok": {"type": "boolean"},
            "value": {},
            "trace_id": {"type": "string"},
            "usage": {"type": "object"},
            "error": {"type": ["string", "null"]}
        },
        "required": ["ok", "value", "trace_id", "usage", "error"],
        "additionalProperties": false
    })
}

/// Store before reporting so a failed write never looks terminal in the UI.
async fn publish_inference(
    plugin: &PluginCtx,
    record: inference_trace::Record,
) -> Result<(), String> {
    let record = store::Record::Inference(record);
    plugin
        .record(&record)
        .await
        .map_err(|error| error.to_string())?;
    let body =
        serde_json::to_value(&record).map_err(|error| error.to_string())?;
    plugin.report(body);
    Ok(())
}

/// This tool exists only in one run's plan, so its budget cannot leak to
/// another run that shares the Codemode plugin.
struct InferTool {
    budget: Arc<Budget>,
    model: String,
    reasoning: Option<ReasoningEffort>,
    parameters: Value,
    output_schema: Value,
}

struct InferObserver {
    budget: Arc<Budget>,
    plugin: PluginCtx,
    trace_id: String,
    attempts: Arc<Mutex<Vec<Attempt>>>,
    reservations: Arc<Mutex<Vec<u32>>>,
    updates: tau_agent::tool::ToolUpdates,
}

struct InferAttempt {
    permit: Permit,
    number: u32,
    attempts: Arc<Mutex<Vec<Attempt>>>,
    reported: bool,
    updates: tau_agent::tool::ToolUpdates,
}

#[async_trait]
impl AskObserver for InferObserver {
    async fn admit(
        &self,
        attempt: u32,
        cancel: &CancellationToken,
    ) -> Result<Box<dyn AskPermit>, String> {
        let permit = self.budget.admit(cancel).await?;
        publish_inference(
            &self.plugin,
            inference_trace::Record::Attempt {
                trace_id: self.trace_id.clone(),
                owner: self.plugin.run.clone(),
                number: attempt,
            },
        )
        .await
        .map_err(|error| {
            format!("cannot reserve inference attempt: {error}")
        })?;
        self.reservations.lock().unwrap().push(attempt);
        self.updates
            .send(ToolOutput::text(format!("attempt {attempt} started")));
        Ok(Box::new(InferAttempt {
            permit,
            number: attempt,
            attempts: self.attempts.clone(),
            reported: false,
            updates: self.updates.clone(),
        }))
    }
}

impl AskPermit for InferAttempt {
    fn report(&mut self, usage: &Usage, end: AskAttemptEnd) {
        if self.reported {
            return;
        }
        self.reported = true;
        if end != AskAttemptEnd::Finished {
            self.permit.block_incomplete();
        }
        self.permit.report(usage);
        let outcome = match end {
            AskAttemptEnd::Finished => AttemptOutcome::Finished,
            AskAttemptEnd::Cancelled => AttemptOutcome::Cancelled,
            AskAttemptEnd::BrokeGrammar => AttemptOutcome::BrokeGrammar,
            AskAttemptEnd::NoTerminal => AttemptOutcome::NoTerminal,
            AskAttemptEnd::OpenFailed => AttemptOutcome::OpenFailed,
        };
        let usage_provenance = if end != AskAttemptEnd::Finished {
            UsageProvenance::Unknown
        } else if *usage == Usage::default() {
            UsageProvenance::SdkZeroOrDefault
        } else {
            UsageProvenance::SdkReported
        };
        self.attempts.lock().unwrap().push(Attempt {
            number: self.number,
            outcome,
            reported_usage: usage.clone(),
            usage_provenance,
        });
        self.updates.send(ToolOutput::text(format!(
            "attempt {} · {:?} · reported ${:.6}{}",
            self.number,
            outcome,
            usage.cost.total,
            if usage_provenance == UsageProvenance::Unknown {
                " · final usage unknown"
            } else {
                ""
            },
        )));
    }
}

/// A dropped tool future must not allow further admissions in this run.
struct TraceCompletionGuard {
    budget: Arc<Budget>,
    terminal_stored: bool,
}

impl Drop for TraceCompletionGuard {
    fn drop(&mut self) {
        if !self.terminal_stored {
            self.budget.block_incomplete();
        }
    }
}

impl InferTool {
    fn output(
        &self,
        value: Value,
        trace_id: &str,
        usage: Usage,
        error: Option<String>,
        output_limit: bool,
        usage_complete: bool,
    ) -> Result<ToolOutput, ToolError> {
        let body = json!({
            "ok": error.is_none(),
            "value": value,
            "trace_id": trace_id,
            "usage": usage,
            "error": error,
        });
        let output = ToolOutput {
            content: vec![text_block(body.to_string())],
            details: Some(
                json!({"provider_output_limit": output_limit, "usage": usage, "usage_complete": usage_complete}),
            ),
            structured: Some(body),
        };
        if output
            .structured
            .as_ref()
            .is_some_and(|body| body["ok"] == false)
        {
            Err(ToolError::output(output))
        } else {
            Ok(output)
        }
    }
}

#[async_trait]
impl AgentTool for InferTool {
    fn name(&self) -> &str {
        "infer"
    }

    fn description(&self) -> &str {
        "Ask an isolated model with an explicit task and JSON context. Returns {ok, value, trace_id, usage, error}."
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn output_schema(&self) -> Option<&Value> {
        Some(&self.output_schema)
    }

    fn exposure(&self) -> Exposure {
        Exposure::Nested
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let trace_id = uuid::Uuid::now_v7().to_string();
        let Some(plugin) = ctx.plugin().cloned() else {
            return self.output(
                Value::Null,
                &trace_id,
                Usage::default(),
                Some("infer requires its plugin context".into()),
                false,
                true,
            );
        };
        let request = match InferRequest::parse(args) {
            Ok(request) => request,
            Err(error) => {
                let started = inference_trace::Record::Started {
                    trace_id: trace_id.clone(),
                    owner: plugin.run.clone(),
                    task: String::new(),
                    context: Value::Null,
                    schema: None,
                    model: self.model.clone(),
                    effort: self
                        .reasoning
                        .map(|effort| format!("{effort:?}").to_lowercase()),
                };
                if let Err(publish_error) =
                    publish_inference(&plugin, started).await
                {
                    return self.output(
                        Value::Null,
                        &trace_id,
                        Usage::default(),
                        Some(format!(
                            "inference trace {trace_id} could not start: {publish_error}; no provider attempt was made"
                        )),
                        false,
                        true,
                    );
                }
                let finished = inference_trace::Record::Finished {
                    trace_id: trace_id.clone(),
                    owner: plugin.run.clone(),
                    complete: true,
                    selected: None,
                    raw_output: None,
                    raw_output_truncated: false,
                    error: Some(error.clone()),
                    attempts: Vec::new(),
                    total_usage: Usage::default(),
                };
                if let Err(publish_error) =
                    publish_inference(&plugin, finished).await
                {
                    self.budget.block_incomplete();
                    return self.output(
                        Value::Null,
                        &trace_id,
                        Usage::default(),
                        Some(format!(
                            "inference trace {trace_id} has no stored terminal record: {publish_error}; incomplete budget, use a new run or fork"
                        )),
                        false,
                        false,
                    );
                }
                return self.output(
                    Value::Null,
                    &trace_id,
                    Usage::default(),
                    Some(error),
                    false,
                    true,
                );
            }
        };
        let output_limit = plugin.supports_output_token_limit();
        let started = inference_trace::Record::Started {
            trace_id: trace_id.clone(),
            owner: plugin.run.clone(),
            task: request.task.clone(),
            context: request.context.clone(),
            schema: request.schema.clone(),
            model: self.model.clone(),
            effort: self
                .reasoning
                .map(|effort| format!("{effort:?}").to_lowercase()),
        };
        if let Err(error) = publish_inference(&plugin, started).await {
            return self.output(
                Value::Null,
                &trace_id,
                Usage::default(),
                Some(format!(
                    "inference trace {trace_id} could not start: {error}; no provider attempt was made"
                )),
                output_limit,
                true,
            );
        }
        let mut completion = TraceCompletionGuard {
            budget: self.budget.clone(),
            terminal_stored: false,
        };
        let settings = Settings {
            model: self.model.clone(),
            instructions: Some("Answer only the user's independent inference request. Do not assume prior conversation, access tools, or request tool execution.".into()),
            tools: Vec::new(),
            reasoning_model: tau_ai::model::find(&self.model).is_some_and(|model| model.reasoning),
            reasoning: self.reasoning,
            text_format: request.text_format(),
            max_output_tokens: output_limit.then_some(2048),
            ..Settings::default()
        };
        let input = [Message::User(UserMessage {
            content: UserContent::Text(request.input_text()),
            timestamp: plugin.now(),
        })];
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let reservations = Arc::new(Mutex::new(Vec::new()));
        let observer = InferObserver {
            budget: self.budget.clone(),
            plugin: plugin.clone(),
            trace_id: trace_id.clone(),
            attempts: attempts.clone(),
            reservations: reservations.clone(),
            updates: ctx.updates.clone(),
        };
        let response = tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => Err("inference cancelled".to_owned()),
            _ = tokio::time::sleep_until(self.budget.deadline()) => Err("inference deadline reached".to_owned()),
            answer = plugin.ask_observed(settings, &input, &observer) => answer.map_err(|error| error.to_string()),
        };
        let mut attempts = attempts.lock().unwrap().clone();
        let reservations = reservations.lock().unwrap().clone();
        for number in reservations {
            if !attempts.iter().any(|attempt| attempt.number == number) {
                attempts.push(Attempt {
                    number,
                    outcome: AttemptOutcome::Interrupted,
                    reported_usage: Usage::default(),
                    usage_provenance: UsageProvenance::Unknown,
                });
            }
        }
        attempts.sort_by_key(|attempt| attempt.number);
        let complete = attempts.iter().all(|attempt| {
            attempt.outcome == AttemptOutcome::Finished
                && attempt.usage_provenance != UsageProvenance::Unknown
        });
        if !complete {
            self.budget.block_incomplete();
        }
        let mut reported = Usage::default();
        for attempt in &attempts {
            reported += &attempt.reported_usage;
        }
        let raw_output = response.as_ref().ok().map(|answer| answer.text());
        let (raw_output, raw_output_truncated) = raw_output
            .as_deref()
            .map(inference_trace::bounded_raw_output)
            .map_or((None, false), |(raw, truncated)| (Some(raw), truncated));
        let result = response.and_then(|answer| {
            if ctx.cancel.is_cancelled() {
                return Err("inference cancelled".into());
            }
            if tokio::time::Instant::now() >= self.budget.deadline() {
                return Err("inference deadline reached".into());
            }
            if answer.tool_calls().next().is_some() {
                return Err(
                    "inference returned tool calls that cannot be executed"
                        .into(),
                );
            }
            match answer.stop_reason {
                StopReason::Stop => request.decode_answer(&answer.text()),
                StopReason::Length => {
                    Err("inference output was truncated".into())
                }
                StopReason::ToolUse => {
                    Err("inference requested a tool that cannot be executed"
                        .into())
                }
                StopReason::Error | StopReason::Aborted => {
                    Err(answer.error_message.unwrap_or_else(|| {
                        format!("inference stopped: {:?}", answer.stop_reason)
                    }))
                }
            }
        });
        let (mut value, mut error) = match result {
            Ok(value) => (value, None),
            Err(error) => (Value::Null, Some(error)),
        };
        if !complete {
            value = Value::Null;
            error = Some(format!(
                "inference trace {trace_id} has incomplete attempt usage; use a new run or fork"
            ));
        }
        let finished = inference_trace::Record::Finished {
            trace_id: trace_id.clone(),
            owner: plugin.run.clone(),
            complete,
            selected: error.is_none().then(|| value.clone()),
            raw_output,
            raw_output_truncated,
            error: error.clone(),
            attempts,
            total_usage: reported.clone(),
        };
        if let Err(publish_error) = publish_inference(&plugin, finished).await {
            return self.output(
                Value::Null,
                &trace_id,
                reported,
                Some(format!(
                    "inference trace {trace_id} has no stored terminal record: {publish_error}; incomplete budget, use a new run or fork"
                )),
                output_limit,
                false,
            );
        }
        completion.terminal_stored = true;
        self.output(value, &trace_id, reported, error, output_limit, complete)
    }
}

#[async_trait]
impl Plugin for Codemode {
    fn name(&self) -> &str {
        PLUGIN
    }

    fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        vec![self.tool.clone()]
    }

    /// Lists the signatures of the run's `Direct` tools that fit
    /// [`CATALOG_BUDGET_TOKENS`]. Only tools in the plan when this runs
    /// are listed, so a plugin that adds tools (tau-mcp) goes before
    /// this one.
    async fn start(
        &self,
        plan: &mut RunPlan,
        ctx: &PluginCtx,
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        for name in crate::module_tools::NAMES {
            if plan.tools().iter().any(|tool| tool.name() == name) {
                return Err(
                    format!("codemode reserves tool name `{name}`").into()
                );
            }
        }
        if plan.tools().iter().any(|tool| tool.name() == "infer") {
            return Err("codemode cannot add infer: a tool with that name already exists".into());
        }
        let records = ctx.records().await.map_err(PluginError::from)?;
        if let Some(repository) = &self.repository
            && modules::pin_for_run(&records, &ctx.run.0)
                .map_err(PluginError::from)?
                .is_none()
        {
            let scratch = modules::fold(&records);
            let pin = repository
                .snapshot(&ctx.run.0, &scratch)
                .map_err(PluginError::from)?;
            let record = store::Record::RepositoryPin(pin);
            ctx.record(&record).await.map_err(PluginError::from)?;
            ctx.report(
                serde_json::to_value(&record).map_err(PluginError::from)?,
            );
        }
        let restored = inference_trace::restore_budget(&records, &ctx.run)
            .map_err(PluginError::from)?;
        let budget = Budget::restored(self.inference_limits, restored)
            .map_err(PluginError::from)?;
        let model = self.inference_model.as_deref().unwrap_or(plan.model());
        let infer = Arc::new(InferTool {
            budget,
            model: model.to_owned(),
            reasoning: plan.reasoning,
            parameters: InferRequest::parameters(),
            output_schema: infer_output_schema(),
        });
        plan.add_tool(infer);
        let tools: Vec<ToolEntry> = plan
            .tools()
            .iter()
            .filter(|tool| {
                tool.exposure() == Exposure::Direct && tool.name() != NAME
            })
            .map(|tool| entry(tool.as_ref(), &[]))
            .collect();
        if let Some(text) = context(&tools) {
            plan.context.push(text);
        }
        for name in crate::module_tools::NAMES {
            plan.add_tool(Arc::new(crate::module_tools::ModuleTool::new(
                name,
                self.module_writes.clone(),
                self.module_test_slots.clone(),
                self.repository
                    .as_ref()
                    .map(|repo| (repo.scope(), repo.key())),
            )));
        }
        Ok(Box::new(()))
    }
}

/// The run context listing `tools`' signatures within the budget, or
/// `None` when none fits.
pub fn context(tools: &[ToolEntry]) -> Option<String> {
    let catalog = signature::catalog(tools, CATALOG_BUDGET_TOKENS);
    if catalog.listed.is_empty() {
        return None;
    }
    Some(format!(
        "{CONTEXT_HEADING}\n\n```luau\n{}\n```",
        catalog.text
    ))
}

/// The `codemode` tool. Made by [`Codemode`]; it needs the plugin's
/// context, so it runs only as the plugin's tool.
pub struct CodemodeTool {
    jev: Option<Arc<dyn Jev>>,
    description: String,
    parameters: Value,
    repository_required: bool,
}

impl CodemodeTool {
    fn new(jev: Option<Arc<dyn Jev>>, repository_required: bool) -> Self {
        Self {
            description: description::description(jev.is_some()),
            jev,
            repository_required,
            parameters: json!({
                "type": "object",
                "properties": {
                    "code": {
                        "type": "string",
                        "description": description::CODE_DESCRIPTION,
                    }
                },
                "required": ["code"],
                "additionalProperties": false,
            }),
        }
    }
}

#[async_trait]
impl AgentTool for CodemodeTool {
    fn name(&self) -> &str {
        NAME
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn exposure(&self) -> Exposure {
        Exposure::ModelOnly
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let code = args
            .get("code")
            .and_then(Value::as_str)
            .ok_or("The code is empty: pass Luau source in `code`.")?;
        let source = options::parse(code).map_err(|error| error.0)?;
        let Some(plugin) = ctx.plugin().cloned() else {
            return Err(NOT_A_PLUGIN.into());
        };
        let before = ctx.catalog();
        let wait = ready(&before, &source.code, &ctx);
        match source.options.timeout_ms {
            Some(ms) => {
                let _ =
                    tokio::time::timeout(Duration::from_millis(ms), wait).await;
            }
            None => wait.await,
        }
        let records = plugin.records().await.map_err(|error| {
            format!("codemode could not read its store: {error}")
        })?;
        let snapshot = store::fold(&records);
        let max_output_tokens = source.options.max_output_tokens();
        let host = Arc::new(LoopHost {
            catalog: ctx.catalog(),
            ctx: ctx.clone(),
            jev: self.jev.clone(),
            plugin: plugin.clone(),
            repository_required: self.repository_required,
        });
        let outcome = run(
            host,
            Request {
                call_id: ctx.call_id().to_owned(),
                source,
                store: snapshot,
                cancel: ctx.cancel.clone(),
            },
        )
        .await;
        let rendered = outcome.render(max_output_tokens);
        let mut is_error = rendered.is_error;
        let mut content: Vec<InputBlock> =
            rendered.content.into_iter().map(block).collect();
        if let Some(writes) = outcome.store.filter(|writes| !writes.is_empty())
            && let Err(error) =
                plugin.try_publish(&store::Record::Store(writes)).await
        {
            is_error = true;
            content.push(text_block(format!(
                "The script's store writes could not be kept: {error}"
            )));
        }
        let output = ToolOutput {
            content,
            details: Some(rendered.details),
            structured: None,
        };
        if is_error {
            Err(ToolError::output(output))
        } else {
            Ok(output)
        }
    }
}

/// Waits until the tool sources have the namespaces `code` needs: all
/// of them when it searches or lists tools, else those it names, else
/// none.
async fn ready(catalog: &Catalog, code: &str, ctx: &ToolCtx) {
    match wanted(code) {
        Wanted::Nothing => {}
        Wanted::All => catalog.ready(None, &ctx.cancel).await,
        Wanted::Namespaces(names) => {
            catalog.ready(Some(&names), &ctx.cancel).await
        }
    }
}

/// The namespaces a script needs ready before it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wanted {
    Nothing,
    /// `mcp__<server>` for each server whose tool it names.
    Namespaces(Vec<String>),
    /// It uses `search_tools`, `describe_namespace` or `ALL_TOOLS`.
    All,
}

/// The globals that see every tool.
const DISCOVERY: [&str; 3] =
    ["search_tools", "describe_namespace", "ALL_TOOLS"];

/// What `code` needs ready. A name `mcp__a__b__c` may be server `a`
/// with tool `b__c`, or server `a__b` with tool `c`: each reading's
/// server is waited for.
pub fn wanted(code: &str) -> Wanted {
    if DISCOVERY.iter().any(|global| code.contains(global)) {
        return Wanted::All;
    }
    let mut names = BTreeSet::new();
    for (at, _) in code.match_indices("mcp__") {
        let before = code[..at].chars().next_back();
        if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let rest = &code[at + "mcp__".len()..];
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        let ident = &rest[..end];
        let splits: Vec<usize> = ident
            .match_indices("__")
            .map(|(split, _)| split)
            .filter(|split| *split > 0)
            .collect();
        if splits.is_empty() {
            if !ident.is_empty() {
                names.insert(format!("mcp__{ident}"));
            }
        } else {
            for split in splits {
                names.insert(format!("mcp__{}", &ident[..split]));
            }
        }
    }
    if names.is_empty() {
        Wanted::Nothing
    } else {
        Wanted::Namespaces(names.into_iter().collect())
    }
}

/// A nested call's readable value and independent failure diagnostic:
///
/// - structured output with a schema stays readable even on failure;
/// - otherwise a success returns its text blocks joined;
/// - otherwise `Err` raises the error's text in the script.
///
/// No JSON field, including MCP's `isError`, overrides the loop's status.
pub fn script_reply(
    has_output_schema: bool,
    result: Result<ToolOutput, ToolError>,
) -> Result<ToolReply, String> {
    script_reply_with_usage(has_output_schema, false, result)
}

#[derive(Deserialize)]
struct InferDetails {
    usage: Usage,
    usage_complete: bool,
}

/// Only the reserved infer tool may supply display usage. Neither the
/// readable JSON value nor another tool's details can set it.
fn script_reply_with_usage(
    has_output_schema: bool,
    infer: bool,
    result: Result<ToolOutput, ToolError>,
) -> Result<ToolReply, String> {
    let usage_of = |output: &ToolOutput| {
        if !infer {
            return None;
        }
        output.details.as_ref().and_then(|details| {
            serde_json::from_value::<InferDetails>(details.clone()).ok()
        })
    };
    match result {
        Ok(output) => {
            let details = usage_of(&output);
            Ok(ToolReply {
                usage: details.as_ref().map(|details| details.usage.clone()),
                usage_complete: details.map(|details| details.usage_complete),
                value: match output.structured {
                    Some(value) if has_output_schema => value,
                    _ => Value::String(output.text_content()),
                },
                error: None,
            })
        }
        Err(ToolError::Output(output)) => {
            let error = output.text_content();
            let details = usage_of(&output);
            match output.structured {
                Some(value) if has_output_schema => Ok(ToolReply {
                    value,
                    error: Some(error),
                    usage: details
                        .as_ref()
                        .map(|details| details.usage.clone()),
                    usage_complete: details
                        .map(|details| details.usage_complete),
                }),
                _ => Err(error),
            }
        }
        Err(error) => Err(error.to_string()),
    }
}

fn text_block(text: String) -> InputBlock {
    InputBlock::Text(TextContent {
        text,
        text_signature: None,
    })
}

fn block(item: Item) -> InputBlock {
    match item {
        Item::Text(text) => text_block(text),
        Item::Image(image) => InputBlock::Image(ImageContent {
            data: image.data,
            mime_type: image.mime_type.to_owned(),
        }),
    }
}

/// `tool` as scripts see it, in the namespace that lists it, else the
/// one its `mcp__<server>__` prefix names.
fn entry(
    tool: &dyn AgentTool,
    namespaces: &[tau_agent::tool::Namespace],
) -> ToolEntry {
    let name = tool.name();
    let namespace = namespaces
        .iter()
        .find(|space| space.tools.iter().any(|t| t == name))
        .map(|space| space.name.clone())
        .or_else(|| {
            let server = name.strip_prefix("mcp__")?;
            let split = server.find("__").filter(|split| *split > 0)?;
            Some(format!("mcp__{}", &server[..split]))
        });
    ToolEntry {
        name: name.to_owned(),
        description: tool.description().to_owned(),
        input_schema: tool.parameters().clone(),
        output_schema: tool.output_schema().cloned(),
        namespace,
        sequential: tool.execution_mode() == ExecutionMode::Sequential,
    }
}

/// A script's way into its run.
struct LoopHost {
    ctx: ToolCtx,
    /// The callable tools when the script started.
    catalog: Catalog,
    jev: Option<Arc<dyn Jev>>,
    plugin: PluginCtx,
    repository_required: bool,
}

#[async_trait]
impl Host for LoopHost {
    async fn module(
        &self,
        name: &str,
        version: Option<&str>,
    ) -> Result<Option<modules::Definition>, String> {
        let records = self.plugin.records().await.map_err(|error| {
            format!("codemode could not read its modules: {error}")
        })?;
        let library = modules::fold(&records);
        let pin = modules::pin_for_run(&records, &self.plugin.run.0)?;
        if self.repository_required && pin.is_none() {
            return Err("repository pin is missing for this run".into());
        }
        Ok(
            modules::resolve_visible(&library, pin.as_ref(), name, version)
                .cloned(),
        )
    }
    fn tools(&self) -> Vec<ToolEntry> {
        let namespaces = self.catalog.namespaces();
        self.catalog
            .tools()
            .iter()
            .map(|tool| entry(tool.as_ref(), namespaces))
            .collect()
    }

    fn namespaces(&self) -> Vec<Namespace> {
        self.catalog
            .namespaces()
            .iter()
            .map(|space| Namespace {
                name: space.name.clone(),
                description: Some(space.description.clone())
                    .filter(|text| !text.is_empty()),
                instructions: space.instructions.clone(),
            })
            .collect()
    }

    /// Goes through `ToolCtx::call`. It sends the call before its first
    /// await, so the loop numbers calls in the order the engine starts
    /// them and the two agree on ids.
    async fn call_tool(&self, call: ToolCall) -> Result<ToolReply, String> {
        let has_output_schema = self
            .catalog
            .get(&call.name)
            .is_some_and(|tool| tool.output_schema().is_some());
        let result = self.ctx.call(&call.name, call.args).await;
        script_reply_with_usage(has_output_schema, call.name == "infer", result)
    }

    fn jev(&self) -> Option<Arc<dyn Jev>> {
        self.jev.clone()
    }

    fn charge(&self, usage: &Usage) {
        self.plugin.charge(usage);
    }

    /// A partial result with no content: the details are for the card.
    fn update(&self, details: Value) {
        self.ctx.updates.send(ToolOutput {
            content: Vec::new(),
            details: Some(details),
            structured: None,
        });
    }
}
