//! A sub-agent's workspace after a restart (`docs/reference/vcs.md`,
//! "Sub-agents: spawn"), in a test binary of its own: a fresh process,
//! as tau is after a restart.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

mod common;

use common::{
    caller_in,
    coder_in,
    heard_sub_agents,
    notes,
    project_with,
    ready_child,
};
use serde_json::json;
use tau_testing::scripted::ScriptedModel;
use tau_ui_plugin::sub_agents::{ChildModel, Ending, Spawn};

/// A sub-agent workspace left behind by an earlier process, as a crash
/// in the middle of a sub-agent leaves it, stays apart from the next
/// process's sub-agents: their names are random, so the first sub-agent
/// after a restart starts on its caller's head, and the stale commit is
/// not in the work it brings back.
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
        .block_on(stale.commit_all("feat: stale", run_head("old")))
        .unwrap();
    drop(stale);
    runtime.block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let llm = ScriptedModel::new()
            .turn(|t| {
                t.tool_call(
                    "write",
                    json!({ "path": "parent.txt", "content": "parent\n" }),
                )
                .tool_call("vcs_commit", json!({ "message": "feat: parent" }))
            })
            .turn(|t| t.tool_call("spawn", json!({ "task": "child" })))
            .turn(|t| t.text("done"));
        let parent = caller_in(&project, "parent");
        let named = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = named.clone();
        let (agents, mut ends) = heard_sub_agents();
        let agent = coder_in(llm, parent.as_ref()).tool(Spawn::new(
            parent.clone(),
            agents,
            &notes(),
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
                Ok(coder_in(script, workspace.as_ref()))
            }),
        ));
        let outcome = agent.run("work, then hand over", &store).await.unwrap();
        assert_eq!(outcome.text, "done");
        let named = named.lock().unwrap().clone();
        assert_eq!(named.len(), 1);
        assert!(named[0].starts_with("parent-sub-"), "{named:?}");
        assert_ne!(named[0], "parent-sub-0");
        let (_, ending) = ends.recv().await.unwrap();
        assert!(matches!(ending, Ending::Done { .. }), "{ending:?}");
        let child = project.workspace_dir(&named[0]);
        for file in ["parent.txt", "child.txt"] {
            assert!(child.join(file).exists(), "{file}");
        }
        assert!(
            !child.join("stale.txt").exists(),
            "the earlier process's commit is in the sub-agent's work"
        );
    });
}

/// The head of run `run` (`Head::Run`).
fn run_head(run: &str) -> tau_vcs_host::Head {
    tau_vcs_host::Head::Run(tau_agent::tool::RunId(run.into()))
}
