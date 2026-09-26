//! Workflow primitives (`docs/reference/api.md`): workflow grouping,
//! typed results, forks and sub-agents, driven through `Agent` with
//! `ScriptedModel`, `Store::memory()` and paused time.

use tau_agent::agent::{Agent, Input};
use tau_store::Store;
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
