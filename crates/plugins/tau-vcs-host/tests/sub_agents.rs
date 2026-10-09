//! The `spawn` tool (ADR 0009, 0026, 0031): a sub-agent works on the
//! caller's code beside it, the caller's turn goes on without it, and
//! it ends with its work committed on its own stack, for the host to
//! land, or dropped.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

mod common;

use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use common::{coder, heard_sub_agents, project_with};
use serde_json::{Value, json};
use tau_agent::{
    agent::Agent,
    event::{LimitKind, RunEvent},
    limits::Limits,
    tool::{AgentTool, RunId, ToolCtx, ToolError, ToolOutput},
};
use tau_ai::responses::request::ReasoningEffort;
use tau_testing::scripted::{ScriptedModel, TurnBuilder};
use tau_vcs_host::{
    Identity,
    Link,
    ProjectRepo,
    RunWorkspace,
    Spawn,
    SubAgents,
    run_workspace::{PLUGIN, bookmark},
    sub_agents::{ChildModel, Ending, MAX_RUNNING, Taken},
};

fn commit(message: &str) -> serde_json::Value {
    json!({ "message": message })
}

fn write(path: &str) -> serde_json::Value {
    json!({ "path": path, "content": format!("{path}\n") })
}

/// A coder whose sub-agents are `agents`, which outlive its runs, each
/// built by `child`.
fn spawning(
    llm: ScriptedModel,
    workspace: &RunWorkspace,
    agents: &SubAgents,
    child: impl Fn(RunWorkspace) -> Result<Agent, ToolError> + Send + Sync + 'static,
) -> Agent {
    coder(llm, workspace, true).tool(Spawn::new(
        workspace.clone(),
        Identity::default(),
        agents.clone(),
        &[],
        ready_child(move |workspace, _: &ChildModel| child(workspace)),
    ))
}

/// A turn that hands `tasks` to sub-agents, and goes on.
fn spawn_all(t: TurnBuilder, tasks: &[Value]) -> TurnBuilder {
    tasks
        .iter()
        .fold(t, |t, task| t.tool_call("spawn", task.clone()))
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn caller(project: &ProjectRepo) -> RunWorkspace {
    RunWorkspace::new(project.clone().into(), "parent", Identity::default())
        .unwrap()
}

/// The workspace `run` worked in, as its last link names it.
async fn workspace_of(
    store: &tau_store::Store,
    project: &ProjectRepo,
    run: &RunId,
) -> PathBuf {
    let link = store
        .plugin_entries(&run.0, PLUGIN)
        .await
        .unwrap()
        .iter()
        .filter_map(|(_, body)| Link::parse(body))
        .next_back()
        .unwrap();
    project.workspace_dir(&link.workspace)
}

/// The descriptions on `run`'s bookmark, oldest first: the work it has
/// for the host to land.
fn stack_of(project: &ProjectRepo, run: &RunId) -> Vec<String> {
    let head = project.bookmark(&bookmark(run)).unwrap().unwrap();
    project
        .stack(&head)
        .unwrap()
        .into_iter()
        .map(|change| change.description.trim().to_owned())
        .collect()
}

/// A sub-agent forks its caller, works on the caller's committed code,
/// and ends with its commit on its own stack, on the caller's: nothing
/// reaches the caller until the host lands it.
#[test]
fn a_sub_agent_works_on_the_callers_code() {
    let home = tempfile::tempdir().unwrap();
    let project = project_with(home.path(), &[("README.md", "hello\n")]);
    runtime().block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let llm = ScriptedModel::new()
            // The caller writes a file and commits it, then spawns.
            .turn(|t| t.tool_call("write", write("parent.txt")))
            .turn(|t| t.tool_call("vcs_commit", commit("feat: parent")))
            .turn(|t| spawn_all(t, &[json!({ "task": "write child.txt" })]))
            .turn(|t| t.text("done"));
        // The sub-agent sees the caller's file, and commits its own.
        let child_llm = ScriptedModel::new()
            .turn(|t| t.tool_call("read", json!({ "path": "parent.txt" })))
            .turn(|t| t.tool_call("write", write("child.txt")))
            .turn(|t| t.tool_call("vcs_commit", commit("feat: child")))
            .turn(|t| t.text("child.txt is written"));
        let parent = caller(&project);
        let (agents, mut ends) = heard_sub_agents();
        let script = child_llm.clone();
        let outcome =
            spawning(llm.clone(), &parent, &agents, move |workspace| {
                Ok(coder(script.clone(), &workspace, true))
            })
            .run("write parent.txt, then hand child.txt over", &store)
            .await
            .unwrap();
        assert_eq!(outcome.text, "done");
        // The caller heard it started, and went on.
        let started = format!("{:?}", llm.requests()[3].transcript);
        assert!(started.contains("started"), "{started}");

        let (run, ending) = ends.recv().await.unwrap();
        assert_eq!(
            ending,
            Ending::Done {
                text: "child.txt is written".into(),
                limit: None,
            }
        );
        child_llm.assert_exhausted();
        // The sub-agent forked the caller: it was asked on the caller's
        // conversation, told which call it runs, and then its task.
        let asked = format!("{:?}", child_llm.requests()[0].transcript);
        assert!(
            asked.contains("write parent.txt, then hand child.txt over"),
            "{asked}"
        );
        assert!(
            asked.contains("You are the sub-agent running this call"),
            "{asked}"
        );
        // It read the caller's file: it worked on its code.
        let read = format!("{:?}", child_llm.requests()[1].transcript);
        assert!(read.contains("parent.txt\\n"), "{read}");

        // Its work waits on its stack, on the caller's commit; the caller
        // has none of it.
        let dir = workspace_of(&store, &project, &run).await;
        for file in ["parent.txt", "child.txt"] {
            assert!(dir.join(file).exists(), "{file}");
        }
        assert!(!parent.dir().join("child.txt").exists());
        assert_eq!(stack_of(&project, &run), ["feat: parent", "feat: child"]);
        assert_eq!(stack_of(&project, &outcome.run), ["feat: parent"]);
        // The caller's links are its own.
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

/// A sub-agent that leaves its work uncommitted has it committed at its
/// end, with a message its model writes from its own task, not from the
/// caller's prompt it inherited.
#[test]
fn a_sub_agents_leftover_is_described_from_its_task() {
    let home = tempfile::tempdir().unwrap();
    let project = project_with(home.path(), &[("README.md", "hello\n")]);
    runtime().block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let llm = ScriptedModel::new()
            .turn(|t| spawn_all(t, &[json!({ "task": "write child.txt" })]))
            .turn(|t| t.text("done"));
        let child_llm = ScriptedModel::new()
            .turn(|t| t.tool_call("write", write("child.txt")))
            .turn(|t| t.text("written"))
            .turn(|t| t.text("still written"))
            .turn(|t| t.text("feat: child"));
        let parent = caller(&project);
        let (agents, mut ends) = heard_sub_agents();
        let script = child_llm.clone();
        let outcome =
            spawning(llm.clone(), &parent, &agents, move |workspace| {
                Ok(coder(script.clone(), &workspace, true))
            })
            .run("hand child.txt to a sub-agent", &store)
            .await
            .unwrap();
        assert_eq!(outcome.text, "done");
        let (run, ending) = ends.recv().await.unwrap();
        assert!(matches!(ending, Ending::Done { .. }), "{ending:?}");
        child_llm.assert_exhausted();
        let asked = format!("{:?}", child_llm.requests()[3].transcript);
        assert!(
            asked.contains("<task>\\nwrite child.txt\\n</task>"),
            "{asked}"
        );
        assert_eq!(stack_of(&project, &run), ["feat: child"]);
    });
}

/// A sub-agent that cannot start leaves nothing behind, and the caller
/// hears why.
#[test]
fn a_failed_sub_agent_is_dropped() {
    let home = tempfile::tempdir().unwrap();
    let project = project_with(home.path(), &[("README.md", "hello\n")]);
    runtime().block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let llm = ScriptedModel::new()
            .turn(|t| spawn_all(t, &[json!({ "task": "anything" })]))
            .turn(|t| t.text("gave up"));
        let parent = caller(&project);
        let agents = SubAgents::default();
        let outcome = spawning(llm.clone(), &parent, &agents, |_| {
            Err("no model for sub-agents".into())
        })
        .run("hand something over", &store)
        .await
        .unwrap();
        assert_eq!(outcome.text, "gave up");
        let result = format!("{:?}", llm.requests()[1].transcript);
        assert!(result.contains("no model for sub-agents"), "{result}");
        assert!(agents.running().is_empty());
        assert_eq!(project.workspaces().unwrap(), ["parent"]);
    });
}

/// A sub-agent that fails after writing: its changes are dropped, its
/// workspace and bookmark go, and the caller's code is as it was.
#[test]
fn a_sub_agent_that_fails_leaves_no_changes() {
    let home = tempfile::tempdir().unwrap();
    let project = project_with(home.path(), &[("README.md", "hello\n")]);
    runtime().block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let llm = ScriptedModel::new()
            .turn(|t| spawn_all(t, &[json!({ "task": "write" })]))
            .turn(|t| t.text("handed over"));
        let child_llm = ScriptedModel::new()
            .turn(|t| t.tool_call("write", write("child.txt")))
            .turn(|t| t.dropped());
        let parent = caller(&project);
        let (agents, mut ends) = heard_sub_agents();
        let script = child_llm.clone();
        let outcome =
            spawning(llm.clone(), &parent, &agents, move |workspace| {
                Ok(coder(script.clone(), &workspace, true))
            })
            .run("hand a write over", &store)
            .await
            .unwrap();
        assert_eq!(outcome.text, "handed over");
        let (_, ending) = ends.recv().await.unwrap();
        assert!(matches!(ending, Ending::Failed { .. }), "{ending:?}");
        assert!(!parent.dir().join("child.txt").exists());
        assert_eq!(project.workspaces().unwrap(), ["parent"]);
        // The only run bookmark left is the caller's.
        assert_eq!(
            project.bookmarks("tau/").unwrap(),
            [bookmark(&outcome.run)]
        );
    });
}

/// A caller with uncommitted work is told to commit before spawning.
#[test]
fn spawning_needs_a_clean_working_copy() {
    let home = tempfile::tempdir().unwrap();
    let project = project_with(home.path(), &[("README.md", "hello\n")]);
    runtime().block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let llm = ScriptedModel::new()
            .turn(|t| t.tool_call("write", write("parent.txt")))
            .turn(|t| spawn_all(t, &[json!({ "task": "anything" })]))
            .turn(|t| t.tool_call("vcs_commit", commit("feat: parent")))
            .turn(|t| t.text("done"));
        let parent = caller(&project);
        let outcome =
            spawning(llm.clone(), &parent, &SubAgents::default(), |_| {
                panic!("no sub-agent starts before the caller commits")
            })
            .run("write, then hand over", &store)
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

/// Two sub-agents called in one batch run side by side, each on the
/// caller's head: each ends with only its own file and commit.
#[test]
fn sub_agents_in_one_batch_each_start_on_the_callers_head() {
    let home = tempfile::tempdir().unwrap();
    let project = project_with(home.path(), &[("README.md", "hello\n")]);
    runtime().block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let llm = ScriptedModel::new()
            .turn(|t| {
                spawn_all(
                    t,
                    &[
                        json!({ "task": "write a.txt" }),
                        json!({ "task": "write b.txt" }),
                    ],
                )
            })
            .turn(|t| t.text("done"));
        let parent = caller(&project);
        let (agents, mut ends) = heard_sub_agents();
        let scripts = writers(&["a.txt", "b.txt"]);
        let outcome =
            spawning(llm.clone(), &parent, &agents, move |workspace| {
                let script = scripts.lock().unwrap().pop_front().unwrap();
                Ok(coder(script, &workspace, true))
            })
            .run("split the work", &store)
            .await
            .unwrap();
        assert_eq!(outcome.text, "done");
        let mut files = Vec::new();
        for _ in 0..2 {
            let (run, ending) = ends.recv().await.unwrap();
            assert!(matches!(ending, Ending::Done { .. }), "{ending:?}");
            assert_eq!(stack_of(&project, &run), ["feat: part"]);
            let dir = workspace_of(&store, &project, &run).await;
            let mut own: Vec<&str> = ["a.txt", "b.txt"]
                .into_iter()
                .filter(|file| dir.join(file).exists())
                .collect();
            assert_eq!(own.len(), 1, "{own:?}");
            files.append(&mut own);
        }
        files.sort_unstable();
        assert_eq!(files, ["a.txt", "b.txt"]);
        assert_eq!(project.workspaces().unwrap().len(), 3);
    });
}

/// Counts the calls running at once, and the most it saw. Each call
/// waits until [`MAX_RUNNING`] run at once, or five seconds pass: the
/// sub-agents' workspaces are made one at a time, under the repository's
/// lock, so they reach the call at different times.
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
        // Wait for the most to run at once, so they overlap however
        // slowly they start; the calls past them never see that many.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while self.now.lock().unwrap().0 < MAX_RUNNING
            && tokio::time::Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        self.now.lock().unwrap().0 -= 1;
        Ok(ToolOutput::text("waited"))
    }
}

/// Two more sub-agents in one batch than may run at once: the most
/// start and end, and the two past them are refused.
#[test]
fn at_most_max_running_sub_agents_run_at_once() {
    let home = tempfile::tempdir().unwrap();
    let project = project_with(home.path(), &[("README.md", "hello\n")]);
    runtime().block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let llm = ScriptedModel::new()
            .turn(|t| {
                let tasks: Vec<Value> = (0..MAX_RUNNING + 2)
                    .map(|n| json!({ "task": format!("{n}") }))
                    .collect();
                spawn_all(t, &tasks)
            })
            .turn(|t| t.text("done"));
        let parent = caller(&project);
        let gauge = Gauge::default();
        let child_gauge = gauge.clone();
        let (agents, mut ends) = heard_sub_agents();
        let outcome =
            spawning(llm.clone(), &parent, &agents, move |workspace| {
                let script = ScriptedModel::new()
                    .turn(|t| t.tool_call("gauge", json!({})))
                    .turn(|t| t.text("waited"));
                Ok(coder(script, &workspace, true).tool(child_gauge.clone()))
            })
            .run("fan out", &store)
            .await
            .unwrap();
        assert_eq!(outcome.text, "done");
        let results = format!("{:?}", llm.requests()[1].transcript);
        assert_eq!(
            results.matches("started").count(),
            MAX_RUNNING,
            "{results}"
        );
        assert_eq!(
            results
                .matches(&format!(
                    "{MAX_RUNNING} sub-agents are running already"
                ))
                .count(),
            2,
            "{results}"
        );
        for _ in 0..MAX_RUNNING {
            let (_, ending) = ends.recv().await.unwrap();
            assert!(matches!(ending, Ending::Done { .. }), "{ending:?}");
        }
        let (running, most) = *gauge.now.lock().unwrap();
        assert_eq!(running, 0);
        assert_eq!(most, MAX_RUNNING);
    });
}

/// A call's model and effort reach the host's builder; left out, they
/// stay the caller's.
#[test]
fn a_call_can_pick_its_model_and_effort() {
    let home = tempfile::tempdir().unwrap();
    let project = project_with(home.path(), &[("README.md", "hello\n")]);
    runtime().block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let llm = ScriptedModel::new()
            .turn(|t| {
                spawn_all(t, &[json!({ "task": "a", "model": "gpt-5.5-mini", "effort": "low" })])
            })
            .turn(|t| spawn_all(t, &[json!({ "task": "b" })]))
            .turn(|t| t.text("done"));
        let parent = caller(&project);
        let asked = Arc::new(Mutex::new(Vec::new()));
        let seen = asked.clone();
        let (agents, mut ends) = heard_sub_agents();
        let agent = coder(llm.clone(), &parent, true).tool(Spawn::new(
            parent.clone(),
            Identity::default(),
            agents,
            &["gpt-5.5".to_owned(), "gpt-5.5-mini".to_owned()],
            ready_child(move |workspace, model: &ChildModel| {
                seen.lock().unwrap().push(model.clone());
                let script = ScriptedModel::new().turn(|t| t.text("ok"));
                Ok(coder(script, &workspace, true))
            }),
        ));
        let outcome = agent.run("pick", &store).await.unwrap();
        assert_eq!(outcome.text, "done");
        for _ in 0..2 {
            ends.recv().await.unwrap();
        }
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
/// any run at a limit does (ADR 0014), and ends ready to land like one
/// that finished, with the limit that cut it short.
#[test]
fn a_sub_agent_at_a_limit_ends_with_its_work_committed() {
    let home = tempfile::tempdir().unwrap();
    let project = project_with(home.path(), &[("README.md", "hello\n")]);
    runtime().block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let llm = ScriptedModel::new()
            .turn(|t| spawn_all(t, &[json!({ "task": "write" })]))
            .turn(|t| t.text("done"));
        let parent = caller(&project);
        let (agents, mut ends) = heard_sub_agents();
        let outcome = spawning(llm.clone(), &parent, &agents, |workspace| {
            let script = ScriptedModel::new()
                .turn(|t| {
                    t.tool_call("write", write("child.txt"))
                        .tool_call("vcs_commit", commit("feat: child"))
                })
                .turn(|t| t.text("never asked"));
            Ok(coder(script, &workspace, true).limits(Limits {
                max_turns: Some(1),
                ..Limits::default()
            }))
        })
        .run("hand a write over", &store)
        .await
        .unwrap();
        assert_eq!(outcome.text, "done");
        let (run, ending) = ends.recv().await.unwrap();
        assert!(
            matches!(
                ending,
                Ending::Done {
                    limit: Some(LimitKind::Turns),
                    ..
                }
            ),
            "{ending:?}"
        );
        assert_eq!(stack_of(&project, &run), ["feat: child"]);
        let dir = workspace_of(&store, &project, &run).await;
        assert!(dir.join("child.txt").exists());
        assert!(!parent.dir().join("child.txt").exists());
    });
}

/// `spawn` answers at once: the caller's turn ends while its sub-agent
/// still works, and the sub-agent outlives it. Ended, it waits to be
/// taken, once, by whoever lands it.
#[test]
fn a_sub_agent_outlives_its_callers_turn() {
    let home = tempfile::tempdir().unwrap();
    let project = project_with(home.path(), &[("README.md", "hello\n")]);
    runtime().block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let parent = caller(&project);
        let agents = SubAgents::default();
        // The sub-agent holds until the caller's run has ended.
        let gate = Arc::new(tokio::sync::Notify::new());
        let child_gate = gate.clone();
        let child = move |workspace: RunWorkspace| {
            let script = ScriptedModel::new()
                .turn(|t| t.tool_call("hold", json!({})))
                .turn(|t| t.tool_call("write", write("child.txt")))
                .turn(|t| t.tool_call("vcs_commit", commit("feat: child")))
                .turn(|t| t.text("child.txt is written"));
            Ok(coder(script, &workspace, true)
                .tool(Hold(child_gate.clone())))
        };
        let first = ScriptedModel::new()
            .turn(|t| t.tool_call("spawn", json!({ "task": "write child.txt" })))
            .turn(|t| t.text("started"));
        let outcome = spawning(first, &parent, &agents, child)
            .run("hand child.txt over", &store)
            .await
            .unwrap();
        assert_eq!(outcome.text, "started");
        let running = agents.running();
        assert_eq!(running.len(), 1, "it still runs");
        assert_eq!(agents.take(&running[0]), Taken::Running);
        gate.notify_one();
        let ending = agents.ended(&running[0]).await.unwrap();
        assert!(
            matches!(&ending, Ending::Done { text, limit: None } if text == "child.txt is written"),
            "{ending:?}"
        );
        // Ended, it waits to be taken: nothing reached the caller.
        assert!(!parent.dir().join("child.txt").exists());
        assert_eq!(agents.taken(&running[0]), Some(false));
        assert_eq!(agents.take(&running[0]), Taken::Now(ending.clone()));
        assert_eq!(agents.take(&running[0]), Taken::Before(ending));
        assert_eq!(agents.taken(&running[0]), Some(true));
        assert_eq!(stack_of(&project, &running[0]), ["feat: child"]);
    });
}

/// A sub-agent whose end went by reads nothing more: whoever hears it
/// can no longer steer it, though its work is still being checked.
#[test]
fn a_sub_agent_that_ended_cannot_be_steered() {
    let home = tempfile::tempdir().unwrap();
    let project = project_with(home.path(), &[("README.md", "hello\n")]);
    runtime().block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let parent = caller(&project);
        let (sink, mut events) = tokio::sync::mpsc::unbounded_channel();
        let agents = SubAgents::new(Some(sink), None);
        let child = |workspace: RunWorkspace| {
            let script = ScriptedModel::new()
                .turn(|t| t.tool_call("write", write("child.txt")))
                .turn(|t| t.tool_call("vcs_commit", commit("feat: child")))
                .turn(|t| t.text("child.txt is written"));
            Ok(coder(script, &workspace, true))
        };
        let first = ScriptedModel::new()
            .turn(|t| {
                t.tool_call("spawn", json!({ "task": "write child.txt" }))
            })
            .turn(|t| t.text("started"));
        spawning(first, &parent, &agents, child)
            .run("hand child.txt over", &store)
            .await
            .unwrap();
        let ended = loop {
            match events.recv().await.unwrap() {
                RunEvent::RunEnd { run, .. } if agents.task(&run).is_some() => {
                    break run;
                }
                _ => {}
            }
        };
        assert!(agents.control(&ended).is_none());
        assert!(
            agents.is_ending(&ended) || !agents.is_running(&ended),
            "it is ending or ended"
        );
        agents.ended(&ended).await.unwrap();
        assert!(!agents.is_ending(&ended));
    });
}

/// A sub-agent the person stops is dropped: its changes, workspace and
/// bookmark go.
#[test]
fn a_stopped_sub_agent_is_dropped() {
    let home = tempfile::tempdir().unwrap();
    let project = project_with(home.path(), &[("README.md", "hello\n")]);
    runtime().block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let parent = caller(&project);
        let agents = SubAgents::default();
        let gate = Arc::new(tokio::sync::Notify::new());
        let child_gate = gate.clone();
        let child = move |workspace: RunWorkspace| {
            let script = ScriptedModel::new()
                .turn(|t| t.tool_call("write", write("child.txt")))
                .turn(|t| t.tool_call("hold", json!({})))
                .turn(|t| t.text("never"));
            Ok(coder(script, &workspace, true).tool(Hold(child_gate.clone())))
        };
        let first = ScriptedModel::new()
            .turn(|t| t.tool_call("spawn", json!({ "task": "write" })))
            .turn(|t| t.text("started"));
        let outcome = spawning(first, &parent, &agents, child)
            .run("hand it over", &store)
            .await
            .unwrap();
        let run = agents.running().pop().unwrap();
        // Stopped while it holds.
        while agents.control(&run).is_none() {
            tokio::task::yield_now().await;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(agents.stop(&run));
        assert_eq!(agents.ended(&run).await, Some(Ending::Stopped));
        assert_eq!(project.workspaces().unwrap(), ["parent"]);
        assert_eq!(
            project.bookmarks("tau/").unwrap(),
            [bookmark(&outcome.run)]
        );
        assert!(!parent.dir().join("child.txt").exists());
    });
}

/// Holds its call until notified, or until the run is cancelled.
struct Hold(Arc<tokio::sync::Notify>);

#[async_trait]
impl AgentTool for Hold {
    fn name(&self) -> &str {
        "hold"
    }
    fn description(&self) -> &str {
        "Waits."
    }
    fn parameters(&self) -> &Value {
        static SCHEMA: std::sync::LazyLock<Value> =
            std::sync::LazyLock::new(|| json!({ "type": "object" }));
        &SCHEMA
    }
    async fn call(
        &self,
        _args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        tokio::select! {
            _ = self.0.notified() => Ok(ToolOutput::text("go on")),
            _ = ctx.cancel.cancelled() => Err("cancelled".into()),
        }
    }
}

/// A sub-agent factory that builds its agent at once, as `Spawn` takes
/// one: a future that is ready.
fn ready_child(
    child: impl Fn(
        tau_vcs_host::RunWorkspace,
        &tau_vcs_host::sub_agents::ChildModel,
    )
        -> Result<tau_agent::agent::Agent, tau_agent::error::ToolError>
    + Send
    + Sync
    + 'static,
) -> impl Fn(
    tau_vcs_host::RunWorkspace,
    &tau_vcs_host::sub_agents::ChildModel,
) -> tau_vcs_host::sub_agents::ChildFuture
+ Send
+ Sync
+ 'static {
    move |workspace, model| {
        Box::pin(std::future::ready(child(workspace, model)))
    }
}
