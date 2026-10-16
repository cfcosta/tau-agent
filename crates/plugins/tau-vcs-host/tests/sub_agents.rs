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
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use common::{coder, heard_sub_agents, project_with};
use serde_json::{Value, json};
use tau_agent::{
    agent::Agent,
    event::RunEvent,
    tool::{AgentTool, RunId, ToolCtx, ToolError, ToolOutput},
};
use tau_ai::{
    message::{InputBlock, Message},
    responses::request::ReasoningEffort,
};
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
        // It read the caller's file: it worked on its code. (Its whole
        // transcript would not do: the caller's own write of the file
        // came with the fork.)
        let read = child_llm.requests()[1].transcript.last().cloned();
        let Some(Message::ToolResult(read)) = read else {
            panic!("{read:?} is no tool result")
        };
        assert_eq!(read.tool_name, "read");
        assert!(!read.is_error, "{read:?}");
        let text: Vec<&str> = read
            .content
            .iter()
            .filter_map(|block| match block {
                InputBlock::Text(text) => Some(text.text.as_str()),
                InputBlock::Image(_) => None,
            })
            .collect();
        assert_eq!(text, ["parent.txt\n"]);

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

/// Counts the calls running at once, and the most it saw. Each call
/// waits until [`MAX_RUNNING`] have run at once: the sub-agents'
/// workspaces are made one at a time, under the repository's lock, so
/// they reach the call at different times, and only that many start.
#[derive(Clone, Default)]
struct Gauge {
    now: Arc<Mutex<(usize, usize)>>,
    /// Told when the most reaches [`MAX_RUNNING`].
    reached: Arc<tokio::sync::Notify>,
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
            if now.1 >= MAX_RUNNING {
                self.reached.notify_waiters();
            }
        }
        // Wait for the most to have run at once, so they overlap however
        // slowly they start. Registered before looking, so the news is
        // not missed. The time limit only keeps a broken store from
        // hanging the test: the most it saw then falls short and fails.
        let wait = async {
            loop {
                let reached = self.reached.notified();
                tokio::pin!(reached);
                reached.as_mut().enable();
                if self.now.lock().unwrap().1 >= MAX_RUNNING {
                    break;
                }
                reached.await;
            }
        };
        let _ = tokio::time::timeout(Duration::from_secs(120), wait).await;
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
