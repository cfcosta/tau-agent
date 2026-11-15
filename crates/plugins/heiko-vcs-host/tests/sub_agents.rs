//! The `spawn` tool (ADR 0009, 0026, 0031): a sub-agent works on the
//! caller's code beside it, the caller's turn goes on without it, and
//! it ends with its work committed on its own stack, for the host to
//! land, or dropped.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use common::{
    Workspace,
    caller_in,
    coder_in,
    heard_sub_agents,
    notes,
    project_with,
    ready_child,
};
use hegel::generators as gs;
use heiko_agent::{
    agent::Agent,
    event::RunEvent,
    tool::{AgentTool, RunId, ToolCtx, ToolError, ToolOutput},
};
use heiko_ai::{
    message::{InputBlock, Message},
    responses::request::ReasoningEffort,
};
use heiko_testing::scripted::{ScriptedModel, TurnBuilder};
use heiko_ui_plugin::{
    sub_agents::{ChildModel, Ending, MAX_RUNNING, Spawn, SubAgents, Taken},
    vcs::{Link, WORKSPACE_PLUGIN},
};
use heiko_vcs_host::{Head, ProjectRepo};
use serde_json::{Value, json};

use crate::common;

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
    workspace: &Workspace,
    agents: &SubAgents,
    child: impl Fn(Workspace) -> Result<Agent, ToolError> + Send + Sync + 'static,
) -> Agent {
    coder_in(llm, workspace.as_ref()).tool(Spawn::new(
        workspace.clone(),
        agents.clone(),
        &notes(),
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

fn caller(project: &ProjectRepo) -> Workspace {
    caller_in(project, "parent")
}

/// The workspace `run` worked in, as its last link names it.
async fn workspace_of(
    store: &heiko_store::Store,
    project: &ProjectRepo,
    run: &RunId,
) -> PathBuf {
    let link = store
        .plugin_entries(&run.0, WORKSPACE_PLUGIN)
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
    let head = project.head(&Head::Run(run.clone())).unwrap().unwrap();
    project
        .stack(&head)
        .unwrap()
        .into_iter()
        .map(|change| change.description.trim().to_owned())
        .collect()
}

/// What each `spawn` call in `transcript`'s last batch got back, in the
/// calls' order, matched by call id: the run it started, or why it was
/// refused.
fn spawned(transcript: &[Message]) -> Vec<Result<RunId, String>> {
    let calls: Vec<&str> = transcript
        .iter()
        .rev()
        .find_map(|message| match message {
            Message::Assistant(turn) => Some(turn),
            _ => None,
        })
        .expect("a turn")
        .tool_calls()
        .filter(|call| call.name == "spawn")
        .map(|call| call.id.as_str())
        .collect();
    calls
        .into_iter()
        .map(|id| {
            let result = transcript
                .iter()
                .find_map(|message| match message {
                    Message::ToolResult(result)
                        if result.tool_call_id == id =>
                    {
                        Some(result)
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("no result for {id}"));
            let text: String = result
                .content
                .iter()
                .filter_map(InputBlock::as_text)
                .collect();
            if result.is_error {
                return Err(text);
            }
            let run = result
                .details
                .as_ref()
                .and_then(|details| details["run"].as_str())
                .unwrap_or_else(|| panic!("{id} started no run: {text}"));
            assert!(
                text.contains(&format!("Sub-agent {run} started")),
                "{text}"
            );
            Ok(RunId(run.into()))
        })
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
        let store = heiko_store_sqlite::memory().await.unwrap();
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
                Ok(coder_in(script.clone(), workspace.as_ref()))
            })
            .run("write parent.txt, then hand child.txt over", &store)
            .await
            .unwrap();
        assert_eq!(outcome.text, "done");
        let (run, ending) = ends.recv().await.unwrap();
        // The caller heard it started, as the run that ended, and went
        // on.
        assert_eq!(spawned(&llm.requests()[3].transcript), [Ok(run.clone())]);
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
            .plugin_entries(&outcome.run.0, WORKSPACE_PLUGIN)
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
        let store = heiko_store_sqlite::memory().await.unwrap();
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
        let [Err(refused)] = &spawned(&llm.requests()[1].transcript)[..] else {
            panic!("one refused call")
        };
        assert!(refused.contains("no model for sub-agents"), "{refused}");
        assert!(agents.running().is_empty());
        assert_eq!(project.workspaces().unwrap(), ["parent"]);
    });
}

/// Counts the calls running at once, and the most it saw. Each call
/// waits until `target` have run at once: the sub-agents' workspaces
/// are made one at a time, under the repository's lock, so they reach
/// the call at different times, and only waiting makes them overlap.
#[derive(Clone)]
struct Gauge {
    target: usize,
    now: Arc<Mutex<(usize, usize)>>,
    /// Told when the most reaches `target`.
    reached: Arc<tokio::sync::Notify>,
}

impl Gauge {
    fn new(target: usize) -> Self {
        Self {
            target,
            now: Arc::default(),
            reached: Arc::default(),
        }
    }
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
            if now.1 >= self.target {
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
                if self.now.lock().unwrap().1 >= self.target {
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

/// Some number of sub-agents in one batch, up to a few past the most
/// that may run at once: as many as may run start, each a run of its
/// own, and run side by side; the rest are refused, each told why; and
/// the runs that end are exactly those that started.
#[hegel::test(test_cases = 16)]
fn at_most_max_running_sub_agents_run_at_once(tc: hegel::TestCase) {
    let asked = tc.draw(
        gs::integers::<usize>()
            .min_value(1)
            .max_value(MAX_RUNNING + 3),
    );
    fan_out(asked);
}

/// Two past the most, the case the limit is for.
#[test]
fn two_sub_agents_past_the_most_are_refused() {
    fan_out(MAX_RUNNING + 2);
}

/// Spawns `asked` sub-agents in one batch, and checks what each call got
/// back against the limit, and the gauge's peak.
fn fan_out(asked: usize) {
    let home = tempfile::tempdir().unwrap();
    let project = project_with(home.path(), &[("README.md", "hello\n")]);
    let starts = asked.min(MAX_RUNNING);
    runtime().block_on(async {
        let store = heiko_store_sqlite::memory().await.unwrap();
        let llm = ScriptedModel::new()
            .turn(|t| {
                let tasks: Vec<Value> = (0..asked)
                    .map(|n| json!({ "task": format!("{n}") }))
                    .collect();
                spawn_all(t, &tasks)
            })
            .turn(|t| t.text("done"));
        let parent = caller(&project);
        let gauge = Gauge::new(starts);
        let child_gauge = gauge.clone();
        let (agents, mut ends) = heard_sub_agents();
        let outcome =
            spawning(llm.clone(), &parent, &agents, move |workspace| {
                let script = ScriptedModel::new()
                    .turn(|t| t.tool_call("gauge", json!({})))
                    .turn(|t| t.text("waited"));
                Ok(coder_in(script, workspace.as_ref())
                    .tool(child_gauge.clone()))
            })
            .run("fan out", &store)
            .await
            .unwrap();
        assert_eq!(outcome.text, "done");
        let results = spawned(&llm.requests()[1].transcript);
        assert_eq!(results.len(), asked);
        let mut started: Vec<RunId> =
            results.iter().filter_map(|r| r.clone().ok()).collect();
        let refused: Vec<String> =
            results.into_iter().filter_map(Result::err).collect();
        assert_eq!(started.len(), starts, "{refused:?}");
        let full = format!("{MAX_RUNNING} sub-agents are running already");
        for reason in &refused {
            assert!(reason.contains(&full), "{reason}");
        }
        let mut ended = Vec::new();
        for _ in 0..starts {
            let (run, ending) = ends.recv().await.unwrap();
            assert!(matches!(ending, Ending::Done { .. }), "{ending:?}");
            ended.push(run);
        }
        started.sort();
        ended.sort();
        started.dedup();
        assert_eq!(started.len(), starts, "a run started twice");
        assert_eq!(ended, started);
        let (running, most) = *gauge.now.lock().unwrap();
        assert_eq!(running, 0);
        assert_eq!(most, starts);
    });
}

/// A call's model and effort reach the host's builder; left out, they
/// stay the caller's.
#[test]
fn a_call_can_pick_its_model_and_effort() {
    let home = tempfile::tempdir().unwrap();
    let project = project_with(home.path(), &[("README.md", "hello\n")]);
    runtime().block_on(async {
        let store = heiko_store_sqlite::memory().await.unwrap();
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
        let agent = coder_in(llm.clone(), parent.as_ref()).tool(Spawn::new(
            parent.clone(),
            agents,
            &notes(),
            &["gpt-5.5".to_owned(), "gpt-5.5-mini".to_owned()],
            ready_child(move |workspace, model: &ChildModel| {
                seen.lock().unwrap().push(model.clone());
                let script = ScriptedModel::new().turn(|t| t.text("ok"));
                Ok(coder_in(script, workspace.as_ref()))
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
        let store = heiko_store_sqlite::memory().await.unwrap();
        let parent = caller(&project);
        let agents = SubAgents::default();
        // The sub-agent holds until the caller's run has ended.
        let gate = Arc::new(tokio::sync::Notify::new());
        let child_gate = gate.clone();
        let child = move |workspace: Workspace| {
            let script = ScriptedModel::new()
                .turn(|t| t.tool_call("hold", json!({})))
                .turn(|t| t.tool_call("write", write("child.txt")))
                .turn(|t| t.tool_call("vcs_commit", commit("feat: child")))
                .turn(|t| t.text("child.txt is written"));
            Ok(coder_in(script, workspace.as_ref())
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
        let store = heiko_store_sqlite::memory().await.unwrap();
        let parent = caller(&project);
        let (sink, mut events) = tokio::sync::mpsc::unbounded_channel();
        let agents = SubAgents::new(Some(sink), None);
        let child = |workspace: Workspace| {
            let script = ScriptedModel::new()
                .turn(|t| t.tool_call("write", write("child.txt")))
                .turn(|t| t.tool_call("vcs_commit", commit("feat: child")))
                .turn(|t| t.text("child.txt is written"));
            Ok(coder_in(script, workspace.as_ref()))
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

/// Stop pressed as a sub-agent's run ends, while its work is not yet
/// checked, is heard: the sub-agent is stopped and its changes dropped,
/// as Stop says, not landed. Once its work is being checked, Stop says
/// it stopped nothing.
#[test]
fn stop_as_a_sub_agent_ends_drops_its_work() {
    let home = tempfile::tempdir().unwrap();
    let project = project_with(home.path(), &[("README.md", "hello\n")]);
    runtime().block_on(async {
        let store = heiko_store_sqlite::memory().await.unwrap();
        let parent = caller(&project);
        let (sink, mut events) = tokio::sync::mpsc::unbounded_channel();
        let agents = SubAgents::new(Some(sink), None);
        let child = |workspace: Workspace| {
            let script = ScriptedModel::new()
                .turn(|t| t.tool_call("write", write("child.txt")))
                .turn(|t| t.tool_call("vcs_commit", commit("feat: child")))
                .turn(|t| t.text("child.txt is written"));
            Ok(coder_in(script, workspace.as_ref()))
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
        let stopped = agents.stop(&ended);
        let ending = agents.ended(&ended).await.unwrap();
        if stopped {
            assert_eq!(ending, Ending::Stopped);
            assert_eq!(project.workspaces().unwrap(), ["parent"]);
        } else {
            assert!(matches!(ending, Ending::Done { .. }), "{ending:?}");
        }
        // Once it ended, there is nothing to stop.
        assert!(!agents.stop(&ended));
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

/// A fresh sub-agent starts from its task alone: none of the caller's
/// conversation, nor the note that it runs one of the caller's calls.
/// Spawned not to land, one that commits nothing, as a reviewer, ends
/// kept with no changes.
#[test]
fn a_fresh_sub_agent_sees_only_its_task() {
    let home = tempfile::tempdir().unwrap();
    let project = project_with(home.path(), &[("README.md", "hello\n")]);
    runtime().block_on(async {
        let store = heiko_store_sqlite::memory().await.unwrap();
        let task = "Review README.md against its spec; do not edit files.";
        let llm = ScriptedModel::new()
            .turn(|t| {
                spawn_all(
                    t,
                    &[json!({ "task": task, "fresh": true, "land": false })],
                )
            })
            .turn(|t| t.text("done"));
        let child_llm = ScriptedModel::new().turn(|t| t.text("PASS"));
        let parent = caller(&project);
        let (agents, mut ends) = heard_sub_agents();
        let script = child_llm.clone();
        spawning(llm.clone(), &parent, &agents, move |workspace| {
            Ok(coder_in(script.clone(), workspace.as_ref()))
        })
        .run(
            "the caller's own plan, which a reviewer must not see",
            &store,
        )
        .await
        .unwrap();

        let (_, ending) = ends.recv().await.unwrap();
        assert_eq!(
            ending,
            Ending::Kept {
                text: "PASS".into(),
                limit: None,
                changes: Vec::new(),
            }
        );
        let asked = format!("{:?}", child_llm.requests()[0].transcript);
        assert!(asked.contains(task), "{asked}");
        assert!(!asked.contains("the caller's own plan"), "{asked}");
        assert!(!asked.contains("You are the sub-agent"), "{asked}");
        let started = format!("{:?}", llm.requests()[1].transcript);
        assert!(started.contains("kept beside yours"), "{started}");
    });
}

/// Spawned not to land, a sub-agent's commits stay beside the caller's:
/// its workspace closes, its bookmark keeps its work, the caller has
/// none of it, and the caller can take its files from the change it
/// names.
#[test]
fn a_sub_agent_not_to_land_keeps_its_work_beside_the_callers() {
    let home = tempfile::tempdir().unwrap();
    let project = project_with(home.path(), &[("README.md", "hello\n")]);
    runtime().block_on(async {
        let store = heiko_store_sqlite::memory().await.unwrap();
        let llm = ScriptedModel::new()
            .turn(|t| {
                spawn_all(
                    t,
                    &[json!({ "task": "write child.txt", "land": false })],
                )
            })
            .turn(|t| t.text("done"));
        let child_llm = ScriptedModel::new()
            .turn(|t| t.tool_call("write", write("child.txt")))
            .turn(|t| t.tool_call("vcs_commit", commit("feat: child")))
            .turn(|t| t.text("child.txt is written"));
        let parent = caller(&project);
        let (agents, mut ends) = heard_sub_agents();
        let script = child_llm.clone();
        let outcome =
            spawning(llm.clone(), &parent, &agents, move |workspace| {
                Ok(coder_in(script.clone(), workspace.as_ref()))
            })
            .run("try child.txt", &store)
            .await
            .unwrap();

        let (run, ending) = ends.recv().await.unwrap();
        let Ending::Kept {
            text,
            limit: None,
            changes,
        } = ending
        else {
            panic!("{ending:?} is not kept");
        };
        assert_eq!(text, "child.txt is written");
        // Its one change is on its bookmark, past the caller's head, and
        // its workspace is gone.
        let head = project.head(&Head::Run(run.clone())).unwrap().unwrap();
        let stack = project.stack(&head).unwrap();
        let ids: Vec<&str> = stack
            .iter()
            .map(|change| change.change_id.as_str())
            .collect();
        assert_eq!(ids, changes.iter().map(String::as_str).collect::<Vec<_>>());
        assert_eq!(stack[0].description.trim(), "feat: child");
        assert!(
            project
                .workspaces()
                .unwrap()
                .iter()
                .all(|name| { !name.starts_with("parent-sub-") })
        );
        assert!(!parent.dir().join("child.txt").exists());
        assert_eq!(outcome.text, "done");

        // The caller takes the file from the change the report names.
        let from = changes[0].clone();
        let taker = ScriptedModel::new()
            .turn(move |t| {
                t.tool_call(
                    "vcs_restore",
                    json!({ "paths": ["child.txt"], "from": from }),
                )
            })
            .turn(|t| t.text("took it"));
        coder_in(taker, parent.as_ref())
            .run("take child.txt", &store)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(parent.dir().join("child.txt")).unwrap(),
            "child.txt\n"
        );
    });
}
