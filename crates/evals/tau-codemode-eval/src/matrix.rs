//! Agent-owned, offline comparison runs. Module source never receives the oracle.

use std::{collections::BTreeMap, fs::File, io::Read as _, path::Path};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tau_agent::agent::{Agent, Checkpoint};
use tau_ai::message::{InputBlock, Message, ToolResultMessage, Usage};
use tau_artifacts::{Bytes, Quotas};
use tau_codemode::{Codemode, modules::Definition};
use tau_store::{Entry, Store};
use tau_testing::{block_on_io, scripted::ScriptedModel};
use tau_tools::{
    path::Root,
    plugin::{CodingTools, Tool},
};

use crate::{
    fixtures::{self, Fixture},
    runner::{CaseReport, Mode, ReportedUsage, grade_case, stage_files},
};

/// Source authored for this offline reference, before the fixture oracle is read.
/// The function takes task parameters and reads the current staged files.
pub const REFERENCE_SOURCE: &str = r#"
return function(task)
    if task.kind == 'search' then
        local listing = tools.grep({pattern=task.pattern, glob=task.glob, literal=true})
        local found = array({})
        for _, line in ipairs(listing.lines) do
            if line.kind == 'match' and not line.truncated then
                table.insert(found, {path=line.path, line=line.line, text=line.text})
            end
        end
        return {result=found, incomplete=not listing.complete}
    elseif task.kind == 'log' then
        local display = tools.read({path=task.path})
        assert(display.artifact ~= nil, display.artifact_error or 'artifact missing')
        local offset, chunks = 0, {}
        while true do
            local page = tools.artifact_read({id=display.artifact.id, offset=offset, limit=8192, encoding='utf8'})
            table.insert(chunks, page.data)
            if page.next_offset == nil or page.next_offset == json.null then break end
            assert(page.next_offset > offset, 'artifact page did not advance')
            offset = page.next_offset
        end
        local content = table.concat(chunks)
        local checksum, position = 0, 0
        for _, chunk in ipairs(chunks) do
            for index = 1, #chunk do
                position = position + 1
                checksum = (checksum + position * string.byte(chunk, index)) % 1000000007
            end
        end
        local failures, incomplete = array({}), false
        for line in string.gmatch(content, '[^\n]+') do
            local name, message = string.match(line, '^FAIL\t([^\t]+)\t(.*)$')
            if name then
                if #failures >= 128 or #name > 256 or #message > 256 then incomplete = true
                else table.insert(failures, {test=name, message=message}) end
            end
        end
        return {result=failures, incomplete=incomplete, bytes=#content, checksum=checksum,
            artifact_id=display.artifact.id, publisher_digest=display.artifact.digest}
    elseif task.kind == 'semantic' then
        local document = tools.read({path=task.path})
        assert(document.complete, 'semantic document incomplete')
        local changes = array({})
        local before, after = string.match(document.text, 'option `([^`]+)` was removed%. Clients must use `([^`]+)` instead')
        if before then table.insert(changes, {component='connection option', before=before, after=after}) end
        if string.find(document.text, 'returns null when no item exists. It previously raised NotFound', 1, true) then
            table.insert(changes, {component='lookup function', before='raises NotFound', after='returns null'})
        end
        return {result=changes, incomplete=false}
    elseif task.kind == 'workflow' then
        local config = json.decode(tools.read({path=task.path}).text)
        return {result={command=config.verify_command or config.test_command}, incomplete=false}
    end
    error('unknown task kind')
end
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceOrigin {
    Human,
    Agent,
}

/// Caller-supplied bytes and claims. Reported usage is unverified, even when complete.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalManifest {
    pub origin: SourceOrigin,
    pub name: String,
    pub version: String,
    pub source: String,
    #[serde(default = "empty_object")]
    pub signatures: Value,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub dependency_sources: Vec<ManifestDependency>,
    #[serde(default)]
    pub development_usage: Option<ReportedUsage>,
    #[serde(default)]
    pub maintenance_usage: Option<ReportedUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestDependency {
    pub name: String,
    pub version: String,
    pub source: String,
    #[serde(default = "empty_object")]
    pub signatures: Value,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
}

fn empty_object() -> Value {
    json!({})
}

impl ExternalManifest {
    pub fn validate(&self) -> Result<(), String> {
        if !self.signatures.is_object() {
            return Err("signatures must be an object".into());
        }
        if self.dependency_sources.len() > 127 {
            return Err("manifest has more than 127 dependency sources".into());
        }
        for usage in [&self.development_usage, &self.maintenance_usage]
            .into_iter()
            .flatten()
        {
            if usage.usd.is_some_and(|usd| !usd.is_finite() || usd < 0.0) {
                return Err(
                    "reported USD must be finite and nonnegative".into()
                );
            }
        }
        let mut available = BTreeMap::new();
        let mut registered_bytes = 0_usize;
        for child in &self.dependency_sources {
            if !child.signatures.is_object() {
                return Err(format!(
                    "dependency {} signatures must be an object",
                    child.name
                ));
            }
            for (name, version) in &child.dependencies {
                if available.get(name) != Some(version) {
                    return Err(format!(
                        "dependency {} requires unavailable {name}@{version}",
                        child.name
                    ));
                }
            }
            let definition = Definition::new(
                child.name.clone(),
                child.source.clone(),
                child.signatures.clone(),
                child.dependencies.clone(),
            )?;
            registered_bytes = registered_bytes
                .checked_add(definition_size(&definition)?)
                .ok_or("module definitions exceed 1 MiB")?;
            if registered_bytes > tau_codemode::modules::MAX_REGISTERED_BYTES {
                return Err("module definitions exceed 1 MiB".into());
            }
            if definition.version() != child.version {
                return Err(format!(
                    "dependency {} version does not match source and metadata",
                    child.name
                ));
            }
            if available
                .insert(child.name.clone(), child.version.clone())
                .is_some()
            {
                return Err(format!(
                    "duplicate dependency source: {}",
                    child.name
                ));
            }
        }
        if available.contains_key(&self.name) {
            return Err("root name duplicates dependency source".into());
        }
        for (name, version) in &self.dependencies {
            if available.get(name) != Some(version) {
                return Err(format!(
                    "root requires unavailable {name}@{version}"
                ));
            }
        }
        let definition = Definition::new(
            self.name.clone(),
            self.source.clone(),
            self.signatures.clone(),
            self.dependencies.clone(),
        )?;
        registered_bytes = registered_bytes
            .checked_add(definition_size(&definition)?)
            .ok_or("module definitions exceed 1 MiB")?;
        if registered_bytes > tau_codemode::modules::MAX_REGISTERED_BYTES {
            return Err("module definitions exceed 1 MiB".into());
        }
        if definition.version() != self.version {
            return Err(
                "module version does not match source and metadata".into()
            );
        }
        Ok(())
    }
}

fn definition_size(definition: &Definition) -> Result<usize, String> {
    serde_json::to_vec(definition)
        .map(|bytes| bytes.len())
        .map_err(|error| error.to_string())
}

// Deliberately noncryptographic; the publisher's SHA-256 is checked separately.
fn weighted_byte_checksum(bytes: &[u8]) -> u64 {
    bytes.iter().enumerate().fold(0_u64, |sum, (index, byte)| {
        (sum + (index as u64 + 1) * u64::from(*byte)) % 1_000_000_007
    })
}

fn publisher_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StageCalls {
    pub codemode: usize,
    pub nested: BTreeMap<String, usize>,
    pub simulated_model_operations: usize,
    pub simulated_sdk_usage: Option<ReportedUsage>,
    pub telemetry_complete: bool,
}
impl StageCalls {
    pub fn total(&self) -> usize {
        self.codemode + self.nested.values().sum::<usize>()
    }
    pub fn add(&mut self, other: &Self) {
        self.codemode += other.codemode;
        self.simulated_model_operations += other.simulated_model_operations;
        self.simulated_sdk_usage =
            match (&self.simulated_sdk_usage, &other.simulated_sdk_usage) {
                (None, Some(usage)) => Some(usage.clone()),
                (Some(left), Some(right)) => sum_usage(left, right),
                _ => None,
            };
        self.telemetry_complete &= other.telemetry_complete;
        for (name, count) in &other.nested {
            *self.nested.entry(name.clone()).or_default() += count;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModuleCase {
    pub case: CaseReport,
    pub calls: StageCalls,
    pub reconstructed_bytes: Option<usize>,
    pub log_evidence_matches_file: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Amortization {
    pub attempted_reuse_runs: usize,
    pub successful_reuse_runs: usize,
    pub cold_cumulative_calls: Option<usize>,
    pub warm_cumulative_calls: Option<usize>,
    pub calls_per_successful_run: Option<f64>,
    pub externally_reported_development_per_run: Option<AmortizedUsage>,
    pub observed_provider_usage_per_run: Option<ReportedUsage>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AmortizedUsage {
    pub uncached_input_tokens: TokenShare,
    pub cached_input_tokens: TokenShare,
    pub cache_write_tokens: TokenShare,
    pub output_tokens: TokenShare,
    pub usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenShare {
    pub total_tokens: u64,
    pub reuse_runs: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModuleReport {
    pub origin: String,
    pub name: String,
    pub version: String,
    pub development: StageCalls,
    pub maintenance: StageCalls,
    pub development_passed: bool,
    pub maintenance_passed: bool,
    pub stage_failure: Option<String>,
    pub cases: Vec<ModuleCase>,
    pub scope_checks: Option<ScopeChecks>,
    pub amortization: Amortization,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScopeChecks {
    pub owner_run_readable: bool,
    pub unrelated_run_denied: bool,
    pub foreign_store_denied: bool,
    pub calls: StageCalls,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatrixReport {
    pub evidence: String,
    pub provider_attempts: usize,
    pub observed_provider_usage: Option<ReportedUsage>,
    pub observed_provider_latency_ms: Option<u64>,
    pub observed_provider_cost_usd: Option<f64>,
    pub policy_cases: Vec<CaseReport>,
    pub modules: Vec<ModuleReport>,
}

fn scripted_codemode_model(code: String) -> ScriptedModel {
    let bounded = format!("-- @options: {{\"timeout_ms\": 30000}}\n{code}");
    ScriptedModel::new()
        .turn(move |turn| turn.tool_call("codemode", json!({"code": bounded})))
        .turn(|turn| turn.text("done"))
}

async fn last_codemode_result(
    store: &Store,
    run: &str,
) -> Result<ToolResultMessage, String> {
    store
        .transcript(run)
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter_map(|entry| {
            if let Entry::Message { body, .. } = entry
                && let Ok(Message::ToolResult(result)) =
                    serde_json::from_str::<Message>(&body)
                && result.tool_name == "codemode"
            {
                return Some(result);
            }
            None
        })
        .next_back()
        .ok_or("missing codemode result".into())
}

fn decode_result_envelope(result: &ToolResultMessage) -> Option<Value> {
    result.content.iter().skip(1).find_map(|block| match block {
        InputBlock::Text(text) => serde_json::from_str(&text.text).ok(),
        _ => None,
    })
}

fn observed_vm_latency_ms(
    details: Option<&Value>,
    timings: bool,
) -> Option<u64> {
    timings.then(|| details?.get("wall_ms")?.as_u64()).flatten()
}

fn tool_failure(result: &ToolResultMessage) -> Option<String> {
    result.is_error.then(|| {
        result
            .content
            .iter()
            .rev()
            .find_map(|block| match block {
                InputBlock::Text(text) => Some(text.text.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "production codemode call failed".into())
    })
}

fn observed_calls(
    result: &ToolResultMessage,
    model: &ScriptedModel,
    usage: &Usage,
) -> StageCalls {
    let mut calls = StageCalls {
        codemode: 1,
        simulated_model_operations: model.requests().len(),
        simulated_sdk_usage: Some(ReportedUsage {
            uncached_input_tokens: Some(usage.input),
            cached_input_tokens: Some(usage.cache_read),
            cache_write_tokens: Some(usage.cache_write),
            output_tokens: Some(usage.output),
            usd: None,
        }),
        telemetry_complete: result
            .details
            .as_ref()
            .and_then(|details| details["complete"].as_bool())
            .unwrap_or(false),
        ..StageCalls::default()
    };
    if let Some(rows) = result
        .details
        .as_ref()
        .and_then(|details| details["calls"].as_array())
    {
        for row in rows {
            if let Some(name) = row["name"].as_str() {
                *calls.nested.entry(name.into()).or_default() += 1;
            }
        }
    }
    calls
}

fn compose_development_script(spec: ModuleSpec<'_>) -> String {
    let ModuleSpec {
        name,
        source,
        signatures,
        dependencies,
        external,
        ..
    } = spec;
    let mut definitions = String::new();
    if let Some(manifest) = external {
        for dependency in &manifest.dependency_sources {
            let definition = json!({"name":&dependency.name,"source":&dependency.source,"signatures":&dependency.signatures,"dependencies":&dependency.dependencies});
            definitions.push_str(&format!(
                "tools.module_define(json.decode({}))\n",
                json!(definition.to_string())
            ));
        }
    }
    let command = "printf 'probe-v1\\n'";
    let config = json!({"test_command": command}).to_string();
    let test_code = format!(
        "assert(require({})({{kind='workflow',path='commands.json'}}).result.command == {})",
        json!(name),
        json!(command)
    );
    let test = json!({"name":name,"code":test_code,"tools":[{"name":"read","args":{"path":"commands.json"},"value":{"text":config,"complete":true}}]});
    let define = json!({"name":name,"source":source,"signatures":signatures,"dependencies":dependencies});
    format!(
        "{definitions}local d=tools.module_define(json.decode({}))\nlocal t=tools.module_test(json.decode({}))\nlocal s=tools.module_select({{name=d.name,version=d.version}})\nreturn {{version=d.version,passed=t.passed,error=t.error,selected=s.version}}",
        json!(define.to_string()),
        json!(test.to_string())
    )
}

fn compose_maintenance_script(name: &str, changed: bool) -> String {
    let command = if changed {
        "printf 'probe-v2\\n'"
    } else {
        "printf 'probe-v1\\n'"
    };
    let config = if changed {
        json!({"verify_command": command})
    } else {
        json!({"test_command": command})
    }
    .to_string();
    let code = format!(
        "assert(require({})({{kind='workflow',path='commands.json'}}).result.command == {})",
        json!(name),
        json!(command)
    );
    let test = json!({"name":name,"code":code,"tools":[{"name":"read","args":{"path":"commands.json"},"value":{"text":config,"complete":true}}]});
    format!(
        "local t=tools.module_test(json.decode({}))\nlocal d=tools.module_inspect({{name={}}})\nlocal s=tools.module_select({{name=d.name,version=d.version}})\nreturn {{passed=t.passed,error=t.error,selected=s.version}}",
        json!(test.to_string()),
        json!(name)
    )
}

fn task_for(fixture: &Fixture) -> Value {
    if fixture.name.starts_with("structured-search") {
        json!({"kind":"search","pattern":"TODO:","glob":"*.rs"})
    } else if fixture.name.starts_with("complete-test-log") {
        json!({"kind":"log","path":"tests.log"})
    } else if fixture.name.starts_with("compatibility-extraction") {
        json!({"kind":"semantic","path":"changes.md"})
    } else {
        json!({"kind":"workflow","path":"commands.json"})
    }
}

fn compose_module_case_script(name: &str, fixture: &Fixture) -> String {
    format!(
        "return require({})(json.decode({}))",
        json!(name),
        json!(task_for(fixture).to_string())
    )
}

fn divide_usage(usage: &ReportedUsage, runs: usize) -> Option<AmortizedUsage> {
    let divide = |value: Option<u64>| {
        value.map(|total_tokens| TokenShare {
            total_tokens,
            reuse_runs: runs,
        })
    };
    if runs == 0
        || [
            usage.uncached_input_tokens,
            usage.cached_input_tokens,
            usage.cache_write_tokens,
            usage.output_tokens,
        ]
        .iter()
        .any(Option::is_none)
    {
        return None;
    }
    Some(AmortizedUsage {
        uncached_input_tokens: divide(usage.uncached_input_tokens)?,
        cached_input_tokens: divide(usage.cached_input_tokens)?,
        cache_write_tokens: divide(usage.cache_write_tokens)?,
        output_tokens: divide(usage.output_tokens)?,
        usd: usage.usd.map(|value| value / runs as f64),
    })
}

fn sum_usage(a: &ReportedUsage, b: &ReportedUsage) -> Option<ReportedUsage> {
    Some(ReportedUsage {
        uncached_input_tokens: Some(
            a.uncached_input_tokens?
                .checked_add(b.uncached_input_tokens?)?,
        ),
        cached_input_tokens: Some(
            a.cached_input_tokens?.checked_add(b.cached_input_tokens?)?,
        ),
        cache_write_tokens: Some(
            a.cache_write_tokens?.checked_add(b.cache_write_tokens?)?,
        ),
        output_tokens: Some(a.output_tokens?.checked_add(b.output_tokens?)?),
        usd: match (a.usd, b.usd) {
            (Some(x), Some(y)) => (x + y).is_finite().then_some(x + y),
            _ => None,
        },
    })
}

fn calculate_amortization(
    development: &StageCalls,
    maintenance: &StageCalls,
    cases: &[ModuleCase],
    external: Option<&ExternalManifest>,
    stages_passed: bool,
) -> Amortization {
    let attempted_reuse_runs = cases.len();
    let successful_reuse_runs = if stages_passed {
        cases.iter().filter(|case| case.case.correct).count()
    } else {
        0
    };
    let cumulative = development.total() + maintenance.total();
    let first_success = cases.iter().position(|case| case.case.correct);
    let cold_cumulative_calls =
        first_success.filter(|_| stages_passed).map(|index| {
            cumulative
                + cases[..=index]
                    .iter()
                    .map(|case| case.calls.total())
                    .sum::<usize>()
        });
    let warm_cumulative_calls = (successful_reuse_runs > 0 && stages_passed)
        .then(|| {
            cumulative
                + cases.iter().map(|case| case.calls.total()).sum::<usize>()
        });
    let externally_reported_development_per_run = external
        .and_then(|manifest| {
            sum_usage(
                manifest.development_usage.as_ref()?,
                manifest.maintenance_usage.as_ref()?,
            )
        })
        .and_then(|usage| divide_usage(&usage, successful_reuse_runs));
    Amortization {
        attempted_reuse_runs,
        successful_reuse_runs,
        cold_cumulative_calls,
        warm_cumulative_calls,
        calls_per_successful_run: warm_cumulative_calls
            .filter(|_| successful_reuse_runs > 0)
            .map(|calls| calls as f64 / successful_reuse_runs as f64),
        externally_reported_development_per_run,
        observed_provider_usage_per_run: None,
    }
}

async fn check_scope(
    id: &str,
    bytes: &Bytes,
    store: &Store,
    owner: &Checkpoint,
) -> Result<ScopeChecks, String> {
    let positive = format!(
        "local page=tools.artifact_read({{id={},offset=0,limit=8}}); return {{readable=page.id=={}}}",
        json!(id),
        json!(id)
    );
    let code = format!(
        "local ok=pcall(function() tools.artifact_read({{id={}}}) end); return {{denied=not ok}}",
        json!(id)
    );
    let mut calls = StageCalls {
        telemetry_complete: true,
        ..StageCalls::default()
    };
    let mut denials = Vec::new();
    let foreign = Store::memory().await.map_err(|error| error.to_string())?;
    let root = tempfile::tempdir().map_err(|error| error.to_string())?;
    let model = scripted_codemode_model(positive);
    let run = Agent::new(model.clone())
        .plugin(Codemode::new(None))
        .plugin(
            CodingTools::new(Root::new(root.path()))
                .only(&[Tool::Read, Tool::Grep])
                .with_artifacts(bytes.clone()),
        )
        .fork(owner)
        .run("read owned artifact grant", store)
        .await
        .map_err(|error| error.to_string())?;
    let result = last_codemode_result(store, &run.run.0).await?;
    calls.add(&observed_calls(&result, &model, &run.usage));
    let owner_run_readable = !result.is_error
        && decode_result_envelope(&result)
            .is_some_and(|value| value["readable"] == true);
    for scope in [store, &foreign] {
        let root = tempfile::tempdir().map_err(|error| error.to_string())?;
        let model = scripted_codemode_model(code.clone());
        let run = Agent::new(model.clone())
            .plugin(Codemode::new(None))
            .plugin(
                CodingTools::new(Root::new(root.path()))
                    .only(&[Tool::Read, Tool::Grep])
                    .with_artifacts(bytes.clone()),
            )
            .run("attempt an ungranted artifact read", scope)
            .await
            .map_err(|error| error.to_string())?;
        let result = last_codemode_result(scope, &run.run.0).await?;
        calls.add(&observed_calls(&result, &model, &run.usage));
        denials.push(
            !result.is_error
                && decode_result_envelope(&result)
                    .is_some_and(|value| value["denied"] == true),
        );
    }
    Ok(ScopeChecks {
        owner_run_readable,
        unrelated_run_denied: denials[0],
        foreign_store_denied: denials[1],
        calls,
    })
}

struct ModuleSpec<'a> {
    source: &'a str,
    name: &'a str,
    signatures: &'a Value,
    dependencies: &'a BTreeMap<String, String>,
    origin: &'a str,
    external: Option<&'a ExternalManifest>,
}

async fn run_module(
    spec: ModuleSpec<'_>,
    fixtures: &[Fixture],
    timings: bool,
) -> Result<ModuleReport, String> {
    let ModuleSpec {
        source,
        name,
        signatures,
        dependencies,
        origin,
        external,
    } = spec;
    let version = Definition::new(
        name.into(),
        source.into(),
        signatures.clone(),
        dependencies.clone(),
    )?
    .version()
    .to_owned();
    let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
    let bytes =
        Bytes::new(directory.path().join("artifacts"), Quotas::default())
            .map_err(|error| error.to_string())?;
    let store = Store::memory().await.map_err(|error| error.to_string())?;
    let development_model =
        scripted_codemode_model(compose_development_script(ModuleSpec {
            source,
            name,
            signatures,
            dependencies,
            origin,
            external,
        }));
    let development = Agent::new(development_model.clone())
        .plugin(Codemode::new(None))
        .run("define and test offline module", &store)
        .await
        .map_err(|error| error.to_string())?;
    let result = last_codemode_result(&store, &development.run.0).await?;
    let development_calls =
        observed_calls(&result, &development_model, &development.usage);
    let result_value = decode_result_envelope(&result);
    let development_passed = !result.is_error
        && development_calls.telemetry_complete
        && result_value.as_ref().is_some_and(|value| {
            value["passed"] == true
                && value["selected"].as_str() == Some(version.as_str())
                && value["version"].as_str() == Some(version.as_str())
        });
    let mut stage_failure = (!development_passed).then(|| {
        let reason = result_value
            .as_ref()
            .and_then(|value| value["error"].as_str())
            .map(str::to_owned)
            .or_else(|| tool_failure(&result))
            .unwrap_or_else(|| "module define or test failed".into());
        format!("development: {reason}")
    });
    let mut maintenance_calls = StageCalls::default();
    let mut maintenance_passed = false;
    if development_passed {
        let model =
            scripted_codemode_model(compose_maintenance_script(name, true));
        let maintenance = Agent::new(model.clone())
            .plugin(Codemode::new(None))
            .resume(&development.run)
            .run("test changed module input", &store)
            .await
            .map_err(|error| error.to_string())?;
        let result = last_codemode_result(&store, &maintenance.run.0).await?;
        maintenance_calls = observed_calls(&result, &model, &maintenance.usage);
        let value = decode_result_envelope(&result);
        maintenance_passed = !result.is_error
            && maintenance_calls.telemetry_complete
            && value.as_ref().is_some_and(|value| {
                value["passed"] == true
                    && value["selected"].as_str() == Some(version.as_str())
            });
        if !maintenance_passed {
            let reason = value
                .as_ref()
                .and_then(|value| value["error"].as_str())
                .map(str::to_owned)
                .or_else(|| tool_failure(&result))
                .unwrap_or_else(|| "module test failed".into());
            stage_failure = Some(format!("maintenance: {reason}"));
        }
    }
    let mut cases = Vec::new();
    let mut scope_checks = None;
    if development_passed && maintenance_passed {
        for fixture in fixtures {
            let staged =
                tempfile::tempdir().map_err(|error| error.to_string())?;
            stage_files(staged.path(), fixture)?;
            let model = scripted_codemode_model(compose_module_case_script(
                name, fixture,
            ));
            let run = Agent::new(model.clone())
                .plugin(Codemode::new(None))
                .plugin(
                    CodingTools::new(Root::new(staged.path()))
                        .only(&[Tool::Read, Tool::Grep])
                        .with_artifacts(bytes.clone()),
                )
                .resume(&development.run)
                .run(&fixture.task, &store)
                .await
                .map_err(|error| error.to_string())?;
            let result = last_codemode_result(&store, &run.run.0).await?;
            let calls = observed_calls(&result, &model, &run.usage);
            let value = decode_result_envelope(&result);
            if scope_checks.is_none()
                && fixture.name.starts_with("complete-test-log")
                && let Some(id) = value
                    .as_ref()
                    .and_then(|value| value["artifact_id"].as_str())
            {
                scope_checks = Some(
                    check_scope(id, &bytes, &store, &run.checkpoint()).await?,
                );
            }
            let answer = value
                .as_ref()
                .and_then(|value| value.get("result"))
                .cloned();
            let out_of_contract_call =
                ["module_define", "module_test", "module_select", "infer"]
                    .iter()
                    .any(|name| {
                        calls.nested.get(*name).copied().unwrap_or(0) > 0
                    });
            let incomplete = result.is_error
                || !calls.telemetry_complete
                || out_of_contract_call
                || value
                    .as_ref()
                    .and_then(|value| value["incomplete"].as_bool())
                    .unwrap_or(true);
            let reconstructed_bytes = value
                .as_ref()
                .and_then(|value| value["bytes"].as_u64())
                .map(|bytes| bytes as usize);
            let is_log = fixture.name.starts_with("complete-test-log");
            let log_evidence_matches_file = is_log.then(|| {
                fixture.files.first().is_some_and(|file| {
                    let expected = file.text.as_bytes();
                    value.as_ref().is_some_and(|value| {
                        value["bytes"].as_u64() == Some(expected.len() as u64)
                            && value["checksum"].as_u64()
                                == Some(weighted_byte_checksum(expected))
                            && value["publisher_digest"].as_str()
                                == Some(publisher_digest(expected).as_str())
                    })
                })
            });
            let artifact_evidence = !is_log
                || (value
                    .as_ref()
                    .and_then(|value| value["artifact_id"].as_str())
                    .is_some()
                    && calls.nested.get("artifact_read").copied().unwrap_or(0)
                        >= 2);
            let case = CaseReport {
                fixture: fixture.name.clone(),
                mode: if external.is_some() {
                    Mode::ExternalImmutableModule
                } else {
                    Mode::AssistantScriptedModule
                },
                correct: !incomplete
                    && answer.as_ref() == Some(&fixture.expected)
                    && log_evidence_matches_file.unwrap_or(true)
                    && artifact_evidence,
                result: answer,
                incomplete,
                tool_calls: calls.nested.values().sum(),
                provider_round_trips: 0,
                simulated_round_trips: 0,
                repairs: 0,
                latency_ms: observed_vm_latency_ms(
                    result.details.as_ref(),
                    timings,
                ),
                reported_usage: ReportedUsage::default(),
                failure: if out_of_contract_call {
                    Some(
                        "module used an out-of-contract tool during reuse"
                            .into(),
                    )
                } else {
                    tool_failure(&result)
                },
            };
            cases.push(ModuleCase {
                case,
                calls,
                reconstructed_bytes,
                log_evidence_matches_file,
            });
        }
    }
    if scope_checks.as_ref().is_some_and(|check| {
        !check.calls.telemetry_complete
            || !check.owner_run_readable
            || !check.unrelated_run_denied
            || !check.foreign_store_denied
    }) {
        for case in &mut cases {
            if case.case.fixture.starts_with("complete-test-log") {
                case.case.correct = false;
            }
        }
        stage_failure = Some("artifact scope check failed".into());
    }
    let amortization = calculate_amortization(
        &development_calls,
        &maintenance_calls,
        &cases,
        external,
        development_passed && maintenance_passed,
    );
    let report = ModuleReport {
        origin: origin.into(),
        name: name.into(),
        version,
        development: development_calls,
        maintenance: maintenance_calls,
        development_passed,
        maintenance_passed,
        stage_failure,
        cases,
        scope_checks,
        amortization,
    };
    drop(store);
    drop(bytes);
    drop(directory);
    Ok(report)
}

async fn grade_policy_cases(
    timings: bool,
    fixtures: &[Fixture],
) -> Result<Vec<CaseReport>, String> {
    let mut cases = Vec::new();
    for fixture in fixtures {
        if fixture.name.starts_with("structured-search") {
            cases.push(
                grade_case(
                    fixture,
                    Mode::TextOnlyReference,
                    super::runner::SEARCH_TEXT,
                    timings,
                )
                .await?,
            );
            cases.push(
                grade_case(
                    fixture,
                    Mode::StructuredTools,
                    super::runner::SEARCH_STRUCTURED,
                    timings,
                )
                .await?,
            );
        } else if fixture.name.starts_with("complete-test-log") {
            cases.push(
                grade_case(
                    fixture,
                    Mode::TextOnlyReference,
                    super::runner::LOG_TEXT,
                    timings,
                )
                .await?,
            );
            cases.push(
                grade_case(
                    fixture,
                    Mode::StructuredTools,
                    super::runner::LOG_STRUCTURED,
                    timings,
                )
                .await?,
            );
        } else if fixture.name.starts_with("compatibility-extraction") {
            cases.push(
                grade_case(
                    fixture,
                    Mode::TextOnlyReference,
                    super::runner::SEMANTIC_TEXT,
                    timings,
                )
                .await?,
            );
            cases.push(
                grade_case(
                    fixture,
                    Mode::ScriptedInferSimulator,
                    super::runner::SEMANTIC_SIMULATOR,
                    timings,
                )
                .await?,
            );
        } else {
            const WORKFLOW_TEXT: &str = "local c=json.decode(tools.read({path='commands.json'})); return {result={command=c.verify_command or c.test_command},incomplete=false}";
            const WORKFLOW_STRUCTURED: &str = "local c=json.decode(tools.read({path='commands.json'}).text); return {result={command=c.verify_command or c.test_command},incomplete=false}";
            cases.push(
                grade_case(
                    fixture,
                    Mode::TextOnlyReference,
                    WORKFLOW_TEXT,
                    timings,
                )
                .await?,
            );
            cases.push(
                grade_case(
                    fixture,
                    Mode::StructuredTools,
                    WORKFLOW_STRUCTURED,
                    timings,
                )
                .await?,
            );
        }
    }
    Ok(cases)
}

/// Runs production Agent/Codemode/CodingTools/Store paths with a scripted parent model.
/// No provider transport is constructed; optional external bytes are never generated here.
pub fn evaluate_matrix(
    manifest: Option<&ExternalManifest>,
    timings: bool,
) -> Result<MatrixReport, String> {
    if let Some(manifest) = manifest {
        manifest.validate()?;
    }
    let fixtures = fixtures::matrix_corpus();
    block_on_io(async {
        let policy_cases = grade_policy_cases(timings, &fixtures).await?;
        let empty_signatures = json!({});
        let empty_dependencies = BTreeMap::new();
        let mut modules = vec![
            run_module(
                ModuleSpec {
                    source: REFERENCE_SOURCE,
                    name: "reference_matrix",
                    signatures: &empty_signatures,
                    dependencies: &empty_dependencies,
                    origin: "assistant_scripted_reference",
                    external: None,
                },
                &fixtures,
                timings,
            )
            .await?,
        ];
        if let Some(manifest) = manifest {
            modules.push(
                run_module(
                    ModuleSpec {
                        source: &manifest.source,
                        name: &manifest.name,
                        signatures: &manifest.signatures,
                        dependencies: &manifest.dependencies,
                        origin: match manifest.origin {
                            SourceOrigin::Human => "external_human_unverified",
                            SourceOrigin::Agent => "external_agent_unverified",
                        },
                        external: Some(manifest),
                    },
                    &fixtures,
                    timings,
                )
                .await?,
            );
        }
        Ok(MatrixReport {
            evidence: if timings {
                "offline_agent_observed_host_runtime"
            } else {
                "offline_agent_deterministic"
            }
            .into(),
            provider_attempts: 0,
            observed_provider_usage: None,
            observed_provider_latency_ms: None,
            observed_provider_cost_usd: None,
            policy_cases,
            modules,
        })
    })
}

pub fn load_manifest(path: &Path) -> Result<ExternalManifest, String> {
    const MAX_MANIFEST_BYTES: u64 = 2 * 1024 * 1024;
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|error| error.to_string())?
        .take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err("module manifest exceeds 2 MiB".into());
    }
    let manifest: ExternalManifest =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    manifest.validate()?;
    Ok(manifest)
}

#[cfg(test)]
mod properties {
    use hegel::{TestCase, generators as gs};
    use serde_json::json;

    use super::*;

    /// Inventory: the selected reference module returns the complete sorted
    /// literal search scan. Oracle: scan every .rs line independently of the
    /// fixture constructor and VM. Generator: short valid path/marker suffixes,
    /// with fixed match, partial-match, and absent-match branches; shrinking
    /// shortens suffixes but preserves punctuation, Unicode, and each branch.
    /// Twelve cases bound the expensive owned Agent/module/filesystem pipeline.
    #[hegel::test(test_cases = 12)]
    fn reference_module_search_matches_sorted_rust_line_scan(tc: TestCase) {
        let segment: String = tc.draw(gs::from_regex("[a-z]{1,8}"));
        let suffix: String = tc.draw(gs::from_regex("[a-z]{0,8}"));
        let path = format!("src/{segment}: 雪 case.rs");
        let marker = format!("TODO:{suffix}");
        let mut fixtures = Vec::new();
        let mut oracles = Vec::new();
        for branch in 0..3 {
            let mut fixture = fixtures::search_with_path(&path);
            fixture.name = format!("structured-search-generated-{branch}");
            fixture.files = vec![
                fixtures::File {
                    path: path.clone(),
                    text: if branch == 0 {
                        format!("// {marker}\nlet n = 1;\n// TODO: second\n")
                    } else {
                        "let n = 1;\n// T O D O absent\n".into()
                    },
                },
                fixtures::File {
                    path: "src/Å: next.rs".into(),
                    text: if branch < 2 {
                        format!("let n = 2;\n// {marker}\n")
                    } else {
                        "let n = 2;\n// no marker\n".into()
                    },
                },
                fixtures::File {
                    path: "src/clean.rs".into(),
                    text: "// T O D O is absent\n".into(),
                },
                fixtures::File {
                    path: "notes.txt".into(),
                    text: format!("{marker} is not a Rust match\n"),
                },
            ];
            assert_eq!(
                task_for(&fixture),
                json!({"kind":"search","pattern":"TODO:","glob":"*.rs"})
            );
            let mut rows: Vec<Value> = fixture
                .files
                .iter()
                .filter(|file| file.path.ends_with(".rs"))
                .flat_map(|file| {
                    file.text
                        .lines()
                        .enumerate()
                        .filter(|(_, line)| line.contains("TODO:"))
                        .map(move |(index, line)| {
                            json!({"path":file.path,"line":index+1,"text":line})
                        })
                })
                .collect();
            rows.sort_by(|left, right| {
                left["path"]
                    .as_str()
                    .cmp(&right["path"].as_str())
                    .then(left["line"].as_u64().cmp(&right["line"].as_u64()))
            });
            let oracle = json!(rows);
            fixture.expected = oracle.clone();
            oracles.push(oracle);
            fixtures.push(fixture);
        }
        let signatures = json!({});
        let dependencies = BTreeMap::new();
        // One enable_all runtime owns the Store for development and every case.
        let report = block_on_io(run_module(
            ModuleSpec {
                source: REFERENCE_SOURCE,
                name: "reference_search_property",
                signatures: &signatures,
                dependencies: &dependencies,
                origin: "assistant_scripted_reference",
                external: None,
            },
            &fixtures,
            false,
        ))
        .unwrap();
        assert!(
            report.development_passed && report.maintenance_passed,
            "{report:?}"
        );
        assert_eq!(report.cases.len(), oracles.len());
        for (case, expected) in report.cases.iter().zip(oracles) {
            assert_eq!(case.case.result, Some(expected), "{case:?}");
            assert!(!case.case.incomplete, "{case:?}");
            assert!(case.case.correct, "{case:?}");
            assert_eq!(case.calls.nested.get("grep"), Some(&1), "{case:?}");
            assert!(case.calls.telemetry_complete, "{case:?}");
        }
    }

    /// Inventory: owned UTF-8 artifact pages reconstruct exact source bytes.
    /// Oracle: the generated file's original UTF-8 bytes, independent of the
    /// reader and VM. Generator: 4..=64 byte pages and a <=2 KiB file; each
    /// case forces 2/3/4-byte markers before, at, and after a page boundary.
    /// Shrinking narrows the page while retaining all nine boundary shapes.
    /// Twelve cases bound the expensive owned Agent/filesystem VM path.
    #[hegel::test(test_cases = 12)]
    fn owned_utf8_artifact_pages_reconstruct_source_bytes(tc: TestCase) {
        let width = tc.draw(gs::integers::<usize>().min_value(4).max_value(64));
        // A single enable_all runtime owns every Store and artifact grant.
        block_on_io(async {
            for marker in ["é", "雪", "🚀"] {
                for displacement in [-1_isize, 0, 1] {
                    let marker_offset = match displacement {
                        -1 => width - 1,
                        0 => width,
                        1 => width + 1,
                        _ => unreachable!(),
                    };
                    let content = format!(
                        "{}{}{}",
                        "a".repeat(marker_offset),
                        marker,
                        "z".repeat(width * 2 + 1)
                    );
                    assert!(content.len() <= 2 * 1024);
                    let root = tempfile::tempdir().unwrap();
                    std::fs::write(root.path().join("input.txt"), &content)
                        .unwrap();
                    let bytes = Bytes::new(
                        root.path().join("artifacts"),
                        Quotas::default(),
                    )
                    .unwrap();
                    let store = Store::memory().await.unwrap();
                    let code = format!(
                        "local display=tools.read({{path='input.txt'}})\n\
                         assert(display.artifact ~= nil)\n\
                         local offset, chunks, pages = 0, {{}}, 0\n\
                         while true do\n\
                           local page=tools.artifact_read({{id=display.artifact.id,offset=offset,limit={width},encoding='utf8'}})\n\
                           assert(page.offset == offset)\n\
                           assert(#page.data <= {width})\n\
                           table.insert(chunks,page.data)\n\
                           pages=pages+1\n\
                           assert(pages <= 1024)\n\
                           if page.next_offset == nil or page.next_offset == json.null then\n\
                             assert(page.eof)\n\
                             break\n\
                           end\n\
                           assert(not page.eof and page.next_offset > offset)\n\
                           offset=page.next_offset\n\
                         end\n\
                         return {{result=table.concat(chunks),pages=pages,bytes=display.artifact.size_bytes}}"
                    );
                    let model = scripted_codemode_model(code);
                    let run = Agent::new(model.clone())
                        .plugin(Codemode::new(None))
                        .plugin(
                            CodingTools::new(Root::new(root.path()))
                                .only(&[Tool::Read])
                                .with_artifacts(bytes),
                        )
                        .run("reconstruct owned UTF-8 artifact", &store)
                        .await
                        .unwrap();
                    let result =
                        last_codemode_result(&store, &run.run.0).await.unwrap();
                    assert!(!result.is_error, "{result:?}");
                    let calls = observed_calls(&result, &model, &run.usage);
                    assert!(calls.telemetry_complete, "{calls:?}");
                    assert_eq!(calls.nested.get("read"), Some(&1));
                    let value = decode_result_envelope(&result).unwrap();
                    let actual = value["result"].as_str().unwrap();
                    assert_eq!(actual.as_bytes(), content.as_bytes());
                    assert_eq!(value["bytes"], json!(content.len()));
                    let pages = value["pages"].as_u64().unwrap();
                    assert!(pages >= 2);
                    assert_eq!(
                        calls.nested.get("artifact_read"),
                        Some(&(pages as usize))
                    );
                }
            }
        });
    }

    /// Inventory: switching the config key preserves the current command.
    /// The key and command suffix are generated directly; shrinking keeps a
    /// valid JSON object and a nonempty command. The oracle reads that JSON
    /// independently of the VM program.
    #[hegel::test]
    fn changed_workflow_inputs_use_current_key(tc: TestCase) {
        let changed = tc.draw(gs::booleans());
        let suffix: String = tc.draw(gs::from_regex("[a-z]{1,8}"));
        let command = format!("printf '{suffix}\\n'");
        let mut fixture = fixtures::repeated_workflow(changed);
        fixture.files[0].text = if changed {
            json!({"verify_command": command}).to_string()
        } else {
            json!({"test_command": command}).to_string()
        };
        let parsed: Value =
            serde_json::from_str(&fixture.files[0].text).unwrap();
        fixture.expected = json!({"command": parsed.get("verify_command").or_else(|| parsed.get("test_command")).unwrap()});
        const SOURCE: &str = "local c=json.decode(tools.read({path='commands.json'}).text); return {result={command=c.verify_command or c.test_command},incomplete=false}";
        let case = block_on_io(grade_case(
            &fixture,
            Mode::StructuredTools,
            SOURCE,
            false,
        ))
        .unwrap();
        assert!(case.correct, "{case:?}");
    }

    /// Inventory: stage token channels are conserved under amortization.
    /// All channels are generated as present values by construction; Hegel
    /// shrinks counts and the positive run count structurally. The independent
    /// oracle uses direct integer addition for each channel.
    #[hegel::test]
    fn reported_token_channels_conserve_stage_totals(tc: TestCase) {
        let development = tc.draw(gs::integers::<u64>().max_value(10_000));
        let maintenance = tc.draw(gs::integers::<u64>().max_value(10_000));
        let runs = tc.draw(gs::integers::<usize>().min_value(1).max_value(16));
        let stage = |count| ReportedUsage {
            uncached_input_tokens: Some(count),
            cached_input_tokens: Some(count + 1),
            cache_write_tokens: Some(count + 2),
            output_tokens: Some(count + 3),
            usd: None,
        };
        let sum = sum_usage(&stage(development), &stage(maintenance)).unwrap();
        let per_run = divide_usage(&sum, runs).unwrap();
        for (index, actual) in [
            per_run.uncached_input_tokens,
            per_run.cached_input_tokens,
            per_run.cache_write_tokens,
            per_run.output_tokens,
        ]
        .into_iter()
        .enumerate()
        {
            let expected = development + maintenance + 2 * index as u64;
            assert_eq!(actual.total_tokens, expected);
            assert_eq!(actual.reuse_runs, runs);
        }
        assert!(per_run.usd.is_none());
        let mut missing = stage(development);
        missing.output_tokens = None;
        assert!(sum_usage(&missing, &stage(maintenance)).is_none());
    }

    #[test]
    fn failed_reuse_calls_remain_in_warm_cumulative_accounting() {
        let stage = |calls| StageCalls {
            codemode: 1,
            nested: BTreeMap::from([("read".into(), calls)]),
            simulated_model_operations: 2,
            simulated_sdk_usage: None,
            telemetry_complete: true,
        };
        let case = |correct, calls| ModuleCase {
            case: CaseReport {
                fixture: "accounting".into(),
                mode: Mode::AssistantScriptedModule,
                result: Some(json!({})),
                correct,
                incomplete: false,
                tool_calls: calls,
                provider_round_trips: 0,
                simulated_round_trips: 0,
                repairs: 0,
                latency_ms: None,
                reported_usage: ReportedUsage::default(),
                failure: None,
            },
            calls: stage(calls),
            reconstructed_bytes: None,
            log_evidence_matches_file: None,
        };
        let cases = [case(true, 1), case(false, 3)];
        let accounted =
            calculate_amortization(&stage(2), &stage(4), &cases, None, true);
        assert_eq!(accounted.attempted_reuse_runs, 2);
        assert_eq!(accounted.successful_reuse_runs, 1);
        assert_eq!(accounted.cold_cumulative_calls, Some(10));
        assert_eq!(accounted.warm_cumulative_calls, Some(14));
        assert_eq!(accounted.calls_per_successful_run, Some(14.0));
        let late_success = [case(false, 3), case(true, 1)];
        let accounted = calculate_amortization(
            &stage(2),
            &stage(4),
            &late_success,
            None,
            true,
        );
        assert_eq!(accounted.cold_cumulative_calls, Some(14));
        assert_eq!(accounted.warm_cumulative_calls, Some(14));
        assert_eq!(accounted.calls_per_successful_run, Some(14.0));
        let no_success = [case(false, 3)];
        let accounted = calculate_amortization(
            &stage(2),
            &stage(4),
            &no_success,
            None,
            true,
        );
        assert_eq!(accounted.cold_cumulative_calls, None);
        assert_eq!(accounted.warm_cumulative_calls, None);
    }

    #[test]
    fn missing_vm_wall_time_stays_unavailable() {
        assert_eq!(observed_vm_latency_ms(None, true), None);
        assert_eq!(
            observed_vm_latency_ms(Some(&json!({"complete":true})), true),
            None
        );
        assert_eq!(
            observed_vm_latency_ms(Some(&json!({"wall_ms":17})), true),
            Some(17)
        );
        assert_eq!(
            observed_vm_latency_ms(Some(&json!({"wall_ms":17})), false),
            None
        );
    }

    #[test]
    fn token_overflow_does_not_wrap() {
        let largest = ReportedUsage {
            uncached_input_tokens: Some(u64::MAX),
            cached_input_tokens: Some(0),
            cache_write_tokens: Some(0),
            output_tokens: Some(0),
            usd: None,
        };
        let one = ReportedUsage {
            uncached_input_tokens: Some(1),
            ..largest.clone()
        };
        assert!(sum_usage(&largest, &one).is_none());
    }

    #[test]
    fn externally_reported_usd_requires_both_stages() {
        let usage = |usd| ReportedUsage {
            uncached_input_tokens: Some(5),
            cached_input_tokens: Some(3),
            cache_write_tokens: Some(2),
            output_tokens: Some(1),
            usd,
        };
        let complete = sum_usage(&usage(Some(0.2)), &usage(Some(0.4))).unwrap();
        let per_run = divide_usage(&complete, 3).unwrap();
        assert_eq!(
            per_run.uncached_input_tokens,
            TokenShare {
                total_tokens: 10,
                reuse_runs: 3
            }
        );
        assert!((per_run.usd.unwrap() * 3.0 - 0.6).abs() < 1e-12);
        let missing_cost = sum_usage(&usage(None), &usage(Some(0.4))).unwrap();
        assert!(divide_usage(&missing_cost, 3).unwrap().usd.is_none());
    }
}
