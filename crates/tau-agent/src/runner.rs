//! The agent loop for one run (`docs/reference/agent-loop.md`).
//!
//! 1. Emit `RunStart` and store the input as a user message.
//! 2. Each turn: emit `TurnStart`; stream the model's response, emitting
//!    its deltas; on an error or cancel, store it and end the run; fail
//!    tool calls cut off by a `length` stop, or run them; store the turn in
//!    one write; check cancellation and limits; emit `TurnEnd`; take one
//!    steering message; loop while there were tool calls or steering.
//! 3. Emit `RunEnd` and record the run's outcome.
//!
//! Tool calls are prepared one at a time in source order (`ToolStart`,
//! lookup, argument repair, validation, `before_tool` hooks), then run
//! together (or one at a time if any tool is sequential). `ToolEnd` comes
//! in completion order; result messages are stored in source order. A
//! running tool's future is never dropped: on cancel, tools observe the
//! token, and calls that never started get a "cancelled" result, so the
//! transcript never holds a call without a result.

use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc,
        Mutex,
        atomic::{AtomicI64, Ordering},
    },
    time::SystemTime,
};

use futures_util::{StreamExt, stream::FuturesUnordered};
use serde_json::Value;
use tau_ai::{
    event::{Accumulator, AssistantEvent, ErrorReason},
    llm::LlmSession,
    message::{
        AssistantBlock,
        AssistantMessage,
        Message,
        StopReason as MessageStop,
        Timestamp,
        ToolCall as MessageToolCall,
        ToolResultMessage,
        Usage,
        UserContent,
        UserMessage,
    },
    model,
    retry::{Class, RetryPolicy},
};
use tau_store::{Entry, Status, Store, StoreError, TurnUsage};
use tokio::{sync::mpsc, time::Instant};
use tokio_util::sync::CancellationToken;

use crate::{
    context::{estimate_context_tokens, is_context_overflow},
    error::describe,
    event::{RunEvent, StopReason},
    hook::{Decision, ToolCall},
    limits::Limits,
    plugin::{
        Charged,
        ContextView,
        FinishedRun,
        PluginCtx,
        PluginRun,
        RequestView,
        Rewrite,
        StopDecision,
        ToolResultView,
        Trigger,
    },
    tool::{
        AgentTool,
        ExecutionMode,
        RunId,
        RunScope,
        ToolCtx,
        ToolError,
        ToolOutput,
        ToolUpdates,
    },
    validation::ArgumentSchema,
};

/// The text of a tool call's result when the response was cut off by the
/// output limit (pi, `agent-loop.ts:489`).
pub const TRUNCATED: &str = "was not executed: the response hit the output token limit, so its arguments may be truncated. Re-issue the tool call with complete arguments.";

/// The text of a tool call's result when the run was cancelled before
/// the call ran.
pub const CANCELLED: &str = "Operation aborted";

/// Milliseconds since the epoch; injected so tests are deterministic.
pub type Clock = Arc<dyn Fn() -> Timestamp + Send + Sync>;

/// The system clock.
pub fn system_clock() -> Clock {
    Arc::new(|| {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as Timestamp)
    })
}

/// A tool as the loop holds it: the tool and its compiled schema.
#[derive(Clone)]
pub(crate) struct LoopTool {
    pub tool: Arc<dyn AgentTool>,
    pub schema: Arc<ArgumentSchema>,
}

/// A plugin's part in a run, with its context.
pub(crate) struct ActivePlugin {
    pub run: Box<dyn PluginRun>,
    pub ctx: PluginCtx,
}

/// Everything one run needs.
pub(crate) struct Runner {
    pub run: RunId,
    pub parent: Option<RunId>,
    pub agent: Arc<str>,
    pub tools: HashMap<String, LoopTool>,
    /// In registration order.
    pub plugins: Vec<ActivePlugin>,
    pub limits: Limits,
    pub session: Box<dyn LlmSession>,
    pub store: Store,
    pub events: Option<mpsc::Sender<RunEvent>>,
    pub steering: mpsc::UnboundedReceiver<String>,
    pub cancel: CancellationToken,
    pub clock: Clock,
    /// Messages the run inherits (a fork's), before its input.
    pub history: Vec<Message>,
    /// The `seq` of the run's last stored entry, -1 before the first.
    /// Shared with plugins, which store records too.
    pub last_seq: Arc<AtomicI64>,
    /// What plugins reported, to emit before the next event.
    pub reports: crate::plugin::Reports,
    /// What plugins charged to the run.
    pub charged: Arc<Mutex<Charged>>,
    pub workflow: Option<Arc<str>>,
    /// The usage of the sub-agent runs this run's tools started.
    pub children: Arc<Mutex<Usage>>,
    pub retry: RetryPolicy,
    /// Warm the session up before the first turn.
    pub warmup: bool,
    /// Turns the run took before this start: a resumed run keeps
    /// counting. Limits apply to the turns of this start alone.
    pub turns_before: u32,
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq)]
pub struct RunResult {
    pub transcript: Vec<Message>,
    pub stop: StopReason,
    /// The model's usage over the whole run, cost included.
    pub usage: Usage,
    /// The text of the last assistant message.
    pub text: String,
    /// The `seq` of the run's last stored entry.
    pub last_seq: i64,
}

enum Prepared {
    /// Answered without running the tool.
    Immediate(ToolOutput),
    Ready {
        tool: Arc<dyn AgentTool>,
        call: ToolCall,
    },
}

impl Runner {
    pub(crate) async fn run(
        mut self,
        input: UserContent,
    ) -> Result<RunResult, StoreError> {
        let started = Instant::now();
        self.emit(RunEvent::RunStart {
            run: self.run.clone(),
            parent: self.parent.clone(),
            agent: self.agent.clone(),
        })
        .await;
        let mut own = Usage::default();
        if self.warmup {
            // A failed warm-up costs only its request: the first turn
            // then goes in full.
            let warm_up = self.session.warm_up((self.clock)());
            tokio::select! {
                biased;
                _ = self.cancel.cancelled() => {}
                result = warm_up => {
                    if let Ok(usage) = result {
                        own += &usage;
                    }
                }
            }
        }
        let first = Message::User(UserMessage {
            content: input,
            timestamp: (self.clock)(),
        });
        self.persist(std::slice::from_ref(&first), &own).await?;
        let mut transcript = std::mem::take(&mut self.history);
        transcript.push(first);
        let mut turn = self.turns_before;
        // The turns of this start, for limits: a resumed run gets a
        // fresh budget.
        let mut taken = 0;
        let mut continuations = 0;

        let stop = loop {
            turn += 1;
            taken += 1;
            self.emit(RunEvent::TurnStart {
                run: self.run.clone(),
                turn,
            })
            .await;

            self.choose_effort(&transcript, turn).await;
            let (mut message, class) = self.respond(&transcript, turn).await;
            let overflow = class == Class::ContextOverflow
                || is_context_overflow(&message);
            if overflow {
                // Rewrite once and retry once; a second overflow fails
                // the run.
                match self
                    .rewrite_context(&mut transcript, Trigger::Overflow, turn)
                    .await?
                {
                    Ok(()) => message = self.respond(&transcript, turn).await.0,
                    Err(failures) => {
                        if !failures.is_empty() {
                            let original = message
                                .error_message
                                .take()
                                .unwrap_or_default();
                            let failures: Vec<String> = failures
                                .iter()
                                .map(|(plugin, error)| {
                                    format!("{plugin} failed: {error}")
                                })
                                .collect();
                            message.error_message = Some(format!(
                                "{original}; {}",
                                failures.join("; ")
                            ));
                        }
                    }
                }
            }
            own += &message.usage;
            let failed = matches!(
                message.stop_reason,
                MessageStop::Error | MessageStop::Aborted
            );
            let calls: Vec<MessageToolCall> = message
                .content
                .iter()
                .filter_map(|block| match block {
                    AssistantBlock::ToolCall(call) => Some(call.clone()),
                    _ => None,
                })
                .collect();
            let results = if failed || calls.is_empty() {
                Vec::new()
            } else if message.stop_reason == MessageStop::Length {
                self.fail_truncated(&calls).await
            } else {
                self.execute(&calls, &transcript, &message).await
            };

            let mut new = vec![Message::Assistant(message.clone())];
            new.extend(results.into_iter().map(Message::ToolResult));
            self.persist_turn(&new, &message.usage).await?;
            transcript.extend(new);

            let verdict = if failed {
                Some(StopReason::from_message(
                    message.stop_reason,
                    message.error_message.as_deref(),
                ))
            } else if self.cancel.is_cancelled() {
                Some(StopReason::Cancelled)
            } else {
                self.limits
                    .reached(taken, &self.total(&own), started.elapsed())
                    .map(StopReason::Limit)
            };
            self.emit(RunEvent::TurnEnd {
                run: self.run.clone(),
                turn,
                usage: message.usage.clone(),
            })
            .await;
            if let Some(stop) = verdict {
                break stop;
            }

            // One steering message per drain, after the tool batch.
            let steered = self.steering.try_recv().ok();
            if let Some(text) = &steered {
                let message = self.user(text.clone());
                self.persist(std::slice::from_ref(&message), &Usage::default())
                    .await?;
                transcript.push(message);
            }
            if calls.is_empty() && steered.is_none() {
                if continuations >= self.limits.max_continuations {
                    break StopReason::Stop;
                }
                let Some((plugin, text)) = self.before_stop(&message).await
                else {
                    break StopReason::Stop;
                };
                continuations += 1;
                self.emit(RunEvent::Continued {
                    run: self.run.clone(),
                    plugin,
                    message: text.clone(),
                })
                .await;
                let message = self.user(text);
                self.persist(std::slice::from_ref(&message), &Usage::default())
                    .await?;
                transcript.push(message);
            }
            // Failures were reported as events; the run goes on with the
            // transcript as it is.
            let _ = self
                .rewrite_context(&mut transcript, Trigger::TurnEnd, turn)
                .await?;
        };

        let total = self.total(&own);
        self.emit(RunEvent::RunEnd {
            run: self.run.clone(),
            parent: self.parent.clone(),
            stop: stop.clone(),
            cost: total.cost.total,
        })
        .await;
        let text = last_text(&transcript);
        let (status, error) = match &stop {
            StopReason::Stop => (Status::Done, None),
            StopReason::Limit(_) => (Status::Limit, None),
            StopReason::Cancelled => (Status::Cancelled, None),
            StopReason::Error(message) => {
                (Status::Failed, Some(message.as_str()))
            }
        };
        self.save_charged().await?;
        self.store
            .finish_run(&self.run.0, status, Some(&text), error)
            .await?;
        let finished = FinishedRun {
            transcript: &transcript,
            stop: &stop,
            usage: &total,
            text: &text,
        };
        for plugin in &mut self.plugins {
            plugin.run.finish(&finished, &plugin.ctx).await;
        }
        // What plugins charged while finishing still counts.
        self.save_charged().await?;
        let usage = self.total(&own);
        Ok(RunResult {
            transcript,
            stop,
            usage,
            text,
            last_seq: self.last_seq.load(Ordering::SeqCst),
        })
    }

    /// Asks each plugin, in order, for the effort of the turn's request,
    /// until one picks it, and sets it on the session.
    async fn choose_effort(&mut self, transcript: &[Message], turn: u32) {
        let settings = self.session.settings();
        let view = RequestView {
            transcript,
            model: &settings.model,
            effort: settings.reasoning,
            turn,
        };
        let mut failures: Failures = Vec::new();
        let mut chosen = None;
        for plugin in &mut self.plugins {
            match plugin.run.before_request(&view, &plugin.ctx).await {
                Ok(None) => {}
                Ok(Some(effort)) => {
                    chosen = Some(effort);
                    break;
                }
                Err(error) => failures
                    .push((plugin.ctx.plugin().into(), describe(&error))),
            }
        }
        if let Some(effort) = chosen {
            self.session.set_reasoning(Some(effort));
        }
        for (plugin, message) in failures {
            self.emit(RunEvent::PluginError {
                run: self.run.clone(),
                plugin,
                message,
            })
            .await;
        }
    }

    /// Offers the transcript to each plugin, in order, until one rewrites
    /// it; stores the rewrite and makes it the working transcript.
    /// `Ok(Ok(()))` when a plugin rewrote it; otherwise the plugins that
    /// failed, and why (each also reported as a `PluginError`).
    async fn rewrite_context(
        &mut self,
        transcript: &mut Vec<Message>,
        trigger: Trigger,
        turn: u32,
    ) -> Result<Result<(), Failures>, StoreError> {
        let tokens = estimate_context_tokens(transcript);
        let window = model::find(&self.session.settings().model)
            .map(|model| model.context_window);
        let view = ContextView {
            transcript,
            tokens,
            window,
            trigger,
            turn,
        };
        let mut failures: Failures = Vec::new();
        let mut chosen = None;
        for plugin in &mut self.plugins {
            let name: Arc<str> = plugin.ctx.plugin().into();
            match plugin.run.rewrite_context(&view, &plugin.ctx).await {
                Ok(None) => {}
                Ok(Some(rewrite)) => {
                    match check_rewrite(transcript, &rewrite) {
                        Ok(()) => {
                            chosen = Some((name, rewrite));
                            break;
                        }
                        Err(problem) => failures.push((
                            name,
                            format!("rejected rewrite: {problem}"),
                        )),
                    }
                }
                Err(error) => failures.push((name, describe(&error))),
            }
        }
        for (plugin, message) in &failures {
            self.emit(RunEvent::PluginError {
                run: self.run.clone(),
                plugin: plugin.clone(),
                message: message.clone(),
            })
            .await;
        }
        let Some((plugin, rewrite)) = chosen else {
            return Ok(Err(failures));
        };
        let mut entries = vec![Entry::Context {
            plugin: plugin.to_string(),
            body: rewrite.details.to_string(),
        }];
        entries.extend(rewrite.messages.iter().map(entry));
        self.persist_entries(entries, &Usage::default(), 0).await?;
        // Every plugin sees what the rewrite dropped before it is gone.
        let mut failures = Vec::new();
        for other in &mut self.plugins {
            if let Err(error) =
                other.run.rewritten(transcript, &rewrite, &other.ctx).await
            {
                failures.push((other.ctx.plugin().into(), describe(&error)));
            }
        }
        for (plugin, message) in failures {
            self.emit(RunEvent::PluginError {
                run: self.run.clone(),
                plugin,
                message,
            })
            .await;
        }
        *transcript = rewrite.messages;
        self.emit(RunEvent::ContextRewritten {
            run: self.run.clone(),
            plugin,
            tokens_before: tokens,
            tokens_after: estimate_context_tokens(transcript),
        })
        .await;
        Ok(Ok(()))
    }

    /// Asks each plugin, in order, whether the run may stop after
    /// `message`. Returns the first plugin that continues it, and the
    /// text to continue with.
    async fn before_stop(
        &mut self,
        message: &AssistantMessage,
    ) -> Option<(Arc<str>, String)> {
        let mut decision = None;
        let mut failures = Vec::new();
        for plugin in &mut self.plugins {
            match plugin.run.before_stop(message, &plugin.ctx).await {
                Ok(StopDecision::Stop) => {}
                Ok(StopDecision::Continue(text)) => {
                    decision = Some((plugin.ctx.plugin().into(), text));
                    break;
                }
                Err(error) => failures
                    .push((plugin.ctx.plugin().into(), error.to_string())),
            }
        }
        for (plugin, message) in failures {
            self.emit(RunEvent::PluginError {
                run: self.run.clone(),
                plugin,
                message,
            })
            .await;
        }
        decision
    }

    /// Stores the usage plugins charged since the last write.
    async fn save_charged(&mut self) -> Result<(), StoreError> {
        let unsaved = std::mem::take(
            &mut self.charged.lock().expect("not poisoned").unsaved,
        );
        if unsaved != Usage::default() {
            self.store
                .append_turn(&self.run.0, &[], turn_usage(&unsaved, 0))
                .await?;
        }
        Ok(())
    }

    /// Asks for a response, retrying failures classified as retryable
    /// with the run's policy. Returns the last response and its class.
    async fn respond(
        &mut self,
        transcript: &[Message],
        turn: u32,
    ) -> (AssistantMessage, Class) {
        let mut attempts = 1;
        loop {
            let (message, class) = self.respond_once(transcript).await;
            if class != Class::Retryable || !self.retry.allows(attempts) {
                return (message, class);
            }
            let delay = self.retry.delay(attempts, tau_ai::retry::jitter());
            attempts += 1;
            self.emit(RunEvent::Retry {
                run: self.run.clone(),
                turn,
                attempt: attempts,
                delay,
                error: message.error_message.clone().unwrap_or_default(),
            })
            .await;
            tokio::select! {
                biased;
                _ = self.cancel.cancelled() => {
                    let model = self.session.settings().model.clone();
                    let aborted = finish(
                        Accumulator::new(),
                        &model,
                        (self.clock)(),
                        ErrorReason::Aborted,
                        CANCELLED,
                    );
                    return (aborted, Class::Fatal);
                }
                _ = tokio::time::sleep(delay) => {}
            }
        }
    }

    /// Streams one response, emitting its deltas. A cancel drops the
    /// stream and ends the message as aborted.
    async fn respond_once(
        &mut self,
        transcript: &[Message],
    ) -> (AssistantMessage, Class) {
        let timestamp = (self.clock)();
        let mut stream = self.session.respond(transcript, timestamp);
        let mut accumulator = Accumulator::new();
        let mut call_id = String::new();
        let mut class = Class::Fatal;
        let model = self.session.settings().model.clone();
        loop {
            let event = tokio::select! {
                biased;
                _ = self.cancel.cancelled() => {
                    drop(stream);
                    let message = finish(accumulator, &model, timestamp, ErrorReason::Aborted, CANCELLED);
                    return (message, Class::Fatal);
                }
                event = stream.next() => event,
            };
            let Some(event) = event else { break };
            match &event {
                AssistantEvent::Error { class: failed, .. } => class = *failed,
                AssistantEvent::TextDelta { delta, .. } => {
                    self.emit(RunEvent::TextDelta {
                        run: self.run.clone(),
                        parent: self.parent.clone(),
                        delta: delta.clone(),
                    })
                    .await
                }
                AssistantEvent::ThinkingDelta { delta, .. } => {
                    self.emit(RunEvent::ThinkingDelta {
                        run: self.run.clone(),
                        delta: delta.clone(),
                    })
                    .await
                }
                AssistantEvent::ToolCallStart { id, .. } => {
                    call_id = id.clone()
                }
                AssistantEvent::ToolCallDelta { delta, .. } => {
                    self.emit(RunEvent::ToolCallDelta {
                        run: self.run.clone(),
                        call_id: call_id.clone(),
                        json_fragment: delta.clone(),
                    })
                    .await
                }
                _ => {}
            }
            if accumulator.push(event).is_err() {
                let message = finish(
                    accumulator,
                    &model,
                    timestamp,
                    ErrorReason::Error,
                    "the model's response broke the event grammar",
                );
                return (message, Class::Fatal);
            }
        }
        if accumulator.is_finished() {
            (accumulator.finish().expect("finished"), class)
        } else {
            let message = finish(
                accumulator,
                &model,
                timestamp,
                ErrorReason::Error,
                "the model's response ended without a terminal event",
            );
            (message, Class::Fatal)
        }
    }

    /// Fails every call of a response cut off by the output limit.
    async fn fail_truncated(
        &mut self,
        calls: &[MessageToolCall],
    ) -> Vec<ToolResultMessage> {
        let mut results = Vec::new();
        for call in calls {
            self.emit_start(call).await;
            let output = ToolOutput::text(format!(
                "Tool call \"{}\" {TRUNCATED}",
                call.name
            ));
            self.emit_end(&call.id, &output, true).await;
            results.push(self.result_message(call, output, true));
        }
        results
    }

    /// Prepares the calls in order, runs them, and returns their result
    /// messages in source order.
    async fn execute(
        &mut self,
        calls: &[MessageToolCall],
        transcript: &[Message],
        message: &AssistantMessage,
    ) -> Vec<ToolResultMessage> {
        let mut outcomes: Vec<Option<(ToolOutput, bool)>> =
            vec![None; calls.len()];
        let mut ready = Vec::new();
        for (index, call) in calls.iter().enumerate() {
            self.emit_start(call).await;
            match self.prepare(call).await {
                Prepared::Immediate(output) => {
                    self.emit_end(&call.id, &output, true).await;
                    outcomes[index] = Some((output, true));
                }
                Prepared::Ready { tool, call } => {
                    ready.push((index, tool, call))
                }
            }
        }

        let sequential = ready.iter().any(|(_, tool, _)| {
            tool.execution_mode() == ExecutionMode::Sequential
        });
        let (updates_tx, mut updates_rx) = mpsc::unbounded_channel();
        let mut queue: VecDeque<(usize, Arc<dyn AgentTool>, ToolCall)> =
            ready.into();
        let mut pending = FuturesUnordered::new();
        let mut skipped = Vec::new();
        let batch = if sequential { 1 } else { usize::MAX };
        self.launch(&mut queue, &mut pending, &mut skipped, &updates_tx, batch);
        while !pending.is_empty() {
            tokio::select! {
                Some((call_id, partial)) = updates_rx.recv() => {
                    self.emit_update(call_id, partial).await;
                }
                Some((index, call, result)) = pending.next() => {
                    // Updates sent before the tool resolved come first.
                    while let Ok((call_id, partial)) = updates_rx.try_recv() {
                        self.emit_update(call_id, partial).await;
                    }
                    let (mut output, is_error) = match result {
                        Ok(output) => (output, false),
                        Err(ToolError::Output(output)) => (*output, true),
                        Err(error) => (ToolOutput::text(error.to_string()), true),
                    };
                    let view = ToolResultView {
                        call: &call,
                        is_error,
                        transcript,
                        message,
                    };
                    let mut failures: Failures = Vec::new();
                    for plugin in &mut self.plugins {
                        if let Err(error) = plugin.run.after_tool_result(&view, &mut output, &plugin.ctx).await {
                            failures.push((plugin.ctx.plugin().into(), describe(&error)));
                        }
                    }
                    for (plugin, message) in failures {
                        self.emit(RunEvent::PluginError {
                            run: self.run.clone(),
                            plugin,
                            message,
                        })
                        .await;
                    }
                    self.emit_end(&call.id, &output, is_error).await;
                    outcomes[index] = Some((output, is_error));
                    if sequential {
                        self.launch(&mut queue, &mut pending, &mut skipped, &updates_tx, 1);
                    }
                }
            }
        }
        for (index, call) in skipped {
            let output = ToolOutput::text(CANCELLED);
            self.emit_end(&call.id, &output, true).await;
            outcomes[index] = Some((output, true));
        }

        calls
            .iter()
            .zip(outcomes)
            .map(|(call, outcome)| {
                let (output, is_error) = outcome
                    .unwrap_or_else(|| (ToolOutput::text(CANCELLED), true));
                self.result_message(call, output, is_error)
            })
            .collect()
    }

    /// Looks the tool up, repairs and validates the arguments, and runs
    /// the `before_tool` hooks.
    async fn prepare(&mut self, call: &MessageToolCall) -> Prepared {
        if self.cancel.is_cancelled() {
            return Prepared::Immediate(ToolOutput::text(CANCELLED));
        }
        let Some(tool) = self.tools.get(&call.name).cloned() else {
            return Prepared::Immediate(ToolOutput::text(format!(
                "Tool {} not found",
                call.name
            )));
        };
        let raw = tool
            .tool
            .prepare_arguments(Value::Object(call.arguments.clone()));
        let args = match tool.schema.validate(&raw) {
            Ok(args) => args,
            Err(error) => {
                return Prepared::Immediate(ToolOutput::text(
                    error.to_string(),
                ));
            }
        };
        let mut hook_call = ToolCall {
            id: call.id.clone(),
            name: call.name.clone(),
            args,
        };
        for plugin in &mut self.plugins {
            let before = hook_call.args.clone();
            match plugin.run.before_tool(&mut hook_call, &plugin.ctx).await {
                Ok(Decision::Allow) => {}
                Ok(Decision::Block(reason)) => {
                    return Prepared::Immediate(ToolOutput::text(reason));
                }
                Err(error) => {
                    return Prepared::Immediate(ToolOutput::text(
                        error.to_string(),
                    ));
                }
            }
            if hook_call.args != before {
                // Changed arguments must still satisfy the tool's schema.
                match tool.schema.validate(&hook_call.args) {
                    Ok(args) => hook_call.args = args,
                    Err(error) => {
                        return Prepared::Immediate(ToolOutput::text(
                            error.to_string(),
                        ));
                    }
                }
            }
        }
        Prepared::Ready {
            tool: tool.tool,
            call: hook_call,
        }
    }

    /// Starts up to `count` queued calls. Once the run is cancelled,
    /// queued calls are skipped instead of started.
    fn launch(
        &self,
        queue: &mut VecDeque<(usize, Arc<dyn AgentTool>, ToolCall)>,
        pending: &mut FuturesUnordered<ToolFuture>,
        skipped: &mut Vec<(usize, ToolCall)>,
        updates: &mpsc::UnboundedSender<(Arc<str>, ToolOutput)>,
        count: usize,
    ) {
        let mut started = 0;
        while started < count {
            let Some((index, tool, call)) = queue.pop_front() else {
                return;
            };
            if self.cancel.is_cancelled() {
                skipped.push((index, call));
                continue;
            }
            let ctx = ToolCtx {
                cancel: self.cancel.clone(),
                updates: ToolUpdates::new(
                    call.id.as_str().into(),
                    updates.clone(),
                ),
                run: self.run.clone(),
                scope: Some(RunScope {
                    store: self.store.clone(),
                    workflow: self.workflow.clone(),
                    events: self.events.clone(),
                    children: self.children.clone(),
                }),
            };
            let args = call.args.clone();
            pending.push(Box::pin(async move {
                let updates = ctx.updates.clone();
                let result = tool.call(args, ctx).await;
                updates.close();
                (index, call, result)
            }));
            started += 1;
        }
    }

    /// The run's own usage plus its children's and what its plugins
    /// charged.
    fn total(&self, own: &Usage) -> Usage {
        let mut total = own.clone();
        total += &self.children.lock().expect("not poisoned");
        total += &self.charged.lock().expect("not poisoned").total;
        total
    }

    async fn emit_update(&mut self, call_id: Arc<str>, partial: ToolOutput) {
        self.emit(RunEvent::ToolUpdate {
            run: self.run.clone(),
            call_id: call_id.to_string(),
            partial: Arc::new(partial),
        })
        .await;
    }

    async fn emit_start(&mut self, call: &MessageToolCall) {
        self.emit(RunEvent::ToolStart {
            run: self.run.clone(),
            call_id: call.id.clone(),
            tool: call.name.as_str().into(),
            args: Value::Object(call.arguments.clone()),
        })
        .await;
    }

    async fn emit_end(
        &mut self,
        call_id: &str,
        output: &ToolOutput,
        is_error: bool,
    ) {
        self.emit(RunEvent::ToolEnd {
            run: self.run.clone(),
            call_id: call_id.to_owned(),
            output: Arc::new(output.clone()),
            is_error,
        })
        .await;
    }

    /// Hands an event to every plugin, in order, then to the subscriber;
    /// plugins' reports go first.
    async fn emit(&mut self, event: RunEvent) {
        let reports =
            std::mem::take(&mut *self.reports.lock().expect("not poisoned"));
        for (plugin, body) in reports {
            self.deliver(RunEvent::PluginReport {
                run: self.run.clone(),
                plugin,
                body,
            })
            .await;
        }
        self.deliver(event).await;
    }

    async fn deliver(&mut self, event: RunEvent) {
        for plugin in &mut self.plugins {
            plugin.run.on_event(&event, &plugin.ctx).await;
        }
        if let Some(events) = &self.events
            && events.send(event).await.is_err()
        {
            // The subscriber is gone; the run goes on without one.
            self.events = None;
        }
    }

    fn user(&self, text: String) -> Message {
        Message::User(UserMessage {
            content: UserContent::Text(text),
            timestamp: (self.clock)(),
        })
    }

    fn result_message(
        &self,
        call: &MessageToolCall,
        output: ToolOutput,
        is_error: bool,
    ) -> ToolResultMessage {
        ToolResultMessage {
            tool_call_id: call.id.clone(),
            tool_name: call.name.clone(),
            content: output.content,
            details: output.details,
            is_error,
            timestamp: (self.clock)(),
        }
    }

    /// Stores messages and the usage they cost in one write, with what
    /// plugins charged since the last write.
    async fn persist(
        &mut self,
        messages: &[Message],
        usage: &Usage,
    ) -> Result<(), StoreError> {
        self.persist_entries(messages.iter().map(entry).collect(), usage, 0)
            .await
    }

    /// Stores a turn's reply and tool results, counting the turn.
    async fn persist_turn(
        &mut self,
        messages: &[Message],
        usage: &Usage,
    ) -> Result<(), StoreError> {
        self.persist_entries(messages.iter().map(entry).collect(), usage, 1)
            .await
    }

    /// Stores entries and the usage they cost in one write, with what
    /// plugins charged since the last write.
    async fn persist_entries(
        &mut self,
        entries: Vec<Entry>,
        usage: &Usage,
        turns: u32,
    ) -> Result<(), StoreError> {
        let mut usage = usage.clone();
        let unsaved = std::mem::take(
            &mut self.charged.lock().expect("not poisoned").unsaved,
        );
        usage += &unsaved;
        let last = match self
            .store
            .append_turn(&self.run.0, &entries, turn_usage(&usage, turns))
            .await
        {
            Ok(last) => last,
            Err(error) => {
                // Not stored: charge it again with the next write.
                self.charged.lock().expect("not poisoned").unsaved += &unsaved;
                return Err(error);
            }
        };
        self.last_seq.fetch_max(last, Ordering::SeqCst);
        Ok(())
    }
}

/// Why a rewrite cannot replace `transcript`, if it cannot: it must not
/// be empty, must end with the message the transcript ends with (the one
/// the next request answers), and must keep every tool call of a
/// completed assistant message paired with its result, calls first.
fn check_rewrite(
    transcript: &[Message],
    rewrite: &Rewrite,
) -> Result<(), String> {
    let Some(last) = rewrite.messages.last() else {
        return Err("it is empty".into());
    };
    if Some(last) != transcript.last() {
        return Err("it does not end with the transcript's last message".into());
    }
    let mut open: Vec<&str> = Vec::new();
    for message in &rewrite.messages {
        match message {
            Message::Assistant(assistant)
                if !matches!(
                    assistant.stop_reason,
                    MessageStop::Error | MessageStop::Aborted
                ) =>
            {
                if let Some(call) = open.first() {
                    return Err(format!("tool call {call} has no result"));
                }
                open = assistant
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        AssistantBlock::ToolCall(call) => {
                            Some(call.id.as_str())
                        }
                        _ => None,
                    })
                    .collect();
            }
            Message::ToolResult(result) => {
                let Some(index) =
                    open.iter().position(|id| *id == result.tool_call_id)
                else {
                    return Err(format!(
                        "tool result {} has no call before it",
                        result.tool_call_id
                    ));
                };
                open.remove(index);
            }
            _ => {
                if let Some(call) = open.first() {
                    return Err(format!("tool call {call} has no result"));
                }
            }
        }
    }
    match open.first() {
        Some(call) => Err(format!("tool call {call} has no result")),
        None => Ok(()),
    }
}

fn entry(message: &Message) -> Entry {
    Entry::Message {
        role: message.role().to_owned(),
        body: serde_json::to_string(message).expect("messages serialize"),
    }
}

fn turn_usage(usage: &Usage, turns: u32) -> TurnUsage {
    let input = usage.input + usage.cache_read + usage.cache_write;
    TurnUsage {
        input_tokens: u32::try_from(input).unwrap_or(u32::MAX),
        output_tokens: u32::try_from(usage.output).unwrap_or(u32::MAX),
        cost_usd: usage.cost.total,
        turns,
    }
}

/// Plugins that failed at a seam, and why.
type Failures = Vec<(Arc<str>, String)>;

/// A running tool call: its index in the batch, the call, and its result.
type ToolFuture = std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = (usize, ToolCall, Result<ToolOutput, ToolError>),
            > + Send,
    >,
>;

/// Ends a partial message with an error of `reason`.
fn finish(
    mut accumulator: Accumulator,
    model: &str,
    timestamp: Timestamp,
    reason: ErrorReason,
    message: &str,
) -> AssistantMessage {
    if accumulator.partial().is_none() {
        let _ = accumulator.push(AssistantEvent::Start {
            model: model.to_owned(),
            response_id: None,
            timestamp,
        });
    }
    let _ = accumulator.push(AssistantEvent::Error {
        reason,
        message: message.to_owned(),
        usage: Usage::default(),
        class: Class::Fatal,
    });
    accumulator
        .finish()
        .expect("an error event always finishes the stream")
}

fn last_text(transcript: &[Message]) -> String {
    transcript
        .iter()
        .rev()
        .find_map(|message| match message {
            Message::Assistant(assistant) => Some(
                assistant
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        AssistantBlock::Text(text) => Some(text.text.as_str()),
                        _ => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {

    use super::*;

    fn user(text: &str) -> Message {
        Message::User(UserMessage {
            content: UserContent::Text(text.into()),
            timestamp: 0,
        })
    }

    fn calls(ids: &[&str], stop: MessageStop) -> Message {
        Message::Assistant(AssistantMessage {
            content: ids
                .iter()
                .map(|id| {
                    AssistantBlock::ToolCall(MessageToolCall {
                        id: (*id).into(),
                        name: "t".into(),
                        arguments: Default::default(),
                    })
                })
                .collect(),
            api: String::new(),
            provider: String::new(),
            model: String::new(),
            response_id: None,
            usage: Usage::default(),
            stop_reason: stop,
            error_message: None,
            timestamp: 0,
        })
    }

    fn result(id: &str) -> Message {
        Message::ToolResult(ToolResultMessage {
            tool_call_id: id.into(),
            tool_name: "t".into(),
            content: Vec::new(),
            details: None,
            is_error: false,
            timestamp: 0,
        })
    }

    fn check(messages: Vec<Message>) -> Result<(), String> {
        let transcript = vec![user("last")];
        check_rewrite(
            &transcript,
            &Rewrite {
                messages,
                details: Value::Null,
            },
        )
    }

    /// A rewrite keeps every completed call paired with its result, in
    /// any order within the batch, and ends with the transcript's last
    /// message. A failed turn's calls need no results.
    #[test]
    fn rewrites_keep_calls_and_results_paired() {
        let ok = MessageStop::ToolUse;
        assert_eq!(
            check(vec![
                calls(&["a", "b"], ok),
                result("b"),
                result("a"),
                user("last")
            ]),
            Ok(())
        );
        assert_eq!(
            check(vec![calls(&["a"], MessageStop::Error), user("last")]),
            Ok(())
        );
        assert_eq!(check(vec![]), Err("it is empty".into()));
        assert_eq!(
            check(vec![user("other")]),
            Err("it does not end with the transcript's last message".into())
        );
        assert_eq!(
            check(vec![calls(&["a", "b"], ok), result("a"), user("last")]),
            Err("tool call b has no result".into())
        );
        assert_eq!(
            check(vec![result("a"), user("last")]),
            Err("tool result a has no call before it".into())
        );
        assert_eq!(
            check(vec![
                calls(&["a"], ok),
                calls(&["b"], ok),
                result("b"),
                user("last")
            ]),
            Err("tool call a has no result".into())
        );
        let transcript = vec![calls(&["a"], ok), result("a")];
        assert_eq!(
            check_rewrite(
                &transcript,
                &Rewrite {
                    messages: vec![calls(&["a"], ok)],
                    details: Value::Null,
                }
            ),
            Err("it does not end with the transcript's last message".into())
        );
        assert_eq!(
            check_rewrite(
                &[calls(&["a"], ok)],
                &Rewrite {
                    messages: vec![calls(&["a"], ok)],
                    details: Value::Null,
                }
            ),
            Err("tool call a has no result".into())
        );
    }

    /// A transcript from `tau_testing::generators::transcript`, then a
    /// user message: it holds complete batches only, every call with its
    /// own id and result.
    fn transcript(tc: &hegel::TestCase) -> Vec<Message> {
        let mut transcript = tc.draw(tau_testing::generators::transcript());
        transcript.push(user("last"));
        transcript
    }

    /// The results of each batch (the run of results right after an
    /// assistant message), as index ranges into `transcript`.
    fn batches(transcript: &[Message]) -> Vec<std::ops::Range<usize>> {
        let mut batches = Vec::new();
        let mut i = 0;
        while i < transcript.len() {
            if matches!(transcript[i], Message::Assistant(_)) {
                let start = i + 1;
                let mut end = start;
                while matches!(
                    transcript.get(end),
                    Some(Message::ToolResult(_))
                ) {
                    end += 1;
                }
                batches.push(start..end);
                i = end;
            } else {
                i += 1;
            }
        }
        batches
    }

    /// A rewrite that keeps the transcript but reorders the results within
    /// each batch is accepted: results pair with calls by id, not by
    /// position.
    #[hegel::test(test_cases = 200)]
    fn results_may_come_in_any_order_within_their_batch(tc: hegel::TestCase) {
        let transcript = transcript(&tc);
        let mut rewritten = transcript.clone();
        for batch in batches(&transcript) {
            let order: Vec<usize> = tc.draw(hegel::generators::permutations(
                batch.clone().collect::<Vec<_>>(),
            ));
            for (slot, from) in batch.zip(order) {
                rewritten[slot] = transcript[from].clone();
            }
        }
        let rewrite = Rewrite {
            messages: rewritten,
            details: Value::Null,
        };
        assert_eq!(check_rewrite(&transcript, &rewrite), Ok(()));
    }

    /// A rewrite that drops any one result is rejected, naming the call
    /// left without it.
    #[hegel::test(test_cases = 200)]
    fn a_dropped_result_is_named(tc: hegel::TestCase) {
        let transcript = transcript(&tc);
        let results: Vec<usize> = (0..transcript.len())
            .filter(|&i| matches!(transcript[i], Message::ToolResult(_)))
            .collect();
        tc.assume(!results.is_empty());
        let drop = tc.draw(hegel::generators::sampled_from(results));
        let Message::ToolResult(dropped) = &transcript[drop] else {
            unreachable!("a result position")
        };
        let mut messages = transcript.clone();
        messages.remove(drop);
        let rewrite = Rewrite {
            messages,
            details: Value::Null,
        };
        assert_eq!(
            check_rewrite(&transcript, &rewrite),
            Err(format!("tool call {} has no result", dropped.tool_call_id))
        );
    }
}
