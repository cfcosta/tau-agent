//! Sub-agents through the real plugins (`docs/reference/vcs.md`,
//! "Sub-agents: spawn"; ADR 0009, 0014, 0015, 0026, 0031): a caller
//! with `Spawn` commits drawn work, or leaves it uncommitted, spawns a
//! batch of drawn sub-agents, up to [`MAX_RUNNING`], and ends its turn.
//! Each sub-agent writes files over one or two turns, commits some of it
//! or none, and then answers, fails, or is stopped by its turn limit. A
//! gate holds each one before its answer and lets them finish one at a
//! time, in a drawn order; the person may stop the rest part way.
//!
//! The model is each stack of changes, each a tree of jj merge terms (as
//! `tests/runs_model.rs` has them), and what the reference promises:
//! - with changes in the caller's `@`, every `spawn` is refused and no
//!   sub-agent starts;
//! - the caller's turn ends while its sub-agents work, and they go on;
//! - each sub-agent starts on the caller's head at the call. One that
//!   answers, or stops at its limit, ends `Done` with its answer, its
//!   commits and its leftovers (committed at its end with a message
//!   asked with its own task) on its bookmark, its workspace kept for
//!   the host to land; nothing reaches the caller;
//! - one that fails, or that the person stops, has its workspace and
//!   bookmark dropped.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

mod common;

use std::{
    collections::{BTreeSet, HashMap},
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use common::{coder, heard_sub_agents, merge::*, project};
use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_agent::{
    event::LimitKind,
    limits::Limits,
    tool::{AgentTool, RunId, ToolCtx, ToolError, ToolOutput},
};
use tau_ai::message::{InputBlock, Message, UserContent};
use tau_store::Entry;
use tau_testing::scripted::ScriptedModel;
use tau_vcs_host::{
    DEFAULT_WORKSPACE,
    Identity,
    Link,
    RunWorkspace,
    Spawn,
    SubAgents,
    run_workspace::PLUGIN,
    sub_agents::{ChildModel, Ending, MAX_RUNNING},
};
use tokio::sync::mpsc::UnboundedReceiver;

const PATHS: [&str; 3] = ["a.txt", "b.txt", "dir/c.txt"];
const VALUES: [&str; 3] = ["", "one\n", "two\n"];
/// Sub-agents in a batch, at most: past it, `spawn` refuses, as
/// `tests/sub_agents.rs` shows.
const MAX_BATCH: usize = MAX_RUNNING;

fn with(tree: &Tree, writes: &[(&'static str, &'static str)]) -> Tree {
    let mut tree = tree.clone();
    for (path, value) in writes {
        set(&mut tree, path, Term::resolved(Some(value)));
    }
    tree
}

fn trunk_tree() -> Tree {
    Tree::from([("a.txt", Term::resolved(Some("one\n")))])
}

/// The project's repository at its head, read with jj-lib.
fn load(home: &Path) -> std::sync::Arc<jj_lib::repo::ReadonlyRepo> {
    use jj_lib::{
        config::{ConfigLayer, ConfigSource, StackedConfig},
        default_backend_factories::{
            default_backend_factories,
            default_working_copy_factories,
        },
        settings::UserSettings,
        workspace::Workspace,
    };
    let mut config = StackedConfig::with_defaults();
    let mut user = ConfigLayer::empty(ConfigSource::User);
    user.set_value("user.name", "model").unwrap();
    user.set_value("user.email", "model@localhost").unwrap();
    config.add_layer(user);
    let settings = UserSettings::from_config(config).unwrap();
    let workspace = Workspace::load(
        &settings,
        &home.join("p").join("main"),
        &default_backend_factories(),
        &default_working_copy_factories(),
    )
    .unwrap();
    pollster::block_on(workspace.repo_loader().load_at_head()).unwrap()
}

/// The paths jj holds in conflict at `commit` (a full hex id).
fn jj_conflicts(home: &Path, commit: &str) -> BTreeSet<String> {
    use jj_lib::{backend::CommitId, repo::Repo as _};
    let repo = load(home);
    let id = CommitId::try_from_hex(commit).unwrap();
    repo.store()
        .get_commit(&id)
        .unwrap()
        .tree()
        .conflicts()
        .map(|(path, _)| path.as_internal_file_string().to_owned())
        .collect()
}

/// A commit on a stack, as [`walk`] finds it.
#[derive(Debug)]
struct Walked {
    commit_id: String,
    description: String,
}

/// The commits from `head` down to `stop` (full hex ids), oldest first,
/// along first parents: the caller's stack, wherever its bookmark is.
fn walk(home: &Path, head: &str, stop: &str) -> Vec<Walked> {
    use jj_lib::{
        backend::CommitId,
        object_id::ObjectId as _,
        repo::Repo as _,
    };
    let repo = load(home);
    let mut found = Vec::new();
    let mut at = CommitId::try_from_hex(head).unwrap();
    while at.hex() != stop {
        let commit = repo.store().get_commit(&at).unwrap();
        found.push(Walked {
            commit_id: at.hex(),
            description: commit.description().to_owned(),
        });
        at = commit.parent_ids()[0].clone();
    }
    found.reverse();
    found
}

/// A sub-agent's script: its turns of writes, each committed or not,
/// and whether it fails, or is stopped by its turn limit at its gate,
/// instead of answering.
#[derive(Debug, Clone)]
struct Sub {
    steps: Vec<(Vec<(&'static str, &'static str)>, bool)>,
    fails: bool,
    limit: bool,
}
hegel::pretty_print_as_debug!(Sub);

#[hegel::composite]
fn writes(tc: &TestCase) -> Vec<(&'static str, &'static str)> {
    let paths: Vec<&'static str> =
        tc.draw(gs::subsequences(PATHS.to_vec()).max_size(2));
    paths
        .into_iter()
        .map(|path| (path, tc.draw(gs::sampled_from(VALUES.to_vec()))))
        .collect()
}

#[hegel::composite]
fn sub(tc: &TestCase) -> Sub {
    let steps = tc.draw(
        gs::vecs(hegel::tuples!(writes(), gs::booleans()))
            .min_size(1)
            .max_size(2),
    );
    let end: u8 = tc.draw(gs::integers().max_value(9));
    Sub {
        steps,
        fails: end < 2,
        limit: (2..4).contains(&end),
    }
}

/// What a sub-agent's run leaves: its commits' trees, oldest first, and
/// whether it left work to commit at its end.
fn sub_commits(sub: &Sub, base: &Tree) -> (Vec<(String, Tree)>, bool) {
    let mut commits = Vec::new();
    let mut wc = base.clone();
    let mut parent = base.clone();
    for (n, (writes, commit)) in sub.steps.iter().enumerate() {
        wc = with(&wc, writes);
        if *commit {
            commits.push((format!("feat: part {}", n + 1), wc.clone()));
            parent = wc.clone();
        }
    }
    let dirty = wc != parent;
    if dirty && !sub.fails {
        commits.push(("feat: leftover".to_owned(), wc));
    }
    (commits, dirty)
}

fn sub_script(index: usize, sub: &Sub, dirty: bool) -> ScriptedModel {
    let mut llm = ScriptedModel::new();
    for (n, (writes, commit)) in sub.steps.iter().enumerate() {
        let writes = writes.clone();
        let commit = *commit;
        llm = llm.turn(move |mut t| {
            // A turn with nothing in it reads instead.
            if writes.is_empty() && !commit {
                return t.tool_call("read", json!({ "path": "a.txt" }));
            }
            for (path, content) in writes {
                t = t.tool_call(
                    "write",
                    json!({ "path": path, "content": content }),
                );
            }
            if commit {
                t = t.tool_call(
                    "vcs_commit",
                    json!({ "message": format!("feat: part {}", n + 1) }),
                );
            }
            t
        });
    }
    let llm = llm.turn(move |t| {
        t.text(format!("at the gate {index}"))
            .tool_call("gate", json!({ "id": index }))
    });
    if sub.fails {
        return llm.turn(|t| t.dropped());
    }
    // A run at its limit is not held: what it left is committed at once.
    if sub.limit {
        return if dirty {
            llm.turn(|t| t.text("feat: leftover"))
        } else {
            llm
        };
    }
    let llm = llm.turn(move |t| t.text(format!("answer {index}")));
    if dirty {
        llm.turn(move |t| t.text(format!("still answer {index}")))
            .turn(|t| t.text("feat: leftover"))
    } else {
        llm
    }
}

/// Holds each sub-agent at its gate until the conductor lets it on.
#[derive(Clone, Default)]
struct Gates {
    state: Arc<Mutex<GateState>>,
}

#[derive(Default)]
struct GateState {
    waiting: BTreeSet<usize>,
    open: BTreeSet<usize>,
}

#[async_trait]
impl AgentTool for Gates {
    fn name(&self) -> &str {
        "gate"
    }
    fn description(&self) -> &str {
        "Waits for its turn."
    }
    fn parameters(&self) -> &Value {
        static SCHEMA: std::sync::LazyLock<Value> =
            std::sync::LazyLock::new(|| json!({ "type": "object" }));
        &SCHEMA
    }
    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let id = args["id"].as_u64().unwrap() as usize;
        self.state.lock().unwrap().waiting.insert(id);
        loop {
            if self.state.lock().unwrap().open.contains(&id) {
                return Ok(ToolOutput::text("open"));
            }
            if ctx.cancel.is_cancelled() {
                return Err("cancelled at the gate".into());
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

/// Lets the sub-agents on one at a time: once every running one waits at
/// its gate, the one first in `rank` goes, and the next waits until it
/// ended. With `stop`, the person stops the rest once that many ended.
/// Returns how each ended, in the order they ended.
async fn conduct(
    gates: Gates,
    agents: SubAgents,
    ends: &mut UnboundedReceiver<(RunId, Ending)>,
    runs: &HashMap<usize, RunId>,
    rank: Vec<usize>,
    stop: Option<usize>,
) -> Vec<(usize, Ending)> {
    let n = rank.len();
    let mut ended = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(120);
    while ended.len() < n {
        let running = n - ended.len();
        let next = loop {
            assert!(
                Instant::now() < deadline,
                "the sub-agents never reached their gates"
            );
            {
                let state = gates.state.lock().unwrap();
                if state.waiting.len() == running {
                    break *state
                        .waiting
                        .iter()
                        .min_by_key(|id| rank[**id])
                        .unwrap();
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        // The person stops the rest while they wait at their gates.
        if stop == Some(ended.len()) {
            let waiting: Vec<usize> = gates
                .state
                .lock()
                .unwrap()
                .waiting
                .iter()
                .copied()
                .collect();
            for i in &waiting {
                assert!(agents.stop(&runs[i]));
            }
            for _ in &waiting {
                let (run, ending) = ends.recv().await.unwrap();
                let i = waiting.iter().find(|i| runs[*i] == run).unwrap();
                ended.push((*i, ending));
            }
            return ended;
        }
        {
            let mut state = gates.state.lock().unwrap();
            state.waiting.remove(&next);
            state.open.insert(next);
        }
        let (run, ending) = ends.recv().await.unwrap();
        assert_eq!(run, runs[&next], "another sub-agent ended");
        ended.push((next, ending));
    }
    ended
}

fn text_of(content: &[InputBlock]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            InputBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect()
}

/// A batch of sub-agents ends as the model says: each that answers or
/// stops at its limit leaves its work committed on its bookmark, ready to
/// land, each that fails or is stopped leaves nothing, and the caller's
/// code is its own; a caller with uncommitted work is refused.
#[hegel::test(
    test_cases = 40,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn a_batch_ends_as_the_model_says(tc: TestCase) {
    let caller_writes = tc.draw(writes());
    let caller_commits = tc.draw(gs::weighted_booleans(0.7));
    // The repository's main chat, which commits on trunk in jj's own
    // workspace, or a run of its own.
    let main_chat = tc.draw(gs::booleans());
    let calls: usize =
        tc.draw(gs::integers().min_value(1).max_value(MAX_BATCH));
    let subs: Vec<Sub> =
        tc.draw(gs::vecs(sub()).min_size(calls).max_size(calls));
    let rank: Vec<usize> =
        tc.draw(gs::permutations((0..subs.len()).collect::<Vec<_>>()));
    // The person stops the rest once this many sub-agents have ended.
    let stop_after: Option<usize> = if tc.draw(gs::weighted_booleans(0.4)) {
        Some(tc.draw(gs::integers().max_value(calls - 1)))
    } else {
        None
    };

    let home = tempfile::tempdir().unwrap();
    let project = project(home.path());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let dirty =
        !caller_commits && with(&trunk_tree(), &caller_writes) != trunk_tree();
    let base = with(&trunk_tree(), &caller_writes);
    if dirty {
        tc.event("the caller has uncommitted work");
    }
    if stop_after.is_some() && !dirty {
        tc.event("the person stops sub-agents during the batch");
    }

    runtime.block_on(async {
        let store = tau_store_sqlite::memory().await.unwrap();
        let trunk = project.trunk().unwrap();
        let caller = if main_chat {
            tc.event("the caller is the main chat");
            RunWorkspace::new(
                project.clone().into(),
                DEFAULT_WORKSPACE,
                Identity::default(),
            )
            .unwrap()
            .commits_to(project.trunk_name().unwrap())
        } else {
            RunWorkspace::new(project.clone().into(), "caller", Identity::default())
                .unwrap()
        };

        // The host brings the main chat up to trunk before its turns:
        // jj's own workspace starts on the root commit.
        if main_chat {
            caller
                .vcs()
                .move_onto(trunk.clone(), project.trunk_name().unwrap(), true)
                .await
                .unwrap();
        }

        // The caller's script: its work, the batch, and the end of its
        // turn, with nothing waiting for the batch.
        let writes = caller_writes.clone();
        let mut llm = ScriptedModel::new().turn(move |mut t| {
            if writes.is_empty() {
                return t.tool_call("read", json!({ "path": "a.txt" }));
            }
            for (path, content) in writes {
                t = t.tool_call(
                    "write",
                    json!({ "path": path, "content": content }),
                );
            }
            if caller_commits {
                t = t.tool_call("vcs_commit", json!({ "message": "feat: caller" }));
            }
            t
        });
        llm = llm.turn(move |mut t| {
            for i in 0..calls {
                t = t.tool_call(
                    "spawn",
                    json!({ "task": format!("task {i}"), "model": format!("m{i}") }),
                );
            }
            t
        });
        llm = llm.turn(|t| t.text("done"));
        if dirty {
            llm = llm
                .turn(|t| t.text("still done"))
                .turn(|t| t.text("feat: caller leftover"));
        }

        // The sub-agents' scripts, by the model their call names.
        let planned: Vec<(Vec<(String, Tree)>, bool)> =
            subs.iter().map(|sub| sub_commits(sub, &base)).collect();
        let scripts: Vec<ScriptedModel> = subs
            .iter()
            .zip(&planned)
            .enumerate()
            .map(|(i, (sub, (_, dirty)))| sub_script(i, sub, *dirty))
            .collect();
        let gates = Gates::default();
        let workspaces: Arc<Mutex<HashMap<usize, RunWorkspace>>> =
            Arc::default();
        let models: Vec<String> = (0..calls).map(|i| format!("m{i}")).collect();
        let (agents, mut ends) = heard_sub_agents();
        let spawn = {
            let subs = subs.clone();
            let scripts = scripts.clone();
            let gates = gates.clone();
            let workspaces = workspaces.clone();
            Spawn::new(
                caller.clone(),
                Identity::default(),
                agents.clone(),
                &models,
                ready_child(move |workspace, model: &ChildModel| {
                    let i: usize = model.model.as_deref().unwrap()[1..]
                        .parse()
                        .unwrap();
                    workspaces.lock().unwrap().insert(i, workspace.clone());
                    let agent = coder(scripts[i].clone(), &workspace, true).tool(gates.clone());
                    if !subs[i].limit {
                        return Ok(agent);
                    }
                    // Stopped at the gate's turn.
                    Ok(agent.limits(Limits {
                        max_turns: Some(subs[i].steps.len() as u32 + 1),
                        ..Limits::default()
                    }))
                }),
            )
        };
        let outcome = coder(llm.clone(), &caller, true)
            .tool(spawn)
            .run("split the work", &store)
            .await
            .unwrap();
        llm.assert_exhausted();
        // The caller's turn ended with its sub-agents at work.
        let started: HashMap<usize, RunWorkspace> = workspaces.lock().unwrap().clone();
        let runs: HashMap<usize, RunId> = started
            .iter()
            .map(|(i, workspace)| {
                // A sub-agent's run is known once it started.
                (*i, workspace.run().unwrap())
            })
            .collect();
        let ended = if dirty {
            Vec::new()
        } else {
            conduct(gates.clone(), agents.clone(), &mut ends, &runs, rank.clone(), stop_after)
                .await
        };
        assert!(agents.running().is_empty());
        if ended.iter().zip(1..).any(|((i, _), at)| *i + 1 != at) {
            tc.event("sub-agents finish out of call order");
        }

        // The spawn results, in call order.
        let transcript: Vec<Message> = store
            .transcript(&outcome.run.0)
            .await
            .unwrap()
            .into_iter()
            .filter_map(|entry| match entry {
                Entry::Message { body, .. } => serde_json::from_str(&body).ok(),
                _ => None,
            })
            .collect();
        let spawned: Vec<(String, bool)> = transcript
            .iter()
            .filter_map(|message| match message {
                Message::ToolResult(result) if result.tool_name == "spawn" => {
                    Some((text_of(&result.content), result.is_error))
                }
                _ => None,
            })
            .collect();
        assert_eq!(spawned.len(), calls);
        let caller_bookmark = caller.bookmark_of(&outcome.run);
        let caller_head = project.bookmark(&caller_bookmark).unwrap().unwrap();

        // What each sub-agent left, as it ended.
        let mut kept_workspaces = Vec::new();
        let mut kept_bookmarks = Vec::new();
        if dirty {
            for (text, error) in &spawned {
                assert!(*error, "{text}");
                assert!(text.contains("Commit your work with vcs_commit"), "{text}");
            }
            assert!(started.is_empty(), "a sub-agent started");
        } else {
            for (text, error) in &spawned {
                assert!(!*error, "{text}");
                assert!(text.contains("started"), "{text}");
            }
            assert_eq!(started.len(), calls);
        }
        for (i, ending) in &ended {
            let i = *i;
            let (commits, held) = &planned[i];
            let run = &runs[&i];
            let name = started[&i].name().to_owned();
            let sub_bookmark = format!("tau/{}", run.0);
            if stop_after.is_some_and(|after| ended.iter().position(|(j, _)| *j == i).unwrap() >= after) {
                assert_eq!(*ending, Ending::Stopped, "sub-agent {i}");
                continue;
            }
            scripts[i].assert_exhausted();
            if subs[i].fails {
                tc.event("a sub-agent fails");
                assert!(matches!(ending, Ending::Failed { .. }), "sub-agent {i}: {ending:?}");
                continue;
            }
            let limit = subs[i].limit;
            let answer = if limit {
                tc.event("a sub-agent stops at its limit");
                format!("at the gate {i}")
            } else if *held {
                format!("still answer {i}")
            } else {
                format!("answer {i}")
            };
            assert_eq!(
                *ending,
                Ending::Done {
                    text: answer,
                    limit: limit.then_some(LimitKind::Turns),
                },
                "sub-agent {i}"
            );
            if *held {
                tc.event("a sub-agent leaves work to commit");
                // The leftover's message is asked with its own task.
                let asked = scripts[i].requests().last().unwrap().transcript.clone();
                let Some(Message::User(user)) = asked.first() else {
                    panic!("no question: {asked:?}");
                };
                let UserContent::Text(question) = &user.content else {
                    panic!("{user:?}");
                };
                assert!(
                    question.contains(&format!("<task>\ntask {i}\n</task>")),
                    "sub-agent {i}'s leftover was described from {question:?}"
                );
            }
            // Its bookmark holds its commits on the caller's head, ready
            // to land.
            let head = project.bookmark(&sub_bookmark).unwrap().unwrap();
            let stack = walk(home.path(), &head, &caller_head);
            assert_eq!(stack.len(), commits.len(), "sub-agent {i}: {stack:?}");
            for (change, (desc, tree)) in stack.iter().zip(commits) {
                assert_eq!(change.description.trim(), desc, "sub-agent {i}");
                assert!(jj_conflicts(home.path(), &change.commit_id).is_empty());
                for path in PATHS {
                    if let Some(value) = get(tree, path).value() {
                        let got = project
                            .file_at(&change.commit_id, path)
                            .unwrap()
                            .map(|(bytes, _)| String::from_utf8(bytes).unwrap());
                        assert_eq!(got.as_deref(), value, "sub-agent {i}, {desc}: {path}");
                    }
                }
            }
            kept_workspaces.push(name);
            kept_bookmarks.push(sub_bookmark);
        }

        // The caller's stack is its own work: nothing landed on it.
        let stack = walk(home.path(), &caller_head, &trunk);
        let mut want: Vec<&str> = Vec::new();
        if caller_commits && !caller_writes.is_empty() {
            want.push("feat: caller");
        }
        if dirty {
            want.push("feat: caller leftover");
        }
        let got: Vec<&str> = stack.iter().map(|c| c.description.trim()).collect();
        assert_eq!(got, want);
        let links: Vec<Link> = store
            .plugin_entries(&outcome.run.0, PLUGIN)
            .await
            .unwrap()
            .iter()
            .filter_map(|(_, body)| Link::parse(body))
            .collect();
        assert!(links.iter().all(|link| link.from.is_none()), "nothing landed");

        // What is left: the caller's, and each sub-agent's ready to land.
        let mut workspaces = if main_chat {
            Vec::new()
        } else {
            vec!["caller".to_owned()]
        };
        workspaces.extend(kept_workspaces);
        workspaces.sort();
        let mut bookmarks = if main_chat {
            Vec::new()
        } else {
            vec![caller_bookmark]
        };
        bookmarks.extend(kept_bookmarks);
        bookmarks.sort();
        let mut got = project.workspaces().unwrap();
        got.sort();
        assert_eq!(got, workspaces);
        let mut got = project.bookmarks("tau/").unwrap();
        got.sort();
        assert_eq!(got, bookmarks);
    });
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
