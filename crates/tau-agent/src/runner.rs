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
    hash::BuildHasher,
    sync::{Arc, Mutex},
    time::SystemTime,
};

use futures_util::{StreamExt, stream::FuturesUnordered};
use serde_json::Value;
use tau_ai::{
    event::{Accumulator, AssistantEvent, ErrorReason},
    llm::{Llm, LlmSession},
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
    responses::request::Settings,
    retry::{Class, RetryPolicy},
};
use tau_store::{Entry, Status, Store, StoreError, TurnUsage};
use tokio::{sync::mpsc, time::Instant};
use tokio_util::sync::CancellationToken;

use crate::{
    compaction::{
        Compaction,
        Record,
        SUMMARIZATION_SYSTEM_PROMPT,
        build_summary_request,
        build_turn_prefix_summary_request,
        check_summary,
        estimate_context_tokens,
        format_file_operations,
        is_context_overflow,
        merge_split_turn_summary,
        plan,
        should_compact,
        summary_max_output_tokens,
        turn_prefix_max_output_tokens,
    },
    event::{RunEvent, StopReason},
    hook::{Decision, HookCtx, RunHook, ToolCall},
    limits::Limits,
    tool::{
        AgentTool,
        ExecutionMode,
        RunId,
        RunScope,
        ToolCtx,
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

/// Everything one run needs.
pub(crate) struct Runner {
    pub run: RunId,
    pub parent: Option<RunId>,
    pub agent: Arc<str>,
    pub tools: HashMap<String, LoopTool>,
    pub hooks: Vec<Arc<dyn RunHook>>,
    pub limits: Limits,
    pub session: Box<dyn LlmSession>,
    pub store: Store,
    pub events: Option<mpsc::Sender<RunEvent>>,
    pub steering: mpsc::UnboundedReceiver<String>,
    pub cancel: CancellationToken,
    pub clock: Clock,
    /// Messages the run inherits (a fork's), before its input.
    pub history: Vec<Message>,
    /// How many entries of its own the run has stored.
    pub stored: i64,
    pub workflow: Option<Arc<str>>,
    /// The usage of the sub-agent runs this run's tools started.
    pub children: Arc<Mutex<Usage>>,
    /// Makes summary requests for compaction.
    pub llm: Arc<dyn Llm>,
    pub compaction: Option<Compaction>,
    /// The latest compaction, whose summary opens the transcript.
    pub compacted: Option<Record>,
    pub retry: RetryPolicy,
    /// Warm the session up before the first turn.
    pub warmup: bool,
}

/// How a compaction attempt went.
enum Compacted {
    Done,
    /// The cut kept everything.
    Nothing,
    Failed(String),
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
        input: String,
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
                        add_usage(&mut own, &usage);
                    }
                }
            }
        }
        let first = self.user(input);
        self.persist(std::slice::from_ref(&first), &own).await?;
        let mut transcript = std::mem::take(&mut self.history);
        transcript.push(first);
        let mut turn = 0;

        let stop = loop {
            turn += 1;
            self.emit(RunEvent::TurnStart {
                run: self.run.clone(),
                turn,
            })
            .await;

            let (mut message, class) = self.respond(&transcript, turn).await;
            let overflow = class == Class::ContextOverflow
                || is_context_overflow(&message);
            if self.compaction.is_some() && overflow {
                // Compact once and retry once; a second overflow fails
                // the run.
                match self.compact(&mut transcript, &mut own).await? {
                    Compacted::Done => {
                        message = self.respond(&transcript, turn).await.0;
                    }
                    Compacted::Nothing => {}
                    Compacted::Failed(error) => {
                        let original =
                            message.error_message.take().unwrap_or_default();
                        message.error_message = Some(format!(
                            "{original}; compaction failed: {error}"
                        ));
                    }
                }
            }
            add_usage(&mut own, &message.usage);
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
                self.execute(&calls).await
            };

            let mut new = vec![Message::Assistant(message.clone())];
            new.extend(results.into_iter().map(Message::ToolResult));
            self.persist(&new, &message.usage).await?;
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
                    .reached(turn, &self.total(&own), started.elapsed())
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
                break StopReason::Stop;
            }
            if self.over_threshold(&transcript) {
                match self.compact(&mut transcript, &mut own).await? {
                    Compacted::Done | Compacted::Nothing => {}
                    // Not compacting only costs context; a real overflow
                    // later compacts again or fails the run.
                    Compacted::Failed(_) => self.compaction = None,
                }
            }
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
        self.store
            .finish_run(&self.run.0, status, Some(&text), error)
            .await?;
        Ok(RunResult {
            transcript,
            stop,
            usage: total,
            text,
            last_seq: self.stored - 1,
        })
    }

    /// Whether the transcript has grown past the compaction threshold.
    fn over_threshold(&self, transcript: &[Message]) -> bool {
        let Some(settings) = &self.compaction else {
            return false;
        };
        let window = settings.context_window.or_else(|| {
            model::find(&self.session.settings().model)
                .map(|model| model.context_window)
        });
        window.is_some_and(|window| {
            should_compact(
                estimate_context_tokens(transcript),
                window,
                settings,
            )
        })
    }

    /// Replaces the transcript's older messages with a summary, stored
    /// as a compaction record followed by the kept messages, in one
    /// write. On failure nothing is stored and the transcript stays.
    async fn compact(
        &mut self,
        transcript: &mut Vec<Message>,
        own: &mut Usage,
    ) -> Result<Compacted, StoreError> {
        let Some(settings) = self.compaction else {
            return Ok(Compacted::Nothing);
        };
        let summarized = usize::from(self.compacted.is_some());
        let Some(plan) =
            plan(transcript, summarized, settings.keep_recent_tokens)
        else {
            return Ok(Compacted::Nothing);
        };
        let tokens_before = estimate_context_tokens(transcript);
        let model = self.session.settings().model.clone();
        let max_output =
            model::find(&model).map_or(0, |model| model.max_output);
        let previous = self.compacted.as_ref().map(|r| r.summary.as_str());

        let mut files = self
            .compacted
            .as_ref()
            .map(Record::files)
            .unwrap_or_default();
        let mut usage = Usage::default();
        let history = &transcript[plan.history.clone()];
        files.extract_from_messages(history);
        let mut summary = if history.is_empty() {
            previous.unwrap_or("No prior history.").to_owned()
        } else {
            let request = build_summary_request(history, previous, None);
            let budget =
                summary_max_output_tokens(settings.reserve_tokens, max_output);
            match self.summarize(&model, request, budget, &mut usage).await {
                Ok(text) => text,
                Err(error) => return Ok(Compacted::Failed(error)),
            }
        };
        if let Some(prefix) = plan.turn_prefix.clone() {
            let prefix = &transcript[prefix];
            files.extract_from_messages(prefix);
            let request = build_turn_prefix_summary_request(prefix);
            let budget = turn_prefix_max_output_tokens(
                settings.reserve_tokens,
                max_output,
            );
            match self.summarize(&model, request, budget, &mut usage).await {
                Ok(text) => summary = merge_split_turn_summary(&summary, &text),
                Err(error) => return Ok(Compacted::Failed(error)),
            }
        }
        let (read_files, modified_files) = files.file_lists();
        summary.push_str(&format_file_operations(&read_files, &modified_files));
        let record = Record {
            summary,
            tokens_before,
            read_files,
            modified_files,
            timestamp: (self.clock)(),
        };

        let kept = transcript.split_off(plan.kept_from);
        let mut entries = vec![Entry::Compaction {
            body: serde_json::to_value(&record).expect("records serialize"),
        }];
        entries.extend(kept.iter().map(entry));
        self.store
            .append_turn(&self.run.0, &entries, turn_usage(&usage))
            .await?;
        self.stored += entries.len() as i64;
        add_usage(own, &usage);
        *transcript = std::iter::once(record.message()).chain(kept).collect();
        self.compacted = Some(record);
        self.emit(RunEvent::Compacted {
            run: self.run.clone(),
            tokens_before,
        })
        .await;
        Ok(Compacted::Done)
    }

    /// One summary request, on a session of its own, so the run's lane
    /// and its continuation are untouched.
    async fn summarize(
        &mut self,
        model: &str,
        request: String,
        max_output_tokens: u64,
        usage: &mut Usage,
    ) -> Result<String, String> {
        let settings = Settings {
            model: model.to_owned(),
            instructions: Some(SUMMARIZATION_SYSTEM_PROMPT.to_owned()),
            reasoning: self.session.settings().reasoning,
            max_output_tokens: Some(max_output_tokens),
            ..Settings::default()
        };
        let mut session =
            self.llm.open(settings).await.map_err(|e| e.to_string())?;
        let input = [self.user(request)];
        let mut attempts = 1;
        let message = loop {
            let timestamp = (self.clock)();
            let mut stream = session.respond(&input, timestamp);
            let mut accumulator = Accumulator::new();
            let mut class = Class::Fatal;
            loop {
                let event = tokio::select! {
                    biased;
                    _ = self.cancel.cancelled() => {
                        return Err("Summarization aborted".to_owned());
                    }
                    event = stream.next() => event,
                };
                let Some(event) = event else { break };
                if let AssistantEvent::Error { class: failed, .. } = &event {
                    class = *failed;
                }
                if accumulator.push(event).is_err() {
                    return Err("Summarization failed: the response broke the event grammar".to_owned());
                }
            }
            let message = accumulator.finish().map_err(|_| {
                "Summarization failed: the response ended without a terminal event"
                    .to_owned()
            })?;
            // The summary request goes through the run's retry policy.
            if class != Class::Retryable || !self.retry.allows(attempts) {
                break message;
            }
            add_usage(usage, &message.usage);
            let delay = self.retry.delay(attempts, jitter());
            attempts += 1;
            tokio::select! {
                biased;
                _ = self.cancel.cancelled() => {
                    return Err("Summarization aborted".to_owned());
                }
                _ = tokio::time::sleep(delay) => {}
            }
        };
        add_usage(usage, &message.usage);
        check_summary(&message).map_err(|error| error.to_string())
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
            let delay = self.retry.delay(attempts, jitter());
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
                        Err(error) => (ToolOutput::text(error.to_string()), true),
                    };
                    let ctx = self.hook_ctx();
                    for hook in self.hooks.clone() {
                        hook.after_tool(&call, &mut output, &ctx).await;
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
        let ctx = self.hook_ctx();
        for hook in self.hooks.clone() {
            let before = hook_call.args.clone();
            match hook.before_tool(&mut hook_call, &ctx).await {
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

    /// The run's own usage plus its children's.
    fn total(&self, own: &Usage) -> Usage {
        let mut total = own.clone();
        add_usage(&mut total, &self.children.lock().expect("not poisoned"));
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

    fn hook_ctx(&self) -> HookCtx {
        HookCtx {
            run: self.run.clone(),
            parent: self.parent.clone(),
        }
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

    /// Hands an event to every hook, in order, then to the subscriber.
    async fn emit(&mut self, event: RunEvent) {
        for hook in &self.hooks {
            hook.on_event(&event).await;
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

    /// Stores messages and the usage they cost in one write.
    async fn persist(
        &mut self,
        messages: &[Message],
        usage: &Usage,
    ) -> Result<(), StoreError> {
        let entries: Vec<Entry> = messages.iter().map(entry).collect();
        self.store
            .append_turn(&self.run.0, &entries, turn_usage(usage))
            .await?;
        self.stored += entries.len() as i64;
        Ok(())
    }
}

/// A uniform sample in `[0, 1)` for backoff jitter. Each `RandomState`
/// is seeded afresh, so runs retrying together spread out.
fn jitter() -> f64 {
    let bits = std::collections::hash_map::RandomState::new().hash_one(0u8);
    (bits >> 11) as f64 / (1u64 << 53) as f64
}

fn entry(message: &Message) -> Entry {
    Entry::Message {
        role: message.role().to_owned(),
        body: serde_json::to_value(message).expect("messages serialize"),
    }
}

fn turn_usage(usage: &Usage) -> TurnUsage {
    let input = usage.input + usage.cache_read + usage.cache_write;
    TurnUsage {
        input_tokens: u32::try_from(input).unwrap_or(u32::MAX),
        output_tokens: u32::try_from(usage.output).unwrap_or(u32::MAX),
        cost_usd: usage.cost.total,
    }
}

/// A running tool call: its index in the batch, the call, and its result.
type ToolFuture = std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = (usize, ToolCall, anyhow::Result<ToolOutput>),
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

pub(crate) fn add_usage(total: &mut Usage, usage: &Usage) {
    total.input += usage.input;
    total.output += usage.output;
    total.cache_read += usage.cache_read;
    total.cache_write += usage.cache_write;
    total.total_tokens += usage.total_tokens;
    total.cost.input += usage.cost.input;
    total.cost.output += usage.cost.output;
    total.cost.cache_read += usage.cost.cache_read;
    total.cost.cache_write += usage.cost.cache_write;
    total.cost.total += usage.cost.total;
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
    use tau_ai::message::UsageCost;

    use super::*;

    /// Jitter samples are uniform-looking draws from `[0, 1)`: every one
    /// in range, and not all the same.
    #[test]
    fn jitter_samples_the_unit_interval() {
        let samples: Vec<f64> = (0..200).map(|_| jitter()).collect();
        assert!(
            samples.iter().all(|x| (0.0..1.0).contains(x)),
            "{samples:?}"
        );
        assert!(samples.iter().any(|x| *x != samples[0]));
        let mean = samples.iter().sum::<f64>() / samples.len() as f64;
        assert!((0.3..0.7).contains(&mean), "{mean}");
    }

    /// Usage adds up field by field, cost included.
    #[test]
    fn usage_adds_field_by_field() {
        let part = |n: u64| Usage {
            input: n,
            output: n + 1,
            cache_read: n + 2,
            cache_write: n + 3,
            reasoning: None,
            total_tokens: n + 4,
            cost: UsageCost {
                input: n as f64,
                output: n as f64 + 0.5,
                cache_read: n as f64 + 0.25,
                cache_write: n as f64 + 0.125,
                total: n as f64 + 1.0,
            },
        };
        let mut total = part(1);
        add_usage(&mut total, &part(10));
        assert_eq!(
            total,
            Usage {
                input: 11,
                output: 13,
                cache_read: 15,
                cache_write: 17,
                reasoning: None,
                total_tokens: 19,
                cost: UsageCost {
                    input: 11.0,
                    output: 12.0,
                    cache_read: 11.5,
                    cache_write: 11.25,
                    total: 13.0,
                },
            }
        );
    }
}
