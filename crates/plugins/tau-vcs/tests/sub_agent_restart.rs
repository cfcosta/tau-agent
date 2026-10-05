//! A sub-agent's workspace after a restart (`docs/reference/vcs.md`,
//! "Sub-agents: spawn and wait"), in a test binary of its own: a fresh
//! process, as tau is after a restart.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

mod common;

use common::{coder, project_with};
use serde_json::json;
use tau_store::Store;
use tau_testing::scripted::ScriptedModel;
use tau_vcs::{
    Identity,
    RunWorkspace,
    Spawn,
    SubAgents,
    Wait,
    sub_agents::ChildModel,
};

/// A sub-agent workspace left behind by an earlier process, as a crash
/// in the middle of a sub-agent leaves it, stays apart from the next
/// process's sub-agents: their names are random, so the first sub-agent
/// after a restart starts on its caller's head, and the stale commit
/// never lands on the caller.
#[test]
fn a_sub_agent_starts_on_its_caller_after_a_restart() {
    let home = tempfile::tempdir().unwrap();
    let project = project_with(home.path(), &[("README.md", "hello\n")]);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    // An earlier process's sub-agent of `parent`, cut off with a commit
    // of its own.
    let stale = project
        .add_workspace("parent-sub-0", &project.trunk().unwrap())
        .unwrap();
    std::fs::write(
        project.workspace_dir("parent-sub-0").join("stale.txt"),
        "stale\n",
    )
    .unwrap();
    runtime
        .block_on(stale.commit_all("feat: stale", "tau/old"))
        .unwrap();
    drop(stale);
    runtime.block_on(async {
        let store = Store::memory().await.unwrap();
        let llm = ScriptedModel::new()
            .turn(|t| {
                t.tool_call(
                    "write",
                    json!({ "path": "parent.txt", "content": "parent\n" }),
                )
                .tool_call("vcs_commit", json!({ "message": "feat: parent" }))
            })
            .turn(|t| {
                t.tool_call("spawn", json!({ "task": "child" }))
                    .tool_call("wait", json!({}))
            })
            .turn(|t| t.text("done"));
        let parent =
            RunWorkspace::new(project.clone().into(), "parent", Identity::default())
                .unwrap();
        let named = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = named.clone();
        let agents = SubAgents::default();
        let agent = coder(llm, &parent, true)
            .tool(Wait::new(parent.clone(), agents.clone()))
            .tool(Spawn::new(
            parent.clone(),
            Identity::default(),
            agents,
            &[],
            ready_child(move |workspace, _: &ChildModel| {
                seen.lock().unwrap().push(workspace.name().to_owned());
                let script = ScriptedModel::new()
                    .turn(|t| {
                        t.tool_call(
                            "write",
                            json!({ "path": "child.txt", "content": "child\n" }),
                        )
                        .tool_call(
                            "vcs_commit",
                            json!({ "message": "feat: child" }),
                        )
                    })
                    .turn(|t| t.text("written"));
                Ok(coder(script, &workspace, true))
            }),
        ));
        let outcome = agent.run("work, then hand over", &store).await.unwrap();
        assert_eq!(outcome.text, "done");
        let named = named.lock().unwrap().clone();
        assert_eq!(named.len(), 1);
        assert!(named[0].starts_with("parent-sub-"), "{named:?}");
        assert_ne!(named[0], "parent-sub-0");
        for file in ["parent.txt", "child.txt"] {
            assert!(parent.dir().join(file).exists(), "{file}");
        }
        assert!(
            !parent.dir().join("stale.txt").exists(),
            "the earlier process's commit landed on the caller"
        );
    });
}

/// A sub-agent factory that builds its agent at once, as `Spawn` takes
/// one: a future that is ready.
fn ready_child(
    child: impl Fn(
        tau_vcs::RunWorkspace,
        &tau_vcs::sub_agents::ChildModel,
    )
        -> Result<tau_agent::agent::Agent, tau_agent::error::ToolError>
    + Send
    + Sync
    + 'static,
) -> impl Fn(
    tau_vcs::RunWorkspace,
    &tau_vcs::sub_agents::ChildModel,
) -> tau_vcs::sub_agents::ChildFuture
+ Send
+ Sync
+ 'static {
    move |workspace, model| {
        Box::pin(std::future::ready(child(workspace, model)))
    }
}
