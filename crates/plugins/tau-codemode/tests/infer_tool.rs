//! Integration inventory: isolated inputs and fixed tool visibility; plain,
//! structured, and null answers; readable nested errors; model settings and
//! provider capability; shared attempt budgets, retries, deadlines, and
//! cancellation. The oracle uses ScriptedModel's recorded requests, run events,
//! and hand-written expected usage, never a generated response as its own
//! expectation. Store I/O needs `block_on_io`: paused time can jump to SQL
//! pool maintenance while waiting for SQLite, exhausting run deadlines before
//! a request starts. Fake response delays exercise real deadline/drop behavior.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use tau_agent::{
    agent::Agent,
    error::PluginError,
    event::RunEvent,
    plugin::{Decision, Plugin, PluginCtx, PluginRun, RunPlan, ToolCall},
    tool::{AgentTool, ToolCtx, ToolOutput},
};
use tau_ai::{
    llm::{Llm, LlmError, LlmSession},
    message::{
        InputBlock,
        Message,
        StopReason,
        ToolResultMessage,
        UserContent,
    },
    responses::request::{ReasoningEffort, Settings},
};
use tau_codemode::{Codemode, inference_budget::Limits};
use tau_store::{Entry, Store};
use tau_testing::{block_on_io as block_on, scripted::ScriptedModel};

#[derive(Default, Clone)]
struct Events {
    seen: Arc<Mutex<Vec<RunEvent>>>,
    block_infer: bool,
}

#[async_trait]
impl PluginRun for Events {
    async fn before_tool(
        &mut self,
        call: &mut ToolCall,
        _ctx: &PluginCtx,
    ) -> Result<Decision, PluginError> {
        if self.block_infer && call.name == "infer" {
            return Ok(Decision::Block("infer blocked by test plugin".into()));
        }
        Ok(Decision::Allow)
    }

    async fn on_event(&mut self, event: &RunEvent, _ctx: &PluginCtx) {
        self.seen.lock().unwrap().push(event.clone());
    }
}

#[async_trait]
impl Plugin for Events {
    fn name(&self) -> &str {
        "infer_test_events"
    }
    async fn start(
        &self,
        _plan: &mut RunPlan,
        _ctx: &PluginCtx,
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        Ok(Box::new(self.clone()))
    }
}

fn nested(events: &Events) -> Vec<(Value, bool, Value)> {
    events
        .seen
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            RunEvent::ToolEnd {
                parent: Some(_),
                output,
                is_error,
                ..
            } => Some((
                output.structured.clone().unwrap_or(Value::Null),
                *is_error,
                output.details.clone().unwrap_or(Value::Null),
            )),
            _ => None,
        })
        .collect()
}

async fn results(store: &Store, run: &str) -> Vec<ToolResultMessage> {
    store
        .transcript(run)
        .await
        .unwrap()
        .into_iter()
        .filter_map(|entry| match entry {
            Entry::Message { body, .. } => {
                match serde_json::from_str::<Message>(&body).unwrap() {
                    Message::ToolResult(result) => Some(result),
                    _ => None,
                }
            }
            _ => None,
        })
        .collect()
}

fn result_text(result: &ToolResultMessage) -> String {
    result
        .content
        .iter()
        .skip(1)
        .find_map(|block| match block {
            InputBlock::Text(text) => Some(text.text.clone()),
            InputBlock::Image(_) => None,
        })
        .expect("script text")
}

fn outer_call(
    script: &str,
) -> impl FnOnce(
    tau_testing::scripted::TurnBuilder,
) -> tau_testing::scripted::TurnBuilder
+ '_ {
    move |turn| turn.tool_call("codemode", json!({"code": script}))
}

fn infer_requests(
    model: &ScriptedModel,
) -> Vec<tau_testing::scripted::Request> {
    model
        .requests()
        .into_iter()
        .filter(|request| request.settings.tools.is_empty())
        .collect()
}

fn single_user_text(request: &tau_testing::scripted::Request) -> &str {
    assert_eq!(
        request.transcript.len(),
        1,
        "infer has an independent session"
    );
    match &request.transcript[0] {
        Message::User(user) => match &user.content {
            UserContent::Text(text) => text,
            _ => panic!("infer context must be text"),
        },
        _ => panic!("infer must receive one user message"),
    }
}

#[test]
fn isolated_plain_structured_and_null_answers_keep_the_script_running() {
    block_on(async {
        let script = r#"
local a = tools.infer({ task = "plain", context = { id = 7 } })
local b = tools.infer({ task = "typed", context = json.null, schema = { type = "object", properties = { n = { type = "integer" } }, required = { "n" }, additionalProperties = false } })
local c = tools.infer({ task = "null answer", context = "solo", schema = { type = "null" } })
return { a.ok, a.value, b.ok, b.value.n, c.ok, c.value == json.null }
"#;
        let model = ScriptedModel::new()
            .turn(outer_call(script))
            .turn(|turn| turn.text("hello"))
            .turn(|turn| turn.text("{\"n\":3}"))
            .turn(|turn| turn.text("null"))
            .turn(|turn| turn.text("done"));
        let events = Events::default();
        let store = Store::memory().await.unwrap();
        let agent = Agent::new(model.clone())
            .model("test-host")
            .reasoning(ReasoningEffort::High)
            .plugin(Codemode::new(None))
            .plugin(events.clone());
        let outcome = agent.run("parent secret", &store).await.unwrap();
        let result = results(&store, &outcome.run.0).await;
        assert_eq!(result.len(), 1);
        assert!(
            !result[0].is_error,
            "result={:?}; nested={:?}",
            result[0],
            nested(&events)
        );
        assert_eq!(
            serde_json::from_str::<Value>(&result_text(&result[0])).unwrap(),
            json!([true, "hello", true, 3, true, true])
        );
        let calls = nested(&events);
        assert_eq!(calls.len(), 3);
        assert!(
            calls
                .iter()
                .all(|(body, error, metadata)| body["ok"] == true
                    && !error
                    && metadata["provider_output_limit"] == true)
        );
        assert_eq!(calls[2].0["value"], Value::Null);
        let requests = model.requests();
        assert_eq!(requests.len(), 5);
        assert!(!requests[0].settings.tools.is_empty());
        assert!(
            requests[0]
                .settings
                .tools
                .iter()
                .all(|tool| tool.name != "infer")
        );
        let infer = infer_requests(&model);
        assert_eq!(infer.len(), 3);
        for request in &infer {
            assert!(request.settings.tools.is_empty());
            assert_eq!(request.settings.model, "test-host");
            assert_eq!(request.settings.reasoning, Some(ReasoningEffort::High));
            assert_eq!(request.settings.max_output_tokens, Some(2048));
            assert!(!single_user_text(request).contains("parent secret"));
        }
        assert!(
            single_user_text(&infer[0])
                .contains("Task:\nplain\n\nContext (JSON):\n{\"id\":7}")
        );
        assert!(
            single_user_text(&infer[1])
                .contains("Task:\ntyped\n\nContext (JSON):\nnull")
        );
        assert!(
            single_user_text(&infer[2])
                .contains("Task:\nnull answer\n\nContext (JSON):\n\"solo\"")
        );
        assert!(infer[1].settings.text_format.is_some());
        model.assert_exhausted();
    });
}

#[test]
fn invalid_arguments_do_not_spend_the_one_allowed_attempt() {
    block_on(async {
        let script = r#"
local accepted = pcall(tools.infer, { task = "bad", context = json.null, extra = true })
local bad = tools.infer({ task = " ", context = json.null })
local good = tools.infer({ task = "good", context = json.null })
local full = tools.infer({ task = "full", context = json.null })
return { accepted, bad.ok, good.value, full.ok }
"#;
        let model = ScriptedModel::new()
            .turn(outer_call(script))
            .turn(|turn| turn.text("accepted"))
            .turn(|turn| turn.text("done"));
        let events = Events::default();
        let store = Store::memory().await.unwrap();
        let agent = Agent::new(model.clone())
            .plugin(Codemode::new(None).with_inference_limits(Limits {
                max_calls: 1,
                ..Limits::default()
            }))
            .plugin(events.clone());
        let outcome = agent.run("go", &store).await.unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&result_text(
                &results(&store, &outcome.run.0).await[0]
            ))
            .unwrap(),
            json!([false, false, "accepted", false])
        );
        let calls = nested(&events);
        assert_eq!(calls.len(), 4);
        assert_eq!(calls[0].0, Value::Null);
        assert!(calls[0].1);
        assert!(
            calls[1].0["error"]
                .as_str()
                .unwrap()
                .contains("nonwhitespace")
        );
        assert_eq!(calls[2].0["value"], "accepted");
        assert!(calls[3].0["error"].as_str().unwrap().contains("call limit"));
        assert_eq!(infer_requests(&model).len(), 1);
        model.assert_exhausted();
    });
}

#[test]
fn bad_answers_and_provider_failures_are_readable_nested_errors() {
    block_on(async {
        let script = r#"
local failures = {}
for _, task in { "schema", "provider", "length", "tool", "integer" } do
    local result = tools.infer({ task = task, context = json.null, schema = { type = "integer" } })
    table.insert(failures, not result.ok and result.value == json.null and type(result.error) == "string")
end
local good = tools.infer({ task = "good", context = json.null })
return { failures, good.ok, good.value }
"#;
        let model = ScriptedModel::new()
            .turn(outer_call(script))
            .turn(|turn| turn.text("\"wrong\""))
            .turn(|turn| {
                turn.error("context_length_exceeded", "provider rejected")
            })
            .turn(|turn| turn.text("1").stop(StopReason::Length))
            .turn(|turn| turn.tool_call("unavailable", json!({})))
            .turn(|turn| turn.text("9007199254740993"))
            .turn(|turn| turn.text("recovered"))
            .turn(|turn| turn.text("done"));
        let events = Events::default();
        let store = Store::memory().await.unwrap();
        let agent = Agent::new(model.clone())
            .plugin(Codemode::new(None))
            .plugin(events.clone());
        let outcome = agent.run("go", &store).await.unwrap();
        let result = results(&store, &outcome.run.0).await;
        assert!(!result[0].is_error, "surrounding script succeeds");
        assert_eq!(
            serde_json::from_str::<Value>(&result_text(&result[0])).unwrap(),
            json!([[true, true, true, true, true], true, "recovered"])
        );
        let calls = nested(&events);
        assert_eq!(calls.len(), 6);
        for (body, is_error, _) in &calls[..5] {
            assert!(*is_error);
            assert_eq!(body["ok"], false);
            assert_eq!(body["value"], Value::Null);
            assert!(
                body["error"].as_str().is_some_and(|text| !text.is_empty())
            );
        }
        assert!(calls[0].0["error"].as_str().unwrap().contains("schema"));
        assert!(
            calls[1].0["error"]
                .as_str()
                .unwrap()
                .contains("provider rejected")
        );
        assert!(calls[2].0["error"].as_str().unwrap().contains("truncated"));
        assert!(calls[3].0["error"].as_str().unwrap().contains("tool"));
        assert!(
            calls[4].0["error"]
                .as_str()
                .unwrap()
                .contains("exact range")
        );
        assert_eq!(calls[5].0["value"], "recovered");
        let rows = events
            .seen
            .lock()
            .unwrap()
            .iter()
            .find_map(|event| match event {
                RunEvent::ToolEnd {
                    parent: None,
                    output,
                    ..
                } => output
                    .details
                    .as_ref()
                    .map(|details| details["calls"].clone()),
                _ => None,
            })
            .expect("codemode rows");
        assert_eq!(rows.as_array().unwrap().len(), 6);
        assert!(
            rows.as_array().unwrap()[..5]
                .iter()
                .all(|row| row["status"] == "error")
        );
        assert_eq!(rows[5]["status"], "ok");
        model.assert_exhausted();
    });
}

#[test]
fn attempts_and_failed_usage_are_shared_across_scripts() {
    block_on(async {
        let first =
            "return tools.infer({ task = 'retry', context = json.null }).value";
        let second = "local x = tools.infer({ task = 'after budget', context = json.null }); return { x.ok, x.error }";
        let model = ScriptedModel::new()
            .turn(outer_call(first))
            .turn(|turn| {
                turn.error("rate_limit_exceeded", "try again")
                    .usage(7, 3)
                    .cost(0.2)
            })
            .turn(|turn| turn.text("won").usage(11, 5).cost(0.3))
            .turn(outer_call(second))
            .turn(|turn| turn.text("done"));
        let events = Events::default();
        let store = Store::memory().await.unwrap();
        let agent = Agent::new(model.clone())
            .plugin(Codemode::new(None).with_inference_limits(Limits {
                max_calls: 2,
                ..Limits::default()
            }))
            .plugin(events.clone());
        let outcome = agent.run("go", &store).await.unwrap();
        let result = results(&store, &outcome.run.0).await;
        assert_eq!(result.len(), 2);
        assert_eq!(result_text(&result[0]), "won");
        assert!(
            result_text(&result[1]).contains("inference call limit reached")
        );
        let calls = nested(&events);
        assert_eq!(calls.len(), 2);
        // The simulator reports uncached input separately: the retry's
        // unchanged user transcript is cached (documented chars/4 estimate).
        let requests = infer_requests(&model);
        let cached = (format!("user:{}", single_user_text(&requests[0]))
            .chars()
            .count() as u64)
            .div_ceil(4);
        assert_eq!(
            calls[0].0["usage"]["input"],
            7 + 11_u64.saturating_sub(cached)
        );
        assert_eq!(calls[0].0["usage"]["cacheRead"], cached);
        assert_eq!(calls[0].0["usage"]["cacheWrite"], 7);
        assert_eq!(calls[0].0["usage"]["output"], 8);
        assert_eq!(calls[0].0["usage"]["cost"]["total"], 0.5);
        assert_eq!(calls[1].0["usage"]["input"], 0);
        assert!(calls[1].1);
        assert_eq!(
            infer_requests(&model).len(),
            2,
            "rejected call opens no session"
        );
        model.assert_exhausted();
    });
}

#[test]
fn queued_inference_reaches_deadline_without_another_provider_request() {
    block_on(async {
        let script = r#"
local a, b = parallel(
    function() return tools.infer({ task = "one", context = json.null }) end,
    function() return tools.infer({ task = "two", context = json.null }) end
)
return { a.ok, b.ok }
"#;
        let model = ScriptedModel::new()
            .turn(outer_call(script))
            .turn(|turn| turn.text("too late").delay(Duration::from_secs(10)))
            .turn(|turn| turn.text("done"));
        let events = Events::default();
        let store = Store::memory().await.unwrap();
        let agent = Agent::new(model.clone())
            .plugin(Codemode::new(None).with_inference_limits(Limits {
                max_concurrency: 1,
                timeout: Duration::from_secs(1),
                ..Limits::default()
            }))
            .plugin(events.clone());
        let outcome = agent.run("go", &store).await.unwrap();
        assert!(!results(&store, &outcome.run.0).await[0].is_error);
        let calls = nested(&events);
        assert_eq!(calls.len(), 2);
        assert!(calls.iter().all(|(body, is_error, _)| *is_error
            && body["error"].as_str().unwrap().contains("deadline")));
        assert_eq!(
            infer_requests(&model).len(),
            1,
            "waiter never reaches provider"
        );
        model.assert_exhausted();
    });
}

#[test]
fn script_timeout_drops_stream_and_frees_slot_for_next_script() {
    block_on(async {
        let timed = "-- @options: {\"timeout_ms\": 50}\nreturn tools.infer({ task = 'slow', context = json.null })";
        let next =
            "return tools.infer({ task = 'next', context = json.null }).value";
        let model = ScriptedModel::new()
            .turn(outer_call(timed))
            .turn(|turn| turn.text("too late").delay(Duration::from_secs(10)))
            .turn(outer_call(next))
            .turn(|turn| turn.text("admitted"))
            .turn(|turn| turn.text("done"));
        let store = Store::memory().await.unwrap();
        let agent = Agent::new(model.clone()).plugin(
            Codemode::new(None).with_inference_limits(Limits {
                max_concurrency: 1,
                timeout: Duration::from_secs(30),
                ..Limits::default()
            }),
        );
        let outcome = agent.run("go", &store).await.unwrap();
        let result = results(&store, &outcome.run.0).await;
        assert_eq!(result.len(), 2);
        assert!(result[0].is_error);
        assert!(!result[1].is_error);
        assert_eq!(result_text(&result[1]), "admitted");
        assert_eq!(infer_requests(&model).len(), 2);
        model.assert_exhausted();
    });
}

#[derive(Clone)]
struct NoOutputLimit(ScriptedModel);

impl Llm for NoOutputLimit {
    fn supports_output_token_limit(&self) -> bool {
        false
    }
    fn open(
        &self,
        settings: Settings,
    ) -> BoxFuture<'static, Result<Box<dyn LlmSession>, LlmError>> {
        self.0.open(settings)
    }
}

#[test]
fn override_model_and_unsupported_output_capability_are_recorded() {
    block_on(async {
        let script = "return tools.infer({ task = 'override', context = json.null }).value";
        let scripted = ScriptedModel::new()
            .turn(outer_call(script))
            .turn(|turn| turn.text("ok"))
            .turn(|turn| turn.text("done"));
        let events = Events::default();
        let store = Store::memory().await.unwrap();
        let agent = Agent::new(NoOutputLimit(scripted.clone()))
            .model("host-model")
            .plugin(Codemode::new(None).with_inference_model("custom-model"))
            .plugin(events.clone());
        let outcome = agent.run("go", &store).await.unwrap();
        assert_eq!(
            result_text(&results(&store, &outcome.run.0).await[0]),
            "ok"
        );
        let request = &infer_requests(&scripted)[0];
        assert_eq!(request.settings.model, "custom-model");
        assert_eq!(request.settings.reasoning, None);
        assert_eq!(request.settings.max_output_tokens, None);
        assert_eq!(nested(&events)[0].2["provider_output_limit"], false);
        scripted.assert_exhausted();
    });
}

#[test]
fn ordinary_plugin_block_prevents_infer_admission() {
    block_on(async {
        let script =
            "return tools.infer({ task = 'blocked', context = json.null })";
        let model = ScriptedModel::new()
            .turn(outer_call(script))
            .turn(|turn| turn.text("done"));
        let events = Events {
            block_infer: true,
            ..Events::default()
        };
        let store = Store::memory().await.unwrap();
        let agent = Agent::new(model.clone())
            .plugin(Codemode::new(None))
            .plugin(events.clone());
        let outcome = agent.run("go", &store).await.unwrap();
        let result = results(&store, &outcome.run.0).await;
        assert!(result[0].is_error);
        assert!(
            result_text(&result[0]).contains("infer blocked by test plugin")
        );
        assert!(infer_requests(&model).is_empty());
        assert!(!events.seen.lock().unwrap().iter().any(|event| matches!(event, RunEvent::ToolEnd { parent: Some(_), output, .. } if output.structured.is_some())));
        assert!(store.plugin_costs(&outcome.run.0).await.unwrap().is_empty());
        model.assert_exhausted();
    });
}

struct ReservedInfer;

#[async_trait]
impl AgentTool for ReservedInfer {
    fn name(&self) -> &str {
        "infer"
    }
    fn description(&self) -> &str {
        "reserved name"
    }
    fn parameters(&self) -> &Value {
        static SCHEMA: std::sync::LazyLock<Value> =
            std::sync::LazyLock::new(|| json!({"type":"object"}));
        &SCHEMA
    }
    async fn call(
        &self,
        _args: Value,
        _ctx: ToolCtx,
    ) -> Result<ToolOutput, tau_agent::error::ToolError> {
        Ok(ToolOutput::text("unused"))
    }
}

#[test]
fn reserved_infer_name_rejects_run_before_model_request() {
    block_on(async {
        let model = ScriptedModel::new();
        let store = Store::memory().await.unwrap();
        let agent = Agent::new(model.clone())
            .tool(ReservedInfer)
            .plugin(Codemode::new(None));
        let error = agent
            .run("go", &store)
            .await
            .expect_err("run start must fail")
            .to_string();
        assert!(error.contains("already exists"), "{error}");
        assert!(model.requests().is_empty());
    });
}
