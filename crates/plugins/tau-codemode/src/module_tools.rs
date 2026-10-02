//! Fixed nested tools for immutable conversation modules.

use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use mlua::{Lua, LuaOptions, StdLib, chunk::ChunkMode};
use serde_json::{Value, json};
use tau_agent::{
    error::ToolError,
    plugin::PluginCtx,
    tool::{AgentTool, Exposure, ToolCtx, ToolOutput},
};

use crate::{
    PLUGIN,
    modules::{
        self,
        Definition,
        ExpectedCall,
        Library,
        ModuleTest,
        Record,
        RepositoryPin,
        TestReport,
    },
    store,
};

pub(crate) const NAMES: [&str; 5] = [
    "module_define",
    "module_list",
    "module_inspect",
    "module_select",
    "module_test",
];

pub(crate) struct ModuleTool {
    name: &'static str,
    description: &'static str,
    parameters: Value,
    output_schema: Value,
    writes: Arc<tokio::sync::Mutex<()>>,
    test_slots: Arc<tokio::sync::Semaphore>,
}

impl ModuleTool {
    pub(crate) fn new(
        name: &'static str,
        writes: Arc<tokio::sync::Mutex<()>>,
        test_slots: Arc<tokio::sync::Semaphore>,
    ) -> Self {
        let (description, parameters, output_schema) = match name {
            "module_define" => (
                "Compile and register a Luau module definition, then select its content version. Does not execute source or run tests.",
                json!({"type":"object","properties":{
                    "name":{"type":"string","description":"ASCII module identifier."},
                    "source":{"type":"string","description":"Luau source returning one function or table when required."},
                    "signatures":{"type":"object","description":"Optional JSON signatures metadata; defaults to {}."},
                    "dependencies":{"type":"object","additionalProperties":{"type":"string"},"description":"Optional map of imported names to exact versions; defaults to {}."}
                },"required":["name","source"],"additionalProperties":false}),
                metadata_schema(),
            ),
            "module_list" => (
                "List selected module versions and metadata, without source.",
                empty_schema(),
                json!({"type":"array","items":metadata_schema()}),
            ),
            "module_inspect" => (
                "Inspect a module definition and its source by selected or exact version.",
                json!({"type":"object","properties":{
                    "name":{"type":"string"},"version":{"type":"string"}
                },"required":["name"],"additionalProperties":false}),
                json!({"type":"object","properties":{
                    "name":{"type":"string"},"version":{"type":"string"},
                    "source":{"type":"string"},"signatures":{},
                    "dependencies":{"type":"object","additionalProperties":{"type":"string"}},
                    "tests":{"type":"array","items":{"type":"object"}}
                },"required":["name","version","source","signatures","dependencies","tests"],"additionalProperties":false}),
            ),
            "module_select" => (
                "Select a registered version of a module; older versions remain available for rollback.",
                json!({"type":"object","properties":{
                    "name":{"type":"string"},"version":{"type":"string"}
                },"required":["name","version"],"additionalProperties":false}),
                metadata_schema(),
            ),
            "module_test" => (
                "Run assertions against a registered scratch module version in a fresh isolated VM. Exact repository dependencies are available; only supplied ordered fake calls are available. The report is saved on the scratch version.",
                json!({"type":"object","properties":{
                    "name":{"type":"string"},"version":{"type":"string"},
                    "code":{"type":"string"},
                    "tools":{"type":"array","maxItems":128,"items":{"type":"object","properties":{
                        "name":{"type":"string"},"args":{"type":"object"},"value":{},"error":{"type":"string"}
                    },"required":["name","args"],"additionalProperties":false}}
                },"required":["name","code"],"additionalProperties":false}),
                json!({"type":"object","properties":{
                    "name":{"type":"string"},"version":{"type":"string"},"passed":{"type":"boolean"},
                    "output":{"type":"string"},"output_truncated":{"type":"boolean"},
                    "calls":{"type":"array"},"error":{"type":["string","null"]},
                    "error_truncated":{"type":"boolean"}
                },"required":["name","version","passed","output","output_truncated","calls","error","error_truncated"],"additionalProperties":false}),
            ),
            _ => {
                unreachable!("only reserved module tool names are constructed")
            }
        };
        Self {
            name,
            description,
            parameters,
            output_schema,
            writes,
            test_slots,
        }
    }
}

fn empty_schema() -> Value {
    json!({"type":"object","properties":{},"additionalProperties":false})
}

fn metadata_schema() -> Value {
    json!({"type":"object","properties":{
        "name":{"type":"string"},"version":{"type":"string"},
        "signatures":{},
        "dependencies":{"type":"object","additionalProperties":{"type":"string"}}
    },"required":["name","version","signatures","dependencies"],"additionalProperties":false})
}

fn field<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("module {key} must be a string").into())
}

fn metadata(definition: &Definition) -> Value {
    json!({
        "name": definition.name(),
        "version": definition.version(),
        "signatures": definition.signatures(),
        "dependencies": definition.dependencies(),
    })
}

fn output(value: Value) -> ToolOutput {
    ToolOutput {
        structured: Some(value.clone()),
        ..ToolOutput::text(value.to_string())
    }
}

fn library(records: &[Value]) -> Library {
    modules::fold(records)
}

async fn current(plugin: &PluginCtx) -> Result<Library, ToolError> {
    let records = plugin.records().await.map_err(ToolError::other)?;
    Ok(library(&records))
}

async fn visible_modules(
    plugin: &PluginCtx,
) -> Result<(Library, Option<RepositoryPin>), ToolError> {
    let records = plugin.records().await.map_err(ToolError::other)?;
    let pin = modules::pin_for_run(&records, &plugin.run.0)
        .map_err(ToolError::from)?;
    Ok((library(&records), pin))
}

fn definition<'a>(
    library: &'a Library,
    name: &str,
    version: Option<&str>,
) -> Result<&'a Definition, ToolError> {
    modules::validate_name(name).map_err(ToolError::from)?;
    let version = match version {
        Some(version) => version,
        None => library
            .selected()
            .get(name)
            .ok_or_else(|| format!("module {name} is not selected"))?,
    };
    library
        .versions()
        .get(version)
        .filter(|definition| definition.name() == name)
        .ok_or_else(|| {
            format!("module {name}@{version} is not registered").into()
        })
}

fn compile(name: &str, version: &str, source: &str) -> Result<(), ToolError> {
    let lua = Lua::new_with(StdLib::ALL_SAFE, LuaOptions::default())
        .map_err(|error| format!("module sandbox: {error}"))?;
    lua.sandbox(true)
        .map_err(|error| format!("module sandbox: {error}"))?;
    lua.set_memory_limit(8 * 1024 * 1024)
        .map_err(|error| format!("module sandbox: {error}"))?;
    lua.load(source)
        .set_name(format!("=module:{name}@{version}"))
        .set_mode(ChunkMode::Text)
        .into_function()
        .map_err(|error| {
            format!("module syntax: {}", crate::error_text(&error))
        })?;
    Ok(())
}

#[async_trait]
impl AgentTool for ModuleTool {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        self.description
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
        let plugin = ctx
            .plugin()
            .filter(|plugin| plugin.plugin() == PLUGIN)
            .ok_or("module tools require the codemode plugin")?;
        if ctx.cancel.is_cancelled() {
            return Err("module call cancelled".into());
        }
        match self.name {
            "module_define" => {
                let name = field(&args, "name")?.to_owned();
                let source = field(&args, "source")?.to_owned();
                let signatures = args
                    .get("signatures")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let dependencies: BTreeMap<String, String> =
                    serde_json::from_value(
                        args.get("dependencies")
                            .cloned()
                            .unwrap_or_else(|| json!({})),
                    )
                    .map_err(|error| format!("module dependencies: {error}"))?;
                let definition =
                    Definition::new(name, source, signatures, dependencies)
                        .map_err(ToolError::from)?;
                compile(
                    definition.name(),
                    definition.version(),
                    definition.source(),
                )?;
                let record = store::Record::Module(Record::Define {
                    definition: definition.clone(),
                });
                let _guard = self.writes.lock().await;
                let mut library = current(plugin).await?;
                library
                    .apply(&Record::Define {
                        definition: definition.clone(),
                    })
                    .map_err(ToolError::from)?;
                if ctx.cancel.is_cancelled() {
                    return Err("module call cancelled".into());
                }
                plugin.record(&record).await.map_err(ToolError::other)?;
                plugin.report(serde_json::to_value(&record)?);
                Ok(output(metadata(&definition)))
            }
            "module_list" => {
                let (library, pin) = visible_modules(plugin).await?;
                let mut selected = pin
                    .as_ref()
                    .map(|pin| pin.selected.clone())
                    .unwrap_or_default();
                selected.extend(library.selected().clone());
                let values = selected
                    .keys()
                    .map(|name| {
                        metadata(
                            modules::resolve_visible(
                                &library,
                                pin.as_ref(),
                                name,
                                None,
                            )
                            .expect("selected module has a definition"),
                        )
                    })
                    .collect::<Vec<_>>();
                Ok(output(Value::Array(values)))
            }
            "module_inspect" => {
                let (library, pin) = visible_modules(plugin).await?;
                let name = field(&args, "name")?;
                modules::validate_name(name).map_err(ToolError::from)?;
                let version = args
                    .get("version")
                    .map(|value| {
                        value.as_str().ok_or("module version must be a string")
                    })
                    .transpose()?;
                let definition = modules::resolve_visible(
                    &library,
                    pin.as_ref(),
                    name,
                    version,
                )
                .ok_or_else(|| match version {
                    Some(version) => {
                        format!("module {name}@{version} is not registered")
                    }
                    None => format!("module {name} is not selected"),
                })?;
                let mut value = metadata(definition);
                value["source"] = Value::String(definition.source().into());
                value["tests"] =
                    serde_json::to_value(library.tests(definition.version()))?;
                Ok(output(value))
            }
            "module_test" => {
                let name = field(&args, "name")?.to_owned();
                let code = field(&args, "code")?.to_owned();
                let version = args
                    .get("version")
                    .map(|value| {
                        value.as_str().ok_or("module version must be a string")
                    })
                    .transpose()?
                    .map(str::to_owned);
                let fixtures: Vec<ExpectedCall> = serde_json::from_value(
                    args.get("tools").cloned().unwrap_or_else(|| json!([])),
                )
                .map_err(|error| format!("module test tools: {error}"))?;
                let (library, pin) = visible_modules(plugin).await?;
                let definition =
                    definition(&library, &name, version.as_deref())?.clone();
                let mut available =
                    pin.map(|pin| pin.versions).unwrap_or_default();
                available.extend(library.versions().clone());
                // Validate source and fixture quotas before acquiring a VM slot.
                let empty = TestReport {
                    name: name.clone(),
                    version: definition.version().into(),
                    passed: false,
                    output: String::new(),
                    output_truncated: false,
                    calls: vec![],
                    error: None,
                    error_truncated: false,
                };
                ModuleTest::new(
                    name.clone(),
                    definition.version().into(),
                    code.clone(),
                    fixtures.clone(),
                    empty,
                )
                .map_err(ToolError::from)?;
                let report = crate::module_tests::run_test(
                    &definition,
                    &available,
                    code.clone(),
                    fixtures.clone(),
                    ctx.cancel.clone(),
                    self.test_slots.clone(),
                )
                .await
                .map_err(ToolError::from)?;
                if ctx.cancel.is_cancelled() {
                    return Err("module test cancelled".into());
                }
                let test = ModuleTest::new(
                    name,
                    definition.version().into(),
                    code,
                    fixtures,
                    report.clone(),
                )
                .map_err(ToolError::from)?;
                let record = store::Record::Module(Record::Test { test });
                let _guard = self.writes.lock().await;
                let mut latest = current(plugin).await?;
                if let store::Record::Module(module_record) = &record {
                    latest.apply(module_record).map_err(ToolError::from)?;
                }
                if ctx.cancel.is_cancelled() {
                    return Err("module test cancelled".into());
                }
                plugin.record(&record).await.map_err(ToolError::other)?;
                plugin.report(serde_json::to_value(&record)?);
                Ok(output(serde_json::to_value(report)?))
            }
            "module_select" => {
                let name = field(&args, "name")?.to_owned();
                let version = field(&args, "version")?.to_owned();
                let record = store::Record::Module(Record::Select {
                    name: name.clone(),
                    version: version.clone(),
                });
                let _guard = self.writes.lock().await;
                let mut library = current(plugin).await?;
                library
                    .apply(&Record::Select {
                        name: name.clone(),
                        version: version.clone(),
                    })
                    .map_err(ToolError::from)?;
                if ctx.cancel.is_cancelled() {
                    return Err("module call cancelled".into());
                }
                plugin.record(&record).await.map_err(ToolError::other)?;
                plugin.report(serde_json::to_value(&record)?);
                Ok(output(metadata(definition(
                    &library,
                    &name,
                    Some(&version),
                )?)))
            }
            _ => unreachable!(),
        }
    }
}
