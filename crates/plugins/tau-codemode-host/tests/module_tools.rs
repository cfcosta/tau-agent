//! Module tool integration through nested calls and tau_store_sqlite::memory().
//! Inventory: immediate require, exact inspection, selection rollback,
//! resume/fork prefixes, hook blocking, syntax and quota rejection, and
//! reserved names. These are scenario tests; the independent generated
//! definition/alias oracle lives in modules.rs and uses workspace hegel.toml.

use async_trait::async_trait;
use serde_json::{Value, json};
use tau_agent::{
    agent::Agent,
    error::{PluginError, ToolError},
    plugin::{Decision, Plugin, PluginCtx, PluginRun, RunPlan, ToolCall},
    tool::{AgentTool, ToolCtx, ToolOutput},
};
use tau_ai::message::{InputBlock, Message, ToolResultMessage};
use tau_codemode_host::{Codemode, PLUGIN};
use tau_store::{Entry, Store};
use tau_testing::{block_on, scripted::ScriptedModel};

fn model(scripts: &[&str]) -> ScriptedModel {
    let mut model = ScriptedModel::new();
    for script in scripts {
        let code = (*script).to_owned();
        model = model
            .turn(move |turn| turn.tool_call("codemode", json!({"code": code})))
            .turn(|turn| turn.text("done"));
    }
    model
}

async fn last_result(store: &Store, run: &str) -> ToolResultMessage {
    store
        .transcript(run)
        .await
        .unwrap()
        .into_iter()
        .filter_map(|entry| {
            if let Entry::Message { body, .. } = entry
                && let Message::ToolResult(result) =
                    serde_json::from_str(&body).unwrap()
            {
                return Some(result);
            }
            None
        })
        .next_back()
        .unwrap()
}

fn values(result: &ToolResultMessage) -> Vec<Value> {
    result
        .content
        .iter()
        .skip(1)
        .filter_map(|block| {
            let InputBlock::Text(text) = block else {
                return None;
            };
            Some(
                serde_json::from_str(&text.text)
                    .unwrap_or_else(|_| json!(text.text)),
            )
        })
        .collect()
}

async fn records(store: &Store, run: &str) -> Vec<Value> {
    store
        .records(run, PLUGIN)
        .await
        .unwrap()
        .into_iter()
        .map(|record| serde_json::from_str(&record).unwrap())
        .collect()
}

#[test]
fn define_and_require_in_one_script() {
    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let agent = Agent::new(model(&[r#"
local made = tools.module_define({ name = 'double', source = 'return function(n) return n * 2 end' })
local listed = tools.module_list({})
local inspected = tools.module_inspect({ name = 'double' })
return { made = made, listed = listed, inspected = inspected, answer = require('double')(21) }
"#])).plugin(Codemode::new(None));
        let run = agent.run("go", &store).await.unwrap();
        let result = last_result(&store, &run.run.0).await;
        assert!(!result.is_error, "{result:?}");
        let value = &values(&result)[0];
        assert_eq!(value["answer"], 42);
        assert_eq!(value["made"]["name"], "double");
        assert_eq!(value["listed"].as_array().unwrap().len(), 1);
        assert!(value["listed"][0].get("source").is_none());
        assert_eq!(
            value["inspected"]["source"],
            "return function(n) return n * 2 end"
        );
        assert_eq!(value["made"]["version"], value["inspected"]["version"]);
        let records = records(&store, &run.run.0).await;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["op"], "define");
        assert_eq!(
            records[0]["definition"]["version"],
            value["made"]["version"]
        );
    });
}

#[test]
fn promotion_tool_only_records_pending_and_catalog_has_no_approval() {
    tau_testing::block_on_io(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let path = std::env::temp_dir()
            .join(format!("module-request-{}", uuid::Uuid::now_v7()));
        let agent = Agent::new(model(&[r#"
local made = tools.module_define({name='candidate',source='return {n=7}'})
local requested = tools.module_promote({name='candidate',version=made.version})
local approval = nil
for _, tool in ipairs(ALL_TOOLS) do
  if tool.name == 'module_approve' then approval = tool.name end
end
return {requested=requested,approval=approval,loaded=require('candidate').n}
"#]))
        .plugin(Codemode::new(None).with_repository(path.clone()));
        let run = agent.run("go", &store).await.unwrap();
        let result = last_result(&store, &run.run.0).await;
        assert!(!result.is_error, "{result:?}");
        let value = &values(&result)[0];
        assert_eq!(value["requested"]["status"], "pending");
        assert_eq!(value["loaded"], 7);
        assert!(value.get("approval").is_none() || value["approval"].is_null());
        let records = records(&store, &run.run.0).await;
        assert_eq!(
            records
                .iter()
                .filter(|value| value["kind"] == "promotion")
                .count(),
            1
        );
        assert_eq!(
            records
                .iter()
                .find(|value| value["kind"] == "promotion")
                .unwrap()["op"],
            "requested"
        );
        assert!(!path.join("selected.json").exists());
        let _ = std::fs::remove_dir_all(path);
    });
}

#[test]
fn replacements_inspection_rollback_and_forked_selection() {
    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let agent = Agent::new(model(&[
            "local d = tools.module_define({name='m',source='return {n=1}'})\nreturn d.version",
            "local old = tools.module_inspect({name='m'}); local loaded = require('m'); store('old',old.version); local new = tools.module_define({name='m',source='return {n=2}'})\nreturn {old=old.version,new=new.version,selected=require('m',new.version).n,pinned=require('m').n,loaded=loaded.n,old_source=tools.module_inspect({name='m',version=old.version}).source}",
            "return {selected=tools.module_list({})[1].version,answer=require('m').n}",
            "local old = tools.module_inspect({name='m',version=load('old')}); tools.module_select({name='m',version=old.version}); return require('m').n",
        ])).plugin(Codemode::new(None));
        let root = agent.run("root", &store).await.unwrap();
        let old = values(&last_result(&store, &root.run.0).await)[0]
            .as_str()
            .unwrap()
            .to_owned();
        let replacement = agent
            .fork(&root.checkpoint())
            .run("replace", &store)
            .await
            .unwrap();
        let change =
            values(&last_result(&store, &replacement.run.0).await)[0].clone();
        assert_eq!(change["old"], old);
        assert_ne!(change["new"], old);
        assert_eq!(change["selected"], 2);
        assert_eq!(change["pinned"], 1);
        assert_eq!(change["loaded"], 1);
        assert_eq!(change["old_source"], "return {n=1}");
        let sibling = agent
            .fork(&root.checkpoint())
            .run("sibling", &store)
            .await
            .unwrap();
        let sibling_value =
            values(&last_result(&store, &sibling.run.0).await)[0].clone();
        assert_eq!(sibling_value["selected"], old);
        assert_eq!(sibling_value["answer"], 1);
        // A resumed run inherits both definitions. Supply the old digest
        // through the script store so the next scripted call can roll back.
        // The fork still has its own selected alias.
        let rollback = agent
            .resume(&replacement.run)
            .run("rollback", &store)
            .await
            .unwrap();
        let result = last_result(&store, &rollback.run.0).await;
        assert!(!result.is_error, "{result:?}");
        assert_eq!(values(&result), [json!(1)]);
        assert_eq!(records(&store, &rollback.run.0).await.len(), 4);
        assert_eq!(records(&store, &sibling.run.0).await.len(), 1);
    });
}

#[derive(Clone)]
struct BlockDefine;

#[async_trait]
impl Plugin for BlockDefine {
    fn name(&self) -> &str {
        "block-definition"
    }
    async fn start(
        &self,
        _: &mut RunPlan,
        _: &PluginCtx,
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        Ok(Box::new(self.clone()))
    }
}

#[async_trait]
impl PluginRun for BlockDefine {
    async fn before_tool(
        &mut self,
        call: &mut ToolCall,
        _: &PluginCtx,
    ) -> Result<Decision, PluginError> {
        if call.name == "module_define" {
            Ok(Decision::Block("definition blocked".into()))
        } else {
            Ok(Decision::Allow)
        }
    }
}

#[test]
fn blocked_syntax_and_quota_failures_leave_no_records() {
    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let blocked = Agent::new(model(&["local ok = pcall(tools.module_define,{name='m',source='return {}'}); return {ok=ok,count=#tools.module_list({})}"]))
            .plugin(Codemode::new(None)).plugin(BlockDefine);
        let run = blocked.run("blocked", &store).await.unwrap();
        assert_eq!(
            values(&last_result(&store, &run.run.0).await),
            [json!({"ok":false,"count":0})]
        );
        assert!(records(&store, &run.run.0).await.is_empty());

        let long_source = "x".repeat(65_537);
        let script = format!(
            r#"
local bad_syntax = pcall(tools.module_define, {{name='m',source='return function('}})
local oversized = pcall(tools.module_define, {{name='m',source={}}})
return {{bad_syntax=bad_syntax,oversized=oversized,count=#tools.module_list({{}})}}
"#,
            serde_json::to_string(&long_source).unwrap()
        );
        let agent = Agent::new(model(&[&script])).plugin(Codemode::new(None));
        let run = agent.run("invalid", &store).await.unwrap();
        assert_eq!(
            values(&last_result(&store, &run.run.0).await),
            [json!({"bad_syntax":false,"oversized":false,"count":0})]
        );
        assert!(records(&store, &run.run.0).await.is_empty());
    });
}

struct ConflictingTool;

#[async_trait]
impl AgentTool for ConflictingTool {
    fn name(&self) -> &str {
        "module_define"
    }
    fn description(&self) -> &str {
        "conflict"
    }
    fn parameters(&self) -> &Value {
        static SCHEMA: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
        SCHEMA.get_or_init(|| json!({"type":"object"}))
    }
    async fn call(
        &self,
        _: Value,
        _: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        unreachable!()
    }
}

#[test]
fn reserved_module_name_rejects_conflicting_tool() {
    block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let agent = Agent::new(model(&["return true"]))
            .tool(ConflictingTool)
            .plugin(Codemode::new(None));
        let error = agent.run("go", &store).await.unwrap_err().to_string();
        assert!(
            error.contains("codemode reserves tool name `module_define`"),
            "{error}"
        );
    });
}
