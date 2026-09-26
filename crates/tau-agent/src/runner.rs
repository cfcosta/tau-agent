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
    sync::{Arc, Mutex},
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
};
use tau_store::{Entry, Status, Store, StoreError, TurnUsage};
use tokio::{sync::mpsc, time::Instant};
use tokio_util::sync::CancellationToken;

use crate::{
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
        let first = self.user(input);
        self.persist(std::slice::from_ref(&first), &Usage::default())
            .await?;
        let mut transcript = std::mem::take(&mut self.history);
        transcript.push(first);
        let mut own = Usage::default();
        let mut turn = 0;

        let stop = loop {
            turn += 1;
            self.emit(RunEvent::TurnStart {
                run: self.run.clone(),
                turn,
            })
            .await;

            let message = self.respond(&transcript).await;
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

    /// Streams one response, emitting its deltas. A cancel drops the
    /// stream and ends the message as aborted.
    async fn respond(&mut self, transcript: &[Message]) -> AssistantMessage {
        let timestamp = (self.clock)();
        let mut stream = self.session.respond(transcript, timestamp);
        let mut accumulator = Accumulator::new();
        let mut call_id = String::new();
        let model = self.session.settings().model.clone();
        loop {
            let event = tokio::select! {
                biased;
                _ = self.cancel.cancelled() => {
                    drop(stream);
                    return finish(accumulator, &model, timestamp, ErrorReason::Aborted, CANCELLED);
                }
                event = stream.next() => event,
            };
            let Some(event) = event else { break };
            match &event {
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
                return finish(
                    accumulator,
                    &model,
                    timestamp,
                    ErrorReason::Error,
                    "the model's response broke the event grammar",
                );
            }
        }
        if accumulator.is_finished() {
            accumulator.finish().expect("finished")
        } else {
            finish(
                accumulator,
                &model,
                timestamp,
                ErrorReason::Error,
                "the model's response ended without a terminal event",
            )
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
        let entries: Vec<Entry> = messages
            .iter()
            .map(|message| Entry::Message {
                role: message.role().to_owned(),
                body: serde_json::to_value(message)
                    .expect("messages serialize"),
            })
            .collect();
        let input = usage.input + usage.cache_read + usage.cache_write;
        let turn = TurnUsage {
            input_tokens: u32::try_from(input).unwrap_or(u32::MAX),
            output_tokens: u32::try_from(usage.output).unwrap_or(u32::MAX),
            cost_usd: usage.cost.total,
        };
        self.store.append_turn(&self.run.0, &entries, turn).await?;
        self.stored += entries.len() as i64;
        Ok(())
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
