//! The `delegate` tool (ADR 0009): a sub-agent works on the caller's
//! code, its changes land on the caller's stack, and it closes.

use std::{
    collections::VecDeque,
    path::Path,
    process::Command,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use serde_json::{Value, json};
use tau_agent::{
    agent::Agent,
    limits::Limits,
    tool::{AgentTool, ToolCtx, ToolError, ToolOutput},
};
use tau_ai::responses::request::ReasoningEffort;
use tau_store::Store;
use tau_testing::scripted::ScriptedModel;
use tau_tools::{path::Root, plugin::CodingTools};
use tau_vcs::{
    Delegate,
    Identity,
    Link,
    Project,
    RunWorkspace,
    VcsPlugin,
    delegate::ChildModel,
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

fn commit(message: &str) -> serde_json::Value {
    json!({ "message": message })
}

fn write(path: &str) -> serde_json::Value {
    json!({ "path": path, "content": format!("{path}\n") })
}

/// A coder on `workspace`, as the host builds one.
fn coder(llm: ScriptedModel, workspace: &RunWorkspace) -> Agent {
    Agent::new(llm)
        .name("coder")
        .plugin(CodingTools::new(Root::new(workspace.dir())))
        .plugin(VcsPlugin::new(workspace.vcs().clone()))
        .plugin(workspace.clone())
}

/// A coder that can delegate, its sub-agents built the same way.
fn delegating(
    llm: ScriptedModel,
    workspace: &RunWorkspace,
    child: impl Fn(RunWorkspace) -> Result<Agent, ToolError> + Send + Sync + 'static,
) -> Agent {
    coder(llm, workspace).tool(Delegate::new(
        workspace.clone(),
        Identity::default(),
        &[],
        move |workspace, _: &ChildModel| child(workspace),
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
            // The caller writes a file and commits it, then delegates.
            .turn(|t| t.tool_call("write", write("parent.txt")))
            .turn(|t| t.tool_call("vcs_commit", commit("feat: parent")))
            .turn(|t| {
                t.tool_call("delegate", json!({ "task": "write child.txt" }))
            })
            // The sub-agent sees the caller's file, and commits its own.
            .turn(|t| t.tool_call("read", json!({ "path": "parent.txt" })))
            .turn(|t| t.tool_call("write", write("child.txt")))
            .turn(|t| t.tool_call("vcs_commit", commit("feat: child")))
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

        // The sub-agent forked the caller: it was asked on the caller's
        // conversation, told which call it runs, and then its task.
        let asked = format!("{:?}", llm.requests()[3].transcript);
        assert!(
            asked.contains("write parent.txt, then delegate child.txt"),
            "{asked}"
        );
        assert!(
            asked.contains("You are the sub-agent running this call"),
            "{asked}"
        );
        // The sub-agent read the caller's file: it worked on its code.
        let read = format!("{:?}", llm.requests()[4].transcript);
        assert!(read.contains("parent.txt\\n"), "{read}");
        // Its answer came back, with what landed.
        let result = format!("{:?}", llm.requests()[7].transcript);
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

        // The caller's links: a snapshot a turn, and in turn 3 the change
        // that landed from the sub-agent before that turn's snapshot.
        let links: Vec<Link> = store
            .plugin_entries(&outcome.run.0, PLUGIN)
            .await
            .unwrap()
            .iter()
            .filter_map(|(_, body)| Link::parse(body))
            .collect();
        let shape: Vec<(u32, bool, bool, bool)> = links
            .iter()
            .map(|link| {
                (link.turn, link.changed, link.from.is_some(), link.snapshot)
            })
            .collect();
        assert_eq!(
            shape,
            [
                (1, true, false, true),
                (2, false, false, true),
                (3, true, true, false),
                (3, true, false, true),
                (4, false, false, true),
            ]
        );
        let child = links[2].from.clone().unwrap();
        let child_bookmark = format!("tau/{child}");
        assert_eq!(project.bookmark(&child_bookmark).unwrap(), None);
        // The caller's stack: its own commit, then the sub-agent's on it.
        let head = project.bookmark(&bookmark(&outcome.run)).unwrap().unwrap();
        assert_eq!(head, links[2].commit_id);
        let stack: Vec<String> = project
            .stack(&head)
            .unwrap()
            .into_iter()
            .map(|change| change.description.trim().to_owned())
            .collect();
        assert_eq!(stack, ["feat: parent", "feat: child"]);
    });
}

/// A sub-agent that leaves its work uncommitted has it committed at its
/// end, with a message its model writes from its own task, not from the
/// caller's prompt it inherited.
#[test]
fn a_sub_agents_leftover_is_described_from_its_task() {
    let home = tempfile::tempdir().unwrap();
    let project = project(home.path());
    runtime().block_on(async {
        let store = Store::memory().await.unwrap();
        let llm = ScriptedModel::new()
            .turn(|t| {
                t.tool_call("delegate", json!({ "task": "write child.txt" }))
            })
            .turn(|t| t.text("done"));
        let child_llm = ScriptedModel::new()
            .turn(|t| t.tool_call("write", write("child.txt")))
            .turn(|t| t.text("written"))
            .turn(|t| t.text("still written"))
            .turn(|t| t.text("feat: child"));
        let parent =
            RunWorkspace::new(project.clone(), "parent", Identity::default())
                .unwrap();
        let script = child_llm.clone();
        let outcome = delegating(llm.clone(), &parent, move |workspace| {
            Ok(coder(script.clone(), &workspace))
        })
        .run("hand child.txt to a sub-agent", &store)
        .await
        .unwrap();
        assert_eq!(outcome.text, "done");
        child_llm.assert_exhausted();
        let asked = format!("{:?}", child_llm.requests()[3].transcript);
        assert!(
            asked.contains("<task>\\nwrite child.txt\\n</task>"),
            "{asked}"
        );
        let head = project.bookmark(&bookmark(&outcome.run)).unwrap().unwrap();
        let stack: Vec<String> = project
            .stack(&head)
            .unwrap()
            .into_iter()
            .map(|change| change.description.trim().to_owned())
            .collect();
        assert_eq!(stack, ["feat: child"]);
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
            Err("no model for sub-agents".into())
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

/// A caller with uncommitted work is told to commit before delegating.
#[test]
fn delegating_needs_a_clean_working_copy() {
    let home = tempfile::tempdir().unwrap();
    let project = project(home.path());
    runtime().block_on(async {
        let store = Store::memory().await.unwrap();
        let llm = ScriptedModel::new()
            .turn(|t| t.tool_call("write", write("parent.txt")))
            .turn(|t| t.tool_call("delegate", json!({ "task": "anything" })))
            .turn(|t| t.tool_call("vcs_commit", commit("feat: parent")))
            .turn(|t| t.text("done"));
        let parent =
            RunWorkspace::new(project.clone(), "parent", Identity::default())
                .unwrap();
        let outcome = delegating(llm.clone(), &parent, |_| {
            panic!("no sub-agent starts before the caller commits")
        })
        .run("write, then delegate", &store)
        .await
        .unwrap();
        assert_eq!(outcome.text, "done");
        let result = format!("{:?}", llm.requests()[2].transcript);
        assert!(
            result.contains("Commit your work with vcs_commit"),
            "{result}"
        );
    });
}

/// Sub-agents that each write their own file, one scripted model each,
/// handed out in the order they start.
fn writers(files: &[&str]) -> Arc<Mutex<VecDeque<ScriptedModel>>> {
    let scripts = files
        .iter()
        .map(|file| {
            let file = (*file).to_owned();
            ScriptedModel::new()
                .turn(move |t| t.tool_call("write", write(&file)))
                .turn(|t| t.tool_call("vcs_commit", commit("feat: part")))
                .turn(|t| t.text("written"))
        })
        .collect();
    Arc::new(Mutex::new(scripts))
}

/// A caller whose sub-agents take the next of `scripts` each.
fn delegating_to(
    llm: ScriptedModel,
    workspace: &RunWorkspace,
    scripts: Arc<Mutex<VecDeque<ScriptedModel>>>,
) -> Agent {
    delegating(llm, workspace, move |workspace| {
        let script = scripts.lock().unwrap().pop_front().unwrap();
        Ok(coder(script, &workspace))
    })
}

/// Two sub-agents called in one batch run side by side, and both land:
/// the caller ends up with each one's file and commit on its stack.
#[test]
fn sub_agents_in_one_batch_both_land() {
    let home = tempfile::tempdir().unwrap();
    let project = project(home.path());
    runtime().block_on(async {
        let store = Store::memory().await.unwrap();
        let llm = ScriptedModel::new()
            .turn(|t| {
                t.tool_call("delegate", json!({ "task": "write a.txt" }))
                    .tool_call("delegate", json!({ "task": "write b.txt" }))
            })
            .turn(|t| t.text("done"));
        let parent =
            RunWorkspace::new(project.clone(), "parent", Identity::default())
                .unwrap();
        let outcome =
            delegating_to(llm.clone(), &parent, writers(&["a.txt", "b.txt"]))
                .run("split the work", &store)
                .await
                .unwrap();
        assert_eq!(outcome.text, "done");
        let results = format!("{:?}", llm.requests()[1].transcript);
        assert_eq!(results.matches("Its 1 change landed").count(), 2);
        assert!(!results.contains("with conflicts"), "{results}");
        for file in ["a.txt", "b.txt"] {
            assert!(parent.dir().join(file).exists(), "{file}");
        }
        assert_eq!(project.workspaces().unwrap(), ["parent"]);
        let head = project.bookmark(&bookmark(&outcome.run)).unwrap().unwrap();
        assert_eq!(project.stack(&head).unwrap().len(), 2);
    });
}

/// Two sub-agents that write the same file: the first lands clean, the
/// second lands its conflict, and its result names the file for the
/// caller to resolve.
#[test]
fn a_clashing_sub_agent_lands_its_conflict() {
    let home = tempfile::tempdir().unwrap();
    let project = project(home.path());
    runtime().block_on(async {
        let store = Store::memory().await.unwrap();
        let llm = ScriptedModel::new()
            .turn(|t| {
                t.tool_call("delegate", json!({ "task": "one" }))
                    .tool_call("delegate", json!({ "task": "two" }))
            })
            .turn(|t| t.text("done"));
        let parent =
            RunWorkspace::new(project.clone(), "parent", Identity::default())
                .unwrap();
        let scripts = ["one\n", "two\n"]
            .into_iter()
            .map(|content| {
                ScriptedModel::new()
                    .turn(move |t| {
                        t.tool_call(
                            "write",
                            json!({ "path": "shared.txt", "content": content }),
                        )
                    })
                    .turn(|t| t.tool_call("vcs_commit", commit("feat: shared")))
                    .turn(|t| t.text("written"))
            })
            .collect();
        let outcome =
            delegating_to(llm.clone(), &parent, Arc::new(Mutex::new(scripts)))
                .run("clash", &store)
                .await
                .unwrap();
        assert_eq!(outcome.text, "done");
        let results = format!("{:?}", llm.requests()[1].transcript);
        assert_eq!(results.matches("Its 1 change landed").count(), 2);
        assert_eq!(
            results.matches("with conflicts in shared.txt").count(),
            1,
            "{results}"
        );
        assert_eq!(project.workspaces().unwrap(), ["parent"]);
    });
}

/// A landing names only the conflicts it brought: after a batch left
/// `shared.txt` in conflict, a later sub-agent that writes another file
/// lands clean, and its note says so.
#[test]
fn a_landing_names_only_the_conflicts_it_brought() {
    let home = tempfile::tempdir().unwrap();
    let project = project(home.path());
    runtime().block_on(async {
        let store = Store::memory().await.unwrap();
        let llm = ScriptedModel::new()
            .turn(|t| {
                t.tool_call("delegate", json!({ "task": "one" }))
                    .tool_call("delegate", json!({ "task": "two" }))
            })
            .turn(|t| t.tool_call("delegate", json!({ "task": "other" })))
            .turn(|t| t.text("done"));
        let parent =
            RunWorkspace::new(project.clone(), "parent", Identity::default())
                .unwrap();
        let scripts = [
            ("shared.txt", "one\n"),
            ("shared.txt", "two\n"),
            ("other.txt", "other\n"),
        ]
        .into_iter()
        .map(|(path, content)| {
            ScriptedModel::new()
                .turn(move |t| {
                    t.tool_call(
                        "write",
                        json!({ "path": path, "content": content }),
                    )
                })
                .turn(|t| t.tool_call("vcs_commit", commit("feat: part")))
                .turn(|t| t.text("written"))
        })
        .collect();
        let outcome =
            delegating_to(llm.clone(), &parent, Arc::new(Mutex::new(scripts)))
                .run("clash, then add", &store)
                .await
                .unwrap();
        assert_eq!(outcome.text, "done");
        let batch = format!("{:?}", llm.requests()[1].transcript);
        assert_eq!(
            batch.matches("with conflicts in shared.txt").count(),
            1,
            "{batch}"
        );
        let last = llm.requests()[2].transcript.last().cloned().unwrap();
        let last = format!("{last:?}");
        assert!(
            last.contains("[Its 1 change landed on top of yours.]"),
            "{last}"
        );
    });
}

/// Counts the calls running at once, and the most it saw.
#[derive(Clone, Default)]
struct Gauge {
    now: Arc<Mutex<(usize, usize)>>,
}

#[async_trait]
impl AgentTool for Gauge {
    fn name(&self) -> &str {
        "gauge"
    }
    fn description(&self) -> &str {
        "Waits for others to run beside it."
    }
    fn parameters(&self) -> &Value {
        static SCHEMA: std::sync::LazyLock<Value> =
            std::sync::LazyLock::new(|| json!({ "type": "object" }));
        &SCHEMA
    }
    async fn call(
        &self,
        _args: Value,
        _ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        {
            let mut now = self.now.lock().unwrap();
            now.0 += 1;
            now.1 = now.1.max(now.0);
        }
        // Wait for four to run at once, so they overlap however slowly
        // they start; the calls past them never see four.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while self.now.lock().unwrap().0 < 4
            && tokio::time::Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        self.now.lock().unwrap().0 -= 1;
        Ok(ToolOutput::text("waited"))
    }
}

/// Six sub-agents in one batch: no more than four run at once, and all
/// of them finish.
#[test]
fn at_most_four_sub_agents_run_at_once() {
    let home = tempfile::tempdir().unwrap();
    let project = project(home.path());
    runtime().block_on(async {
        let store = Store::memory().await.unwrap();
        let llm = ScriptedModel::new()
            .turn(|mut t| {
                for n in 0..6 {
                    t = t.tool_call(
                        "delegate",
                        json!({ "task": format!("{n}") }),
                    );
                }
                t
            })
            .turn(|t| t.text("done"));
        let parent =
            RunWorkspace::new(project.clone(), "parent", Identity::default())
                .unwrap();
        let gauge = Gauge::default();
        let child_gauge = gauge.clone();
        let outcome = delegating(llm.clone(), &parent, move |workspace| {
            let script = ScriptedModel::new()
                .turn(|t| t.tool_call("gauge", json!({})))
                .turn(|t| t.text("waited"));
            Ok(coder(script, &workspace).tool(child_gauge.clone()))
        })
        .run("fan out", &store)
        .await
        .unwrap();
        assert_eq!(outcome.text, "done");
        let results = format!("{:?}", llm.requests()[1].transcript);
        assert_eq!(results.matches("It changed no files").count(), 6);
        let (running, most) = *gauge.now.lock().unwrap();
        assert_eq!(running, 0);
        assert_eq!(most, 4);
        assert_eq!(project.workspaces().unwrap(), ["parent"]);
    });
}

/// A call's model and effort reach the host's builder; left out, they
/// stay the caller's.
#[test]
fn a_call_can_pick_its_model_and_effort() {
    let home = tempfile::tempdir().unwrap();
    let project = project(home.path());
    runtime().block_on(async {
        let store = Store::memory().await.unwrap();
        let llm = ScriptedModel::new()
            .turn(|t| {
                t.tool_call(
                    "delegate",
                    json!({ "task": "a", "model": "gpt-5.5-mini", "effort": "low" }),
                )
            })
            .turn(|t| t.tool_call("delegate", json!({ "task": "b" })))
            .turn(|t| t.text("done"));
        let parent =
            RunWorkspace::new(project.clone(), "parent", Identity::default())
                .unwrap();
        let asked = Arc::new(Mutex::new(Vec::new()));
        let seen = asked.clone();
        let agent = coder(llm.clone(), &parent).tool(Delegate::new(
            parent.clone(),
            Identity::default(),
            &["gpt-5.5".to_owned(), "gpt-5.5-mini".to_owned()],
            move |workspace, model: &ChildModel| {
                seen.lock().unwrap().push(model.clone());
                let script = ScriptedModel::new().turn(|t| t.text("ok"));
                Ok(coder(script, &workspace))
            },
        ));
        let outcome = agent.run("pick", &store).await.unwrap();
        assert_eq!(outcome.text, "done");
        assert_eq!(
            asked.lock().unwrap().clone(),
            [
                ChildModel {
                    model: Some("gpt-5.5-mini".into()),
                    effort: Some(ReasoningEffort::Low),
                },
                ChildModel::default(),
            ]
        );
    });
}

/// A sub-agent stopped by a limit has its work committed at its end, as
/// any run at a limit does (ADR 0014), and lands like one that finished;
/// its result says a limit cut it short.
#[test]
fn a_sub_agent_at_a_limit_lands_its_work() {
    let home = tempfile::tempdir().unwrap();
    let project = project(home.path());
    runtime().block_on(async {
        let store = Store::memory().await.unwrap();
        let llm = ScriptedModel::new()
            .turn(|t| t.tool_call("delegate", json!({ "task": "write" })))
            .turn(|t| t.text("done"));
        let parent =
            RunWorkspace::new(project.clone(), "parent", Identity::default())
                .unwrap();
        let outcome = delegating(llm.clone(), &parent, |workspace| {
            let script = ScriptedModel::new()
                .turn(|t| {
                    t.tool_call("write", write("child.txt"))
                        .tool_call("vcs_commit", commit("feat: child"))
                })
                .turn(|t| t.text("never asked"));
            Ok(coder(script, &workspace).limits(Limits {
                max_turns: Some(1),
                ..Limits::default()
            }))
        })
        .run("delegate a write", &store)
        .await
        .unwrap();
        assert_eq!(outcome.text, "done");
        let result = format!("{:?}", llm.requests()[1].transcript);
        assert!(parent.dir().join("child.txt").exists(), "{result}");
        assert!(
            result.contains(
                "[It stopped at its turn limit. Its 1 change landed on top \
                 of yours.]"
            ),
            "{result}"
        );
        assert!(!result.contains("is_error: true"), "{result}");
        assert_eq!(project.workspaces().unwrap(), ["parent"]);
        assert_eq!(
            project.bookmarks("tau/").unwrap(),
            [bookmark(&outcome.run)]
        );
    });
}
