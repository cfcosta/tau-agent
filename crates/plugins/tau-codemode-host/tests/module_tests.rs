//! Isolated module tests through nested tools and conversation records.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use serde_json::{Value, json};
use tau_agent::{
    agent::Agent,
    error::ToolError,
    tool::{AgentTool, ToolCtx, ToolOutput},
};
use tau_ai::message::{InputBlock, Message, ToolResultMessage};
use tau_codemode::modules;
use tau_codemode_host::{Codemode, PLUGIN};
use tau_store::{Entry, Store};
use tau_testing::{block_on_io, scripted::ScriptedModel};

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

fn value(result: &ToolResultMessage) -> Value {
    let InputBlock::Text(text) = &result.content[1] else {
        panic!("expected text")
    };
    serde_json::from_str(&text.text).unwrap()
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
fn pass_failure_and_fake_call_accounting_are_saved() {
    block_on_io(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let agent = Agent::new(model(&[r#"
local d = tools.module_define({name='calc',source='return function(n) return n * 2 end'})
local code = "assert(require('calc', '" .. d.version .. "')(21) == 42); assert(tools.fake({n=21}).answer == 42); text('checked')"
local pass = tools.module_test({name='calc',version=d.version,code=code,tools={{name='fake',args={n=21},value={answer=42}}}})
local assertion = tools.module_test({name='calc',code="assert(require('calc', '" .. d.version .. "')(21) == 43)"})
local wrong = tools.module_test({name='calc',code="pcall(function() tools.fake({n=22}) end)",tools={{name='fake',args={n=21},value=42}}})
local missing = tools.module_test({name='calc',code='return true',tools={{name='fake',args={},value=1}}})
local extra = tools.module_test({name='calc',code="tools.fake({}); pcall(function() tools.fake({}) end)",tools={{name='fake',args={},value=1}}})
local inspected = tools.module_inspect({name='calc',version=d.version})
return {pass=pass,assertion=assertion,wrong=wrong,missing=missing,extra=extra,tests=#inspected.tests}
"#])).plugin(Codemode::new(None));
        let run = agent.run("test", &store).await.unwrap();
        let result = last_result(&store, &run.run.0).await;
        assert!(!result.is_error, "{result:?}");
        let report = value(&result);
        assert_eq!(report["pass"]["passed"], true);
        assert_eq!(report["pass"]["output"], "checked");
        assert_eq!(report["pass"]["calls"][0]["status"], "ok");
        assert_eq!(report["assertion"]["passed"], false);
        assert!(
            report["assertion"]["error"]
                .as_str()
                .unwrap()
                .contains("assertion")
        );
        assert_eq!(report["wrong"]["calls"][0]["status"], "mismatch");
        assert_eq!(report["wrong"]["passed"], false);
        assert_eq!(report["missing"]["passed"], false);
        assert_eq!(report["missing"]["calls"].as_array().unwrap().len(), 0);
        assert_eq!(report["extra"]["calls"][1]["status"], "unexpected");
        assert_eq!(report["extra"]["passed"], false);
        assert_eq!(report["tests"], 5);
        let records = records(&store, &run.run.0).await;
        assert_eq!(records.len(), 6);
        let library = modules::fold(&records);
        assert_eq!(
            library
                .tests(report["pass"]["version"].as_str().unwrap())
                .len(),
            5
        );
    });
}

#[derive(Clone)]
struct LiveTool {
    name: &'static str,
    calls: Arc<AtomicUsize>,
    schema: Value,
}

#[async_trait]
impl AgentTool for LiveTool {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        "A live tool that tests must never call"
    }
    fn parameters(&self) -> &Value {
        &self.schema
    }
    async fn call(
        &self,
        _: Value,
        _: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolOutput::text("LIVE"))
    }
}

#[test]
fn fake_infer_and_bash_stay_isolated_and_versions_do_not_transfer_tests() {
    block_on_io(async {
        let calls = Arc::new(AtomicUsize::new(0));
        let store = tau_store_sqlite::memory().await.unwrap();
        let scripted = model(&[r#"
local old = tools.module_define({name='m',source='return {value=1}'})
local newer = tools.module_define({name='m',source='return {value=2}'})
local code = "assert(require('m').value == 1); assert(tools.infer({task='x',context={}}) == 'fake'); assert(tools.bash({command='x'}) == 'fake')"
local report = tools.module_test({name='m',version=old.version,code=code,tools={{name='infer',args={task='x',context={}},value='fake'},{name='bash',args={command='x'},value='fake'}}})
return {report=report,old=tools.module_inspect({name='m',version=old.version}),newer=tools.module_inspect({name='m',version=newer.version})}
"#]);
        let agent = Agent::new(scripted.clone())
            .tool(LiveTool {
                name: "bash",
                calls: calls.clone(),
                schema: json!({"type":"object"}),
            })
            .plugin(Codemode::new(None));
        let run = agent.run("test", &store).await.unwrap();
        let result = last_result(&store, &run.run.0).await;
        assert!(!result.is_error, "{result:?}");
        let report = value(&result);
        assert_eq!(report["report"]["passed"], true);
        assert_eq!(report["old"]["tests"].as_array().unwrap().len(), 1);
        assert!(
            report["old"]["tests"][0]["code"]
                .as_str()
                .unwrap()
                .starts_with("assert(require('m')")
        );
        assert_eq!(report["newer"]["tests"].as_array().unwrap().len(), 0);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(scripted.requests().len(), 2);
    });
}

#[test]
fn infinite_loop_times_out_with_a_persisted_failure() {
    block_on_io(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let agent = Agent::new(model(&[r#"
local d = tools.module_define({name='m',source='return {}'})
local report = tools.module_test({name='m',version=d.version,code='while true do end'})
return {report=report,tests=#tools.module_inspect({name='m',version=d.version}).tests}
"#])).plugin(Codemode::new(None));
        let run = agent.run("test", &store).await.unwrap();
        let result = last_result(&store, &run.run.0).await;
        assert!(!result.is_error, "{result:?}");
        let report = value(&result);
        assert_eq!(report["report"]["passed"], false);
        assert!(
            report["report"]["error"]
                .as_str()
                .unwrap()
                .contains("timed out")
        );
        assert_eq!(report["tests"], 1);
    });
}

#[test]
fn target_initialization_fails_before_vacuous_assertions() {
    block_on_io(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let agent = Agent::new(model(&[r#"
local broken = tools.module_define({name='broken',source="error('broken source')"})
local missing = tools.module_define({name='missing',source='return true',dependencies={dep=string.rep('0',64)}})
local a = tools.module_test({name='broken',version=broken.version,code='return true'})
local b = tools.module_test({name='missing',version=missing.version,code='return true'})
return {a=a,b=b}
"#])).plugin(Codemode::new(None));
        let run = agent.run("test", &store).await.unwrap();
        let result = last_result(&store, &run.run.0).await;
        assert!(!result.is_error, "{result:?}");
        let report = value(&result);
        assert_eq!(report["a"]["passed"], false);
        assert_eq!(report["b"]["passed"], false);
        assert!(
            report["a"]["error"]
                .as_str()
                .unwrap()
                .contains("broken source")
        );
        assert!(report["b"]["error"].as_str().unwrap().contains("dep"));
        let records = records(&store, &run.run.0).await;
        assert_eq!(records.len(), 4);
        let library = modules::fold(&records);
        for key in ["a", "b"] {
            let version = report[key]["version"].as_str().unwrap();
            assert_eq!(library.tests(version).len(), 1);
            assert!(!library.tests(version)[0].result().passed);
        }
    });
}

#[test]
fn explicit_null_fixture_remains_readable_even_with_empty_error() {
    block_on_io(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let agent = Agent::new(model(&[r#"
local d = tools.module_define({name='m',source='return {}'})
local r = tools.module_test({name='m',version=d.version,code="assert(tools.fake({}) == json.null)",tools={{name='fake',args={},value=json.null,error=''}}})
local saved = tools.module_inspect({name='m',version=d.version}).tests[1]
return {r.passed,r.calls[1].status,saved.tools[1].value == json.null}
"#])).plugin(Codemode::new(None));
        let run = agent.run("test", &store).await.unwrap();
        let result = last_result(&store, &run.run.0).await;
        assert!(!result.is_error, "{result:?}");
        assert_eq!(value(&result), json!([true, "error", true]));
        let data = records(&store, &run.run.0).await;
        let library = modules::fold(&data);
        let version = library.selected()["m"].clone();
        let saved = serde_json::to_value(&library.tests(&version)[0]).unwrap();
        assert_eq!(saved["tools"][0].get("value"), Some(&Value::Null));
    });
}

#[test]
fn large_diagnostics_save_bounded_reports() {
    block_on_io(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let agent = Agent::new(model(&[r#"
local d = tools.module_define({name='m',source='return {}'})
local thrown = tools.module_test({name='m',version=d.version,code="error(string.rep('x', 1048576))"})
local printed = tools.module_test({name='m',version=d.version,code="text(string.rep(string.char(0), 60000))"})
local mismatch = tools.module_test({name='m',version=d.version,code="pcall(function() tools.fake({x='small'}) end)",tools={{name='fake',args={x=string.rep('x', 900000)},value=true}}})
return {
  thrown={passed=thrown.passed,error_truncated=thrown.error_truncated,version=thrown.version},
  printed={passed=printed.passed,output_truncated=printed.output_truncated},
  mismatch={passed=mismatch.passed,error_truncated=mismatch.error_truncated,calls={{status=mismatch.calls[1].status}}}
}
"#])).plugin(Codemode::new(None));
        let run = agent.run("test", &store).await.unwrap();
        let result = last_result(&store, &run.run.0).await;
        assert!(!result.is_error, "{result:?}");
        let report = value(&result);
        assert_eq!(report["thrown"]["passed"], false);
        assert_eq!(report["thrown"]["error_truncated"], true);
        assert_eq!(report["printed"]["passed"], true);
        assert_eq!(report["printed"]["output_truncated"], true);
        assert_eq!(report["mismatch"]["passed"], false);
        assert_eq!(report["mismatch"]["calls"][0]["status"], "mismatch");
        assert_eq!(report["mismatch"]["error_truncated"], false);
        let records = records(&store, &run.run.0).await;
        assert_eq!(records.len(), 4);
        let library = modules::fold(&records);
        let saved =
            library.tests(report["thrown"]["version"].as_str().unwrap());
        assert_eq!(saved.len(), 3);
        for test in saved {
            assert!(
                serde_json::to_vec(test.result()).unwrap().len()
                    <= modules::MAX_TEST_REPORT_BYTES
            );
        }
        assert!(saved[0].result().error_truncated);
        assert!(saved[1].result().output_truncated);
        assert!(!saved[1].result().output.is_empty());
        assert!(!saved[2].result().passed);
    });
}
