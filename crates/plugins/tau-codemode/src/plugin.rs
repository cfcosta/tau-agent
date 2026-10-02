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

use std::{collections::BTreeSet, sync::Arc, time::Duration};

use async_trait::async_trait;
use serde_json::{Value, json};
use tau_agent::{
    error::{PluginError, ToolError},
    plugin::{Plugin, PluginCtx, PluginRun, RunPlan},
    tool::{AgentTool, Catalog, ExecutionMode, Exposure, ToolCtx, ToolOutput},
};
use tau_ai::message::{ImageContent, InputBlock, TextContent, Usage};
use tau_jev::Jev;

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
}

impl Codemode {
    /// A plugin whose scripts ask `jev`, when given one; without one,
    /// `jev` is nil and the description says so.
    pub fn new(jev: Option<Arc<dyn Jev>>) -> Self {
        Self {
            tool: Arc::new(CodemodeTool::new(jev)),
        }
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
