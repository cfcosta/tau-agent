//! tau closing at any point of a multi-step write (ADR 0009, 0024, 0026),
//! against a model of what the person did: chats start and take
//! messages, land through main's queue or at once, or are dropped, main
//! commits and hands work to sub-agents that land as they end, and trunk
//! takes what came upstream.
//!
//! Its `crash` rule closes tau in the middle of one of those: the store
//! and the project stand behind a [`Fuse`] that lets a drawn number of
//! writes through, then fails that one and every write after it, as if
//! the process had died there. The host is dropped with its runtime, and
//! a new one opens the same store, project and plugin directories, and
//! starts as tau does: it finishes what it can, lands what waits, and
//! brings in what came upstream. jj's operations are atomic, so the fuse
//! sits between them: after a landing's restack and before its record,
//! between forgetting a workspace and removing a head, and so on.
//! [`every_write_is_a_place_to_close`] walks every write of each step
//! the same way, one after the other.
//!
//! What a start must leave, whatever was cut:
//! - every message the person sent that a turn took is in its chat;
//! - every chat is open, landed or dropped, and the store's word on it
//!   and the project agree: a landed chat's commits are on main's line
//!   once (trunk, or main's stack an update left behind it), and its head
//!   is gone; a dropped one's are nowhere; an open one keeps its head;
//! - nothing lands twice, and no workspace serves two runs;
//! - a sub-agent that finished lands on main, or waits in its queue;
//! - the queue is what its records give, and a second start finds
//!   nothing to finish and shows the same;
//! - every chat open takes a message again, its tools working.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use futures_util::{FutureExt as _, future::BoxFuture};
use hegel::{TestCase, generators as gs};
use tau_agent::{agent::Agent, event::RunEvent, tool::RunId};
use tau_ai::{
    llm::{EventStream, Llm, LlmError, LlmSession},
    message::{Message, Timestamp},
    responses::request::Settings,
};
use tau_store::{
    AgentCost,
    Backend,
    Entry,
    NewRun,
    PluginCost,
    RunKind,
    RunRecord,
    Status,
    Store,
    StoreError,
    Table,
    TurnUsage,
};
use tau_testing::{git::git, scripted::ScriptedModel};
use tau_ui::{
    accounts::Credentials,
    host::{
        DrainReport,
        Host,
        HostConfig,
        QUEUE_PLUGIN,
        queue::{Lane, Record},
    },
};
use tau_ui_plugin::{
    registry::HostFuture,
    vcs::{
        After,
        Discard,
        FileDiff,
        Held,
        Land,
        Landing,
        LandingRecord,
        Link,
        Newest,
        Owner,
        PushError,
        Pushed,
        Remote,
        Replayed,
        Repository,
        RunSpec,
        RunWorkspace,
        StackChange,
        Updated,
    },
};
use tau_ui_remote::{
    models::ModelChoice,
    view::{Ending, LANDING_RECORD},
};
use tau_vcs_host::{Head, Identity, JjRepository, Project, ProjectRepo};
use tokio::sync::mpsc::UnboundedReceiver;

const REPO: &str = "repo";
/// How long a run, or a host settling, has before the test fails.
const WAIT: Duration = Duration::from_secs(60);
/// What a write fails with once tau has closed.
const CLOSED: &str = "tau closed";

/// Where tau closes: the fuse lets `left` writes through, then fails
/// the next and every one after it. Disarmed, every write goes through.
#[derive(Default)]
struct Fuse(Mutex<FuseState>);

#[derive(Default)]
struct FuseState {
    left: Option<usize>,
    /// It blows at the write called this, after letting that many of
    /// them through.
    at: Option<(String, usize)>,
    /// The write it blew at.
    blown: Option<String>,
    /// The writes since it was armed.
    counted: usize,
}

impl Fuse {
    fn arm(&self, left: usize) {
        let mut state = self.0.lock().unwrap();
        state.left = Some(left);
        state.counted = 0;
    }

    /// Arms it to blow at the write `what`, once `skip` of them went
    /// through.
    fn arm_at(&self, what: &str, skip: usize) {
        self.0.lock().unwrap().at = Some((what.to_owned(), skip));
    }

    /// Disarms it: how many writes went through since it was armed.
    fn disarm(&self) -> usize {
        let mut state = self.0.lock().unwrap();
        state.left = None;
        state.counted
    }

    fn blown(&self) -> Option<String> {
        self.0.lock().unwrap().blown.clone()
    }

    /// A write, called `what`, about to happen.
    fn write(&self, what: &str) -> anyhow::Result<()> {
        let mut state = self.0.lock().unwrap();
        if state.blown.is_some() {
            anyhow::bail!(CLOSED);
        }
        state.counted += 1;
        if let Some((at, skip)) = &mut state.at
            && at == what
        {
            if *skip == 0 {
                state.blown = Some(what.to_owned());
                anyhow::bail!(CLOSED)
            }
            *skip -= 1;
        }
        match state.left {
            Some(0) => {
                state.blown = Some(what.to_owned());
                anyhow::bail!(CLOSED)
            }
            Some(left) => state.left = Some(left - 1),
            None => {}
        }
        Ok(())
    }
}

/// The store behind a [`Fuse`]: reads always go through.
#[derive(Clone)]
struct FusedStore {
    inner: Store,
    fuse: Arc<Fuse>,
}

impl std::fmt::Debug for FusedStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FusedStore")
    }
}

impl FusedStore {
    fn write(&self, what: &str) -> tau_store::Result<()> {
        self.fuse
            .write(what)
            .map_err(|_| StoreError::Database(CLOSED.into()))
    }
}

#[async_trait]
impl Backend for FusedStore {
    async fn query(&self, sql: &str, limit: usize) -> tau_store::Result<Table> {
        self.inner.query(sql, limit).await
    }

    async fn create_run(&self, run: &NewRun<'_>) -> tau_store::Result<()> {
        self.write("create_run")?;
        self.inner.create_run(run).await
    }

    async fn append_charged(
        &self,
        run: &str,
        entries: &[Entry],
        usage: TurnUsage,
        plugins: &[(&str, TurnUsage)],
    ) -> tau_store::Result<i64> {
        let what = match entries {
            [Entry::Plugin { plugin, .. }, ..] => format!("record {plugin}"),
            [Entry::Message { role, .. }, ..] => format!("message {role}"),
            _ => "entries".to_owned(),
        };
        self.write(&what)?;
        self.inner
            .append_charged(run, entries, usage, plugins)
            .await
    }

    async fn plugin_costs(
        &self,
        run: &str,
    ) -> tau_store::Result<Vec<PluginCost>> {
        self.inner.plugin_costs(run).await
    }

    async fn charge_plugin(
        &self,
        plugin: &str,
        usage: TurnUsage,
    ) -> tau_store::Result<()> {
        self.write("charge_plugin")?;
        self.inner.charge_plugin(plugin, usage).await
    }

    async fn plugin_spend(
        &self,
        since: &str,
    ) -> tau_store::Result<Vec<(String, f64)>> {
        self.inner.plugin_spend(since).await
    }

    async fn transcript(&self, run: &str) -> tau_store::Result<Vec<Entry>> {
        self.inner.transcript(run).await
    }

    async fn first_prompt(
        &self,
        run: &str,
    ) -> tau_store::Result<Option<String>> {
        self.inner.first_prompt(run).await
    }

    async fn timeline(&self, run: &str) -> tau_store::Result<Vec<Entry>> {
        self.inner.timeline(run).await
    }

    async fn records(
        &self,
        run: &str,
        plugin: &str,
    ) -> tau_store::Result<Vec<String>> {
        self.inner.records(run, plugin).await
    }

    async fn finish_run(
        &self,
        run: &str,
        status: Status,
        result: Option<&str>,
        error: Option<&str>,
    ) -> tau_store::Result<()> {
        self.write("finish_run")?;
        self.inner.finish_run(run, status, result, error).await
    }

    async fn interrupt_running(&self) -> tau_store::Result<Vec<String>> {
        self.write("interrupt_running")?;
        self.inner.interrupt_running().await
    }

    async fn set_title(&self, run: &str, title: &str) -> tau_store::Result<()> {
        self.write("set_title")?;
        self.inner.set_title(run, title).await
    }

    async fn reopen_run(
        &self,
        run: &str,
        model: &str,
    ) -> tau_store::Result<RunRecord> {
        self.write("reopen_run")?;
        self.inner.reopen_run(run, model).await
    }

    async fn subagents(
        &self,
        parent: &str,
    ) -> tau_store::Result<Vec<RunRecord>> {
        self.inner.subagents(parent).await
    }

    async fn run(&self, run: &str) -> tau_store::Result<Option<RunRecord>> {
        self.inner.run(run).await
    }

    async fn recent_runs(
        &self,
        limit: u32,
    ) -> tau_store::Result<Vec<RunRecord>> {
        self.inner.recent_runs(limit).await
    }

    async fn retained_runs(&self) -> tau_store::Result<Vec<RunRecord>> {
        self.inner.retained_runs().await
    }

    async fn retention_entries(
        &self,
        run: &str,
    ) -> tau_store::Result<Vec<(i64, String)>> {
        self.inner.retention_entries(run).await
    }

    async fn plugin_entries(
        &self,
        run: &str,
        plugin: &str,
    ) -> tau_store::Result<Vec<(i64, String)>> {
        self.inner.plugin_entries(run, plugin).await
    }

    async fn plugin_entries_everywhere(
        &self,
        plugin: &str,
    ) -> tau_store::Result<Vec<(String, String)>> {
        self.inner.plugin_entries_everywhere(plugin).await
    }

    async fn workflow_cost(
        &self,
        workflow: &str,
    ) -> tau_store::Result<Vec<AgentCost>> {
        self.inner.workflow_cost(workflow).await
    }

    async fn spent_since(
        &self,
        runs: &[String],
        since: &str,
    ) -> tau_store::Result<f64> {
        self.inner.spent_since(runs, since).await
    }
}

/// The project behind a [`Fuse`], as the kernel asks it: each call that
/// changes it is a write, one jj job of its own or a few.
struct FusedRepository {
    inner: Arc<dyn Repository>,
    fuse: Arc<Fuse>,
}

/// A call refused because tau closed.
fn closed<'a, T: Send + 'a>(
    error: anyhow::Error,
) -> HostFuture<'a, anyhow::Result<T>> {
    Box::pin(async move { Err(error) })
}

impl Repository for FusedRepository {
    fn main_workspace(&self) -> &str {
        self.inner.main_workspace()
    }

    fn workspace_dir(&self, name: &str) -> PathBuf {
        self.inner.workspace_dir(name)
    }

    fn root(&self) -> &Path {
        self.inner.root()
    }

    fn trunk_name(&self) -> HostFuture<'_, anyhow::Result<String>> {
        self.inner.trunk_name()
    }

    fn unpushed(&self) -> HostFuture<'_, anyhow::Result<Vec<StackChange>>> {
        self.inner.unpushed()
    }

    fn run_head<'a>(
        &'a self,
        run: &'a RunId,
    ) -> HostFuture<'a, anyhow::Result<Option<String>>> {
        self.inner.run_head(run)
    }

    fn current(
        &self,
        links: Vec<Link>,
    ) -> HostFuture<'_, anyhow::Result<Vec<Link>>> {
        self.inner.current(links)
    }

    fn diff<'a>(
        &'a self,
        from: &'a str,
        to: &'a str,
    ) -> HostFuture<'a, anyhow::Result<Vec<FileDiff>>> {
        self.inner.diff(from, to)
    }

    fn update(&self, from: Remote) -> HostFuture<'_, anyhow::Result<Updated>> {
        match self.fuse.write("update") {
            Ok(()) => self.inner.update(from),
            Err(error) => closed(error),
        }
    }

    fn push_trunk(
        &self,
        to: Remote,
    ) -> HostFuture<'_, Result<Pushed, PushError>> {
        match self.fuse.write("push_trunk") {
            Ok(()) => self.inner.push_trunk(to),
            Err(error) => {
                Box::pin(async move { Err(PushError::Failed(error)) })
            }
        }
    }

    fn replay_run<'a>(
        &'a self,
        run: &'a RunId,
        after: Option<After>,
    ) -> HostFuture<'a, anyhow::Result<Replayed>> {
        self.inner.replay_run(run, after)
    }

    fn push_branch(
        &self,
        to: Remote,
        branch: String,
        expected: Option<String>,
        head: String,
    ) -> HostFuture<'_, anyhow::Result<String>> {
        match self.fuse.write("push_branch") {
            Ok(()) => self.inner.push_branch(to, branch, expected, head),
            Err(error) => closed(error),
        }
    }

    fn workspaces(&self) -> HostFuture<'_, anyhow::Result<Vec<String>>> {
        self.inner.workspaces()
    }

    fn catch_up<'a>(
        &'a self,
        workspace: &'a str,
    ) -> HostFuture<'a, anyhow::Result<()>> {
        match self.fuse.write("catch_up") {
            Ok(()) => self.inner.catch_up(workspace),
            Err(error) => closed(error),
        }
    }

    fn conflicts<'a>(
        &'a self,
        workspace: &'a str,
    ) -> HostFuture<'a, anyhow::Result<Vec<String>>> {
        self.inner.conflicts(workspace)
    }

    // A run's own workspace and what it does in it go straight to the
    // project, as before the slot: the fuse stands only between the
    // kernel and the project.
    fn run_workspace(
        &self,
        spec: RunSpec,
    ) -> HostFuture<'_, anyhow::Result<Arc<dyn RunWorkspace>>> {
        self.inner.run_workspace(spec)
    }

    fn newest<'a>(
        &'a self,
        run: &'a RunId,
        workspace: &'a str,
    ) -> HostFuture<'a, anyhow::Result<Newest>> {
        self.inner.newest(run, workspace)
    }

    fn ready_workspace<'a>(
        &'a self,
        name: &'a str,
    ) -> HostFuture<'a, anyhow::Result<()>> {
        match self.fuse.write("ready_workspace") {
            Ok(()) => self.inner.ready_workspace(name),
            Err(error) => closed(error),
        }
    }

    fn land(&self, land: Land) -> HostFuture<'_, anyhow::Result<Landing>> {
        // Only a landing that lands writes.
        if land.confirm
            && let Err(error) = self.fuse.write("land")
        {
            return closed(error);
        }
        self.inner.land(land)
    }

    fn landed<'a>(
        &'a self,
        child: &'a str,
    ) -> HostFuture<'a, anyhow::Result<Option<Landing>>> {
        self.inner.landed(child)
    }

    fn forget_workspaces(
        &self,
        names: Vec<String>,
    ) -> HostFuture<'_, anyhow::Result<()>> {
        match self.fuse.write("forget_workspaces") {
            Ok(()) => self.inner.forget_workspaces(names),
            Err(error) => closed(error),
        }
    }

    fn forget_run<'a>(
        &'a self,
        run: &'a RunId,
    ) -> HostFuture<'a, anyhow::Result<()>> {
        match self.fuse.write("forget_run") {
            Ok(()) => self.inner.forget_run(run),
            Err(error) => closed(error),
        }
    }

    fn discard(&self, discard: Discard) -> HostFuture<'_, anyhow::Result<()>> {
        match self.fuse.write("discard") {
            Ok(()) => self.inner.discard(discard),
            Err(error) => closed(error),
        }
    }

    fn held(&self) -> HostFuture<'_, anyhow::Result<Held>> {
        self.inner.held()
    }

    fn sweep(
        &self,
        owners: Vec<Owner>,
        held: Held,
    ) -> HostFuture<'_, anyhow::Result<()>> {
        match self.fuse.write("sweep") {
            Ok(()) => self.inner.sweep(owners, held),
            Err(error) => closed(error),
        }
    }
}

/// What the model answers, read from the turn's message: `«write f»`
/// writes and commits `f`, `«spawn f»` hands writing `f` to a
/// sub-agent; anything else, tau's own messages included, is answered
/// in words.
#[derive(Debug, Clone, PartialEq)]
enum Reply {
    Text,
    Write(String),
    Commit(String),
    Spawn(String),
}

/// The marker `«verb arg»` in `text`.
fn marker(text: &str) -> Option<(&str, &str)> {
    let start = text.find('«')? + '«'.len_utf8();
    let end = start + text[start..].find('»')?;
    text[start..end].split_once(' ')
}

/// The reply to `transcript`, by the message its turn started with and
/// how many replies the turn has had.
fn reply(transcript: &[Message]) -> Reply {
    let Some(at) = transcript
        .iter()
        .rposition(|message| matches!(message, Message::User(_)))
    else {
        return Reply::Text;
    };
    let Message::User(user) = &transcript[at] else {
        unreachable!()
    };
    let text = user.content.text();
    let step = transcript[at..]
        .iter()
        .filter(|message| matches!(message, Message::Assistant(_)))
        .count();
    // tau's reports quote the sub-agents' tasks.
    if text.contains("from tau") {
        return Reply::Text;
    }
    // Held for files a turn cut off left: committed.
    if let Some((_, files)) = text.split_once("Uncommitted: ") {
        return match step {
            0 => Reply::Commit(files.trim().trim_end_matches('.').to_owned()),
            _ => Reply::Text,
        };
    }
    match (marker(&text), step) {
        (Some(("write", file)), 0) => Reply::Write(file.to_owned()),
        (Some(("write", file)), 1) => Reply::Commit(file.to_owned()),
        (Some(("spawn", file)), 0) => Reply::Spawn(file.to_owned()),
        _ => Reply::Text,
    }
}

/// A model that answers every request as [`reply`] says, whichever run
/// asks and in whatever order: runs and sub-agents go side by side.
#[derive(Clone, Default)]
struct Coder {
    model: ScriptedModel,
    /// Held from scripting a turn to drawing it, so a session draws its
    /// own.
    drawing: Arc<Mutex<()>>,
}

impl Llm for Coder {
    fn open(
        &self,
        settings: Settings,
    ) -> BoxFuture<'static, Result<Box<dyn LlmSession>, LlmError>> {
        // A plugin's side request, such as a title, has no tools.
        let side = settings.tools.is_empty();
        let (coder, open) = (self.clone(), self.model.open(settings));
        async move {
            let inner = open.await?;
            Ok(Box::new(Session { coder, inner, side }) as Box<dyn LlmSession>)
        }
        .boxed()
    }
}

struct Session {
    coder: Coder,
    inner: Box<dyn LlmSession>,
    side: bool,
}

impl LlmSession for Session {
    fn settings(&self) -> &Settings {
        self.inner.settings()
    }

    fn set_reasoning(
        &mut self,
        effort: Option<tau_ai::responses::request::ReasoningEffort>,
    ) {
        self.inner.set_reasoning(effort);
    }

    fn respond(
        &mut self,
        transcript: &[Message],
        timestamp: Timestamp,
    ) -> EventStream {
        let reply = match self.side {
            true => Reply::Text,
            false => reply(transcript),
        };
        let _drawing = self.coder.drawing.lock().unwrap();
        let _ = self.coder.model.clone().turn(|turn| match reply {
            Reply::Text => turn.text("Done."),
            Reply::Write(file) => turn.tool_call(
                "write",
                serde_json::json!({ "path": file, "content": format!("{file}\n") }),
            ),
            Reply::Commit(file) => turn.tool_call(
                "vcs_commit",
                serde_json::json!({ "message": format!("feat: {file}") }),
            ),
            Reply::Spawn(file) => turn.tool_call(
                "spawn",
                serde_json::json!({ "task": format!("«write {file}»") }),
            ),
        });
        self.inner.respond(transcript, timestamp)
    }
}

/// What outlives tau: the upstream repository, the project, the store,
/// the repository list and the plugins' directories.
struct Disk {
    dir: tempfile::TempDir,
    upstream: tempfile::TempDir,
    /// Trunk as the project was made: what every change counted is after.
    base: String,
}

impl Disk {
    fn new() -> Self {
        let upstream = tempfile::tempdir().unwrap();
        git(upstream.path(), &["init", "--quiet"]);
        std::fs::write(upstream.path().join("README.md"), "hello\n").unwrap();
        git(upstream.path(), &["add", "README.md"]);
        git(upstream.path(), &["commit", "--quiet", "-m", "first"]);
        let dir = tempfile::tempdir().unwrap();
        let project = ProjectRepo::import(
            upstream.path().to_str().unwrap(),
            dir.path().join("p"),
            Identity::default(),
        )
        .unwrap();
        let base = project.trunk().unwrap();
        Self {
            dir,
            upstream,
            base,
        }
    }

    fn db(&self) -> PathBuf {
        self.dir.path().join("runs.db")
    }

    /// A commit upstream that adds `file`.
    fn push_upstream(&self, file: &str) {
        let src = self.upstream.path();
        std::fs::write(src.join(file), format!("{file}\n")).unwrap();
        git(src, &["add", file]);
        git(src, &["commit", "--quiet", "-m", &format!("up: {file}")]);
    }

    fn remote(&self) -> Remote {
        Remote {
            url: self.upstream.path().to_str().unwrap().to_owned(),
            token: None,
        }
    }
}

/// One run of tau over the [`Disk`], from its start to its close.
struct Life {
    host: Host,
    events: UnboundedReceiver<RunEvent>,
    fuse: Arc<Fuse>,
    /// The store as it is, past the fuse: what a later start reads.
    raw: Store,
    repository: Arc<dyn Repository>,
    project: Project,
    /// The events since the last [`Life::mark`].
    seen: Vec<RunEvent>,
}

impl Life {
    /// tau started on what is on disk: everything opened again, nothing
    /// done yet.
    fn open(disk: &Disk) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let fuse = Arc::new(Fuse::default());
        let raw = runtime.block_on(tau_store_sqlite::open(disk.db())).unwrap();
        let store: Store = Arc::new(FusedStore {
            inner: raw.clone(),
            fuse: fuse.clone(),
        });
        let root = disk.dir.path();
        // tau lists only repositories from GitHub, as this one stands for:
        // so the list keeps its main chat across starts.
        let list = root.join("repos.json");
        if let Ok(text) = std::fs::read_to_string(&list) {
            let mut saved: serde_json::Value =
                serde_json::from_str(&text).unwrap();
            for repo in saved["repos"].as_array_mut().into_iter().flatten() {
                repo["forge"] = serde_json::json!("owner/repo");
            }
            std::fs::write(&list, saved.to_string()).unwrap();
        }
        let config = HostConfig {
            credentials: Credentials::new(root.join("config")),
            model: Some("gpt-6-luna".into()),
            store: disk.db(),
            repos: root.join("repos"),
            settings: root.join("models.json"),
            repo_list: list,
            home: None,
            services: Default::default(),
        };
        let agent = Agent::new(Coder::default()).name("coder");
        let (host, events) = Host::with_agent(runtime, agent, store, config);
        // Memory's pass after each run would ask the model on the side.
        let host = host.with_plugin_settings(
            tau_memory::NAME,
            serde_json::json!({ "after_each_run": false }),
        );
        let project = Project::from(
            ProjectRepo::open(root.join("p"), Identity::default()).unwrap(),
        );
        let repository: Arc<dyn Repository> = Arc::new(FusedRepository {
            inner: Arc::new(JjRepository::new(
                project.clone(),
                Identity::default(),
            )),
            fuse: fuse.clone(),
        });
        let host = host.with_repository(REPO, repository.clone());
        Self {
            host,
            events,
            fuse,
            raw,
            repository,
            project,
            seen: Vec::new(),
        }
    }

    fn main(&self) -> RunId {
        self.host.block_on(self.host.main_of(REPO)).unwrap()
    }

    /// Forgets the events so far: what comes next is the next step's.
    fn mark(&mut self) {
        while self.events.try_recv().is_ok() {}
        self.seen.clear();
    }

    fn take_events(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            self.seen.push(event);
        }
    }

    fn ended(&self, run: &RunId) -> bool {
        self.seen.iter().any(
            |event| matches!(event, RunEvent::RunEnd { run: ended, .. } if ended == run),
        )
    }

    /// Whether a turn of `run` started since the mark: it stored the
    /// message it started on.
    fn took(&self, run: &RunId) -> bool {
        self.seen.iter().any(
            |event| matches!(event, RunEvent::TurnStart { run: started, .. } if started == run),
        )
    }

    /// Waits for `run` to stop: it ended, or tau closed under it.
    /// Whether it ended.
    fn wait(&mut self, run: &RunId) -> bool {
        let deadline = Instant::now() + WAIT;
        loop {
            self.take_events();
            let stopped = !self.host.is_running(run);
            if self.ended(run) && stopped {
                return true;
            }
            if stopped && self.fuse.blown().is_some() {
                return false;
            }
            assert!(Instant::now() < deadline, "{} did not stop", run.0);
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// The sub-agents main started since the mark.
    fn sub_agents(&mut self, main: &RunId) -> Vec<RunId> {
        self.take_events();
        self.seen
            .iter()
            .filter_map(|event| match event {
                RunEvent::RunStart {
                    run,
                    parent: Some(parent),
                    ..
                } if parent == main => Some(run.clone()),
                _ => None,
            })
            .collect()
    }

    /// Does what the interface does with a drain: tau's turn on main
    /// with what it reports, then main's turn's end, until nothing more
    /// is reported. `None` when tau closed in between.
    fn follow(&mut self, report: anyhow::Result<DrainReport>) -> Option<()> {
        let mut report = match report {
            Ok(report) => report,
            Err(_) if self.fuse.blown().is_some() => return None,
            Err(error) => panic!("a drain failed: {error:#}"),
        };
        for _ in 0..8 {
            let Some(prompt) = report.resolve.take() else {
                return Some(());
            };
            let main = report.main.clone();
            self.mark();
            match self
                .host
                .block_on(self.host.start_resolving(&main, &prompt))
            {
                Ok(()) => {}
                Err(_) if self.fuse.blown().is_some() => return None,
                Err(error) => panic!("tau's turn did not start: {error:#}"),
            }
            if !self.wait(&main) {
                return None;
            }
            report = match self.host.block_on(self.host.main_turn_ended(&main))
            {
                Ok(report) => report,
                Err(_) if self.fuse.blown().is_some() => return None,
                Err(error) => panic!("main's end failed: {error:#}"),
            };
        }
        panic!("tau kept reporting to main");
    }

    /// Starts as tau does: finishes what the last tau left, lands what
    /// waits, and brings in what came upstream. The landings it
    /// finished.
    fn start(&mut self, disk: &Disk) -> Vec<LandingRecord> {
        let finished = self.host.block_on(self.host.recover()).unwrap();
        self.mark();
        for report in self.host.block_on(self.host.drain_all()) {
            self.follow(Ok(report)).unwrap();
        }
        self.host
            .block_on(self.repository.update(disk.remote()))
            .unwrap();
        finished
    }

    /// The tool results `run` got in its last turn that failed.
    fn failed_tools(&self, run: &RunId, text: &str) -> Vec<String> {
        let transcript =
            self.host.block_on(self.raw.transcript(&run.0)).unwrap();
        let messages: Vec<Message> = transcript
            .iter()
            .filter_map(|entry| match entry {
                Entry::Message { body, .. } => serde_json::from_str(body).ok(),
                _ => None,
            })
            .collect();
        // Plugins may hold the turn's stop with messages of their own.
        let last = messages
            .iter()
            .rposition(|message| match message {
                Message::User(user) => user.content.text().contains(text),
                _ => false,
            })
            .unwrap_or(0);
        messages[last..]
            .iter()
            .filter_map(|message| match message {
                Message::ToolResult(result) if result.is_error => {
                    Some(format!("{}: {:?}", result.tool_name, result.content))
                }
                _ => None,
            })
            .collect()
    }

    /// The sub-agent `spawn` started in `run`'s turn on `text`.
    fn spawned(&self, run: &RunId, text: &str) -> Option<RunId> {
        let transcript =
            self.host.block_on(self.raw.transcript(&run.0)).unwrap();
        let messages: Vec<Message> = transcript
            .iter()
            .filter_map(|entry| match entry {
                Entry::Message { body, .. } => serde_json::from_str(body).ok(),
                _ => None,
            })
            .collect();
        let at = messages.iter().rposition(|message| match message {
            Message::User(user) => user.content.text().contains(text),
            _ => false,
        })?;
        messages[at..].iter().find_map(|message| match message {
            Message::ToolResult(result)
                if result.tool_name == "spawn" && !result.is_error =>
            {
                let details = result.details.as_ref()?;
                Some(RunId(details["run"].as_str()?.into()))
            }
            _ => None,
        })
    }

    /// The person's messages stored in `run`.
    fn said(&self, run: &RunId) -> Vec<String> {
        // A run never stored has said nothing.
        let timeline = self
            .host
            .block_on(self.raw.timeline(&run.0))
            .unwrap_or_default();
        timeline
            .iter()
            .filter_map(|entry| match entry {
                Entry::Message { role, body } if role == "user" => {
                    match serde_json::from_str::<Message>(body) {
                        Ok(Message::User(user)) => Some(user.content.text()),
                        _ => None,
                    }
                }
                _ => None,
            })
            .collect()
    }

    fn status(&self, run: &RunId) -> Option<Status> {
        self.host
            .block_on(self.raw.run(&run.0))
            .unwrap()
            .map(|record| record.status)
    }

    /// What the person sees of tau's state, as stored: every run's view
    /// and main's queue.
    fn shown(&self) -> String {
        let mut views = self.host.block_on(self.host.history()).unwrap();
        views.sort_by(|a, b| a.id.0.cmp(&b.id.0));
        let main = self.main();
        let queue = self.host.block_on(self.host.landing_queue(&main)).unwrap();
        format!("{views:#?}\n{queue:#?}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fate {
    Open,
    Landed,
    Dropped,
}

#[derive(Debug, Clone)]
struct Chat {
    id: RunId,
    fate: Fate,
    /// The files its turns committed.
    files: Vec<String>,
    /// Files a turn tau's closing cut off was writing: in its last
    /// commit or not.
    maybe: Vec<String>,
}

#[derive(Debug, Clone)]
struct SubAgent {
    id: RunId,
    file: String,
}

/// What the person does, each with a drawn place for tau to close.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    StartChat,
    WriteToChat,
    WriteToMain,
    Spawn,
    Land,
    Drop,
    Upstream,
}

/// The steps tau closes in, landings twice as often: they have the
/// most steps of their own.
const STEPS: [Step; 8] = [
    Step::StartChat,
    Step::WriteToChat,
    Step::WriteToMain,
    Step::Spawn,
    Step::Land,
    Step::Land,
    Step::Drop,
    Step::Upstream,
];

struct Machine {
    disk: Disk,
    life: Option<Life>,
    main: RunId,
    chats: Vec<Chat>,
    /// Files on trunk for sure: main's, landed sub-agents', upstream's.
    on_trunk: BTreeSet<String>,
    /// Files a step tau's closing cut off was bringing to trunk.
    maybe_on_trunk: BTreeSet<String>,
    sub_agents: Vec<SubAgent>,
    /// Each message a turn took, by run.
    said: Vec<(RunId, String)>,
    /// Each workspace name with the run that worked in it first.
    workspaces: BTreeMap<String, String>,
    /// Chats tau closed while starting, before the host answered, by
    /// message and file: found once it starts again, or never made.
    pending_starts: Vec<(String, String)>,
    /// Chats tau closed while landing or dropping.
    unsure: BTreeSet<RunId>,
    /// A turn of main's tau's closing cut off may have left files in its
    /// working copy.
    main_dirty: bool,
    next: usize,
}

impl Machine {
    fn new() -> Self {
        let disk = Disk::new();
        let mut life = Life::open(&disk);
        life.start(&disk);
        let main = life.main();
        Self {
            disk,
            life: Some(life),
            main,
            chats: Vec::new(),
            on_trunk: BTreeSet::new(),
            maybe_on_trunk: BTreeSet::new(),
            sub_agents: Vec::new(),
            said: Vec::new(),
            workspaces: BTreeMap::new(),
            pending_starts: Vec::new(),
            unsure: BTreeSet::new(),
            main_dirty: false,
            next: 0,
        }
    }

    fn life(&mut self) -> &mut Life {
        self.life.as_mut().unwrap()
    }

    fn file(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("{prefix}{}.txt", self.next)
    }

    fn blown(&self) -> bool {
        self.life.as_ref().unwrap().fuse.blown().is_some()
    }

    /// A chat to act on: an open one, mostly; sometimes one that ended,
    /// which must refuse.
    fn pick(&self, tc: &TestCase) -> RunId {
        let open: Vec<&Chat> = self
            .chats
            .iter()
            .filter(|chat| chat.fate == Fate::Open)
            .collect();
        let any = tc.draw(gs::integers::<u8>().max_value(4)) == 0;
        let from: Vec<&Chat> = match open.is_empty() || any {
            true => self.chats.iter().collect(),
            false => open,
        };
        from[tc.draw(gs::integers().max_value(from.len() - 1))]
            .id
            .clone()
    }

    fn chat_mut(&mut self, id: &RunId) -> &mut Chat {
        self.chats.iter_mut().find(|chat| chat.id == *id).unwrap()
    }

    /// A turn of `run` on `text` ended: it took the message and its
    /// tools worked.
    fn turn_whole(&mut self, run: &RunId, text: &str) {
        let life = self.life.as_ref().unwrap();
        assert!(life.took(run), "{} did not take {text:?}", run.0);
        assert_eq!(life.status(run), Some(Status::Done), "{text:?}");
        let mut failed = life.failed_tools(run, text);
        // A turn cut off may leave main with files it never committed:
        // `spawn` refuses until main commits them, which its turn does
        // as it is held for them.
        if *run == self.main && self.main_dirty {
            failed.retain(|failure| !failure.contains("uncommitted changes"));
        }
        assert!(failed.is_empty(), "{text:?}: {failed:?}");
        self.said.push((run.clone(), text.to_owned()));
    }

    /// The host took the message `text` to `run`, and tau may have closed
    /// since: the host answers once the message is stored, so it is kept.
    fn sent(&mut self, run: &RunId, text: &str) {
        assert!(
            self.life.as_ref().unwrap().took(run),
            "the host answered before {} took {text:?}",
            run.0
        );
        self.said.push((run.clone(), text.to_owned()));
    }

    fn start_chat(&mut self) {
        let file = self.file("c");
        let text = format!("«write {file}»");
        let life = self.life();
        life.mark();
        let started = life.host.block_on(life.host.start(
            text.as_str(),
            &ModelChoice::default(),
            REPO,
        ));
        let id = match started {
            Ok(view) => view.id,
            Err(_) if self.blown() => {
                // It may be stored already: found again once tau starts.
                self.pending_starts.push((text, file));
                return;
            }
            Err(error) => panic!("a chat did not start: {error:#}"),
        };
        let ended = self.life().wait(&id);
        if ended && !self.blown() {
            self.turn_whole(&id, &text);
            self.chats.push(Chat {
                id,
                fate: Fate::Open,
                files: vec![file],
                maybe: Vec::new(),
            });
        } else {
            // The host answered once the chat and its message were
            // stored: it is there, whatever came of its turn.
            self.sent(&id, &text);
            self.chats.push(Chat {
                id,
                fate: Fate::Open,
                files: Vec::new(),
                maybe: vec![file],
            });
        }
    }

    fn write_to_chat(&mut self, tc: &TestCase) {
        if self.chats.is_empty() {
            tc.reject();
        }
        let id = self.pick(tc);
        self.write_to(&id);
    }

    /// The person writes to the chat `id`: it writes and commits a file.
    fn write_to(&mut self, id: &RunId) {
        let id = id.clone();
        let fate = self.chat_mut(&id).fate;
        let file = self.file("c");
        let text = format!("«write {file}»");
        let life = self.life();
        life.mark();
        let resumed = life.host.block_on(life.host.resume(
            &id,
            text.as_str(),
            &ModelChoice::default(),
        ));
        match (resumed, fate) {
            // A chat that ended for good takes no more messages.
            (Ok(()), Fate::Landed | Fate::Dropped) => {
                panic!("{} took a message after it ended", id.0)
            }
            (Err(_), Fate::Landed | Fate::Dropped) => return,
            (Err(_), Fate::Open) if self.blown() => return,
            (Err(error), Fate::Open) => {
                panic!("an open chat refused a message: {error:#}")
            }
            (Ok(()), Fate::Open) => {}
        }
        let ended = self.life().wait(&id);
        if ended && !self.blown() {
            self.turn_whole(&id, &text);
            let chat = self.chat_mut(&id);
            // What a cut turn wrote is committed with this one.
            chat.files.push(file);
        } else {
            self.sent(&id, &text);
            self.chat_mut(&id).maybe.push(file);
        }
    }

    /// Main's turn on `text`, and what the interface does as it ends.
    /// Whether it ran whole.
    fn main_turn(&mut self, text: &str) -> bool {
        let main = self.main.clone();
        let life = self.life();
        life.mark();
        let resumed = life.host.block_on(life.host.resume(
            &main,
            text,
            &ModelChoice::default(),
        ));
        match resumed {
            Ok(()) => {}
            Err(_) if self.blown() => {
                self.main_dirty = true;
                return false;
            }
            Err(error) => panic!("main refused a message: {error:#}"),
        }
        let ended = self.life().wait(&main);
        if !ended || self.blown() {
            self.sent(&main, text);
            self.main_dirty = true;
            return false;
        }
        self.turn_whole(&main, text);
        true
    }

    /// Main's turn ended: what waits lands.
    fn main_ended(&mut self) -> bool {
        let main = self.main.clone();
        let life = self.life();
        let report = life.host.block_on(life.host.main_turn_ended(&main));
        life.follow(report).is_some()
    }

    fn write_to_main(&mut self) {
        let file = self.file("m");
        let text = format!("«write {file}»");
        if self.main_turn(&text) && self.main_ended() {
            // Its commit took whatever a cut turn left.
            self.main_dirty = false;
            self.on_trunk.insert(file);
        } else {
            self.maybe_on_trunk.insert(file);
        }
    }

    fn spawn(&mut self) {
        let file = self.file("s");
        let text = format!("«spawn {file}»");
        let whole = self.main_turn(&text);
        let main = self.main.clone();
        // A whole turn says what `spawn` started; a cut one, maybe the
        // sub-agent's own start.
        let started = match whole {
            true => self.life().spawned(&main, &text).into_iter().collect(),
            false => self.life().sub_agents(&main),
        };
        assert!(started.len() <= 1, "{started:?}");
        let Some(child) = started.first().cloned() else {
            assert!(!whole || self.main_dirty, "main spawned no sub-agent");
            // Its turn ended all the same: what waits on main may land.
            if whole && !self.blown() {
                self.main_ended();
            }
            self.maybe_on_trunk.insert(file);
            return;
        };
        self.sub_agents.push(SubAgent {
            id: child.clone(),
            file: file.clone(),
        });
        // The interface's order: main's end, then the sub-agent's. Main's
        // turn ended whatever its sub-agent does: the interface says so
        // as it does, and what waits on main's queue may land then.
        let main_ended = whole && !self.blown() && self.main_ended();
        let ended = self.life().wait(&child);
        let landed = main_ended && ended && !self.blown() && {
            let life = self.life();
            let report = life
                .host
                .block_on(life.host.sub_agent_ended(&child, Vec::new()));
            life.follow(report).is_some()
        };
        if landed {
            self.on_trunk.insert(file);
        } else {
            self.maybe_on_trunk.insert(file);
        }
    }

    fn land(&mut self, tc: &TestCase) {
        if self.chats.is_empty() {
            tc.reject();
        }
        let id = self.pick(tc);
        // Landed at once, as `wait` lands a sub-agent, or through main's
        // queue, as the person lands a chat.
        match tc.draw(gs::booleans()) {
            true => self.land_now(&id),
            false => self.land_queued(&id),
        }
    }

    /// Lands `id` through main's queue.
    fn land_queued(&mut self, id: &RunId) {
        let id = id.clone();
        let chat = self.chat_mut(&id).clone();
        let life = self.life();
        life.mark();
        let report = life.host.block_on(life.host.queue_landing(&id));
        let report = match (report, chat.fate) {
            // Nothing lands twice, nor after it was dropped.
            (Ok(_), Fate::Landed | Fate::Dropped) => {
                panic!("{} was queued after it ended", id.0)
            }
            (Err(_), Fate::Landed | Fate::Dropped) => return,
            (Err(_), Fate::Open) if self.blown() => {
                self.cut_landing(&id);
                return;
            }
            (Err(error), Fate::Open) => panic!("queueing failed: {error:#}"),
            (Ok(report), Fate::Open) => report,
        };
        let landed = report.landed.iter().any(|(run, _)| *run == id);
        let failed = report.failed.iter().find(|(run, _)| *run == id).cloned();
        let followed = self.life().follow(Ok(report));
        if followed.is_none() || self.blown() {
            self.cut_landing(&id);
            return;
        }
        match failed {
            // Only a chat a crash cut off can have work it never
            // committed.
            Some((_, why)) => {
                assert!(
                    !chat.maybe.is_empty(),
                    "{} could not land: {why}",
                    id.0
                );
            }
            None => {
                assert!(landed, "{} neither landed nor failed", id.0);
                self.chat_mut(&id).fate = Fate::Landed;
            }
        }
    }

    /// Lands `id` on main at once, without the queue.
    fn land_now(&mut self, id: &RunId) {
        let chat = self.chat_mut(id).clone();
        let life = self.life();
        let landed = life.host.block_on(life.host.land(id));
        match (landed, chat.fate) {
            (Ok(_), Fate::Landed | Fate::Dropped) => {
                panic!("{} landed after it ended", id.0)
            }
            (Err(_), Fate::Landed | Fate::Dropped) => {}
            (Err(_), Fate::Open) if self.blown() => self.cut_landing(id),
            // Only a chat a crash cut off can have work it never
            // committed.
            (Err(error), Fate::Open) => assert!(
                !chat.maybe.is_empty(),
                "{} could not land: {error:#}",
                id.0
            ),
            (Ok(_), Fate::Open) => self.chat_mut(id).fate = Fate::Landed,
        }
    }

    /// tau closed while `id` was landing: it landed, or it did not.
    fn cut_landing(&mut self, id: &RunId) {
        self.unsure.insert(id.clone());
    }

    fn drop_chat(&mut self, tc: &TestCase) {
        if self.chats.is_empty() {
            tc.reject();
        }
        let id = self.pick(tc);
        self.drop_one(&id);
    }

    /// Drops `id`.
    fn drop_one(&mut self, id: &RunId) {
        let id = id.clone();
        let fate = self.chat_mut(&id).fate;
        let life = self.life();
        let dropped = life.host.block_on(life.host.drop_child(&id));
        match (dropped, fate) {
            (Ok(()), Fate::Landed) => {
                panic!("{} dropped after it landed", id.0)
            }
            (Err(_), Fate::Landed) => {}
            // Dropping again finds nothing left to drop.
            (_, Fate::Dropped) => {}
            (Err(_), Fate::Open) if self.blown() => {
                self.unsure.insert(id);
            }
            (Err(error), Fate::Open) => panic!("dropping failed: {error:#}"),
            (Ok(()), Fate::Open) => self.chat_mut(&id).fate = Fate::Dropped,
        }
    }

    fn upstream(&mut self) {
        let file = self.file("u");
        self.disk.push_upstream(&file);
        let remote = self.disk.remote();
        let life = self.life();
        let updated = life.host.block_on(life.repository.update(remote));
        match updated {
            Ok(_) => {
                self.on_trunk.insert(file);
            }
            Err(_) if self.blown() => {
                // The next start brings it in.
                self.on_trunk.insert(file);
            }
            Err(error) => panic!("the update failed: {error:#}"),
        }
    }

    fn step(&mut self, tc: &TestCase, step: Step) {
        match step {
            Step::StartChat => self.start_chat(),
            Step::WriteToChat => self.write_to_chat(tc),
            Step::WriteToMain => self.write_to_main(),
            Step::Spawn => self.spawn(),
            Step::Land => self.land(tc),
            Step::Drop => self.drop_chat(tc),
            Step::Upstream => self.upstream(),
        }
    }

    /// tau closes, and starts again on what it left.
    fn reopen(&mut self) -> Vec<LandingRecord> {
        drop(self.life.take());
        let mut life = Life::open(&self.disk);
        let finished = life.start(&self.disk);
        self.life = Some(life);
        finished
    }

    /// After tau started again, what the steps it cut off came to.
    fn settle_cut(&mut self) {
        let life = self.life.as_ref().unwrap();
        // Chats a cut start may have made, by their first message.
        let runs = life.host.block_on(life.raw.retained_runs()).unwrap();
        for (text, file) in std::mem::take(&mut self.pending_starts) {
            let found = runs.iter().find(|record| {
                matches!(&record.kind, RunKind::Fork { parent, .. } if *parent == *self.main.0)
                    && life
                        .host
                        .block_on(life.raw.first_prompt(&record.id))
                        .unwrap()
                        .as_deref()
                        == Some(text.as_str())
            });
            if let Some(record) = found {
                self.chats.push(Chat {
                    id: RunId(record.id.as_str().into()),
                    fate: Fate::Open,
                    files: Vec::new(),
                    maybe: vec![file],
                });
            }
        }
        // A landing or a drop cut off happened, or did not: the store
        // says which, and the checks hold it to the project.
        for id in std::mem::take(&mut self.unsure) {
            let ending = life.host.block_on(life.host.ending_of(&id)).unwrap();
            let fate = match ending {
                None => Fate::Open,
                Some(Ending::Landed { .. }) => Fate::Landed,
                Some(Ending::Dropped) => Fate::Dropped,
            };
            if let Some(chat) = self.chats.iter_mut().find(|chat| chat.id == id)
            {
                chat.fate = fate;
            }
        }
        // Sub-agents a cut turn of main's started, by their task.
        let life = self.life.as_ref().unwrap();
        let called = life
            .host
            .block_on(life.raw.subagents(&self.main.0))
            .unwrap();
        for record in called {
            if self.sub_agents.iter().any(|sub| *sub.id.0 == record.id) {
                continue;
            }
            let task = life
                .host
                .block_on(life.raw.first_prompt(&record.id))
                .unwrap()
                .unwrap_or_default();
            if let Some(("write", file)) = marker(&task) {
                self.sub_agents.push(SubAgent {
                    id: RunId(record.id.as_str().into()),
                    file: file.to_owned(),
                });
                self.maybe_on_trunk.insert(file.to_owned());
            }
        }
        // What a cut step brought to trunk is there for good, or never.
        let maybe = std::mem::take(&mut self.maybe_on_trunk);
        for file in maybe {
            let there =
                committed(life.project.blocking(), &self.disk.base, &file);
            if there {
                self.on_trunk.insert(file);
            }
        }
    }

    fn crash_in(&mut self, tc: &TestCase) {
        let step = STEPS[tc.draw(gs::integers().max_value(STEPS.len() - 1))];
        // About as many as the step has, so that tau closes inside it.
        let most = match step {
            Step::StartChat | Step::WriteToChat => 11,
            Step::WriteToMain => 14,
            Step::Spawn => 44,
            Step::Land => 13,
            Step::Drop => 3,
            Step::Upstream => 1,
        };
        let writes: usize = tc.draw(gs::integers().max_value(most));
        // A step that has nothing to act on is rejected before the fuse
        // is armed.
        if matches!(step, Step::WriteToChat | Step::Land | Step::Drop)
            && self.chats.is_empty()
        {
            tc.reject();
        }
        tc.note(&format!("tau closes in {step:?} after {writes} writes"));
        self.life().fuse.arm(writes);
        self.step(tc, step);
        let life = self.life.as_ref().unwrap();
        let Some(at) = life.fuse.blown() else {
            let counted = life.fuse.disarm();
            tc.note(&format!("{step:?} ran whole, in {counted} writes"));
            return;
        };
        tc.note(&format!("tau closed at {at}"));
        self.closed();
    }

    /// tau closed: it starts again on what it left, and the model learns
    /// what the step it cut off came to. Started again on the same disk,
    /// tau then finds nothing more to finish, and shows the same.
    fn closed(&mut self) {
        self.reopen();
        self.settle_cut();
        let shown = self.life.as_ref().unwrap().shown();
        let finished = self.reopen();
        assert!(finished.is_empty(), "a second start finished {finished:?}");
        assert_eq!(self.life.as_ref().unwrap().shown(), shown);
    }
}

/// The runs waiting in `main`'s queue.
fn queued(life: &Life, main: &RunId) -> Vec<String> {
    life.host
        .block_on(life.host.landing_queue(main))
        .unwrap()
        .0
        .into_iter()
        .map(|waiting| waiting.run)
        .collect()
}

#[hegel::state_machine]
impl Machine {
    #[rule(weight = 3)]
    fn start_a_chat(&mut self, tc: TestCase) {
        self.step(&tc, Step::StartChat);
    }

    #[rule(weight = 2)]
    fn write_to_a_chat(&mut self, tc: TestCase) {
        self.step(&tc, Step::WriteToChat);
    }

    #[rule]
    fn write_to_main_chat(&mut self, tc: TestCase) {
        self.step(&tc, Step::WriteToMain);
    }

    #[rule]
    fn hand_work_to_a_sub_agent(&mut self, tc: TestCase) {
        self.step(&tc, Step::Spawn);
    }

    #[rule(weight = 2)]
    fn land_a_chat(&mut self, tc: TestCase) {
        self.step(&tc, Step::Land);
    }

    #[rule]
    fn drop_a_chat(&mut self, tc: TestCase) {
        self.step(&tc, Step::Drop);
    }

    #[rule]
    fn bring_in_upstream(&mut self, tc: TestCase) {
        self.step(&tc, Step::Upstream);
    }

    /// tau closes in the middle of a step, after a drawn number of its
    /// writes, and starts again.
    #[rule(weight = 4)]
    fn crash(&mut self, tc: TestCase) {
        self.crash_in(&tc);
    }

    /// tau closes between steps, and starts again.
    #[rule]
    fn restart(&mut self, _tc: TestCase) {
        let finished = self.reopen();
        assert!(finished.is_empty(), "{finished:?}");
    }

    #[invariant(always_run)]
    fn the_host_is_the_model(&mut self, _tc: TestCase) {
        check(self);
    }
}

/// Main's line: trunk and the commit main's working copy stands on,
/// and the changes either has since `base`, each once.
fn main_line(
    project: &ProjectRepo,
    base: &str,
) -> (Vec<String>, Vec<StackChange>) {
    let mut tips = vec![project.trunk().unwrap()];
    if let Some(wc) = project.workspace_head(project.main_workspace()).unwrap()
    {
        tips.push(wc);
    }
    let mut changes: Vec<StackChange> = Vec::new();
    for tip in &tips {
        for change in project.changes_between(base, tip).unwrap() {
            if !changes
                .iter()
                .any(|known| known.commit_id == change.commit_id)
            {
                changes.push(change);
            }
        }
    }
    (tips, changes)
}

/// Whether a commit described `description` brings `file`: `feat: a`,
/// or `feat: a, b` for what a turn cut off left.
fn brings(description: &str, file: &str) -> bool {
    description
        .strip_prefix("feat: ")
        .is_some_and(|files| files.split(", ").any(|known| known == file))
}

/// Whether a commit that brings `file` is on main's line. A turn cut
/// off may have left it written in main's working copy, but not
/// committed.
fn committed(project: &ProjectRepo, base: &str, file: &str) -> bool {
    main_line(project, base).1.iter().any(|change| {
        let description = change.description.trim();
        brings(description, file)
            || description.strip_prefix("up: ") == Some(file)
    })
}

/// Everything a start must leave, against the model.
fn check(machine: &mut Machine) {
    let life = machine.life.as_ref().unwrap();
    let main = &machine.main;
    let host = &life.host;
    let project = life.project.blocking();
    let (tips, changes) = main_line(project, &machine.disk.base);
    let mut descriptions: Vec<&str> = changes
        .iter()
        .map(|change| change.description.trim())
        .filter(|description| !description.is_empty())
        .collect();
    descriptions.sort_unstable();
    let before = descriptions.len();
    descriptions.dedup();
    assert_eq!(before, descriptions.len(), "a change is on trunk twice");
    // On trunk, or on main's stack, which an update left behind trunk
    // until main catches up.
    let on_trunk = |file: &str| {
        tips.iter()
            .any(|tip| project.file_at(tip, file).unwrap().is_some())
            && descriptions.iter().any(|description| {
                brings(description, file)
                    || description.strip_prefix("up: ") == Some(file)
            })
    };
    for file in &machine.on_trunk {
        assert!(on_trunk(file), "{file} is not on trunk");
    }

    let records: Vec<LandingRecord> = host
        .block_on(life.raw.plugin_entries(&main.0, LANDING_RECORD))
        .unwrap()
        .iter()
        .filter_map(|(_, body)| serde_json::from_str(body).ok())
        .collect();
    for chat in &machine.chats {
        let id = &chat.id;
        let ending = host.block_on(host.ending_of(id)).unwrap();
        let landings = records
            .iter()
            .filter(|record| *record.from == *id.0)
            .count();
        let head = project.head(&Head::Run(id.clone())).unwrap();
        match chat.fate {
            Fate::Open => {
                assert_eq!(ending, None, "{}", id.0);
                assert_eq!(landings, 0, "{} has a landing's record", id.0);
                for file in &chat.files {
                    assert!(!on_trunk(file), "{file} of open {} landed", id.0);
                }
                // It keeps the head its commits are on.
                if !chat.files.is_empty() {
                    assert!(head.is_some(), "open {} lost its head", id.0);
                }
            }
            Fate::Landed => {
                assert!(
                    matches!(ending, Some(Ending::Landed { ref on, .. }) if on == main),
                    "{}: {ending:?}",
                    id.0
                );
                assert_eq!(landings, 1, "{} landed {landings} times", id.0);
                for file in &chat.files {
                    assert!(
                        on_trunk(file),
                        "{file} of landed {} is gone",
                        id.0
                    );
                }
                assert_eq!(head, None, "landed {} keeps its head", id.0);
            }
            Fate::Dropped => {
                assert_eq!(ending, Some(Ending::Dropped), "{}", id.0);
                assert_eq!(landings, 0, "dropped {} landed", id.0);
                for file in chat.files.iter().chain(&chat.maybe) {
                    assert!(!on_trunk(file), "{file} of dropped {}", id.0);
                }
            }
        }
    }

    // The queue is what its records give, and holds only what may land.
    let stored: Vec<Record> = host
        .block_on(life.raw.plugin_entries(&main.0, QUEUE_PLUGIN))
        .unwrap()
        .iter()
        .filter_map(|(_, body)| serde_json::from_str(body).ok())
        .collect();
    let lane = host.block_on(host.landing_queue(main)).unwrap();
    assert_eq!(Lane::restore(&stored).waiting(), lane.0.as_slice());
    for waiting in &lane.0 {
        let chat = machine.chats.iter().find(|chat| *chat.id.0 == waiting.run);
        assert!(
            chat.is_none_or(|chat| chat.fate == Fate::Open),
            "{} waits after it ended",
            waiting.run
        );
    }

    // Each message a turn took is in its chat.
    for (run, text) in &machine.said {
        let said = life.said(run);
        assert!(
            said.iter().any(|said| said.contains(text.as_str())),
            "{}'s message {text:?} is lost: {said:?}",
            run.0
        );
    }

    // No workspace serves two runs.
    let runs = host.block_on(life.raw.retained_runs()).unwrap();
    for record in &runs {
        let entries = host
            .block_on(
                life.raw
                    .plugin_entries(&record.id, tau_ui_plugin::HOST_RECORD),
            )
            .unwrap();
        for (_, body) in entries {
            let Ok(tag) =
                serde_json::from_str::<tau_ui_plugin::HostRecord>(&body)
            else {
                continue;
            };
            let Some(dir) = tag.workspace else { continue };
            let name = Path::new(&dir)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let first = machine
                .workspaces
                .entry(name.clone())
                .or_insert_with(|| record.id.clone());
            assert_eq!(*first, record.id, "{name} served two runs");
        }
    }

    // A sub-agent that finished lands on main, or waits to.
    let queued = queued(life, main);
    for sub in &machine.sub_agents {
        if life.status(&sub.id) == Some(Status::Done)
            && !queued.contains(&sub.id.0.to_string())
            && !machine.maybe_on_trunk.contains(&sub.file)
        {
            assert!(on_trunk(&sub.file), "sub-agent {} is stranded", sub.id.0);
        }
    }
}

fn run_machine(tc: TestCase, steps: i64) {
    hegel::stateful::machine(Machine::new())
        .steps(steps)
        .run(tc);
}

#[hegel::test(
    test_cases = 50,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn a_crash_anywhere_loses_nothing(tc: TestCase) {
    run_machine(tc, 12);
}

#[ignore = "nightly"]
#[hegel::test(profile = "nightly_slow")]
fn a_crash_anywhere_loses_nothing_nightly(tc: TestCase) {
    run_machine(tc, 30);
}

/// A sub-agent finished, and tau closed as main's turn ended, before it
/// took the sub-agent's end (found by `a_crash_anywhere_loses_nothing`).
/// Started again, tau puts it in main's queue as if it had never closed,
/// and it lands.
#[test]
fn a_sub_agent_that_finished_as_tau_closed_lands_at_start() {
    let mut machine = Machine::new();
    // Main's turn catches up as it starts, then again as it ends.
    machine.life().fuse.arm_at("catch_up", 1);
    machine.spawn();
    assert!(machine.blown());
    machine.reopen();
    machine.settle_cut();
    check(&mut machine);
}

/// The host answers a new chat, or a message to one, only once the
/// person's message is stored: tau closing before then fails it, and
/// what it answered outlives tau closing (found by
/// `a_crash_anywhere_loses_nothing`: the host showed a chat whose run it
/// never stored, and it was gone at the next start).
#[test]
fn a_chat_tau_showed_outlives_tau_closing() {
    // Before anything of the chat is stored, then before its message.
    for at in ["create_run", "message user"] {
        let mut machine = Machine::new();
        let text = "«write c1.txt»";
        let life = machine.life();
        life.fuse.arm_at(at, 0);
        let started = life.host.block_on(life.host.start(
            text,
            &ModelChoice::default(),
            REPO,
        ));
        if let Ok(view) = &started {
            life.wait(&view.id);
        }
        assert!(machine.blown());
        machine.reopen();
        if let Ok(view) = started {
            let said = machine.life().said(&view.id);
            assert!(
                said.iter().any(|said| said.contains(text)),
                "tau closed at {at}: the chat tau showed is gone"
            );
        }
    }
    // A message to a chat, cut off before it is stored.
    let mut machine = Machine::new();
    machine.start_chat();
    let chat = machine.chats[0].id.clone();
    let text = "«write c9.txt»";
    let life = machine.life();
    life.fuse.arm_at("message user", 0);
    let resumed = life.host.block_on(life.host.resume(
        &chat,
        text,
        &ModelChoice::default(),
    ));
    life.wait(&chat);
    assert!(machine.blown());
    machine.reopen();
    if resumed.is_ok() {
        let said = machine.life().said(&chat);
        assert!(
            said.iter().any(|said| said.contains(text)),
            "the message tau took is gone: {said:?}"
        );
    }
}

/// tau closed as a chat started, before its first turn recorded where
/// it works (found by `a_crash_anywhere_loses_nothing`). Started again,
/// the chat took no message: every one failed, as it worked in no
/// listed repository. It works in its main chat's.
#[test]
fn a_chat_cut_off_before_its_first_turn_takes_a_message() {
    let mut machine = Machine::new();
    machine.life().fuse.arm_at("record tau-host", 0);
    machine.start_chat();
    assert!(machine.blown());
    machine.reopen();
    machine.settle_cut();
    let chat = machine
        .chats
        .first()
        .expect("the chat is stored")
        .id
        .clone();
    machine.write_to(&chat);
    let files = &machine.chat_mut(&chat).files;
    assert_eq!(files.len(), 1, "it took the message whole");
    check(&mut machine);
}

/// What [`every_write_is_a_place_to_close`] cuts off.
#[derive(Debug, Clone, Copy)]
enum Cut {
    StartChat,
    WriteToChat,
    WriteToMain,
    Spawn,
    LandQueued,
    LandNow,
    Drop,
    Upstream,
}

/// Each step, with tau closing before each of its writes in turn, from
/// the first to the last: what the drawn crashes of
/// [`a_crash_anywhere_loses_nothing`] reach by chance, walked whole.
/// After each start, the checks hold, and every chat open and main take
/// a message again.
#[test]
fn every_write_is_a_place_to_close() {
    let cuts = [
        Cut::StartChat,
        Cut::WriteToChat,
        Cut::WriteToMain,
        Cut::Spawn,
        Cut::LandQueued,
        Cut::LandNow,
        Cut::Drop,
        Cut::Upstream,
    ];
    for cut in cuts {
        for writes in 0.. {
            // Main with a commit, and a chat with one.
            let mut machine = Machine::new();
            machine.write_to_main();
            machine.start_chat();
            let chat = machine.chats[0].id.clone();
            machine.life().fuse.arm(writes);
            match cut {
                Cut::StartChat => machine.start_chat(),
                Cut::WriteToChat => machine.write_to(&chat),
                Cut::WriteToMain => machine.write_to_main(),
                Cut::Spawn => machine.spawn(),
                Cut::LandQueued => machine.land_queued(&chat),
                Cut::LandNow => machine.land_now(&chat),
                Cut::Drop => machine.drop_one(&chat),
                Cut::Upstream => machine.upstream(),
            }
            if !machine.blown() {
                break;
            }
            let at = machine.life.as_ref().unwrap().fuse.blown();
            eprintln!("{cut:?}: tau closed at write {writes}, {at:?}");
            machine.closed();
            check(&mut machine);
            let open: Vec<RunId> = machine
                .chats
                .iter()
                .filter(|chat| chat.fate == Fate::Open)
                .map(|chat| chat.id.clone())
                .collect();
            for chat in open {
                machine.write_to(&chat);
            }
            machine.write_to_main();
            check(&mut machine);
        }
    }
}

/// The threads this process has.
fn threads() -> usize {
    std::fs::read_dir("/proc/self/task").unwrap().count()
}

/// tau closing lets go of all it opened: the store's connections and
/// their threads too (found by `a_crash_anywhere_loses_nothing_nightly`:
/// tau-mcp's tools outside runs, among the host's services, kept a
/// `HostCx`, which carries those services, and the host's model, also
/// among them, kept the slots, whose fillers keep a `HostCx`: so every
/// start's store outlived it, until no thread could be made).
#[test]
fn closing_lets_go_of_what_tau_opened() {
    let mut machine = Machine::new();
    machine.reopen();
    let before = threads();
    for _ in 0..8 {
        machine.reopen();
    }
    // A connection's thread ends a moment after its store is dropped.
    let mut after = threads();
    for _ in 0..50 {
        if after <= before + 4 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        after = threads();
    }
    assert!(after <= before + 4, "{before} threads, then {after}");
}
