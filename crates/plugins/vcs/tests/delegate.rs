//! The `delegate` tool (ADR 0009): a sub-agent works on the caller's
//! code, its changes land on the caller's stack, and it closes.

use std::{path::Path, process::Command};

use serde_json::json;
use tau_agent::agent::Agent;
use tau_store::Store;
use tau_testing::scripted::ScriptedModel;
use tau_tools::{path::Root, plugin::CodingTools};
use tau_vcs::{
    Delegate,
    Identity,
    Link,
    Project,
    RunWorkspace,
    run_workspace::{PLUGIN, bookmark},
};

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(["-c", "init.defaultBranch=main"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
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

fn write(path: &str) -> serde_json::Value {
    json!({ "path": path, "content": format!("{path}\n") })
}

/// A coder on `workspace`, as the host builds one.
fn coder(llm: ScriptedModel, workspace: &RunWorkspace) -> Agent {
    Agent::new(llm)
        .name("coder")
        .plugin(CodingTools::new(Root::new(workspace.dir())))
        .plugin(workspace.clone())
}

/// A coder that can delegate, its sub-agents built the same way.
fn delegating(
    llm: ScriptedModel,
    workspace: &RunWorkspace,
    child: impl Fn(RunWorkspace) -> anyhow::Result<Agent> + Send + Sync + 'static,
) -> Agent {
    coder(llm, workspace).tool(Delegate::new(
        workspace.clone(),
        Identity::default(),
        child,
    ))
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn a_sub_agent_lands_its_changes_on_the_caller() {
    let home = tempfile::tempdir().unwrap();
    let project = project(home.path());
    runtime().block_on(async {
        let store = Store::memory().await.unwrap();
        let llm = ScriptedModel::new()
            // The caller writes a file, then delegates in its next turn.
            .turn(|t| t.tool_call("write", write("parent.txt")))
            .turn(|t| {
                t.tool_call("delegate", json!({ "task": "write child.txt" }))
            })
            // The sub-agent sees the caller's file, and writes its own.
            .turn(|t| t.tool_call("read", json!({ "path": "parent.txt" })))
            .turn(|t| t.tool_call("write", write("child.txt")))
            .turn(|t| t.text("child.txt is written"))
            .turn(|t| t.text("done"));
        let parent =
            RunWorkspace::new(project.clone(), "parent", Identity::default())
                .unwrap();
        let child_llm = llm.clone();
        let outcome = delegating(llm.clone(), &parent, move |workspace| {
            Ok(coder(child_llm.clone(), &workspace))
        })
        .run("write parent.txt, then delegate child.txt", &store)
        .await
        .unwrap();
        assert_eq!(outcome.text, "done");

        // The sub-agent read the caller's file: it worked on its code.
        let read = format!("{:?}", llm.requests()[3].transcript);
        assert!(read.contains("parent.txt\\n"), "{read}");
        // Its answer came back, with what landed.
        let result = format!("{:?}", llm.requests()[5].transcript);
        assert!(result.contains("child.txt is written"), "{result}");
        assert!(
            result.contains("Its 1 change landed on top of yours"),
            "{result}"
        );

        // The caller has both files, and the sub-agent is closed.
        for file in ["parent.txt", "child.txt"] {
            assert!(parent.dir().join(file).exists(), "{file}");
        }
        assert_eq!(project.workspaces().unwrap(), ["parent"]);

        // The caller's links: its turn 1, then in turn 2 the landed
        // change from the sub-agent, then its own turns.
        let links: Vec<Link> = store
            .plugin_entries(&outcome.run.0, PLUGIN)
            .await
            .unwrap()
            .iter()
            .filter_map(|(_, body)| Link::parse(body))
            .collect();
        let shape: Vec<(u32, bool, bool)> = links
            .iter()
            .map(|link| (link.turn, link.changed, link.from.is_some()))
            .collect();
        assert_eq!(
            shape,
            [
                (1, true, false),
                (2, true, true),
                (2, false, false),
                (3, false, false)
            ]
        );
        let child = links[1].from.clone().unwrap();
        let child_bookmark = format!("tau/{child}");
        assert_eq!(project.bookmark(&child_bookmark).unwrap(), None);
        assert_eq!(
            project.bookmark(&bookmark(&outcome.run)).unwrap(),
            Some(links[1].commit_id.clone())
        );
        assert_eq!(
            project.parent_of(&links[1].commit_id).unwrap(),
            Some(links[0].commit_id.clone()),
            "the landed change sits on the caller's turn 1"
        );
    });
}

/// A sub-agent that cannot start leaves nothing behind, and the caller
/// hears why.
#[test]
fn a_failed_sub_agent_is_dropped() {
    let home = tempfile::tempdir().unwrap();
    let project = project(home.path());
    runtime().block_on(async {
        let store = Store::memory().await.unwrap();
        let llm = ScriptedModel::new()
            .turn(|t| t.tool_call("delegate", json!({ "task": "anything" })))
            .turn(|t| t.text("gave up"));
        let parent =
            RunWorkspace::new(project.clone(), "parent", Identity::default())
                .unwrap();
        let outcome = delegating(llm.clone(), &parent, |_| {
            anyhow::bail!("no model for sub-agents")
        })
        .run("delegate something", &store)
        .await
        .unwrap();
        assert_eq!(outcome.text, "gave up");
        let result = format!("{:?}", llm.requests()[1].transcript);
        assert!(result.contains("no model for sub-agents"), "{result}");
        assert_eq!(project.workspaces().unwrap(), ["parent"]);
    });
}

/// A sub-agent that fails after writing: its changes are dropped, its
/// workspace and bookmark go, and the caller's code is as it was.
#[test]
fn a_sub_agent_that_fails_leaves_no_changes() {
    let home = tempfile::tempdir().unwrap();
    let project = project(home.path());
    runtime().block_on(async {
        let store = Store::memory().await.unwrap();
        let llm = ScriptedModel::new()
            .turn(|t| t.tool_call("delegate", json!({ "task": "write" })))
            .turn(|t| t.tool_call("write", write("child.txt")))
            .turn(|t| t.dropped())
            .turn(|t| t.text("it failed"));
        let parent =
            RunWorkspace::new(project.clone(), "parent", Identity::default())
                .unwrap();
        let child_llm = llm.clone();
        let outcome = delegating(llm.clone(), &parent, move |workspace| {
            Ok(coder(child_llm.clone(), &workspace))
        })
        .run("delegate a write", &store)
        .await
        .unwrap();
        assert_eq!(outcome.text, "it failed");
        assert!(!parent.dir().join("child.txt").exists());
        assert_eq!(project.workspaces().unwrap(), ["parent"]);
        // The only run bookmark left is the caller's.
        assert_eq!(
            project.bookmarks("tau/").unwrap(),
            [bookmark(&outcome.run)]
        );
        // Nothing of the child's reached the caller's links.
        let links: Vec<Link> = store
            .plugin_entries(&outcome.run.0, PLUGIN)
            .await
            .unwrap()
            .iter()
            .filter_map(|(_, body)| Link::parse(body))
            .collect();
        assert!(links.iter().all(|link| link.from.is_none()));
    });
}
