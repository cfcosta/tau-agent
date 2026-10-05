//! The workflow examples of `docs/reference/api.md`, as integration
//! tests: a typed pipeline, a supervisor with sub-agents, and a fork
//! fan-out. Each is written the way the doc writes it, against
//! `ScriptedModel` and `Store::memory()`.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::time::Duration;

use futures_util::StreamExt;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tau_agent::{
    agent::{Agent, AgentError, Input},
    event::{RunEvent, StopReason},
    limits::Limits,
};
use tau_ai::message::{Message, UserContent};
use tau_store::{RunKind, Store};
use tau_testing::{block_on, scripted::ScriptedModel};

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct Changes {
    features: Vec<String>,
    fixes: Vec<String>,
    breaking: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct Review {
    approved: bool,
    problems: Vec<String>,
}

/// "Typed pipeline with parallel steps": a scanner produces typed
/// changes, a writer drafts two styles in parallel, and a reviewer
/// approves the first acceptable draft. The reviewer sees the changes as
/// JSON and each draft, and the pipeline returns the approved one.
#[test]
fn typed_pipeline_with_parallel_steps() {
    let scanner = ScriptedModel::new().turn(|t| {
        t.text(r#"{"features":["lanes"],"fixes":["reconnect"],"breaking":[]}"#)
    });
    let writer = ScriptedModel::new()
        .turn(|t| t.text("draft one"))
        .turn(|t| t.text("draft two"));
    let reviewer = ScriptedModel::new()
        .turn(|t| t.text(r#"{"approved":false,"problems":["too terse"]}"#))
        .turn(|t| t.text(r#"{"approved":true,"problems":[]}"#));
    let scanner_agent = Agent::new(scanner).name("scanner");
    let writer_agent = Agent::new(writer).name("writer");
    let reviewer_agent = Agent::new(reviewer.clone()).name("reviewer");

    let chosen = block_on(async {
        let store = Store::memory().await.unwrap();
        let pipeline = async {
            let changes = scanner_agent
                .run_typed::<Changes>(
                    Input::new("v1.4.0..v1.5.0").workflow("release"),
                    &store,
                )
                .await?;
            let (terse, detailed) = tokio::try_join!(
                writer_agent.run(
                    Input::new(format!("Terse style.\n{}", changes.json()))
                        .workflow("release"),
                    &store
                ),
                writer_agent.run(
                    Input::new(format!("Detailed style.\n{}", changes.json()))
                        .workflow("release"),
                    &store
                ),
            )?;
            for draft in [&terse, &detailed] {
                let review = reviewer_agent
                    .run_typed::<Review>(
                        Input::new(format!(
                            "{}\n---\n{}",
                            changes.json(),
                            draft.text
                        ))
                        .workflow("release"),
                        &store,
                    )
                    .await?;
                if review.value.approved {
                    assert!(review.value.problems.is_empty());
                    return Ok::<_, AgentError>(Some(draft.text.clone()));
                }
            }
            Ok(None)
        };
        let chosen = pipeline.await.unwrap();
        let cost = store.workflow_cost("release").await.unwrap();
        let runs: Vec<(&str, i64)> =
            cost.iter().map(|c| (c.agent.as_str(), c.runs)).collect();
        assert_eq!(runs, vec![("reviewer", 2), ("scanner", 1), ("writer", 2)]);
        chosen
    });

    // The second draft reviewed was approved.
    let requests = reviewer.requests();
    let reviewed = |i: usize| match &requests[i].transcript[0] {
        Message::User(user) => match &user.content {
            UserContent::Text(text) => text.clone(),
            UserContent::Blocks(_) => unreachable!(),
        },
        _ => unreachable!(),
    };
    let second = reviewed(1);
    assert!(
        second.starts_with(
            r#"{"features":["lanes"],"fixes":["reconnect"],"breaking":[]}"#
        ),
        "{second}"
    );
    assert_eq!(
        Some(second.rsplit("---\n").next().unwrap()),
        chosen.as_deref()
    );
}

/// "Supervisor with sub-agents": the lead delegates research and an
/// implementation, one subscriber follows every run, and the outcome
/// includes what the sub-agents cost.
///
/// This one runs on the real clock: it has a timeout limit, and under a
/// paused clock the runtime jumps time forward whenever it idles on a
/// store write (to sqlx's acquire timeout), which would trip it.
#[test]
fn supervisor_with_sub_agents() {
    let lead_llm = ScriptedModel::new()
        .turn(|t| {
            t.tool_call(
                "research",
                json!({"input": "How do retries work now?"}),
            )
            .cost(0.1)
        })
        .turn(|t| {
            t.tool_call("implement", json!({"input": "Add jitter to retries."}))
                .cost(0.1)
        })
        .turn(|t| t.text("Retries now use full jitter.").cost(0.1));
    let researcher_llm = ScriptedModel::new()
        .turn(|t| t.text("Fixed 2s backoff, no jitter.").cost(0.5));
    let coder_llm =
        ScriptedModel::new().turn(|t| t.text("Changed backoff.rs.").cost(1.0));
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let store = Store::memory().await.unwrap();
            let researcher = Agent::new(researcher_llm).name("researcher");
            let coder = Agent::new(coder_llm).name("coder");
            let lead = Agent::new(lead_llm)
                .name("lead")
                .model("gpt-5.5")
                .instructions(
                    "Break the task down. Delegate. Verify before finishing.",
                )
                .tool(researcher.as_tool(
                    "research",
                    "Investigate a question and report findings.",
                ))
                .tool(coder.as_tool(
                    "implement",
                    "Make a scoped code change and report what changed.",
                ))
                .limits(
                    Limits::default()
                        .max_usd(5.0)
                        .timeout(Duration::from_secs(1800)),
                );

            let mut run =
                lead.start("Add retry with jitter to the HTTP client.", &store);
            let lead_id = run.id();
            let mut tools = Vec::new();
            let mut printed = String::new();
            {
                let mut events = run.events();
                while let Some(event) = events.next().await {
                    match event {
                        RunEvent::ToolStart { run, tool, .. } => {
                            tools.push((run == lead_id, tool.to_string()))
                        }
                        RunEvent::TextDelta {
                            parent: None,
                            delta,
                            ..
                        } => printed.push_str(&delta),
                        _ => {}
                    }
                }
            }
            let outcome = run.outcome().await.unwrap();
            assert_eq!(outcome.stop, StopReason::Stop);
            assert_eq!(printed, "Retries now use full jitter.");
            assert_eq!(
                tools,
                vec![
                    (true, "research".to_owned()),
                    (true, "implement".to_owned())
                ]
            );
            assert!((outcome.usage.cost.total - 1.8).abs() < 1e-9);
        });
}

/// "Fork fan-out": after an investigation, three forks try different
/// fixes from the same checkpoint, concurrently. Each starts from the
/// whole investigation plus its own instruction, and is stored as a fork
/// of it.
#[test]
fn fork_fan_out() {
    let mut llm = ScriptedModel::new()
        .turn(|t| t.text("The clock is read twice per tick."));
    for _ in 0..3 {
        llm = llm.turn(|t| t.text("Implemented."));
    }
    block_on(async {
        let store = Store::memory().await.unwrap();
        let debugger = Agent::new(llm.clone()).name("debugger");
        let investigation = debugger
            .run("Find the root cause. Don't fix it yet.", &store)
            .await
            .unwrap();
        let base = investigation.checkpoint();
        let attempts = [
            "minimal fix",
            "fix plus regression test",
            "refactor clock injection",
        ]
        .map(|s| {
            debugger
                .fork(&base)
                .run(format!("Now implement: {s}"), &store)
        });
        let results = futures_util::future::join_all(attempts).await;

        let investigated = &llm.requests()[0].transcript;
        let mut instructions = Vec::new();
        for request in &llm.requests()[1..] {
            let seen = &request.transcript;
            assert_eq!(seen.len(), 3);
            assert_eq!(seen[0], investigated[0]);
            let Message::User(user) = &seen[2] else {
                unreachable!()
            };
            let UserContent::Text(text) = &user.content else {
                unreachable!()
            };
            instructions.push(text.clone());
        }
        instructions.sort();
        assert_eq!(
            instructions,
            vec![
                "Now implement: fix plus regression test",
                "Now implement: minimal fix",
                "Now implement: refactor clock injection",
            ]
        );
        for result in results {
            let outcome = result.unwrap();
            let record = store.run(&outcome.run.0).await.unwrap().unwrap();
            assert_eq!(
                record.kind,
                RunKind::Fork {
                    parent: investigation.run.0.to_string(),
                    fork_seq: base.seq(),
                }
            );
        }
    });
}
