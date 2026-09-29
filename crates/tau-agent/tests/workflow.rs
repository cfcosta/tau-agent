//! Workflow primitives (`docs/reference/api.md`): workflow grouping,
//! typed results, forks and sub-agents, driven through `Agent` with
//! `ScriptedModel`, `Store::memory()` and paused time.

use std::{collections::HashMap, time::Duration};

use async_trait::async_trait;
use futures_util::StreamExt;
use hegel::{TestCase, generators as gs};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tau_agent::{
    agent::{Agent, AgentError, Input},
    event::{LimitKind, RunEvent, StopReason},
    limits::Limits,
    schema::to_strict,
    tool::{
        AgentTool,
        RunId,
        ToolCtx,
        ToolOutput,
        ToolUpdates,
        TypedTool,
        typed,
    },
};
use tau_ai::message::{AssistantBlock, Message, UserContent};
use tau_store::{RunKind, Status, Store};
use tau_testing::{block_on, scripted::ScriptedModel};

mod common;
use common::{assert_grammar, stored};

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

/// Echoes its `text` argument back.
struct Echo;

#[derive(Deserialize, JsonSchema)]
struct EchoArgs {
    text: String,
}

#[async_trait]
impl TypedTool for Echo {
    type Args = EchoArgs;
    const NAME: &'static str = "echo";
    const DESCRIPTION: &'static str = "Echoes text.";
    async fn call(
        &self,
        args: EchoArgs,
        _ctx: ToolCtx,
    ) -> anyhow::Result<ToolOutput> {
        Ok(ToolOutput::text(args.text))
    }
}

/// A message reduced to its role and what it says, so the model below
/// can be written without timestamps, ids or usage.
fn project(message: &Message) -> (String, String) {
    let text = match message {
        Message::User(user) => match &user.content {
            UserContent::Text(text) => text.clone(),
            UserContent::Blocks(_) => panic!("text only"),
        },
        Message::Assistant(assistant) => assistant
            .content
            .iter()
            .map(|block| match block {
                AssistantBlock::Text(text) => text.text.clone(),
                AssistantBlock::ToolCall(call) => {
                    format!("call {}", call.arguments["text"])
                }
                AssistantBlock::Thinking(_) => String::new(),
            })
            .collect(),
        Message::ToolResult(result) => format!("{:?}", result.content),
    };
    (message.role().to_owned(), text)
}

/// Fork trees, against a model. A root run, then forks, each from the
/// end of a drawn earlier run (the root or a fork, so forks of forks
/// too); each run makes a drawn number of tool calls before answering.
/// The model says a run's transcript is the transcript of the run it
/// forked from followed by its own messages. The store, the requests the
/// model received, and the fork records agree with it, and no run's
/// transcript changes when it is forked.
#[hegel::test(test_cases = 40)]
fn forks_continue_from_their_checkpoint(tc: TestCase) {
    let plan: Vec<(usize, u8)> = {
        let root_calls = tc.draw(gs::integers::<u8>().max_value(2));
        let forks = tc.draw(gs::integers::<usize>().max_value(4));
        let mut plan = vec![(0, root_calls)];
        for i in 0..forks {
            let from = tc.draw(gs::integers::<usize>().max_value(i));
            plan.push((from, tc.draw(gs::integers::<u8>().max_value(2))));
        }
        plan
    };
    let mut llm = ScriptedModel::new();
    for (i, &(_, calls)) in plan.iter().enumerate() {
        for c in 0..calls {
            llm = llm.turn(|t| {
                t.tool_call("echo", json!({"text": format!("{i}.{c}")}))
            });
        }
        llm = llm.turn(|t| t.text(format!("answer {i}")));
    }
    block_on(async {
        let store = Store::memory().await.unwrap();
        let agent = Agent::new(llm.clone()).tool(typed(Echo));
        // Per run: its outcome and the model's transcript for it.
        let mut runs: Vec<(tau_agent::agent::Outcome, Vec<(String, String)>)> =
            Vec::new();
        for (i, &(from, calls)) in plan.iter().enumerate() {
            let input = format!("input {i}");
            let before = llm.requests().len();
            let (outcome, mut expected) = if i == 0 {
                (agent.run(input.clone(), &store).await.unwrap(), Vec::new())
            } else {
                let (parent, parent_transcript) = &runs[from];
                let outcome = agent
                    .fork(&parent.checkpoint())
                    .run(input.clone(), &store)
                    .await
                    .unwrap();
                (outcome, parent_transcript.clone())
            };
            let inherited = expected.len();
            expected.push(("user".to_owned(), input));
            for c in 0..calls {
                let text = format!("{i}.{c}");
                expected
                    .push(("assistant".to_owned(), format!("call \"{text}\"")));
                expected.push((
                    "toolResult".to_owned(),
                    format!("{:?}", ToolOutput::text(text).content),
                ));
            }
            expected.push(("assistant".to_owned(), format!("answer {i}")));

            let transcript = stored(&store, &outcome.run.0).await;
            let projected: Vec<_> = transcript.iter().map(project).collect();
            assert_eq!(projected, expected, "run {i}");
            // The model saw everything but the answer, in its first
            // request the inherited transcript and the input.
            let first = &llm.requests()[before].transcript;
            assert_eq!(first[..], transcript[..inherited + 1], "run {i}");

            let own = (expected.len() - inherited) as i64;
            assert_eq!(outcome.checkpoint().seq(), own - 1);
            assert_eq!(outcome.checkpoint().run(), &outcome.run);
            let record = store.run(&outcome.run.0).await.unwrap().unwrap();
            if i > 0 {
                let parent = runs[from].0.checkpoint();
                assert_eq!(
                    record.kind,
                    RunKind::Fork {
                        parent: parent.run().0.to_string(),
                        fork_seq: parent.seq(),
                    }
                );
            }
            runs.push((outcome, expected));
        }
        for (i, (outcome, expected)) in runs.iter().enumerate() {
            let transcript = stored(&store, &outcome.run.0).await;
            let projected: Vec<_> = transcript.iter().map(project).collect();
            assert_eq!(&projected, expected, "run {i} after forking");
        }
        llm.assert_exhausted();
    });
}

/// A fork joins the workflow of the run it forks from, unless its input
/// names one; a typed fork parses its final message like any typed run.
#[test]
fn forks_join_their_parents_workflow() {
    let llm = ScriptedModel::new()
        .turn(|t| t.text("base"))
        .turn(|t| t.text("same"))
        .turn(|t| t.text(r#"{"items":[1]}"#));
    block_on(async {
        let store = Store::memory().await.unwrap();
        let agent = Agent::new(llm);
        let base = agent
            .run(Input::new("go").workflow("w1"), &store)
            .await
            .unwrap();
        let same = agent
            .fork(&base.checkpoint())
            .run("on", &store)
            .await
            .unwrap();
        let other = agent
            .fork(&base.checkpoint())
            .run_typed::<Page<u8>>(Input::new("on").workflow("w2"), &store)
            .await
            .unwrap();
        assert_eq!(other.value.items, vec![1]);
        let workflow = |run: &str| {
            let store = store.clone();
            let run = run.to_owned();
            async move { store.run(&run).await.unwrap().unwrap().workflow_id }
        };
        assert_eq!(workflow(&same.run.0).await.as_deref(), Some("w1"));
        assert_eq!(workflow(&other.outcome.run.0).await.as_deref(), Some("w2"));
    });
}

fn run_of(event: &RunEvent) -> &RunId {
    event.run()
}

/// A supervisor with sub-agents, over drawn scripts: some turns, each
/// calling the sub-agent a drawn number of times in parallel, then an
/// answer. Every call starts a sub-agent run that the store records
/// under the supervisor and its workflow; its answer is the call's
/// result. The supervisor's usage includes its children's, and equals
/// the workflow's cost. One subscriber sees every run: each run's events
/// follow the grammar, the children's carry the supervisor as parent,
/// and each child runs inside one of the supervisor's tool batches.
#[hegel::test(test_cases = 40)]
fn supervisor_with_sub_agents(tc: TestCase) {
    let batches: Vec<usize> = tc.draw(
        gs::vecs(gs::integers::<usize>().min_value(1).max_value(3)).max_size(3),
    );
    let children: usize = batches.iter().sum();
    let mut lead_llm = ScriptedModel::new();
    for &calls in &batches {
        lead_llm = lead_llm.turn(|mut t| {
            for c in 0..calls {
                t = t.tool_call("research", json!({"input": format!("q{c}")}));
            }
            t.cost(0.5)
        });
    }
    lead_llm = lead_llm.turn(|t| t.text("done").cost(0.5));
    let mut child_llm = ScriptedModel::new();
    for _ in 0..children {
        child_llm = child_llm.turn(|t| t.text("found").cost(0.25));
    }
    block_on(async {
        let store = Store::memory().await.unwrap();
        let researcher = Agent::new(child_llm.clone()).name("researcher");
        let lead = Agent::new(lead_llm.clone())
            .name("lead")
            .tool(researcher.as_tool("research", "Investigates."));
        let mut run = lead.start(Input::new("task").workflow("wf"), &store);
        let lead_id = run.id();
        let events: Vec<RunEvent> = run.events().collect().await;
        let outcome = run.outcome().await.unwrap();

        assert_eq!(outcome.stop, StopReason::Stop);
        assert_eq!(outcome.text, "done");
        let expected =
            0.5 * (batches.len() + 1) as f64 + 0.25 * children as f64;
        assert_eq!(outcome.usage.cost.total, expected);
        let cost = store.workflow_cost("wf").await.unwrap();
        assert_eq!(cost.iter().map(|c| c.usd).sum::<f64>(), expected);
        assert_eq!(
            cost.iter()
                .map(|c| (c.agent.as_str(), c.runs))
                .collect::<Vec<_>>(),
            if children == 0 {
                vec![("lead", 1)]
            } else {
                vec![("lead", 1), ("researcher", children as i64)]
            }
        );

        // Every call's result is its sub-agent's answer.
        let transcript = stored(&store, &lead_id.0).await;
        let results: Vec<_> = transcript
            .iter()
            .filter_map(|m| match m {
                Message::ToolResult(r) => {
                    Some((r.is_error, format!("{:?}", r.content)))
                }
                _ => None,
            })
            .collect();
        let found = format!("{:?}", ToolOutput::text("found").content);
        assert_eq!(results, vec![(false, found); children]);
        // Each result names the sub-agent run that produced it.
        let mut named: Vec<String> = transcript
            .iter()
            .filter_map(|m| match m {
                Message::ToolResult(r) => {
                    Some(r.details.as_ref()?["run"].as_str()?.to_owned())
                }
                _ => None,
            })
            .collect();
        named.sort();

        // One subscriber, every run.
        let mut by_run: HashMap<RunId, Vec<(usize, RunEvent)>> = HashMap::new();
        for (index, event) in events.iter().enumerate() {
            by_run
                .entry(run_of(event).clone())
                .or_default()
                .push((index, event.clone()));
        }
        assert_eq!(by_run.len(), children + 1);
        let mut child_runs: Vec<String> = by_run
            .keys()
            .filter(|id| **id != lead_id)
            .map(|id| id.0.to_string())
            .collect();
        child_runs.sort();
        assert_eq!(named, child_runs);
        let lead_events = &by_run[&lead_id];
        for (id, run_events) in &by_run {
            let plain: Vec<RunEvent> =
                run_events.iter().map(|(_, e)| e.clone()).collect();
            assert_grammar(&plain);
            if id == &lead_id {
                continue;
            }
            let RunEvent::RunStart { parent, agent, .. } = &plain[0] else {
                unreachable!()
            };
            assert_eq!(parent.as_ref(), Some(&lead_id));
            assert_eq!(agent.as_ref(), "researcher");
            let record = store.run(&id.0).await.unwrap().unwrap();
            assert_eq!(
                record.kind,
                RunKind::Subagent {
                    parent: lead_id.0.to_string()
                }
            );
            assert_eq!(record.workflow_id.as_deref(), Some("wf"));
            // Inside one lead turn, after one of its tool calls started.
            let (first, last) = (run_events[0].0, run_events.last().unwrap().0);
            let before = lead_events.iter().filter(|(i, _)| *i < first);
            let turn_started =
                before.clone().rev().find_map(|(_, e)| match e {
                    RunEvent::TurnStart { turn, .. } => Some(*turn),
                    _ => None,
                });
            let turn_ended = lead_events
                .iter()
                .filter(|(i, _)| *i > last)
                .find_map(|(_, e)| match e {
                    RunEvent::TurnEnd { turn, .. } => Some(*turn),
                    _ => None,
                });
            assert!(turn_started.is_some());
            assert_eq!(turn_started, turn_ended, "child {id} spans lead turns");
            let mut before = before;
            let last_lead = before.next_back().map(|(_, e)| e.clone());
            assert!(
                matches!(
                    last_lead,
                    Some(RunEvent::ToolStart { .. } | RunEvent::ToolEnd { .. })
                ),
                "child {id} started outside a tool batch: {last_lead:?}"
            );
        }
        lead_llm.assert_exhausted();
        child_llm.assert_exhausted();
    });
}

/// Sub-agent usage counts toward the caller's limits: the caller's own
/// turns cost nothing, but after two calls its children cost 1.2 USD,
/// over its 1 USD limit, so it stops before its third turn.
#[test]
fn sub_agent_usage_counts_toward_limits() {
    let lead_llm = ScriptedModel::new()
        .turn(|t| t.tool_call("research", json!({"input": "a"})))
        .turn(|t| t.tool_call("research", json!({"input": "b"})))
        .turn(|t| t.text("never"));
    let child_llm = ScriptedModel::new()
        .turn(|t| t.text("found").cost(0.6))
        .turn(|t| t.text("found").cost(0.6));
    block_on(async {
        let store = Store::memory().await.unwrap();
        let outcome = Agent::new(lead_llm.clone())
            .tool(Agent::new(child_llm).as_tool("research", "Investigates."))
            .limits(Limits::default().max_usd(1.0))
            .run("task", &store)
            .await
            .unwrap();
        assert_eq!(outcome.stop, StopReason::Limit(LimitKind::Usd));
        assert_eq!(lead_llm.requests().len(), 2);
        assert_eq!(outcome.usage.cost.total, 1.2);
        // The store keeps each run's own cost.
        let record = store.run(&outcome.run.0).await.unwrap().unwrap();
        assert_eq!(record.cost_usd, 0.0);
    });
}

/// Cancelling the caller cancels the sub-agent run it is waiting on:
/// both end cancelled, and the call gets an error result.
#[test]
fn cancelling_the_caller_cancels_the_sub_agent() {
    let lead_llm = ScriptedModel::new()
        .turn(|t| t.tool_call("research", json!({"input": "a"})))
        .turn(|t| t.text("never"));
    let child_llm = ScriptedModel::new()
        .turn(|t| t.text("late").delay(Duration::from_secs(3600)));
    block_on(async {
        let store = Store::memory().await.unwrap();
        let mut run = Agent::new(lead_llm.clone())
            .tool(Agent::new(child_llm).as_tool("research", "Investigates."))
            .start("task", &store);
        let lead_id = run.id();
        let child_id = {
            let mut events = run.events();
            loop {
                match events.next().await {
                    Some(RunEvent::TurnStart { run, .. }) if run != lead_id => {
                        break run;
                    }
                    Some(_) => {}
                    None => panic!("the sub-agent never started a turn"),
                }
            }
        };
        run.cancel();
        let outcome = run.outcome().await.unwrap();
        assert_eq!(outcome.stop, StopReason::Cancelled);
        let child = store.run(&child_id.0).await.unwrap().unwrap();
        assert_eq!(child.status, Status::Cancelled);
        let transcript = stored(&store, &lead_id.0).await;
        let Some(Message::ToolResult(result)) = transcript.last() else {
            panic!("{transcript:?}")
        };
        assert!(result.is_error);
        assert_eq!(lead_llm.requests().len(), 1);
    });
}

/// A sub-agent run that fails makes the call an error result that says
/// how the run ended; the caller goes on.
#[test]
fn a_failed_sub_agent_is_an_error_result() {
    let lead_llm = ScriptedModel::new()
        .turn(|t| t.tool_call("research", json!({"input": "a"})))
        .turn(|t| t.text("recovered"));
    let child_llm =
        ScriptedModel::new().turn(|t| t.error("insufficient_quota", "boom"));
    block_on(async {
        let store = Store::memory().await.unwrap();
        let outcome = Agent::new(lead_llm.clone())
            .tool(Agent::new(child_llm).as_tool("research", "Investigates."))
            .run("task", &store)
            .await
            .unwrap();
        assert_eq!(outcome.text, "recovered");
        let transcript = stored(&store, &outcome.run.0).await;
        let Message::ToolResult(result) = &transcript[2] else {
            panic!("{transcript:?}")
        };
        assert!(result.is_error);
        let text = format!("{:?}", result.content);
        assert!(text.contains("research ended with Error"), "{text}");
        assert!(text.contains("boom"), "{text}");
    });
}

/// A sub-agent tool has one string argument, `input`, in strict form,
/// and called outside a run (no run to join) it fails.
#[test]
fn a_sub_agent_tool_outside_a_run_fails() {
    let tool =
        Agent::new(ScriptedModel::new()).as_tool("research", "Investigates.");
    assert_eq!(tool.name(), "research");
    assert_eq!(tool.description(), "Investigates.");
    assert_eq!(to_strict(tool.parameters()).unwrap(), *tool.parameters());
    assert_eq!(tool.parameters()["required"], json!(["input"]));
    assert!(format!("{tool:?}").contains("research"));
    block_on(async {
        let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
        let ctx = ToolCtx::new(
            Default::default(),
            ToolUpdates::for_tests("call_1", sender),
            RunId("run_1".into()),
        );
        let error = tool.call(json!({"input": "a"}), ctx).await.unwrap_err();
        assert!(
            error.to_string().contains("only be called from a run"),
            "{error}"
        );
    });
}

/// Limits over drawn scripts, against a model: the lead's turns call a
/// sub-agent a drawn number of times and cost drawn amounts, and so do
/// the sub-agent's. The model adds up, after each lead turn, the usage
/// the lead's and every finished child's `TurnEnd` events report. The
/// run ends with `StopReason::Limit` if and only if some turn's total
/// reaches a limit, at the first such turn, with the kind
/// `Limits::reached` names; otherwise it stops normally after its last
/// turn.
#[hegel::test(test_cases = 60)]
fn limits_end_the_run_at_the_first_turn_that_reaches_one(tc: TestCase) {
    let costs = || gs::sampled_from(vec![0.0, 0.25, 0.5]);
    let turns: Vec<(usize, f64, u64)> = tc.draw(
        gs::vecs(hegel::tuples!(
            gs::integers::<usize>().max_value(2),
            costs(),
            gs::integers::<u64>().max_value(1_000),
        ))
        .min_size(1)
        .max_size(4),
    );
    let children: usize = turns.iter().map(|(calls, _, _)| calls).sum();
    let child_costs: Vec<f64> =
        tc.draw(gs::vecs(costs()).min_size(children).max_size(children));
    let limits = Limits {
        max_turns: tc.draw(gs::optional(
            gs::integers::<u32>().min_value(1).max_value(5),
        )),
        max_tokens: tc
            .draw(gs::optional(gs::integers::<u64>().max_value(20_000))),
        max_usd: tc
            .draw(gs::optional(gs::sampled_from(vec![0.25, 0.5, 1.0, 2.0]))),
        timeout: None,
        ..Limits::default()
    };

    let mut lead_llm = ScriptedModel::new();
    for (i, &(calls, cost, tokens)) in turns.iter().enumerate() {
        let last = i + 1 == turns.len();
        lead_llm = lead_llm.turn(|mut t| {
            if last {
                t = t.text("done");
            } else {
                t = t.text("working");
                for c in 0..calls {
                    t = t.tool_call(
                        "helper",
                        json!({"input": format!("{i}.{c}")}),
                    );
                }
                if calls == 0 {
                    t = t.tool_call("echo", json!({"text": "x"}));
                }
            }
            t.cost(cost).usage(tokens, 10)
        });
    }
    let mut child_llm = ScriptedModel::new();
    for &cost in &child_costs {
        child_llm = child_llm.turn(|t| t.text("helped").cost(cost));
    }
    block_on(async {
        let store = Store::memory().await.unwrap();
        let lead = Agent::new(lead_llm)
            .tool(typed(Echo))
            .tool(Agent::new(child_llm).as_tool("helper", "Helps."))
            .limits(limits);
        let mut run = lead.start("go", &store);
        let lead_id = run.id();
        let events: Vec<RunEvent> = run.events().collect().await;
        let outcome = run.outcome().await.unwrap();

        let mut total = tau_ai::message::Usage::default();
        let mut expected = None;
        let mut lead_turns = 0;
        for event in &events {
            let RunEvent::TurnEnd { run, turn, usage } = event else {
                continue;
            };
            total.input += usage.input;
            total.output += usage.output;
            total.cache_read += usage.cache_read;
            total.cache_write += usage.cache_write;
            total.cost.total += usage.cost.total;
            if *run == lead_id {
                lead_turns = *turn;
                if expected.is_none() {
                    expected = limits
                        .reached(*turn, &total, Duration::ZERO)
                        .map(|kind| (StopReason::Limit(kind), *turn));
                }
            }
        }
        match expected {
            Some((stop, turn)) => {
                assert_eq!(outcome.stop, stop, "{limits:?}");
                assert_eq!(lead_turns, turn, "no turn after the limit");
                tc.note(&format!("stopped by {stop:?} at turn {turn}"));
            }
            None => {
                assert_eq!(outcome.stop, StopReason::Stop, "{limits:?}");
                assert_eq!(lead_turns as usize, turns.len());
            }
        }
        assert_eq!(outcome.usage.cost.total, total.cost.total);
    });
}

/// A finished chat can go on on another model: the same run and
/// conversation, the next requests on the new model, and the store
/// records it.
#[test]
fn a_run_resumes_on_another_model() {
    let llm = ScriptedModel::new()
        .turn(|t| t.text("hi"))
        .turn(|t| t.text("still here"));
    block_on(async {
        let store = Store::memory().await.unwrap();
        let first = Agent::new(llm.clone())
            .model("gpt-5.5")
            .run("hello", &store)
            .await
            .unwrap();
        let mut run = Agent::new(llm.clone())
            .model("gpt-6-sol")
            .resume(&first.run)
            .start("go on", &store);
        assert_eq!(run.id(), first.run, "the same run");
        run.events().for_each(|_| async {}).await;
        run.outcome().await.unwrap();
        let asked = llm.requests();
        assert_eq!(asked[0].settings.model, "gpt-5.5");
        assert_eq!(asked[1].settings.model, "gpt-6-sol");
        assert_eq!(
            asked[1].transcript.len(),
            3,
            "the conversation so far, then the message"
        );
        let record = store.run(&first.run.0).await.unwrap().unwrap();
        assert_eq!(record.model, "gpt-6-sol");
    });
}

/// A finished run goes on like a chat: the same run, the whole
/// conversation sent again, turns that keep counting, usage that keeps
/// adding up, and a fresh turn budget for each message.
#[test]
fn a_finished_run_resumes_like_a_chat() {
    let llm = ScriptedModel::new()
        .turn(|t| t.text("hi, what next?"))
        .turn(|t| t.tool_call("echo", json!({"text": "x"})))
        .turn(|t| t.text("done"));
    block_on(async {
        let store = Store::memory().await.unwrap();
        let agent = Agent::new(llm.clone())
            .tool(typed(Echo))
            .limits(Limits::default().max_turns(3));
        let first = agent.run("hello", &store).await.unwrap();

        let mut run = agent.resume(&first.run).start("now echo x", &store);
        assert_eq!(run.id(), first.run, "the same run");
        let turns: Vec<u32> = run
            .events()
            .filter_map(|event| async move {
                match event {
                    RunEvent::TurnStart { turn, .. } => Some(turn),
                    _ => None,
                }
            })
            .collect()
            .await;
        let second = run.outcome().await.unwrap();
        // Two more turns, numbered after the first. max_turns(3) counts
        // this message's turns alone; counted from the start, turn 3
        // would have hit it.
        assert_eq!(turns, [2, 3]);
        assert_eq!(second.stop, StopReason::Stop);
        assert_eq!(second.text, "done");

        // The model saw the conversation so far, then the new message.
        let asked = llm.requests();
        let users = |request: &tau_testing::scripted::Request| {
            request
                .transcript
                .iter()
                .filter_map(|message| match message {
                    Message::User(user) => match &user.content {
                        UserContent::Text(text) => Some(text.clone()),
                        UserContent::Blocks(_) => None,
                    },
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(users(&asked[1]), ["hello", "now echo x"]);

        let record = store.run(&first.run.0).await.unwrap().unwrap();
        assert_eq!(record.status, Status::Done);
        assert_eq!(record.turns, 3);
        assert_eq!(record.kind, RunKind::Root);
        assert_eq!(stored(&store, &first.run.0).await.len(), 6);
    });
}

/// Only a finished run can go on; one still going refuses.
#[test]
fn a_running_run_does_not_resume() {
    let llm = ScriptedModel::new()
        .turn(|t| t.delay(Duration::from_secs(30)).text("late"));
    block_on(async {
        let store = Store::memory().await.unwrap();
        let agent = Agent::new(llm);
        let mut running = agent.start("wait", &store);
        // Its first event means it is stored and running.
        running.events().next().await;
        let error = agent
            .resume(&running.id())
            .run("more", &store)
            .await
            .unwrap_err();
        assert!(
            matches!(
                error,
                AgentError::Store(tau_store::StoreError::StillRunning(_))
            ),
            "{error:?}"
        );
        running.cancel();
    });
}

/// A fork told where it forks counts its turns on from there, and so
/// does the store: resuming the fork later keeps counting.
#[test]
fn a_fork_counts_turns_from_its_fork_point() {
    let llm = ScriptedModel::new()
        .turn(|t| t.text("one"))
        .turn(|t| t.text("fork"))
        .turn(|t| t.text("more"));
    block_on(async {
        let store = Store::memory().await.unwrap();
        let agent = Agent::new(llm);
        let base = agent.run("start", &store).await.unwrap();
        let mut fork = agent
            .fork(&base.checkpoint())
            .after_turn(1)
            .start("instead", &store);
        let mut turns = Vec::new();
        while let Some(event) = fork.events().next().await {
            if let RunEvent::TurnStart { turn, .. } = event {
                turns.push(turn);
            }
        }
        let fork = fork.outcome().await.unwrap();
        assert_eq!(turns, [2]);
        let record = store.run(&fork.run.0).await.unwrap().unwrap();
        assert_eq!(record.turns, 2, "inherited and own");

        let mut more = agent.resume(&fork.run).start("more", &store);
        let mut turns = Vec::new();
        while let Some(event) = more.events().next().await {
            if let RunEvent::TurnStart { turn, .. } = event {
                turns.push(turn);
            }
        }
        more.outcome().await.unwrap();
        assert_eq!(turns, [3]);
    });
}
