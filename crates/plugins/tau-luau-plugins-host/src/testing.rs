//! `plugin_test`: a run in the plugins repository tests a plugin as it
//! stands in the run's workspace, before committing it (ADR 0027).

use std::{
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
};

use async_trait::async_trait;
use serde_json::{Value, json};
use tau_agent::{
    error::ToolError,
    plugin::Plugin,
    tool::{AgentTool, ToolCtx, ToolOutput},
};

use crate::{
    TestResult,
    runtime::{Files, load},
};

/// The tool's name.
pub const TOOL: &str = "plugin_test";

/// Adds `plugin_test` to a run whose workspace is `dir`.
pub struct PluginTesting {
    dir: PathBuf,
}

impl PluginTesting {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }
}

impl Plugin for PluginTesting {
    fn name(&self) -> &str {
        "tau-luau-plugin-test"
    }

    fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        vec![Arc::new(PluginTest {
            dir: self.dir.clone(),
        })]
    }
}

struct PluginTest {
    dir: PathBuf,
}

#[async_trait]
impl AgentTool for PluginTest {
    fn name(&self) -> &str {
        TOOL
    }

    fn description(&self) -> &str {
        "Loads a plugin from its folder in this workspace and runs its \
         tests under `tests/` against fake runs. Run it before committing \
         a plugin: a version whose tests fail does not activate."
    }

    fn parameters(&self) -> &Value {
        static SCHEMA: LazyLock<Value> = LazyLock::new(|| {
            json!({
                "type": "object",
                "properties": {
                    "plugin": {
                        "type": "string",
                        "description": "The plugin's folder, which is its name."
                    }
                },
                "required": ["plugin"],
                "additionalProperties": false
            })
        });
        &SCHEMA
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let Some(plugin) = args["plugin"].as_str() else {
            return Err("`plugin` is the plugin's folder".into());
        };
        if plugin.is_empty()
            || plugin.contains(['/', '\\'])
            || plugin.starts_with('.')
        {
            return Err(format!("{plugin:?} is not a plugin's folder").into());
        }
        let tests = test(&self.dir, plugin, ctx.cancel.clone()).await?;
        let passed = tests.iter().filter(|test| test.passed).count();
        let mut text =
            format!("{plugin} loads; {passed} of {} tests pass.", tests.len());
        if tests.is_empty() {
            text = format!("{plugin} loads, and has no tests under `tests/`.");
        }
        for test in tests.iter().filter(|test| !test.passed) {
            text.push_str(&format!(
                "\n- {}: {} failed: {}",
                test.file,
                test.name,
                test.error.as_deref().unwrap_or("")
            ));
        }
        Ok(ToolOutput {
            details: Some(json!({ "tested": plugin, "tests": tests })),
            ..ToolOutput::text(text)
        })
    }
}

/// Loads `plugin` from `dir` and runs its tests.
async fn test(
    dir: &Path,
    plugin: &str,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<Vec<TestResult>, ToolError> {
    let folder = dir.join(plugin);
    let files = tokio::task::spawn_blocking(move || Files::read(&folder))
        .await
        .map_err(|error| error.to_string())??;
    let loaded = load(plugin, files)
        .await
        .map_err(|error| format!("{plugin} does not load: {error}"))?;
    Ok(loaded.test(cancel).await)
}
