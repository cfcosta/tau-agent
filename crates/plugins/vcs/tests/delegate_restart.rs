//! A sub-agent's workspace name after a restart (`docs/reference/vcs.md`,
//! "Delegating to a sub-agent"). Its own test binary: the names count
//! from zero in each process, as they do in each run of tau.

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
/// in the middle of a delegation leaves it, is taken over by the first
/// sub-agent of the next process: it starts on that workspace's stale
/// commit instead of its caller's head, and the stale commit lands on
/// the caller.
///
/// `child_name` in `src/delegate.rs` numbers sub-agent workspaces with a
/// counter that starts at zero in each process, so the main chat's first
/// sub-agent after a restart is `default-sub-0` again.
/// `Project::add_workspace` opens a workspace that exists as it is, so
/// `RunWorkspace::with_base` is ignored.
///
/// Fix options:
/// - name sub-agent workspaces uniquely across processes: after the
///   sub-agent's run id, or with a random suffix;
/// - have `Delegate::call` forget a workspace that exists under the name
///   before it makes one (its commits stay in the operation log);
/// - have the host forget its runs' leftover workspaces when it opens a
///   project.
///
/// The first is the smallest and is the recommendation; the third also
/// cleans up after any run, not only sub-agents.
#[test]
#[ignore = "bug: a sub-agent takes over a workspace an earlier process left"]
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
        assert_eq!(*named.lock().unwrap(), ["parent-sub-0"]);
        for file in ["parent.txt", "child.txt"] {
            assert!(parent.dir().join(file).exists(), "{file}");
        }
        assert!(
            !parent.dir().join("stale.txt").exists(),
            "the earlier process's commit landed on the caller"
        );
    });
}
