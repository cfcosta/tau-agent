//! The agent loop (`docs/reference/agent-loop.md`) against a model of
//! it, one step at a time. The test is the world the run lives in: it
//! streams the model's response block by block and ends it with tool
//! calls or an answer, or fails it (in a way that may be retried, one
//! that may not, or with the context too long); it ends tools or has
//! them call other tools; the person steers and cancels; a plugin sends
//! an answer back (`before_stop`); compaction rewrites, declines or
//! fails; a store write fails; and a run that ended starts again, fresh
//! or resumed. After each step the test waits until the run waits on it
//! again (time is paused: the clock moves only when every task is idle),
//! and checks where the run is against where the model says it is: a
//! response streaming, these tool calls running, or the run ended, and
//! how.
//!
//! What the model holds to after every step:
//! - every steer is read once, in the order sent, or left unread at the
//!   run's end for the host to hand on (ADR 0026); a run that stops has
//!   read every steer, and a steer the run says it read is stored;
//! - every request, and the stored transcript, is valid for the
//!   provider: each tool call has exactly one result, right after its
//!   message;
//! - after a cancel nothing more is asked, and the run ends cancelled;
//! - a failed or declined compaction never stops the loop offering the
//!   next one;
//! - retries keep to the policy: nothing is asked after a failure that
//!   may not be retried or after the last attempt, and nothing before the
//!   backoff is over;
//! - the store replays what the run saw: what it stores is the last
//!   request's transcript and the one the run finished with, and what the
//!   person and plugins said is stored in the order the events said it.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

mod common;

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    pin::Pin,
    sync::{
        Arc,
        Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    task::{Context, Poll, Waker},
    time::Duration,
};

use async_trait::async_trait;
use common::{assert_grammar, assert_results_follow_calls, counter_clock};
use futures_util::{FutureExt, Stream, future::BoxFuture};
use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_agent::{
    agent::{Agent, AgentError, Outcome, RunControl},
    error::{PluginError, ToolError},
    event::{RunEvent, StopReason},
    plugin::{
        ContextView,
        FinishedRun,
        Plugin,
        PluginCtx,
        PluginRun,
        Rewrite,
        RunPlan,
        StopDecision,
        Trigger,
    },
    tool::{AgentTool, ExecutionMode, Exposure, RunId, ToolCtx, ToolOutput},
};
use tau_ai::{
    event::{AssistantEvent, DoneReason, ErrorReason},
    llm::{EventStream, Llm, LlmError, LlmSession},
    message::{
        AssistantMessage,
        Message,
        TextContent,
        ThinkingContent,
        Timestamp,
        ToolCall,
        Usage,
        UserContent,
        UserMessage,
    },
    responses::request::{ReasoningEffort, Settings},
    retry::{Class, RetryPolicy},
};
use tau_store::{
    AgentCost,
    Backend,
    Entry,
    NewRun,
    PluginCost,
    RunRecord,
    Status,
    Store,
    StoreError,
    Table,
    TurnUsage,
};
use tokio::{runtime::Runtime, sync::mpsc, task::JoinHandle, time::Instant};

/// The tools the model calls: one that runs alongside the others, one
/// that makes its batch run one call at a time, and one nobody has.
const WORK: &str = "work";
const SERIAL: &str = "serial";
const MISSING: &str = "missing";
/// The tool tools call, never declared to the model.
const INNER: &str = "inner";
/// How a compaction's summary starts.
const SUMMARY: &str = "summary";

/// Orders what the run emitted against what it asked the model.
type Ticks = Arc<AtomicU64>;

// The model's side: a response the test streams event by event.

/// One response: the events the test pushed and the loop has not read.
#[derive(Default)]
struct Pipe {
    events: VecDeque<AssistantEvent>,
    waker: Option<Waker>,
    /// The loop found nothing to read, and waits.
    waiting: bool,
    /// The loop read its terminal event.
    ended: bool,
    /// The loop dropped it: a cancel or a steer cut it short.
    dropped: bool,
}

struct PipeStream(Arc<Mutex<Pipe>>);

impl Stream for PipeStream {
    type Item = AssistantEvent;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<AssistantEvent>> {
        let mut pipe = self.0.lock().unwrap();
        if let Some(event) = pipe.events.pop_front() {
            pipe.waiting = false;
            if matches!(
                event,
                AssistantEvent::Done { .. } | AssistantEvent::Error { .. }
            ) {
                pipe.ended = true;
            }
            return Poll::Ready(Some(event));
        }
        if pipe.ended {
            return Poll::Ready(None);
        }
        pipe.waiting = true;
        pipe.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

impl Drop for PipeStream {
    fn drop(&mut self) {
        self.0.lock().unwrap().dropped = true;
    }
}

/// A request the model had: when, and the transcript it answers.
struct Request {
    tick: u64,
    at: Instant,
    transcript: Vec<Message>,
}

#[derive(Default)]
struct Wire {
    requests: Vec<Request>,
    /// The response the loop reads now, or read last.
    pipe: Option<Arc<Mutex<Pipe>>>,
}

/// An [`Llm`] whose every response the test streams.
#[derive(Clone)]
struct Puppet {
    wire: Arc<Mutex<Wire>>,
    ticks: Ticks,
}

struct PuppetSession {
    puppet: Puppet,
    settings: Settings,
}

impl Llm for Puppet {
    fn open(
        &self,
        settings: Settings,
    ) -> BoxFuture<'static, Result<Box<dyn LlmSession>, LlmError>> {
        let session = PuppetSession {
            puppet: self.clone(),
            settings,
        };
        futures_util::future::ready(
            Ok(Box::new(session) as Box<dyn LlmSession>),
        )
        .boxed()
    }
}

impl LlmSession for PuppetSession {
    fn settings(&self) -> &Settings {
        &self.settings
    }

    fn set_reasoning(&mut self, effort: Option<ReasoningEffort>) {
        self.settings.reasoning = effort;
    }

    fn respond(
        &mut self,
        transcript: &[Message],
        timestamp: Timestamp,
    ) -> EventStream {
        let pipe = Arc::new(Mutex::new(Pipe::default()));
        pipe.lock()
            .unwrap()
            .events
            .push_back(AssistantEvent::Start {
                model: self.settings.model.clone(),
                response_id: None,
                timestamp,
            });
        let mut wire = self.puppet.wire.lock().unwrap();
        wire.requests.push(Request {
            tick: self.puppet.ticks.fetch_add(1, Ordering::SeqCst),
            at: Instant::now(),
            transcript: transcript.to_vec(),
        });
        wire.pipe = Some(pipe.clone());
        Box::pin(PipeStream(pipe))
    }
}

// The tools' side: calls that wait for the test to end them.

enum Command {
    End {
        ok: bool,
    },
    /// Call [`INNER`], and keep waiting.
    Nest,
}

/// The calls running now, by id, and how to reach each.
#[derive(Default)]
struct Desk {
    running: BTreeMap<String, mpsc::UnboundedSender<Command>>,
}

/// A tool that runs until the test ends it or the run cancels it, and
/// calls [`INNER`] when told to.
struct Worker {
    name: &'static str,
    mode: ExecutionMode,
    exposure: Exposure,
    schema: Value,
    desk: Arc<Mutex<Desk>>,
}

impl Worker {
    fn new(
        name: &'static str,
        mode: ExecutionMode,
        exposure: Exposure,
        desk: &Arc<Mutex<Desk>>,
    ) -> Self {
        Self {
            name,
            mode,
            exposure,
            schema: json!({"type": "object"}),
            desk: desk.clone(),
        }
    }
}

enum Next {
    Cancelled,
    Command(Option<Command>),
    Answered,
}

#[async_trait]
impl AgentTool for Worker {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        "Works until told to stop."
    }
    fn parameters(&self) -> &Value {
        &self.schema
    }
    fn execution_mode(&self) -> ExecutionMode {
        self.mode
    }
    fn exposure(&self) -> Exposure {
        self.exposure
    }
    async fn call(
        &self,
        _args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let id = ctx.call_id().to_owned();
        let (commands_tx, mut commands) = mpsc::unbounded_channel();
        self.desk
            .lock()
            .unwrap()
            .running
            .insert(id.clone(), commands_tx);
        let mut nested: Option<
            BoxFuture<'static, Result<ToolOutput, ToolError>>,
        > = None;
        let result = loop {
            let calling = nested.is_some();
            let next = tokio::select! {
                biased;
                _ = ctx.cancel.cancelled() => Next::Cancelled,
                command = commands.recv() => Next::Command(command),
                _ = async { nested.as_mut().expect("guarded").await },
                    if calling => Next::Answered,
            };
            match next {
                Next::Cancelled => break Err("cancelled".into()),
                Next::Command(Some(Command::End { ok: true })) => {
                    break Ok(ToolOutput::text(format!("{id} done")));
                }
                Next::Command(Some(Command::End { ok: false }))
                | Next::Command(None) => {
                    break Err(format!("{id} failed").into());
                }
                Next::Command(Some(Command::Nest)) => {
                    let ctx = ctx.clone();
                    nested = Some(
                        async move { ctx.call(INNER, json!({})).await }.boxed(),
                    );
                }
                Next::Answered => nested = None,
            }
        };
        self.desk.lock().unwrap().running.remove(&id);
        result
    }
}

// The plugins' side: one plugin that records, sends answers back and
// compacts.

/// What compaction does when offered the context.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    hegel::DefaultGenerator,
    hegel::PrettyPrintable,
)]
enum Compaction {
    #[default]
    Declines,
    Fails,
    Rewrites,
}

/// What one start of a run showed the plugin.
#[derive(Default)]
struct Start {
    events: Vec<(u64, Instant, RunEvent)>,
    /// Each offer of the context: why, and the turn.
    offers: Vec<(Trigger, u32)>,
    /// The transcript the run finished with.
    finished: Option<Vec<Message>>,
}

#[derive(Default)]
struct World {
    starts: Vec<Start>,
    /// Answers to send back.
    refusals: u32,
    compaction: Compaction,
    summaries: u32,
    sent_back: u32,
}

#[derive(Clone)]
struct Harness {
    world: Arc<Mutex<World>>,
    ticks: Ticks,
}

struct HarnessRun {
    world: Arc<Mutex<World>>,
    ticks: Ticks,
    start: usize,
}

#[async_trait]
impl Plugin for Harness {
    fn name(&self) -> &str {
        "harness"
    }

    async fn start(
        &self,
        _plan: &mut RunPlan,
        _ctx: &PluginCtx,
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        let mut world = self.world.lock().unwrap();
        world.starts.push(Start::default());
        Ok(Box::new(HarnessRun {
            world: self.world.clone(),
            ticks: self.ticks.clone(),
            start: world.starts.len() - 1,
        }))
    }
}

#[async_trait]
impl PluginRun for HarnessRun {
    async fn on_event(&mut self, event: &RunEvent, _ctx: &PluginCtx) {
        let tick = self.ticks.fetch_add(1, Ordering::SeqCst);
        self.world.lock().unwrap().starts[self.start].events.push((
            tick,
            Instant::now(),
            event.clone(),
        ));
    }

    async fn rewrite_context(
        &mut self,
        view: &ContextView<'_>,
        _ctx: &PluginCtx,
    ) -> Result<Option<Rewrite>, PluginError> {
        let mut world = self.world.lock().unwrap();
        world.starts[self.start]
            .offers
            .push((view.trigger, view.turn));
        match world.compaction {
            Compaction::Declines => Ok(None),
            Compaction::Fails => {
                Err(PluginError::Message("the summary failed".into()))
            }
            Compaction::Rewrites => {
                world.summaries += 1;
                let n = world.summaries;
                Ok(Some(Rewrite {
                    messages: compact(view.transcript, n),
                    drops_conversation: true,
                    details: json!({ "summary": n }),
                }))
            }
        }
    }

    async fn before_stop(
        &mut self,
        _message: &AssistantMessage,
        _ctx: &PluginCtx,
    ) -> Result<StopDecision, PluginError> {
        let mut world = self.world.lock().unwrap();
        if world.refusals == 0 {
            return Ok(StopDecision::Stop);
        }
        world.refusals -= 1;
        world.sent_back += 1;
        Ok(StopDecision::Continue(format!(
            "not done yet {}",
            world.sent_back
        )))
    }

    async fn finish(&mut self, run: &FinishedRun<'_>, _ctx: &PluginCtx) {
        self.world.lock().unwrap().starts[self.start].finished =
            Some(run.transcript.to_vec());
    }
}

fn user(text: String) -> Message {
    Message::User(UserMessage {
        content: UserContent::Text(text),
        timestamp: 0,
    })
}

/// A user message's text.
fn said(message: &Message) -> Option<String> {
    match message {
        Message::User(user) => Some(user.content.text()),
        _ => None,
    }
}

fn is_summary(message: &Message) -> bool {
    said(message).is_some_and(|text| text.starts_with(SUMMARY))
}

/// A summary of `transcript`: every turn but the last goes, every user
/// message stays. The last message's turn stays whole, so its calls keep
/// their results.
fn compact(transcript: &[Message], n: u32) -> Vec<Message> {
    let tail = match transcript.last() {
        Some(Message::ToolResult(_)) => transcript
            .iter()
            .rposition(|m| matches!(m, Message::Assistant(_)))
            .unwrap_or(0),
        _ => transcript.len().saturating_sub(1),
    };
    let mut messages = vec![user(format!("{SUMMARY} {n}"))];
    messages.extend(
        transcript[..tail]
            .iter()
            .filter(|m| matches!(m, Message::User(_)) && !is_summary(m))
            .cloned(),
    );
    messages.extend(transcript[tail..].iter().cloned());
    messages
}

// The store's side: one that fails the write it is told to, and counts
// what it is doing.

#[derive(Debug)]
struct Flaky {
    inner: Store,
    /// How many writes go through before one fails.
    armed: Mutex<Option<u32>>,
    /// Calls on their way: sqlite answers from a thread of its own, which
    /// paused time does not wait for.
    busy: AtomicUsize,
}

impl Flaky {
    async fn track<T>(&self, call: impl Future<Output = T>) -> T {
        self.busy.fetch_add(1, Ordering::SeqCst);
        let out = call.await;
        self.busy.fetch_sub(1, Ordering::SeqCst);
        out
    }
}

#[async_trait]
impl Backend for Flaky {
    async fn query(&self, sql: &str, limit: usize) -> tau_store::Result<Table> {
        self.track(self.inner.query(sql, limit)).await
    }
    async fn create_run(&self, run: &NewRun<'_>) -> tau_store::Result<()> {
        self.track(self.inner.create_run(run)).await
    }
    async fn append_charged(
        &self,
        run: &str,
        entries: &[Entry],
        usage: TurnUsage,
        plugins: &[(&str, TurnUsage)],
    ) -> tau_store::Result<i64> {
        let fails = {
            let mut armed = self.armed.lock().unwrap();
            match *armed {
                Some(0) => {
                    *armed = None;
                    true
                }
                Some(n) => {
                    *armed = Some(n - 1);
                    false
                }
                None => false,
            }
        };
        if fails {
            return Err(StoreError::Database("the disk is full".into()));
        }
        self.track(self.inner.append_charged(run, entries, usage, plugins))
            .await
    }
    async fn plugin_costs(
        &self,
        run: &str,
    ) -> tau_store::Result<Vec<PluginCost>> {
        self.track(self.inner.plugin_costs(run)).await
    }
    async fn charge_plugin(
        &self,
        plugin: &str,
        usage: TurnUsage,
    ) -> tau_store::Result<()> {
        self.track(self.inner.charge_plugin(plugin, usage)).await
    }
    async fn plugin_spend(
        &self,
        since: &str,
    ) -> tau_store::Result<Vec<(String, f64)>> {
        self.track(self.inner.plugin_spend(since)).await
    }
    async fn transcript(&self, run: &str) -> tau_store::Result<Vec<Entry>> {
        self.track(self.inner.transcript(run)).await
    }
    async fn first_prompt(
        &self,
        run: &str,
    ) -> tau_store::Result<Option<String>> {
        self.track(self.inner.first_prompt(run)).await
    }
    async fn timeline(&self, run: &str) -> tau_store::Result<Vec<Entry>> {
        self.track(self.inner.timeline(run)).await
    }
    async fn records(
        &self,
        run: &str,
        plugin: &str,
    ) -> tau_store::Result<Vec<String>> {
        self.track(self.inner.records(run, plugin)).await
    }
    async fn finish_run(
        &self,
        run: &str,
        status: Status,
        result: Option<&str>,
        error: Option<&str>,
    ) -> tau_store::Result<()> {
        self.track(self.inner.finish_run(run, status, result, error))
            .await
    }
    async fn interrupt_running(&self) -> tau_store::Result<Vec<String>> {
        self.track(self.inner.interrupt_running()).await
    }
    async fn set_title(&self, run: &str, title: &str) -> tau_store::Result<()> {
        self.track(self.inner.set_title(run, title)).await
    }
    async fn reopen_run(
        &self,
        run: &str,
        model: &str,
    ) -> tau_store::Result<RunRecord> {
        self.track(self.inner.reopen_run(run, model)).await
    }
    async fn subagents(
        &self,
        parent: &str,
    ) -> tau_store::Result<Vec<RunRecord>> {
        self.track(self.inner.subagents(parent)).await
    }
    async fn run(&self, run: &str) -> tau_store::Result<Option<RunRecord>> {
        self.track(self.inner.run(run)).await
    }
    async fn recent_runs(
        &self,
        limit: u32,
    ) -> tau_store::Result<Vec<RunRecord>> {
        self.track(self.inner.recent_runs(limit)).await
    }
    async fn retained_runs(&self) -> tau_store::Result<Vec<RunRecord>> {
        self.track(self.inner.retained_runs()).await
    }
    async fn retention_entries(
        &self,
        run: &str,
    ) -> tau_store::Result<Vec<(i64, String)>> {
        self.track(self.inner.retention_entries(run)).await
    }
    async fn plugin_entries(
        &self,
        run: &str,
        plugin: &str,
    ) -> tau_store::Result<Vec<(i64, String)>> {
        self.track(self.inner.plugin_entries(run, plugin)).await
    }
    async fn plugin_entries_everywhere(
        &self,
        plugin: &str,
    ) -> tau_store::Result<Vec<(String, String)>> {
        self.track(self.inner.plugin_entries_everywhere(plugin))
            .await
    }
    async fn workflow_cost(
        &self,
        workflow: &str,
    ) -> tau_store::Result<Vec<AgentCost>> {
        self.track(self.inner.workflow_cost(workflow)).await
    }
    async fn spent_since(
        &self,
        runs: &[String],
        since: &str,
    ) -> tau_store::Result<f64> {
        self.track(self.inner.spent_since(runs, since)).await
    }
}

// The model.

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    Stop,
    Cancelled,
    Error,
    /// A store write failed: the run fails with the store's error, and
    /// without a `RunEnd`.
    Store,
}

/// What the run waits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The model's response.
    Streaming,
    /// The tool calls [`Tools::running`].
    Tools,
    Ended(End),
}

/// Where the run was seen waiting.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Seen {
    Streaming,
    Tools(BTreeSet<String>),
    Ended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, hegel::PrettyPrintable)]
enum Block {
    Text,
    Thinking,
    Call,
    /// A text block left open: a cut keeps what streamed of it.
    OpenText,
    /// A tool call left open: a cut drops it.
    OpenCall,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    hegel::DefaultGenerator,
    hegel::PrettyPrintable,
)]
enum Failure {
    Retryable,
    Fatal,
    Overflow,
}

/// The block streaming now.
#[derive(Debug)]
enum Open {
    Text { index: usize, text: String },
    Call { index: usize, call: ToolCall },
}

/// The response streaming now.
#[derive(Debug)]
struct Response {
    /// Its finished tool calls, in order, with their tools.
    calls: Vec<(String, &'static str)>,
    open: Option<Open>,
    next_index: usize,
    /// The attempt this is, of the turn's request.
    attempts: u32,
    /// The request is the one retried after an overflow.
    overflowed: bool,
}

impl Response {
    fn new(attempts: u32, overflowed: bool) -> Self {
        Self {
            calls: Vec::new(),
            open: None,
            next_index: 0,
            attempts,
            overflowed,
        }
    }
}

/// A tool batch.
#[derive(Debug, Default)]
struct Tools {
    running: BTreeSet<String>,
    /// Calls of a batch that runs one at a time, not started yet.
    queued: VecDeque<String>,
    /// Calls waiting on a call they made.
    calling: BTreeSet<String>,
    /// How many calls each call made.
    made: BTreeMap<String, u32>,
}

impl Tools {
    /// `id` returned: the calls it left running are cancelled, and its
    /// caller has its answer.
    fn ended(&mut self, id: &str) {
        let nested = format!("{id}/");
        self.running
            .retain(|call| call != id && !call.starts_with(&nested));
        self.calling
            .retain(|call| call != id && !call.starts_with(&nested));
        if let Some((caller, _)) = id.rsplit_once('/') {
            self.calling.remove(caller);
        }
    }
}

/// One start of a run, as the model sees it.
#[derive(Debug, Default)]
struct Steers {
    /// Sent while the run went on, in order.
    sent: Vec<String>,
    read: Vec<String>,
    /// Waiting in the channel.
    queue: VecDeque<String>,
    /// The one that cut the response short: the next one read.
    preempted: Option<String>,
    /// Sent after the run ended.
    late: Vec<String>,
}

#[derive(Debug)]
struct Model {
    phase: Phase,
    response: Response,
    tools: Tools,
    steers: Steers,
    /// Requests the model had, over every run.
    requests: usize,
    /// `Retry` events, over every run.
    retries: usize,
    max_attempts: u32,
    /// How many store writes go through before one fails.
    armed: Option<u32>,
    refusals: u32,
    compaction: Compaction,
    continuations: u32,
    cancelled: bool,
}

impl Model {
    /// A store write: false when it fails, and so ends the run.
    #[must_use]
    fn write(&mut self) -> bool {
        match self.armed {
            Some(0) => {
                self.armed = None;
                self.phase = Phase::Ended(End::Store);
                false
            }
            Some(n) => {
                self.armed = Some(n - 1);
                true
            }
            None => true,
        }
    }

    /// The context is offered for compaction: false when the rewrite's
    /// write fails.
    #[must_use]
    fn offer(&mut self) -> bool {
        self.compaction != Compaction::Rewrites || self.write()
    }

    /// A run starts: its input is stored, a resumed run's transcript is
    /// offered for compaction, and its first turn starts.
    fn start(&mut self, resumed: bool) {
        self.steers = Steers::default();
        self.continuations = 0;
        self.cancelled = false;
        self.tools = Tools::default();
        if !self.write() || (resumed && !self.offer()) {
            return;
        }
        self.next_turn();
    }

    /// A turn starts and asks the model. A steer already waiting cuts
    /// the response short before anything in it finished.
    fn next_turn(&mut self) {
        self.requests += 1;
        match self.steers.queue.pop_front() {
            Some(steer) => {
                self.steers.preempted = Some(steer);
                self.end_turn(false, None);
            }
            None => {
                self.response = Response::new(1, false);
                self.phase = Phase::Streaming;
            }
        }
    }

    /// The response is in and its tools are done: the turn is stored,
    /// and the run ends or goes on, reading a steer or sending the answer
    /// back first.
    fn end_turn(&mut self, called: bool, failed: Option<End>) {
        if !self.write() {
            return;
        }
        if let Some(end) = failed {
            self.phase = Phase::Ended(end);
            return;
        }
        if self.cancelled {
            self.phase = Phase::Ended(End::Cancelled);
            return;
        }
        let steer = self
            .steers
            .preempted
            .take()
            .or_else(|| self.steers.queue.pop_front());
        if let Some(steer) = steer {
            // Read once stored: one the run could not store stays unread.
            if !self.write() {
                return;
            }
            self.steers.read.push(steer);
        } else if !called {
            if self.continuations == 3 || self.refusals == 0 {
                self.phase = Phase::Ended(End::Stop);
                return;
            }
            self.refusals -= 1;
            self.continuations += 1;
            if !self.write() {
                return;
            }
        }
        if self.offer() {
            self.next_turn();
        }
    }

    /// The response ended with `calls`: they run, or fail at once.
    fn run_calls(&mut self, calls: &[(String, &'static str)]) {
        let ready: Vec<&String> = calls
            .iter()
            .filter(|(_, tool)| *tool != MISSING)
            .map(|(id, _)| id)
            .collect();
        if ready.is_empty() {
            self.end_turn(true, None);
            return;
        }
        self.tools = Tools::default();
        if calls.iter().any(|(_, tool)| *tool == SERIAL) {
            self.tools.running.insert(ready[0].clone());
            self.tools.queued =
                ready[1..].iter().map(|id| (*id).clone()).collect();
        } else {
            self.tools.running = ready.into_iter().cloned().collect();
        }
        self.phase = Phase::Tools;
    }

    fn answered(&mut self, length: bool) {
        let calls = std::mem::take(&mut self.response.calls);
        if calls.is_empty() {
            self.end_turn(false, None);
        } else if length {
            // Cut off by the output limit: every call fails, unrun.
            self.end_turn(true, None);
        } else {
            self.run_calls(&calls);
        }
    }

    fn failed(&mut self, failure: Failure) {
        let Response {
            attempts,
            overflowed,
            ..
        } = self.response;
        match failure {
            Failure::Retryable if attempts < self.max_attempts => {
                self.retries += 1;
                self.requests += 1;
                self.response = Response::new(attempts + 1, overflowed);
            }
            Failure::Overflow
                if !overflowed && self.compaction == Compaction::Rewrites =>
            {
                if self.write() {
                    self.requests += 1;
                    self.response = Response::new(1, true);
                }
            }
            _ => self.end_turn(false, Some(End::Error)),
        }
    }

    fn steered(&mut self, steer: String) {
        match self.phase {
            Phase::Streaming => {
                self.steers.sent.push(steer.clone());
                self.steers.preempted = Some(steer);
                // What finished is kept: its calls run first. An open
                // call is dropped.
                let calls = std::mem::take(&mut self.response.calls);
                if calls.is_empty() {
                    self.end_turn(false, None);
                } else {
                    self.run_calls(&calls);
                }
            }
            Phase::Tools => {
                self.steers.sent.push(steer.clone());
                self.steers.queue.push_back(steer);
            }
            Phase::Ended(_) => self.steers.late.push(steer),
        }
    }

    fn cancel(&mut self) {
        self.cancelled = true;
        match self.phase {
            Phase::Streaming => self.end_turn(false, Some(End::Cancelled)),
            Phase::Tools => {
                self.tools.running.clear();
                self.tools.queued.clear();
                self.end_turn(true, None);
            }
            Phase::Ended(_) => {}
        }
    }

    fn tool_ended(&mut self, id: &str) {
        self.tools.ended(id);
        if !self.tools.running.is_empty() {
            return;
        }
        match self.tools.queued.pop_front() {
            Some(next) => {
                self.tools.running.insert(next);
            }
            None => self.end_turn(true, None),
        }
    }

    fn nested(&mut self, caller: &str) -> String {
        let made = self.tools.made.entry(caller.to_owned()).or_default();
        *made += 1;
        let id = format!("{caller}/{made}");
        self.tools.running.insert(id.clone());
        self.tools.calling.insert(caller.to_owned());
        id
    }
}

// The machine.

struct Machine {
    rt: Runtime,
    store: Arc<Flaky>,
    agent: Agent,
    wire: Arc<Mutex<Wire>>,
    desk: Arc<Mutex<Desk>>,
    world: Arc<Mutex<World>>,
    policy: RetryPolicy,
    run: RunId,
    control: RunControl,
    outcome: Option<JoinHandle<Result<Outcome, AgentError>>>,
    ended: Option<Result<Outcome, AgentError>>,
    /// The first of the run's starts, among the plugin's.
    first_start: usize,
    /// Whether the run's current start is a resumed one.
    resumed: bool,
    /// Requests the model had when the person cancelled.
    cancelled_at: Option<usize>,
    seen: Seen,
    model: Model,
    calls: u32,
    steers: u32,
    inputs: u32,
}

impl Machine {
    fn new(policy: RetryPolicy) -> Self {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .start_paused(true)
            .build()
            .unwrap();
        let ticks = Ticks::default();
        let wire = Arc::new(Mutex::new(Wire::default()));
        let desk = Arc::new(Mutex::new(Desk::default()));
        let world = Arc::new(Mutex::new(World::default()));
        let inner = rt.block_on(tau_store_sqlite::memory()).unwrap();
        let store = Arc::new(Flaky {
            inner,
            armed: Mutex::new(None),
            busy: AtomicUsize::new(0),
        });
        let agent = Agent::new(Puppet {
            wire: wire.clone(),
            ticks: ticks.clone(),
        })
        .tool(Worker::new(
            WORK,
            ExecutionMode::Parallel,
            Exposure::Direct,
            &desk,
        ))
        .tool(Worker::new(
            SERIAL,
            ExecutionMode::Sequential,
            Exposure::Direct,
            &desk,
        ))
        .tool(Worker::new(
            INNER,
            ExecutionMode::Parallel,
            Exposure::Nested,
            &desk,
        ))
        .plugin(Harness {
            world: world.clone(),
            ticks,
        })
        .retry(policy)
        .clock(counter_clock());
        let model = Model {
            phase: Phase::Streaming,
            response: Response::new(1, false),
            tools: Tools::default(),
            steers: Steers::default(),
            requests: 0,
            retries: 0,
            max_attempts: policy.max_attempts,
            armed: None,
            refusals: 0,
            compaction: Compaction::default(),
            continuations: 0,
            cancelled: false,
        };
        let (run, control, outcome) =
            Self::launch(&rt, &agent, &store, None, 0);
        let mut machine = Self {
            rt,
            store,
            agent,
            wire,
            desk,
            world,
            policy,
            run,
            control,
            outcome: Some(outcome),
            ended: None,
            first_start: 0,
            resumed: false,
            cancelled_at: None,
            seen: Seen::Ended,
            model,
            calls: 0,
            steers: 0,
            inputs: 0,
        };
        machine.model.start(false);
        machine.settle();
        machine
    }

    /// Starts a run, or resumes `resume`, on input `start {n}`.
    fn launch(
        rt: &Runtime,
        agent: &Agent,
        store: &Arc<Flaky>,
        resume: Option<&RunId>,
        n: u32,
    ) -> (RunId, RunControl, JoinHandle<Result<Outcome, AgentError>>) {
        let store: Store = store.clone();
        let input = format!("start {n}");
        rt.block_on(async {
            let run = match resume {
                Some(id) => agent.resume(id).start(input, &store),
                None => agent.start(input, &store),
            };
            let control = run.control();
            (run.id(), control, tokio::spawn(run.outcome()))
        })
    }

    /// Where the run waits now, if it waits on the test.
    fn waiting(&self) -> Option<Seen> {
        if self.outcome.as_ref().is_none_or(JoinHandle::is_finished) {
            return Some(Seen::Ended);
        }
        let streaming =
            self.wire.lock().unwrap().pipe.as_ref().is_some_and(|pipe| {
                let pipe = pipe.lock().unwrap();
                pipe.waiting
                    && pipe.events.is_empty()
                    && !pipe.ended
                    && !pipe.dropped
            });
        if streaming {
            return Some(Seen::Streaming);
        }
        let running: BTreeSet<String> =
            self.desk.lock().unwrap().running.keys().cloned().collect();
        (!running.is_empty()).then_some(Seen::Tools(running))
    }

    /// Lets the run go until it waits on the test again or ends.
    fn settle(&mut self) {
        // A guard, not part of the test: a run that never waits again
        // is a failure, not a hang.
        let wall = std::time::Instant::now();
        let seen = self.rt.block_on(async {
            loop {
                assert!(
                    wall.elapsed() < Duration::from_secs(60),
                    "the run neither waits on the test nor ends"
                );
                if self.store.busy.load(Ordering::SeqCst) > 0 {
                    // Not idle: the clock stays where it is.
                    tokio::task::yield_now().await;
                    continue;
                }
                // Returns once every task is idle.
                tokio::time::sleep(Duration::from_millis(1)).await;
                if self.store.busy.load(Ordering::SeqCst) == 0
                    && let Some(seen) = self.waiting()
                {
                    break seen;
                }
            }
        });
        if seen == Seen::Ended
            && let Some(outcome) = self.outcome.take()
        {
            self.ended = Some(self.rt.block_on(outcome).expect("no panic"));
        }
        self.seen = seen;
    }

    fn compact(&mut self, compaction: Compaction) {
        self.world.lock().unwrap().compaction = compaction;
        self.model.compaction = compaction;
    }

    /// The write after `after` more fails.
    fn arm(&mut self, after: u32) {
        *self.store.armed.lock().unwrap() = Some(after);
        self.model.armed = Some(after);
    }

    /// Every invariant, for a regression that drives the machine by hand.
    fn check(&self, tc: &TestCase) {
        self.the_run_is_where_the_model_says(tc.clone());
        self.every_steer_is_read_once_or_left_unread(tc.clone());
        self.every_request_is_valid_for_the_provider(tc.clone());
        self.nothing_is_asked_after_a_cancel(tc.clone());
        self.compaction_is_offered_after_every_turn_that_goes_on(tc.clone());
        self.retries_keep_to_the_policy(tc.clone());
        self.the_store_replays_what_the_run_saw(tc.clone());
    }

    fn push(&self, events: Vec<AssistantEvent>) {
        let wire = self.wire.lock().unwrap();
        let mut pipe = wire.pipe.as_ref().expect("a response").lock().unwrap();
        pipe.events.extend(events);
        if let Some(waker) = pipe.waker.take() {
            waker.wake();
        }
    }

    /// Closes the open block: it finishes as it streamed.
    fn close(&mut self) -> Vec<AssistantEvent> {
        match self.model.response.open.take() {
            None => Vec::new(),
            Some(Open::Text { index, text }) => vec![AssistantEvent::TextEnd {
                index,
                content: TextContent {
                    text,
                    text_signature: None,
                },
            }],
            Some(Open::Call { index, call }) => {
                self.model
                    .response
                    .calls
                    .push((call.id.clone(), tool_of(&call.name)));
                vec![AssistantEvent::ToolCallEnd {
                    index,
                    tool_call: call,
                }]
            }
        }
    }

    fn send(&self, id: &str, command: Command) {
        self.desk.lock().unwrap().running[id]
            .send(command)
            .unwrap_or_else(|_| panic!("{id} is gone"));
    }

    fn requests(&self) -> usize {
        self.wire.lock().unwrap().requests.len()
    }

    /// The run's stored transcript, its latest rewrite applied.
    fn stored(&self) -> Vec<Message> {
        self.rt
            .block_on(self.store.inner.transcript(&self.run.0))
            .unwrap()
            .into_iter()
            .filter_map(|entry| match entry {
                Entry::Message { body, .. } => {
                    Some(serde_json::from_str(&body).unwrap())
                }
                _ => None,
            })
            .collect()
    }

    /// The events of the run's current start.
    fn events(&self) -> Vec<RunEvent> {
        let world = self.world.lock().unwrap();
        world
            .starts
            .last()
            .map(|start| {
                start.events.iter().map(|(_, _, e)| e.clone()).collect()
            })
            .unwrap_or_default()
    }

    /// What the events of the run's every start say the person and the
    /// plugins said, in order.
    fn said_live(&self) -> Vec<String> {
        let world = self.world.lock().unwrap();
        world.starts[self.first_start..]
            .iter()
            .flat_map(|start| &start.events)
            .filter_map(|(_, _, event)| match event {
                RunEvent::Steered { text, .. } => Some(text.clone()),
                RunEvent::Continued { message, .. } => Some(message.clone()),
                _ => None,
            })
            .collect()
    }
}

fn tool_of(name: &str) -> &'static str {
    [WORK, SERIAL, MISSING]
        .into_iter()
        .find(|tool| *tool == name)
        .expect("a model's tool")
}

fn usage() -> Usage {
    Usage {
        input: 100,
        output: 10,
        total_tokens: 110,
        ..Usage::default()
    }
}

/// The most a backoff before attempt `attempt` (the second attempt is
/// 2) may wait, as `retry.rs` computes it.
fn backoff_cap(policy: &RetryPolicy, attempt: u32) -> Duration {
    policy
        .base
        .saturating_mul(1 << (attempt - 2))
        .min(policy.max_delay)
}

#[hegel::state_machine]
impl Machine {
    /// The model streams a block of its response.
    #[rule(weight = 6)]
    fn the_model_streams_a_block(&mut self, tc: TestCase) {
        if self.model.phase != Phase::Streaming {
            tc.reject();
        }
        // Calls first and most: they lead to the tool batches.
        let block = tc.draw(gs::sampled_from(vec![
            Block::Call,
            Block::Text,
            Block::Call,
            Block::Thinking,
            Block::OpenCall,
            Block::OpenText,
        ]));
        let mut events = self.close();
        let index = self.model.response.next_index;
        self.model.response.next_index += 1;
        match block {
            Block::Text => {
                let text = format!("text {index}");
                events.extend([
                    AssistantEvent::TextStart { index },
                    AssistantEvent::TextDelta {
                        index,
                        delta: text.clone(),
                    },
                    AssistantEvent::TextEnd {
                        index,
                        content: TextContent {
                            text,
                            text_signature: None,
                        },
                    },
                ]);
            }
            Block::OpenText => {
                let text = format!("text {index}");
                events.extend([
                    AssistantEvent::TextStart { index },
                    AssistantEvent::TextDelta {
                        index,
                        delta: text.clone(),
                    },
                ]);
                self.model.response.open = Some(Open::Text { index, text });
            }
            Block::Thinking => {
                let thinking = format!("thinking {index}");
                events.extend([
                    AssistantEvent::ThinkingStart { index },
                    AssistantEvent::ThinkingDelta {
                        index,
                        delta: thinking.clone(),
                    },
                    AssistantEvent::ThinkingEnd {
                        index,
                        content: ThinkingContent {
                            thinking,
                            thinking_signature: None,
                            redacted: None,
                        },
                    },
                ]);
            }
            Block::Call | Block::OpenCall => {
                let tool = tc.draw(gs::sampled_from(vec![
                    WORK, WORK, WORK, SERIAL, MISSING,
                ]));
                self.calls += 1;
                let n = self.calls;
                let mut arguments = serde_json::Map::new();
                arguments.insert("n".into(), json!(n));
                let call = ToolCall {
                    id: format!("call_{n}|fc_{n}"),
                    name: tool.into(),
                    arguments,
                };
                events.extend([
                    AssistantEvent::ToolCallStart {
                        index,
                        id: call.id.clone(),
                        name: call.name.clone(),
                    },
                    AssistantEvent::ToolCallDelta {
                        index,
                        delta: format!("{{\"n\":{n}}}"),
                    },
                ]);
                if block == Block::Call {
                    self.model.response.calls.push((call.id.clone(), tool));
                    events.push(AssistantEvent::ToolCallEnd {
                        index,
                        tool_call: call,
                    });
                } else {
                    self.model.response.open = Some(Open::Call { index, call });
                }
            }
        }
        self.push(events);
        self.settle();
    }

    /// The model ends its response: with the tool calls in it, with an
    /// answer, or cut off by the output limit.
    #[rule(weight = 4)]
    fn the_model_ends_its_response(&mut self, tc: TestCase) {
        if self.model.phase != Phase::Streaming {
            tc.reject();
        }
        let length = tc.draw(gs::weighted_booleans(0.15));
        let mut events = self.close();
        let reason = if length {
            DoneReason::Length
        } else if self.model.response.calls.is_empty() {
            DoneReason::Stop
        } else {
            DoneReason::ToolUse
        };
        events.push(AssistantEvent::Done {
            reason,
            usage: usage(),
            response_id: None,
        });
        self.push(events);
        self.model.answered(length);
        self.settle();
    }

    /// The response fails: in a way that may be retried, one that may
    /// not, or because the context is too long.
    #[rule(weight = 2)]
    fn the_response_fails(&mut self, tc: TestCase) {
        if self.model.phase != Phase::Streaming {
            tc.reject();
        }
        let failure = tc.draw(gs::default::<Failure>());
        let (message, class) = match failure {
            Failure::Retryable => ("server_error: try again", Class::Retryable),
            Failure::Fatal => ("insufficient_quota: no credit", Class::Fatal),
            Failure::Overflow => {
                ("context_length_exceeded: too long", Class::ContextOverflow)
            }
        };
        self.push(vec![AssistantEvent::Error {
            reason: ErrorReason::Error,
            message: message.into(),
            usage: Usage::default(),
            class,
        }]);
        self.model.failed(failure);
        self.settle();
    }

    /// A running tool returns, or fails.
    #[rule(weight = 6)]
    fn a_tool_ends(&mut self, tc: TestCase) {
        if self.model.phase != Phase::Tools {
            tc.reject();
        }
        let running: Vec<String> =
            self.model.tools.running.iter().cloned().collect();
        let id = tc.draw(gs::sampled_from(running));
        let ok = tc.draw(gs::booleans());
        self.send(&id, Command::End { ok });
        self.model.tool_ended(&id);
        self.settle();
    }

    /// A running tool calls another through the loop.
    #[rule(weight = 2)]
    fn a_tool_calls_a_tool(&mut self, tc: TestCase) {
        if self.model.phase != Phase::Tools {
            tc.reject();
        }
        let free: Vec<String> = self
            .model
            .tools
            .running
            .difference(&self.model.tools.calling)
            .cloned()
            .collect();
        if free.is_empty() {
            tc.reject();
        }
        let id = tc.draw(gs::sampled_from(free));
        self.send(&id, Command::Nest);
        self.model.nested(&id);
        self.settle();
    }

    /// The person writes to the run, whatever it is doing.
    #[rule(weight = 3)]
    fn the_person_steers(&mut self, _tc: TestCase) {
        self.steers += 1;
        let steer = format!("steer {}", self.steers);
        self.control.steer(steer.clone());
        self.model.steered(steer);
        self.settle();
    }

    #[rule]
    fn the_person_cancels(&mut self, tc: TestCase) {
        if matches!(self.model.phase, Phase::Ended(_)) {
            tc.reject();
        }
        self.cancelled_at = Some(self.requests());
        self.control.cancel();
        self.model.cancel();
        self.settle();
    }

    /// A plugin will send the next answer back for more work.
    #[rule]
    fn a_plugin_will_refuse_an_answer(&mut self, _tc: TestCase) {
        self.world.lock().unwrap().refusals += 1;
        self.model.refusals += 1;
    }

    /// From now on, compaction rewrites the context, declines, or fails.
    #[rule]
    fn compaction_changes(&mut self, tc: TestCase) {
        self.compact(tc.draw(gs::default::<Compaction>()));
    }

    /// A store write fails: the next, or one a few writes on.
    #[rule]
    fn a_store_write_will_fail(&mut self, tc: TestCase) {
        if self.model.armed.is_some() {
            tc.reject();
        }
        let after = tc.draw(gs::integers::<u32>().max_value(3));
        self.arm(after);
    }

    /// The person writes again once the run ended: it goes on from
    /// where it stopped, or a new run starts. A run whose end could not
    /// be stored cannot go on.
    #[rule(weight = 4)]
    fn the_person_starts_again(&mut self, tc: TestCase) {
        let Phase::Ended(end) = self.model.phase else {
            tc.reject();
        };
        let resume = end != End::Store && tc.draw(gs::booleans());
        self.inputs += 1;
        if !resume {
            self.first_start = self.world.lock().unwrap().starts.len();
        }
        let (run, control, outcome) = Self::launch(
            &self.rt,
            &self.agent,
            &self.store,
            resume.then_some(&self.run),
            self.inputs,
        );
        self.run = run;
        self.control = control;
        self.outcome = Some(outcome);
        self.ended = None;
        self.resumed = resume;
        self.cancelled_at = None;
        self.model.start(resume);
        self.settle();
    }

    /// The run waits where the model says, has asked the model what the
    /// model counts, and ended as the model says.
    #[invariant(always_run)]
    fn the_run_is_where_the_model_says(&self, _tc: TestCase) {
        match (&self.seen, self.model.phase) {
            (Seen::Streaming, Phase::Streaming) => {}
            (Seen::Tools(running), Phase::Tools) => {
                assert_eq!(running, &self.model.tools.running, "running calls");
            }
            (Seen::Ended, Phase::Ended(end)) => {
                let ended = self.ended.as_ref().expect("an outcome");
                match (ended, end) {
                    (Ok(outcome), End::Stop) => {
                        assert_eq!(outcome.stop, StopReason::Stop)
                    }
                    (Ok(outcome), End::Cancelled) => {
                        assert_eq!(outcome.stop, StopReason::Cancelled)
                    }
                    (Ok(outcome), End::Error) => assert!(
                        matches!(outcome.stop, StopReason::Error(_)),
                        "{outcome:?}"
                    ),
                    (Err(AgentError::Store(_)), End::Store) => {}
                    (ended, end) => panic!("ended {ended:?}, not {end:?}"),
                }
                if end != End::Store && !self.resumed {
                    assert_grammar(&self.events());
                }
            }
            (seen, phase) => panic!("the run is at {seen:?}, not {phase:?}"),
        }
        assert_eq!(self.requests(), self.model.requests, "requests asked");
        assert_eq!(
            *self.store.armed.lock().unwrap(),
            self.model.armed,
            "the failing write is the one the model expects"
        );
    }

    /// Every steer is read once, in the order sent, or is still unread:
    /// the host hands those on when the run ends. A run that stopped
    /// read them all, and every steer it read is stored, once.
    #[invariant(always_run)]
    fn every_steer_is_read_once_or_left_unread(&self, _tc: TestCase) {
        let read: Vec<String> = self
            .events()
            .into_iter()
            .filter_map(|event| match event {
                RunEvent::Steered { text, .. } => Some(text),
                _ => None,
            })
            .collect();
        let steers = &self.model.steers;
        assert!(
            steers.sent.starts_with(&read),
            "read {read:?} of {:?}",
            steers.sent
        );
        assert_eq!(read, steers.read, "steers read");
        if self.model.phase == Phase::Ended(End::Stop) {
            assert_eq!(read, steers.sent, "a run stopped with steers unread");
        }
        // Every steer the run's starts read, and no other, is stored
        // once.
        let world = self.world.lock().unwrap();
        let read_in_run: BTreeSet<String> = world.starts[self.first_start..]
            .iter()
            .flat_map(|start| &start.events)
            .filter_map(|(_, _, event)| match event {
                RunEvent::Steered { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        drop(world);
        let stored: Vec<String> = self
            .stored()
            .iter()
            .filter_map(said)
            .filter(|text| text.starts_with("steer "))
            .collect();
        let mut sorted = stored.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), stored.len(), "a steer stored twice");
        assert_eq!(
            sorted.into_iter().collect::<BTreeSet<_>>(),
            read_in_run,
            "the steers stored are the ones read"
        );
        for request in &self.wire.lock().unwrap().requests {
            let steers: Vec<String> = request
                .transcript
                .iter()
                .filter_map(said)
                .filter(|text| text.starts_with("steer "))
                .collect();
            let unique: BTreeSet<&String> = steers.iter().collect();
            assert_eq!(unique.len(), steers.len(), "a steer sent twice");
        }
    }

    /// Every request, and the stored transcript, is valid for the
    /// provider.
    #[invariant(always_run)]
    fn every_request_is_valid_for_the_provider(&self, _tc: TestCase) {
        for request in &self.wire.lock().unwrap().requests {
            assert_results_follow_calls(&request.transcript);
        }
        assert_results_follow_calls(&self.stored());
    }

    /// After a cancel the model is asked nothing more, and the run ends
    /// cancelled, unless storing that failed.
    #[invariant(always_run)]
    fn nothing_is_asked_after_a_cancel(&self, _tc: TestCase) {
        let Some(at) = self.cancelled_at else {
            return;
        };
        assert_eq!(self.requests(), at, "asked after the cancel");
        assert_eq!(self.seen, Seen::Ended, "still going after the cancel");
        match self.ended.as_ref().unwrap() {
            Ok(outcome) => assert_eq!(outcome.stop, StopReason::Cancelled),
            Err(error) => {
                assert!(matches!(error, AgentError::Store(_)), "{error:?}")
            }
        }
    }

    /// Every turn after which the run goes on offers the context for
    /// compaction, however earlier offers went.
    #[invariant(always_run)]
    fn compaction_is_offered_after_every_turn_that_goes_on(
        &self,
        _tc: TestCase,
    ) {
        let world = self.world.lock().unwrap();
        for start in &world.starts {
            let turns: Vec<u32> = start
                .events
                .iter()
                .filter_map(|(_, _, event)| match event {
                    RunEvent::TurnStart { turn, .. } => Some(*turn),
                    _ => None,
                })
                .collect();
            for turn in turns.iter().skip(1) {
                assert!(
                    start.offers.iter().any(|(trigger, offered)| {
                        matches!(trigger, Trigger::TurnEnd)
                            && *offered == turn - 1
                    }),
                    "turn {} went on without an offer: {:?}",
                    turn - 1,
                    start.offers
                );
            }
        }
    }

    /// Retries keep to the policy: no more attempts than it allows, each
    /// announced with a delay within its backoff, and no request before
    /// that delay is over.
    #[invariant(always_run)]
    fn retries_keep_to_the_policy(&self, _tc: TestCase) {
        let world = self.world.lock().unwrap();
        let wire = self.wire.lock().unwrap();
        let mut retries = 0;
        for (tick, at, event) in world.starts.iter().flat_map(|s| &s.events) {
            let RunEvent::Retry { attempt, delay, .. } = event else {
                continue;
            };
            retries += 1;
            assert!(
                (2..=self.policy.max_attempts).contains(attempt),
                "attempt {attempt}"
            );
            assert!(*delay <= backoff_cap(&self.policy, *attempt));
            if let Some(next) = wire.requests.iter().find(|r| r.tick > *tick) {
                assert!(
                    next.at >= *at + *delay,
                    "asked {:?} into a backoff of {delay:?}",
                    next.at - *at
                );
            }
        }
        assert_eq!(retries, self.model.retries, "retries");
    }

    /// The store replays what the run saw: while the model streams, the
    /// stored transcript is the request's; once the run ended, the one
    /// it finished with; and what the person and the plugins said is
    /// stored in the order the events said it.
    #[invariant(always_run)]
    fn the_store_replays_what_the_run_saw(&self, _tc: TestCase) {
        let stored = self.stored();
        if self.seen == Seen::Streaming {
            let wire = self.wire.lock().unwrap();
            let last = &wire.requests.last().expect("a request").transcript;
            assert_eq!(&stored, last, "the stored transcript");
        }
        if let Phase::Ended(end) = self.model.phase
            && end != End::Store
        {
            let world = self.world.lock().unwrap();
            let finished = world.starts.last().unwrap().finished.as_ref();
            assert_eq!(Some(&stored), finished, "the finished transcript");
        }
        let timeline = self
            .rt
            .block_on(self.store.inner.timeline(&self.run.0))
            .unwrap();
        let said_stored: Vec<String> = timeline
            .into_iter()
            .filter_map(|entry| match entry {
                Entry::Message { role, body } if role == "user" => {
                    said(&serde_json::from_str(&body).unwrap())
                }
                _ => None,
            })
            .filter(|text| !text.starts_with("start "))
            .collect();
        assert_eq!(said_stored, self.said_live(), "what was said");
    }
}

/// The loop against the model, over drawn steps and a drawn retry
/// policy.
#[hegel::test(test_cases = 300)]
fn the_loop_keeps_to_its_model(tc: TestCase) {
    the_loop_keeps_to_its_model_body(tc, 30)
}

/// [`the_loop_keeps_to_its_model`] with more cases and longer runs, for
/// the nightly tier.
#[hegel::test(profile = "nightly_slow")]
#[ignore = "nightly"]
fn the_loop_keeps_to_its_model_nightly(tc: TestCase) {
    the_loop_keeps_to_its_model_body(tc, 60)
}

fn the_loop_keeps_to_its_model_body(tc: TestCase, steps: i64) {
    let max_attempts = tc.draw(gs::integers::<u32>().min_value(1).max_value(3));
    let mut machine = Machine::new(policy(max_attempts));
    machine.compact(tc.draw(gs::default::<Compaction>()));
    hegel::stateful::machine(machine).steps(steps).run(tc);
}

fn policy(max_attempts: u32) -> RetryPolicy {
    RetryPolicy {
        max_attempts,
        base: Duration::from_millis(10),
        max_delay: Duration::from_millis(40),
    }
}

// Regressions the model found, as histories it checks. Each runs once.

/// Runs `steps` on a machine whose policy allows `max_attempts`,
/// checking every invariant after each.
fn regression(max_attempts: u32, steps: fn(&mut Machine, &TestCase)) {
    hegel::Hegel::new(move |tc| {
        let mut machine = Machine::new(policy(max_attempts));
        machine.check(&tc);
        steps(&mut machine, &tc);
    })
    .settings(hegel::Settings::new().test_cases(1))
    .run();
}

/// A steer cuts a response short, and its write fails: the run had
/// announced that it read the steer (`Steered`) before storing it, so
/// the host took it off the unread messages it hands on, and the message
/// was in no transcript. A steer the run could not store is unread.
#[test]
fn a_steer_the_run_could_not_store_is_left_unread() {
    regression(1, |machine, tc| {
        // The cut turn's write goes through; the steer's fails.
        machine.arm(1);
        machine.the_person_steers(tc.clone());
        machine.check(tc);
    });
}

/// A plugin sends the answer back, and the write of its message fails:
/// the run had announced the message (`Continued`) before storing it,
/// so the events showed a message no transcript holds.
#[test]
fn a_message_sent_back_is_announced_once_stored() {
    regression(1, |machine, tc| {
        machine.a_plugin_will_refuse_an_answer(tc.clone());
        // The answer's turn is stored; the message sent back is not.
        machine.arm(1);
        machine.the_model_ends_its_response(tc.clone());
        machine.check(tc);
    });
}
