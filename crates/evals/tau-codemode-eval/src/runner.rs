//! Reproducible, offline scripts in the real codemode VM over real coding tools.
//! The scripts and scripted inference records are authored independently of
//! `Fixture::expected`; only `grade` reads the oracle.

use std::{
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tau_agent::tool::{AgentTool, ToolCtx};
use tau_ai::message::InputBlock;
use tau_codemode::{
    CancellationToken,
    Host,
    Request,
    ToolCall,
    ToolEntry,
    ToolReply,
    inference::InferRequest,
    options,
    run,
    store::Snapshot,
};
use tau_tools::{grep::Grep, path::Root, read::Read};

use crate::fixtures::{self, Fixture};

pub(crate) const SEARCH_TEXT: &str = r#"
local display = tools.grep({pattern = 'TODO:', glob = '*.rs', literal = true})
local found = array({})
for line in string.gmatch(display, '[^\n]+') do
    local path, number, text = string.match(line, '^(.*):(%d+): (.*)$')
    if path then table.insert(found, {path = path, line = tonumber(number), text = text}) end
end
return {result = found, incomplete = string.find(display, 'limit reached', 1, true) ~= nil}
"#;

pub(crate) const SEARCH_STRUCTURED: &str = r#"
local listing = tools.grep({pattern = 'TODO:', glob = '*.rs', literal = true})
local settled = map(array(listing.lines), function(line)
    if line.kind ~= 'match' or line.truncated then error('partial match') end
    return {path = line.path, line = line.line, text = line.text}
end, 2)
local found = array({})
for _, entry in ipairs(settled) do
    if not entry.ok then error(entry.error) end
    table.insert(found, entry.value)
end
return {result = found, incomplete = not listing.complete}
"#;

pub(crate) const LOG_TEXT: &str = r#"
local display = tools.read({path = 'tests.log'})
local failures = array({})
for line in string.gmatch(display, '[^\n]+') do
    local test, message = string.match(line, '^FAIL\t([^\t]+)\t(.*)$')
    if test then table.insert(failures, {test = test, message = message}) end
end
return {result = failures, incomplete = string.find(display, 'Use offset=', 1, true) ~= nil}
"#;

pub(crate) const LOG_STRUCTURED: &str = r#"
local matches = tools.grep({pattern = '^FAIL\\t', path = 'tests.log', limit = 100})
local failures = array({})
for _, line in ipairs(matches.lines) do
    local test, message = string.match(line.text, '^FAIL\t([^\t]+)\t(.*)$')
    if line.kind == 'match' and test then
        table.insert(failures, {test = test, message = message})
    end
end
return {result = failures, incomplete = not matches.complete}
"#;

// A task-specific text parser is a legitimate old-program reference; it does
// not represent the entire older VM's capability.
pub(crate) const SEMANTIC_TEXT: &str = r#"
local text = tools.read({path = 'changes.md'})
local changes = array({})
local before, after = string.match(text, 'option `([^`]+)` was removed%. Clients must use `([^`]+)` instead')
if before then table.insert(changes, {component = 'connection option', before = before, after = after}) end
if string.find(text, 'returns null when no item exists. It previously raised NotFound', 1, true) then
    table.insert(changes, {component = 'lookup function', before = 'raises NotFound', after = 'returns null'})
end
return {result = changes, incomplete = string.find(text, 'Use offset=', 1, true) ~= nil}
"#;

pub(crate) const SEMANTIC_SIMULATOR: &str = r#"
local document = tools.read({path = 'changes.md'})
if not document.complete then error('semantic context is incomplete') end
local changes = tools.infer({
    task = 'Extract changes requiring existing clients to change code',
    context = document.text,
    schema = {type = 'array', items = {type = 'object', properties = {
        component = {type = 'string'}, before = {type = 'string'}, after = {type = 'string'}
    }, required = {'component', 'before', 'after'}, additionalProperties = false}}
})
return {result = changes.value, incomplete = false}
"#;

/// Requiring all fields is a gate even though live transport is unsupported.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LiveLimits {
    pub max_provider_attempts: Option<u32>,
    pub max_usd: Option<f64>,
    pub max_seconds: Option<u64>,
    pub max_output_tokens: Option<u64>,
}

impl LiveLimits {
    pub fn validate(self) -> Result<(), String> {
        if self.max_provider_attempts.is_none_or(|n| n == 0)
            || self.max_usd.is_none_or(|n| !n.is_finite() || n <= 0.0)
            || self.max_seconds.is_none_or(|n| n == 0)
            || self.max_output_tokens.is_none_or(|n| n == 0)
        {
            return Err("live evaluation requires positive max provider attempts, USD, seconds, and output tokens".into());
        }
        Ok(())
    }
}

pub fn reject_live(limits: LiveLimits) -> Result<(), String> {
    limits.validate()?;
    Err("live evaluation is unsupported: no guarded provider transport is installed".into())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    TextOnlyReference,
    StructuredTools,
    ScriptedInferSimulator,
    AssistantScriptedModule,
    ExternalImmutableModule,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReportedUsage {
    pub uncached_input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaseReport {
    pub fixture: String,
    pub mode: Mode,
    pub result: Option<Value>,
    pub correct: bool,
    pub incomplete: bool,
    pub tool_calls: usize,
    pub provider_round_trips: usize,
    pub simulated_round_trips: usize,
    pub repairs: usize,
    pub latency_ms: Option<u64>,
    pub reported_usage: ReportedUsage,
    pub failure: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvalReport {
    pub format_version: u32,
    pub evidence: String,
    pub cases: Vec<CaseReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matrix: Option<crate::matrix::MatrixReport>,
}

struct OfflineHost {
    tools: Vec<Arc<dyn AgentTool>>,
    structured: bool,
    simulator: bool,
    infer_calls: AtomicUsize,
}

impl OfflineHost {
    fn new(root: Root, structured: bool, simulator: bool) -> Self {
        Self {
            tools: vec![
                Arc::new(Grep::new(root.clone())),
                Arc::new(Read::new(root)),
            ],
            structured,
            simulator,
            infer_calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl Host for OfflineHost {
    fn tools(&self) -> Vec<ToolEntry> {
        let mut entries: Vec<_> = self
            .tools
            .iter()
            .map(|tool| ToolEntry {
                name: tool.name().into(),
                description: tool.description().into(),
                input_schema: tool.parameters().clone(),
                output_schema: self
                    .structured
                    .then(|| tool.output_schema().cloned())
                    .flatten(),
                namespace: None,
                sequential: false,
            })
            .collect();
        if self.simulator {
            entries.push(ToolEntry {
                name: "infer".into(),
                description: "Scripted offline semantic response".into(),
                input_schema: InferRequest::parameters(),
                output_schema: Some(json!(true)),
                namespace: None,
                sequential: false,
            });
        }
        entries
    }

    async fn call_tool(&self, call: ToolCall) -> Result<ToolReply, String> {
        if call.name == "infer" && self.simulator {
            self.infer_calls.fetch_add(1, Ordering::Relaxed);
            let request = InferRequest::parse(call.args)?;
            let answer = answer_scripted_semantic_request(&request)?;
            let value = request.decode_answer(answer)?;
            return Ok(ToolReply::success(json!({
                "ok": true, "value": value, "trace_id": null,
                "usage": tau_ai::message::Usage::default(), "error": null
            })));
        }
        let tool = self
            .tools
            .iter()
            .find(|tool| tool.name() == call.name)
            .ok_or_else(|| format!("unknown offline tool: {}", call.name))?;
        let output = tool
            .call(call.args, ToolCtx::detached())
            .await
            .map_err(|error| error.to_string())?;
        if self.structured {
            return output
                .structured
                .map(ToolReply::success)
                .ok_or("missing structured output".into());
        }
        let text = output
            .content
            .iter()
            .filter_map(|block| match block {
                InputBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        Ok(ToolReply::success(Value::String(text)))
    }
}

// Handcrafted response record; the fixture's expected field is never consulted.
// An altered document has no record, so it fails closed instead of recycling an
// answer for evidence that no longer supports it.
fn answer_scripted_semantic_request(
    request: &InferRequest,
) -> Result<&'static str, String> {
    const DOCUMENT: &str = "# Changes\n\nThe connection option `retry_count` was removed. Clients must use `max_attempts` instead.\n\nThe default theme is now blue. Existing configuration keys are still accepted.\n\nThe lookup function now returns null when no item exists. It previously raised NotFound.\n";
    const ANSWER: &str = r#"[{"component":"connection option","before":"retry_count","after":"max_attempts"},{"component":"lookup function","before":"raises NotFound","after":"returns null"}]"#;
    if request.task
        == "Extract changes requiring existing clients to change code"
        && request.context.as_str() == Some(DOCUMENT)
    {
        Ok(ANSWER)
    } else {
        Err("no scripted response for this semantic input".into())
    }
}

pub(crate) fn stage_files(
    root: &Path,
    fixture: &Fixture,
) -> Result<(), String> {
    for file in &fixture.files {
        let relative = Path::new(&file.path);
        if relative.is_absolute()
            || relative.components().any(|component| {
                !matches!(component, std::path::Component::Normal(_))
            })
        {
            return Err(format!("invalid fixture path: {}", file.path));
        }
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("file has a parent"))
            .map_err(|error| error.to_string())?;
        fs::write(path, &file.text).map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub(crate) async fn grade_case(
    fixture: &Fixture,
    mode: Mode,
    source: &str,
    timings: bool,
) -> Result<CaseReport, String> {
    let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
    stage_files(directory.path(), fixture)?;
    let structured = mode != Mode::TextOnlyReference;
    let simulator = mode == Mode::ScriptedInferSimulator;
    let host = Arc::new(OfflineHost::new(
        Root::new(directory.path()),
        structured,
        simulator,
    ));
    let outcome = run(
        host.clone(),
        Request {
            call_id: "offline".into(),
            source: options::parse(source)
                .map_err(|error| error.to_string())?,
            store: Snapshot::new(),
            cancel: CancellationToken::new(),
        },
    )
    .await;
    let failure = outcome.failure.as_ref().map(|error| error.head());
    let envelope = outcome
        .items
        .iter()
        .find_map(|item| serde_json::from_str::<Value>(item.text()?).ok());
    let result = envelope
        .as_ref()
        .and_then(|value| value.get("result"))
        .cloned();
    let incomplete = failure.is_some()
        || envelope
            .as_ref()
            .and_then(|value| value.get("incomplete"))
            .and_then(Value::as_bool)
            .unwrap_or(true);
    Ok(CaseReport {
        fixture: fixture.name.clone(),
        mode,
        correct: !incomplete && result.as_ref() == Some(&fixture.expected),
        result,
        incomplete,
        tool_calls: outcome.calls_total,
        provider_round_trips: 0,
        simulated_round_trips: host.infer_calls.load(Ordering::Relaxed),
        repairs: 0,
        latency_ms: timings.then_some(outcome.wall.as_millis() as u64),
        reported_usage: ReportedUsage::default(),
        failure,
    })
}

/// Execute six fixed scenarios. No provider object is constructed or opened.
pub async fn evaluate_offline() -> Result<EvalReport, String> {
    evaluate_offline_with_timings(false).await
}

/// Execute the offline scenarios, optionally reporting observed VM wall time.
pub async fn evaluate_offline_with_timings(
    timings: bool,
) -> Result<EvalReport, String> {
    let scenarios = [
        (
            fixtures::search(),
            SEARCH_TEXT,
            SEARCH_STRUCTURED,
            Mode::StructuredTools,
        ),
        (
            fixtures::test_log(2_400),
            LOG_TEXT,
            LOG_STRUCTURED,
            Mode::StructuredTools,
        ),
        (
            fixtures::semantic_changes(),
            SEMANTIC_TEXT,
            SEMANTIC_SIMULATOR,
            Mode::ScriptedInferSimulator,
        ),
    ];
    let mut cases = Vec::new();
    for (fixture, text, improved, improved_mode) in scenarios {
        cases.push(
            grade_case(&fixture, Mode::TextOnlyReference, text, timings)
                .await?,
        );
        cases.push(
            grade_case(&fixture, improved_mode, improved, timings).await?,
        );
    }
    let evidence = if timings {
        "offline_observed_host_runtime"
    } else {
        "offline_deterministic_scripts"
    };
    Ok(EvalReport {
        format_version: 1,
        evidence: evidence.into(),
        cases,
        matrix: None,
    })
}

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]
mod tests {
    use super::*;

    #[test]
    fn live_gate_rejects_default_and_partial_budgets() {
        assert!(
            reject_live(LiveLimits::default())
                .unwrap_err()
                .contains("requires positive")
        );
        let mut limits = LiveLimits {
            max_provider_attempts: Some(2),
            max_usd: Some(0.25),
            max_seconds: Some(10),
            max_output_tokens: None,
        };
        assert!(
            reject_live(limits)
                .unwrap_err()
                .contains("requires positive")
        );
        limits.max_output_tokens = Some(1_000);
        assert!(reject_live(limits).unwrap_err().contains("unsupported"));
        limits.max_usd = Some(f64::NAN);
        assert!(
            reject_live(limits)
                .unwrap_err()
                .contains("requires positive")
        );
    }

    #[test]
    fn fixed_tool_definitions_expose_only_offline_operations() {
        let root = Root::new("/tmp/codemode-eval-tool-schema");
        let text = OfflineHost::new(root.clone(), false, false).tools();
        assert_eq!(
            text.iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            ["grep", "read"]
        );
        assert!(text.iter().all(|tool| tool.output_schema.is_none()));
        let structured = OfflineHost::new(root, true, true).tools();
        assert_eq!(
            structured
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            ["grep", "read", "infer"]
        );
        assert!(structured.iter().all(|tool| tool.output_schema.is_some()));
        assert_eq!(structured[2].input_schema, InferRequest::parameters());
    }

    #[test]
    fn scripted_classifier_rejects_changed_negative_evidence_and_bad_schema() {
        let fixture = fixtures::semantic_changes();
        let original = &fixture.files[0].text;
        let request = |context: String, schema: Value| {
            InferRequest::parse(json!({
            "task": "Extract changes requiring existing clients to change code", "context": context, "schema": schema
        })).unwrap()
        };
        let schema = json!({"type": "array", "items": {"type": "object"}});
        assert!(
            answer_scripted_semantic_request(&request(
                original.clone(),
                schema.clone()
            ))
            .is_ok()
        );
        let changed = original.replace("was removed", "was retained");
        assert!(
            answer_scripted_semantic_request(&request(changed, schema))
                .is_err()
        );
        let wrong_schema =
            request(original.clone(), json!({"type": "integer"}));
        assert!(
            wrong_schema
                .decode_answer(
                    answer_scripted_semantic_request(&wrong_schema).unwrap()
                )
                .is_err()
        );
        assert!(InferRequest::parse(json!({"task": "x", "context": original, "schema": {"type": "not-a-type"}})).is_err());
    }

    #[tokio::test]
    async fn oracle_flags_wrong_answers() {
        let mut fixture = fixtures::search();
        fixture.expected = json!([]);
        let graded = grade_case(
            &fixture,
            Mode::StructuredTools,
            SEARCH_STRUCTURED,
            false,
        )
        .await
        .unwrap();
        assert!(!graded.correct);
        assert!(!graded.incomplete);
        assert_eq!(graded.tool_calls, 1);
    }

    #[tokio::test]
    async fn simulated_round_trips_count_each_infer_call() {
        // Inventory: a repeated valid simulator request makes two VM calls;
        // the independent call-count oracle is the two calls in this program.
        // The fixed semantic fixture needs no generator or shrinking. Hegel's
        // workspace hegel.toml governs the generated fixture properties in CI.
        let source = r#"
local document = tools.read({path = 'changes.md'})
local request = {
    task = 'Extract changes requiring existing clients to change code',
    context = document.text,
    schema = {type = 'array', items = {type = 'object', properties = {
        component = {type = 'string'}, before = {type = 'string'}, after = {type = 'string'}
    }, required = {'component', 'before', 'after'}, additionalProperties = false}}
}
tools.infer(request)
local second = tools.infer(request)
return {result = second.value, incomplete = false}
"#;
        let fixture = fixtures::semantic_changes();
        let graded =
            grade_case(&fixture, Mode::ScriptedInferSimulator, source, false)
                .await
                .unwrap();
        assert!(graded.correct);
        assert_eq!(graded.simulated_round_trips, 2);
        assert_eq!(graded.provider_round_trips, 0);
        assert_eq!(graded.reported_usage, ReportedUsage::default());
    }
}
