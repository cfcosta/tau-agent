//! Delegation through the real plugins (`docs/reference/vcs.md`,
//! "Delegating to a sub-agent"; ADR 0009, 0014, 0015): a caller with
//! `Delegate` commits drawn work, or leaves it uncommitted, and calls a
//! batch of drawn sub-agents, more than [`MAX_RUNNING`] at times. Each
//! sub-agent writes files over one or two turns, commits some of it or
//! none, and then answers or fails. A gate holds each one before its
//! answer and lets them finish one at a time, in a drawn order among the
//! ones running, each once the one before has closed.
//!
//! The model is the caller's stack of changes, each a tree of jj merge
//! terms (as `tests/runs_model.rs` has them), and what the reference
//! promises:
//! - with changes in the caller's `@`, every call is refused and no
//!   sub-agent starts;
//! - each sub-agent starts on the caller's head at the call; its commits,
//!   and its leftovers committed at its end with its model's message,
//!   land on the caller in the order they finish, each rebased as jj
//!   rebases, keeping its change id; a failed one lands nothing;
//! - each result is the sub-agent's answer and `landing_note`'s line,
//!   naming the paths in conflict in the caller's new head that were not
//!   before the landing;
//! - the caller links each landed change, in order and from its
//!   sub-agent, before its turn's snapshot;
//! - afterwards only the caller's workspace and bookmark are left;
//! - the leftover commit's message is asked with the sub-agent's task.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::Path,
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_agent::{
    agent::{Agent, RunControl},
    event::StopReason,
    tool::{AgentTool, ToolCtx, ToolError, ToolOutput},
};
use tau_ai::message::{InputBlock, Message, UserContent};
use tau_store::{Entry, Store};
use tau_testing::scripted::ScriptedModel;
use tau_tools::{path::Root, plugin::CodingTools};
use tau_vcs::{
    DEFAULT_WORKSPACE,
    Delegate,
    Identity,
    Link,
    Project,
    RunWorkspace,
    VcsPlugin,
    delegate::{ChildModel, MAX_RUNNING},
    run_workspace::PLUGIN,
};

const PATHS: [&str; 3] = ["a.txt", "b.txt", "dir/c.txt"];
const VALUES: [&str; 3] = ["", "one\n", "two\n"];
/// Sub-agents in a batch, at most.
const MAX_BATCH: usize = 6;

/// A file's contents; `None` is no file.
type Val = Option<&'static str>;

/// A path's value in a tree: jj's merge terms, counted (+1 for each
/// add, -1 for each remove, zeros dropped). One value counted once is a
/// resolved file; anything else is a conflict.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Term(BTreeMap<Val, i32>);

impl Term {
    fn resolved(value: Val) -> Self {
        Self(BTreeMap::from([(value, 1)]))
    }

    fn value(&self) -> Option<Val> {
        match self.0.iter().collect::<Vec<_>>().as_slice() {
            [(value, 1)] => Some(**value),
            _ => None,
        }
    }

    /// `old`, rebased from `base` onto `onto`: jj's `onto + old - base`,
    /// resolved as `trivial_merge` does with `same-change = accept`.
    fn rebase(onto: &Term, base: &Term, old: &Term) -> Term {
        let mut counts = onto.0.clone();
        for (value, n) in &old.0 {
            *counts.entry(*value).or_default() += n;
        }
        for (value, n) in &base.0 {
            *counts.entry(*value).or_default() -= n;
        }
        counts.retain(|_, n| *n != 0);
        let positive: Vec<Val> = counts
            .iter()
            .filter(|(_, n)| **n > 0)
            .map(|(value, _)| *value)
            .collect();
        match (counts.len(), positive.as_slice()) {
            (1, [value]) | (2, [value]) => Term::resolved(*value),
            _ => Term(counts),
        }
    }
}

/// Path to value; a path missing is no file.
type Tree = BTreeMap<&'static str, Term>;

fn get(tree: &Tree, path: &'static str) -> Term {
    tree.get(path)
        .cloned()
        .unwrap_or_else(|| Term::resolved(None))
}

fn set(tree: &mut Tree, path: &'static str, term: Term) {
    if term == Term::resolved(None) {
        tree.remove(path);
    } else {
        tree.insert(path, term);
    }
}

fn rebase_tree(onto: &Tree, base: &Tree, old: &Tree) -> Tree {
    let mut tree = Tree::new();
    for path in PATHS {
        set(
            &mut tree,
            path,
            Term::rebase(&get(onto, path), &get(base, path), &get(old, path)),
        );
    }
    tree
}

fn conflicts(tree: &Tree) -> Vec<String> {
    tree.iter()
        .filter(|(_, term)| term.value().is_none())
        .map(|(path, _)| (*path).to_owned())
        .collect()
}

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

/// A project whose trunk holds `a.txt`: `one`.
fn project(home: &Path) -> Project {
    let src = home.join("src");
    std::fs::create_dir_all(&src).unwrap();
    git(&src, &["init", "--quiet"]);
    std::fs::write(src.join("a.txt"), "one\n").unwrap();
    git(&src, &["add", "a.txt"]);
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
    change_id: String,
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
            change_id: commit.change_id().reverse_hex(),
            commit_id: at.hex(),
            description: commit.description().to_owned(),
        });
        at = commit.parent_ids()[0].clone();
    }
    found.reverse();
    found
}

/// A sub-agent's script: its turns of writes, each committed or not,
/// and whether it fails instead of answering.
#[derive(Debug, Clone)]
struct Sub {
    steps: Vec<(Vec<(&'static str, &'static str)>, bool)>,
    fails: bool,
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
    let fails = tc.draw(gs::weighted_booleans(0.2));
    Sub { steps, fails }
}

/// What a sub-agent's run leaves: its commits' trees, oldest first, and
/// whether it was held once to commit.
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
    let llm = llm.turn(move |t| t.tool_call("gate", json!({ "id": index })));
    if sub.fails {
        return llm.turn(|t| t.dropped());
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
/// its gate, the one first in `rank` goes, and the next waits until its
/// workspace is gone. With `cancel`, the caller is cancelled once that
/// many went. Returns the order they went in.
async fn conduct(
    gates: Gates,
    project: Project,
    names: Arc<Mutex<HashMap<usize, String>>>,
    rank: Vec<usize>,
    cancel: Option<(usize, RunControl)>,
) -> Vec<usize> {
    let n = rank.len();
    let mut order = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(120);
    while order.len() < n {
        let running = MAX_RUNNING.min(n - order.len());
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
        // The cancel comes while the running ones wait at their gates.
        if let Some((after, control)) = &cancel
            && order.len() == *after
        {
            control.cancel();
            return order;
        }
        {
            let mut state = gates.state.lock().unwrap();
            state.waiting.remove(&next);
            state.open.insert(next);
        }
        let name = names.lock().unwrap()[&next].clone();
        loop {
            assert!(Instant::now() < deadline, "sub-agent {next} never closed");
            let project = project.clone();
            let workspaces =
                tokio::task::spawn_blocking(move || project.workspaces())
                    .await
                    .unwrap()
                    .unwrap();
            if !workspaces.contains(&name) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        order.push(next);
    }
    order
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

fn note(changes: usize, conflicts: &[String]) -> String {
    let landed = match changes {
        0 => return "[It changed no files.]".to_owned(),
        1 => "Its 1 change landed on top of yours".to_owned(),
        n => format!("Its {n} changes landed on top of yours"),
    };
    if conflicts.is_empty() {
        return format!("[{landed}.]");
    }
    format!(
        "[{landed}, with conflicts in {}: resolve their conflict markers, \
         then commit.]",
        conflicts.join(", ")
    )
}

/// A batch of sub-agents lands on the caller as the model says: in the
/// order they finish, each change rebased and keeping its change id,
/// conflicts reported, failures dropped, every sub-agent closed; a caller
/// with uncommitted work is refused.
#[hegel::test(
    test_cases = 40,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn a_batch_lands_as_the_model_says(tc: TestCase) {
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
    // The caller is cancelled once this many sub-agents have finished.
    let cancel_after: Option<usize> = if tc.draw(gs::weighted_booleans(0.4)) {
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
    let cancelled = cancel_after.is_some() && !dirty;
    if cancelled {
        tc.event("the caller is cancelled during the batch");
    }
    if subs.len() > MAX_RUNNING {
        tc.event("more sub-agents than run at once");
    }

    runtime.block_on(async {
        let store = Store::memory().await.unwrap();
        let trunk = project.trunk().unwrap();
        let caller = if main_chat {
            tc.event("the caller is the main chat");
            RunWorkspace::new(
                project.clone(),
                DEFAULT_WORKSPACE,
                Identity::default(),
            )
            .unwrap()
            .commits_to(project.trunk_name().unwrap())
        } else {
            RunWorkspace::new(project.clone(), "caller", Identity::default())
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

        // The caller's script.
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
                    "delegate",
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
        let names: Arc<Mutex<HashMap<usize, String>>> = Arc::default();
        let workspaces: Arc<Mutex<HashMap<usize, RunWorkspace>>> =
            Arc::default();
        let models: Vec<String> = (0..calls).map(|i| format!("m{i}")).collect();
        let delegate = {
            let scripts = scripts.clone();
            let gates = gates.clone();
            let names = names.clone();
            let workspaces = workspaces.clone();
            Delegate::new(
                caller.clone(),
                Identity::default(),
                &models,
                move |workspace, model: &ChildModel| {
                    let i: usize = model.model.as_deref().unwrap()[1..]
                        .parse()
                        .unwrap();
                    names.lock().unwrap().insert(i, workspace.name().to_owned());
                    workspaces.lock().unwrap().insert(i, workspace.clone());
                    Ok(coder(scripts[i].clone(), &workspace).tool(gates.clone()))
                },
            )
        };
        let run = coder(llm.clone(), &caller)
            .tool(delegate)
            .start("split the work", &store);
        let conductor = (!dirty).then(|| {
            tokio::spawn(conduct(
                gates.clone(),
                project.clone(),
                names.clone(),
                rank.clone(),
                cancel_after.map(|after| (after, run.control())),
            ))
        });
        let outcome = run.outcome().await.unwrap();
        if cancelled {
            assert_eq!(outcome.stop, StopReason::Cancelled);
        } else {
            llm.assert_exhausted();
        }
        let order = match conductor {
            Some(conductor) => conductor.await.unwrap(),
            None => Vec::new(),
        };
        if order.iter().zip(1..).any(|(i, at)| *i + 1 != at) {
            tc.event("sub-agents finish out of call order");
        }
        for &i in &order {
            scripts[i].assert_exhausted();
        }

        // The results, in call order.
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
        let results: Vec<(String, bool, Value)> = transcript
            .iter()
            .filter_map(|message| match message {
                Message::ToolResult(result) if result.tool_name == "delegate" => {
                    Some((
                        text_of(&result.content),
                        result.is_error,
                        result.details.clone().unwrap_or(Value::Null),
                    ))
                }
                _ => None,
            })
            .collect();
        assert_eq!(results.len(), calls);
        let caller_bookmark = caller.bookmark_of(&outcome.run);
        let head = project.bookmark(&caller_bookmark).unwrap().unwrap();
        let stack = walk(home.path(), &head, &trunk);
        let mut at = 0;
        let mut want_stack: Vec<(String, Tree)> = Vec::new();
        if caller_commits && !caller_writes.is_empty() {
            assert_eq!(stack[0].description.trim(), "feat: caller");
            want_stack.push(("feat: caller".to_owned(), base.clone()));
            at = 1;
        }
        let links: Vec<Link> = store
            .plugin_entries(&outcome.run.0, PLUGIN)
            .await
            .unwrap()
            .iter()
            .filter_map(|(_, body)| Link::parse(body))
            .collect();

        if dirty {
            for (text, error, _) in &results {
                assert!(*error, "{text}");
                assert!(text.contains("Commit your work with vcs_commit"), "{text}");
            }
            assert!(workspaces.lock().unwrap().is_empty(), "a sub-agent started");
            want_stack.push(("feat: caller leftover".to_owned(), base.clone()));
        } else {
            // Landings, in the order the sub-agents finished.
            let mut head_tree = base.clone();
            let mut from_links = Vec::new();
            for &i in &order {
                let (commits, held) = &planned[i];
                let (text, error, details) = &results[i];
                let workspace = workspaces.lock().unwrap()[&i].clone();
                let run = workspace.run().unwrap().0.to_string();
                if subs[i].fails {
                    tc.event("a sub-agent fails");
                    assert!(*error, "sub-agent {i} failed: {text}");
                    continue;
                }
                assert!(!*error, "sub-agent {i}: {text}");
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
                let held_before = conflicts(&head_tree);
                let mut parent = base.clone();
                for (desc, tree) in commits {
                    let new = rebase_tree(&head_tree, &parent, tree);
                    want_stack.push((desc.clone(), new.clone()));
                    parent = tree.clone();
                    head_tree = new;
                }
                // The note names only the conflicts this landing brought.
                let all = conflicts(&head_tree);
                let landed: Vec<String> = all
                    .iter()
                    .filter(|path| !held_before.contains(path))
                    .cloned()
                    .collect();
                if !landed.is_empty() {
                    tc.event("a landing conflicts");
                }
                if landed.len() < all.len() {
                    tc.event("a landing on a head already in conflict");
                }
                let answer = if *held {
                    format!("still answer {i}")
                } else {
                    format!("answer {i}")
                };
                assert_eq!(
                    *text,
                    format!("{answer}\n\n{}", note(commits.len(), &landed)),
                    "sub-agent {i}'s result"
                );
                assert_eq!(details["run"], json!(run));
                assert_eq!(details["conflicts"], json!(landed));
                let landing = &details["landing"];
                assert_eq!(landing["changes"].as_array().unwrap().len(), commits.len());
                // Its landed changes, oldest first, as the caller links them.
                let ids: Vec<String> = landing["changes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .rev()
                    .map(|c| c["change_id"].as_str().unwrap().to_owned())
                    .collect();
                for id in ids {
                    from_links.push((id, run.clone()));
                }
            }
            // Nothing lands from the sub-agents a cancel stopped.
            for (i, (text, error, _)) in results.iter().enumerate() {
                if !order.contains(&i) {
                    assert!(cancelled, "sub-agent {i} never finished");
                    assert!(*error, "sub-agent {i} after the cancel: {text}");
                }
            }
            // The caller links each landed change, in landing order, from
            // its sub-agent, then the turn's snapshot.
            let turn2: Vec<&Link> = links.iter().filter(|l| l.turn == 2).collect();
            let got: Vec<(String, String)> = turn2
                .iter()
                .filter(|l| l.from.is_some())
                .map(|l| (l.change_id.clone(), l.from.clone().unwrap()))
                .collect();
            assert_eq!(got, from_links, "the caller's links of landed changes");
            assert!(turn2.last().unwrap().snapshot, "the snapshot comes last");
            let stacked: Vec<String> =
                stack[at..].iter().map(|c| c.change_id.clone()).collect();
            let linked: Vec<String> = from_links.iter().map(|(id, _)| id.clone()).collect();
            assert_eq!(stacked, linked, "the stack is the landed changes, ids kept");
        }

        // The caller's stack: descriptions, files and conflicts.
        assert_eq!(stack.len(), want_stack.len(), "{stack:?}");
        for (change, (desc, tree)) in stack.iter().zip(&want_stack) {
            assert_eq!(change.description.trim(), desc);
            let want_conflicts: BTreeSet<String> = conflicts(tree).into_iter().collect();
            assert_eq!(
                jj_conflicts(home.path(), &change.commit_id),
                want_conflicts,
                "{desc}'s conflicts"
            );
            for path in PATHS {
                if let Some(value) = get(tree, path).value() {
                    let got = project
                        .file_at(&change.commit_id, path)
                        .unwrap()
                        .map(|(bytes, _)| String::from_utf8(bytes).unwrap());
                    assert_eq!(got.as_deref(), value, "{desc}: {path}");
                }
            }
        }

        // Every sub-agent closed: its workspace and bookmark are gone.
        let (workspaces, bookmarks) = if main_chat {
            (Vec::new(), Vec::new())
        } else {
            (vec!["caller".to_owned()], vec![caller_bookmark])
        };
        assert_eq!(project.workspaces().unwrap(), workspaces);
        assert_eq!(project.bookmarks("tau/").unwrap(), bookmarks);
    });
}
