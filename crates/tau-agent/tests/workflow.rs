//! Workflow primitives (`docs/reference/api.md`): workflow grouping,
//! typed results, forks and sub-agents, driven through `Agent` with
//! `ScriptedModel`, `Store::memory()` and paused time.

use async_trait::async_trait;
use hegel::{TestCase, generators as gs};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tau_agent::{
    agent::{Agent, AgentError, Input},
    event::StopReason,
    schema::to_strict,
    tool::{ToolCtx, ToolOutput, TypedTool, typed},
};
use tau_store::{Status, Store};
use tau_testing::{block_on, scripted::ScriptedModel};

/// Runs started with a workflow id are grouped under it: the store
/// records the id, and the workflow's cost adds up the runs of each
/// agent. A run started without one belongs to no workflow.
#[test]
fn runs_are_grouped_by_workflow() {
    let llm = ScriptedModel::new()
        .turn(|t| t.text("a").cost(0.5))
        .turn(|t| t.text("b").cost(0.25))
        .turn(|t| t.text("c").cost(0.125));
    block_on(async {
        let store = Store::memory().await.unwrap();
        let scanner = Agent::new(llm.clone()).name("scanner");
        let writer = Agent::new(llm).name("writer");

        let first = scanner
            .run(Input::new("scan").workflow("release"), &store)
            .await
            .unwrap();
        let second = writer
            .run(Input::new("write").workflow("release"), &store)
            .await
            .unwrap();
        let loose = writer.run("write", &store).await.unwrap();

        for outcome in [&first, &second] {
            let record = store.run(&outcome.run.0).await.unwrap().unwrap();
            assert_eq!(record.workflow_id.as_deref(), Some("release"));
        }
        let record = store.run(&loose.run.0).await.unwrap().unwrap();
        assert_eq!(record.workflow_id, None);

        let cost = store.workflow_cost("release").await.unwrap();
        let agents: Vec<(&str, i64, f64)> = cost
            .iter()
            .map(|c| (c.agent.as_str(), c.runs, c.usd))
            .collect();
        assert_eq!(agents, vec![("scanner", 1, 0.5), ("writer", 1, 0.25),]);
    });
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
struct Changes {
    features: Vec<String>,
    fixes: Vec<String>,
    breaking: Option<Breaking>,
    approved: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
struct Breaking {
    summary: String,
    severity: Option<u8>,
}

#[hegel::composite]
fn changes(tc: TestCase) -> Changes {
    let text = || gs::text().max_size(20);
    let breaking = if tc.draw(gs::booleans()) {
        Some(Breaking {
            summary: tc.draw(text()),
            severity: tc.draw(gs::optional(gs::integers::<u8>())),
        })
    } else {
        None
    };
    Changes {
        features: tc.draw(gs::vecs(text()).max_size(4)),
        fixes: tc.draw(gs::vecs(text()).max_size(4)),
        breaking,
        approved: tc.draw(gs::booleans()),
    }
}

/// Round trip through a typed run: whatever value the model's final
/// message serializes, `run_typed` gives back, and `Typed::json` gives
/// back that message. Every request carries the strict schema of the
/// type as `text.format`, and the message is valid under it, so a model
/// constrained to that schema can say every value of the type (optional
/// fields as `null` included).
#[hegel::test(test_cases = 60)]
fn typed_run_round_trips_the_final_message(tc: TestCase) {
    let value = tc.draw(changes());
    let message = serde_json::to_string(&value).unwrap();
    let llm = ScriptedModel::new().turn(|t| t.text(message.clone()));
    block_on(async {
        let store = Store::memory().await.unwrap();
        let typed = Agent::new(llm.clone())
            .run_typed::<Changes>("scan", &store)
            .await
            .unwrap();
        assert_eq!(typed.value, value);
        assert_eq!(typed.json(), message);
        assert_eq!(typed.outcome.stop, StopReason::Stop);
        assert_eq!(typed.outcome.text, message);

        let schema =
            serde_json::to_value(schemars::schema_for!(Changes)).unwrap();
        let strict = to_strict(&schema).unwrap();
        let format = llm.requests()[0].settings.text_format.clone();
        assert_eq!(
            format,
            Some(json!({
                "type": "json_schema",
                "name": "Changes",
                "schema": strict,
                "strict": true,
            }))
        );
        let validator = jsonschema::validator_for(&strict).unwrap();
        let sent: Value = serde_json::from_str(&message).unwrap();
        assert!(validator.is_valid(&sent), "{message} under {strict}");
    });
}

#[derive(Deserialize, JsonSchema)]
struct LookupArgs {
    topic: String,
}

struct Lookup;

#[async_trait]
impl TypedTool for Lookup {
    type Args = LookupArgs;
    const NAME: &'static str = "lookup";
    const DESCRIPTION: &'static str = "Looks a topic up.";
    async fn call(
        &self,
        args: LookupArgs,
        _ctx: ToolCtx,
    ) -> anyhow::Result<ToolOutput> {
        Ok(ToolOutput::text(format!("notes on {}", args.topic)))
    }
}

/// Tools stay available in a typed run: the model may call them before
/// its final message, and every request carries both the tools and the
/// output format.
#[test]
fn tools_stay_available_in_a_typed_run() {
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("lookup", json!({"topic": "ws"})))
        .turn(|t| {
            t.text(r#"{"features":["ws"],"fixes":[],"breaking":null,"approved":true}"#)
        });
    block_on(async {
        let store = Store::memory().await.unwrap();
        let typed = Agent::new(llm.clone())
            .tool(typed(Lookup))
            .run_typed::<Changes>("scan", &store)
            .await
            .unwrap();
        assert_eq!(typed.value.features, vec!["ws".to_owned()]);
        let requests = llm.requests();
        assert_eq!(requests.len(), 2);
        for request in requests {
            assert_eq!(request.settings.tools[0].name, "lookup");
            assert!(request.settings.text_format.is_some());
        }
    });
}

/// A final message that is not a value of the type fails the typed run,
/// and the error keeps the outcome. The run itself finished normally and
/// is stored as done, with the message as its result.
#[test]
fn an_invalid_final_message_keeps_the_outcome() {
    let llm = ScriptedModel::new().turn(|t| t.text("not json"));
    block_on(async {
        let store = Store::memory().await.unwrap();
        let error = Agent::new(llm)
            .run_typed::<Changes>("scan", &store)
            .await
            .unwrap_err();
        let AgentError::Output { outcome, .. } = &error else {
            panic!("expected an output error, got {error:?}");
        };
        assert_eq!(outcome.text, "not json");
        assert_eq!(outcome.stop, StopReason::Stop);
        assert!(error.to_string().contains("not a valid output"), "{error}");
        let record = store.run(&outcome.run.0).await.unwrap().unwrap();
        assert_eq!(record.status, Status::Done);
        assert_eq!(record.result.as_deref(), Some("not json"));
    });
}

/// An output type whose schema is not an object has no strict form; the
/// typed run fails before it asks the model anything.
#[test]
fn a_non_object_output_type_is_rejected_up_front() {
    let llm = ScriptedModel::new().turn(|t| t.text("[]"));
    block_on(async {
        let store = Store::memory().await.unwrap();
        let error = Agent::new(llm.clone())
            .run_typed::<Vec<String>>("scan", &store)
            .await
            .unwrap_err();
        assert!(matches!(error, AgentError::OutputSchema(_)), "{error:?}");
        assert!(error.to_string().contains("no strict schema"), "{error}");
        assert!(llm.requests().is_empty());
    });
}

#[derive(Debug, Deserialize, JsonSchema)]
struct Page<T> {
    items: Vec<T>,
}

/// The format's name is built from the type's schema name, with every
/// character OpenAI does not allow replaced, so generic types work too.
#[test]
fn generic_output_types_get_a_valid_format_name() {
    let llm = ScriptedModel::new().turn(|t| t.text(r#"{"items":[true]}"#));
    block_on(async {
        let store = Store::memory().await.unwrap();
        let typed = Agent::new(llm.clone())
            .run_typed::<Page<bool>>("list", &store)
            .await
            .unwrap();
        assert_eq!(typed.value.items, vec![true]);
        let format = llm.requests()[0].settings.text_format.clone().unwrap();
        let name = format["name"].as_str().unwrap();
        assert!(
            !name.is_empty()
                && name.len() <= 64
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_-".contains(c)),
            "{name}"
        );
        assert!(name.starts_with("Page"), "{name}");
    });
}
