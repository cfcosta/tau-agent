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
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
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
    modules,
    options,
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
}

impl Codemode {
    /// A plugin whose scripts ask `jev`, when given one; without one,
    /// `jev` is nil and the description says so.
    pub fn new(jev: Option<Arc<dyn Jev>>) -> Self {
        Self {
            tool: Arc::new(CodemodeTool::new(jev)),
            module_writes: Arc::new(tokio::sync::Mutex::new(())),
            inference_limits: Limits::default(),
            inference_model: None,
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
            "trace_id": {"type": "null"},
            "usage": {"type": "object"},
            "error": {"type": ["string", "null"]}
        },
        "required": ["ok", "value", "trace_id", "usage", "error"],
        "additionalProperties": false
    })
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
    usage: Arc<Mutex<Usage>>,
}

struct InferAttempt {
    permit: Permit,
    usage: Arc<Mutex<Usage>>,
}

#[async_trait]
impl AskObserver for InferObserver {
    async fn admit(
        &self,
        _attempt: u32,
        cancel: &CancellationToken,
    ) -> Result<Box<dyn AskPermit>, String> {
        Ok(Box::new(InferAttempt {
            permit: self.budget.admit(cancel).await?,
            usage: self.usage.clone(),
        }))
    }
}

impl AskPermit for InferAttempt {
    fn report(&mut self, usage: &Usage, _end: AskAttemptEnd) {
        self.permit.report(usage);
        *self.usage.lock().unwrap() += usage;
    }
}

impl InferTool {
    fn output(
        &self,
        value: Value,
        usage: Usage,
        error: Option<String>,
        output_limit: bool,
    ) -> Result<ToolOutput, ToolError> {
        let body = json!({
            "ok": error.is_none(),
            "value": value,
            "trace_id": null,
            "usage": usage,
            "error": error,
        });
        let output = ToolOutput {
            content: vec![text_block(body.to_string())],
            details: Some(json!({"provider_output_limit": output_limit})),
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
        let request = match InferRequest::parse(args) {
            Ok(request) => request,
            Err(error) => {
                return self.output(
                    Value::Null,
                    Usage::default(),
                    Some(error),
                    false,
                );
            }
        };
        let Some(plugin) = ctx.plugin().cloned() else {
            return self.output(
                Value::Null,
                Usage::default(),
                Some("infer requires its plugin context".into()),
                false,
            );
        };
        let output_limit = plugin.supports_output_token_limit();
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
        let usage = Arc::new(Mutex::new(Usage::default()));
        let observer = InferObserver {
            budget: self.budget.clone(),
            usage: usage.clone(),
        };
        let response = tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => Err("inference cancelled".to_owned()),
            _ = tokio::time::sleep_until(self.budget.deadline()) => Err("inference deadline reached".to_owned()),
            answer = plugin.ask_observed(settings, &input, &observer) => answer.map_err(|error| error.to_string()),
        };
        let reported = usage.lock().unwrap().clone();
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
        match result {
            Ok(value) => self.output(value, reported, None, output_limit),
            Err(error) => {
                self.output(Value::Null, reported, Some(error), output_limit)
            }
        }
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
        _ctx: &PluginCtx,
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
        let budget =
            Budget::new(self.inference_limits).map_err(PluginError::from)?;
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
}

impl CodemodeTool {
    fn new(jev: Option<Arc<dyn Jev>>) -> Self {
        Self {
            description: description::description(jev.is_some()),
            jev,
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
    match result {
        Ok(output) => Ok(ToolReply::success(match output.structured {
            Some(value) if has_output_schema => value,
            _ => Value::String(output.text_content()),
        })),
        Err(ToolError::Output(output)) => {
            let error = output.text_content();
            match output.structured {
                Some(value) if has_output_schema => Ok(ToolReply {
                    value,
                    error: Some(error),
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
        let version = version
            .or_else(|| library.selected().get(name).map(String::as_str));
        Ok(version
            .and_then(|version| library.versions().get(version))
            .filter(|definition| definition.name() == name)
            .cloned())
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
        script_reply(has_output_schema, result)
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
