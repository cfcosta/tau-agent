//! A sub-agent's workspace after a restart (`docs/reference/vcs.md`,
//! "Delegating to a sub-agent"), in a test binary of its own: a fresh
//! process, as tau is after a restart.

use std::{path::Path, process::Command};

use serde_json::json;
use tau_agent::agent::Agent;
use tau_store::Store;
use tau_testing::scripted::ScriptedModel;
use tau_tools::{path::Root, plugin::CodingTools};
use tau_vcs::{
    Delegate,
    Identity,
    Project,
    RunWorkspace,
    VcsPlugin,
    delegate::ChildModel,
};

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(["-c", "init.defaultBranch=main"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
}

fn project(home: &Path) -> Project {
    let src = home.join("src");
    std::fs::create_dir_all(&src).unwrap();
    git(&src, &["init", "--quiet"]);
    std::fs::write(src.join("README.md"), "hello\n").unwrap();
    git(&src, &["add", "README.md"]);
    git(&src, &["commit", "--quiet", "-m", "first"]);
    Project::import(src.to_str().unwrap(), home.join("p"), Identity::default())
        .unwrap()
}

fn coder(llm: ScriptedModel, workspace: &RunWorkspace) -> Agent {
    Agent::new(llm)
        .name("coder")
        .plugin(CodingTools::new(Root::new(workspace.dir())))
        .plugin(VcsPlugin::new(workspace.vcs().clone()))
        .plugin(workspace.clone())
}

/// A sub-agent workspace left behind by an earlier process, as a crash
/// in the middle of a delegation leaves it, stays apart from the next
/// process's sub-agents: their names are random, so the first sub-agent
/// after a restart starts on its caller's head, and the stale commit
/// never lands on the caller.
#[test]
fn a_sub_agent_starts_on_its_caller_after_a_restart() {
    let home = tempfile::tempdir().unwrap();
    let project = project(home.path());
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
            .turn(|t| t.tool_call("delegate", json!({ "task": "child" })))
            .turn(|t| t.text("done"));
        let parent =
            RunWorkspace::new(project.clone(), "parent", Identity::default())
                .unwrap();
        let named = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = named.clone();
        let agent = coder(llm, &parent).tool(Delegate::new(
            parent.clone(),
            Identity::default(),
            &[],
            move |workspace, _: &ChildModel| {
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
                Ok(coder(script, &workspace))
            },
        ));
        let outcome = agent.run("work, then delegate", &store).await.unwrap();
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
