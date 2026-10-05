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
//!
//! A running tool can call other tools (`ToolCtx::call`). Those nested
//! calls come back to the loop on a channel it polls alongside the
//! batch, so they go through the same preparation and plugins, one
//! plugin call at a time, and their futures join the batch's. Their
//! results go back to the calling tool, never into the transcript.

use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc,
        Mutex,
        atomic::{AtomicBool, AtomicI64, Ordering},
    },
    time::SystemTime,
};

use futures_util::{StreamExt, stream::FuturesUnordered};
use serde_json::Value;
use tau_ai::{
    event::{Accumulator, AssistantEvent, ErrorReason},
    llm::LlmSession,
    message::{
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
use tau_store::{Entry, RewriteStats, Status, Store, StoreError, TurnUsage};
use tokio::{
    sync::{mpsc, oneshot},
    time::Instant,
};
use tokio_util::sync::CancellationToken;

use crate::{
    context::{
        estimate_context_tokens,
        estimate_message_tokens,
        is_context_overflow,
    },
    error::describe,
    event::{RunEvent, StopReason},
    limits::Limits,
    plugin::{
        Charged,
        ContextView,
        Decision,
        FinishedRun,
        PluginCtx,
        PluginRun,
        RequestView,
        Rewrite,
        StopDecision,
        ToolCall,
        ToolResultView,
        Trigger,
    },
    tool::{
        AgentTool,
        Catalog,
        ENDED,
        ExecutionMode,
        NestedRequest,
        Nesting,
        RunId,
        RunScope,
        ToolCtx,
        ToolError,
        ToolOutput,
        ToolSource,
        ToolUpdates,
    },
    validation::ArgumentSchema,
};

mod batch;
pub(crate) mod respond;
mod seams;
mod toolbox;

pub(crate) use self::toolbox::{LoopTool, Toolbox};

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
    /// The parent's tool call that started the run, for a sub-agent.
    pub call: Option<Arc<str>>,
    pub tools: Arc<Toolbox>,
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
    /// Messages the run starts with, stored as its own before its
    /// input: a forking sub-agent's copy of its caller's pending turn.
    pub prelude: Vec<Message>,
    /// The assistant message whose tool calls are running.
    pub pending_turn: Option<Arc<AssistantMessage>>,
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
            call: self.call.clone(),
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
        let mut start = std::mem::take(&mut self.prelude);
        start.push(first);
        self.persist(&start, &own).await?;
        let mut transcript = std::mem::take(&mut self.history);
        let inherited = transcript.len() + start.len() > 1;
        transcript.extend(start);
        if inherited {
            // A transcript from another run, or another model, may not
            // fit this one's window. Failures were reported as events.
            let _ = self
                .rewrite_context(
                    &mut transcript,
                    Trigger::Start,
                    self.turns_before + 1,
                )
                .await?;
        }
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
            let calls: Vec<MessageToolCall> =
                message.tool_calls().cloned().collect();
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
                self.emit(RunEvent::Steered {
                    run: self.run.clone(),
                    text: text.clone(),
                })
                .await;
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

    /// The run's own usage plus its children's and what its plugins
    /// charged.
    fn total(&self, own: &Usage) -> Usage {
        let mut total = own.clone();
        total += &self.children.lock().expect("not poisoned");
        total += &self.charged.lock().expect("not poisoned").total;
        total
    }

    async fn emit_update(
        &mut self,
        call_id: Arc<str>,
        partial: ToolOutput,
        parent: Option<Arc<str>>,
    ) {
        self.emit(RunEvent::ToolUpdate {
            run: self.run.clone(),
            call_id: call_id.to_string(),
            partial: Arc::new(partial),
            parent: parent.map(|parent| parent.to_string()),
        })
        .await;
    }

    async fn emit_start(
        &mut self,
        call_id: &str,
        tool: &str,
        args: Value,
        parent: Option<String>,
    ) {
        self.emit(RunEvent::ToolStart {
            run: self.run.clone(),
            call_id: call_id.to_owned(),
            tool: tool.into(),
            args,
            parent,
        })
        .await;
    }

    async fn emit_end(
        &mut self,
        call_id: &str,
        output: &ToolOutput,
        is_error: bool,
        parent: Option<String>,
    ) {
        self.emit(RunEvent::ToolEnd {
            run: self.run.clone(),
            call_id: call_id.to_owned(),
            output: Arc::new(output.clone()),
            is_error,
            parent,
        })
        .await;
    }

    async fn emit(&mut self, event: RunEvent) {
        let starts = matches!(event, RunEvent::RunStart { .. });
        if starts {
            self.deliver(event.clone()).await;
        }
        let charges = std::mem::take(
            &mut self.charged.lock().expect("not poisoned").unreported,
        );
        for (plugin, usage) in charges {
            self.deliver(RunEvent::PluginCharged {
                run: self.run.clone(),
                plugin,
                usage,
            })
            .await;
        }
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
        if !starts {
            self.deliver(event).await;
        }
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
        let (unsaved, by) =
            self.charged.lock().expect("not poisoned").take_unsaved();
        usage += &unsaved;
        let plugins = plugin_usage(&by);
        let plugins: Vec<(&str, TurnUsage)> = plugins
            .iter()
            .map(|(plugin, usage)| (&**plugin, *usage))
            .collect();
        let last = match self
            .store
            .append_charged(
                &self.run.0,
                &entries,
                turn_usage(&usage, turns),
                &plugins,
            )
            .await
        {
            Ok(last) => last,
            Err(error) => {
                // Not stored: charge it again with the next write.
                self.charged
                    .lock()
                    .expect("not poisoned")
                    .untake(&unsaved, by);
                return Err(error);
            }
        };
        self.last_seq.fetch_max(last, Ordering::SeqCst);
        Ok(())
    }
}

fn entry(message: &Message) -> Entry {
    Entry::Message {
        role: message.role().to_owned(),
        body: serde_json::to_string(message).expect("messages serialize"),
    }
}

/// What each plugin charged, as the store takes it.
fn plugin_usage(by: &[(Arc<str>, Usage)]) -> Vec<(Arc<str>, TurnUsage)> {
    by.iter()
        .map(|(plugin, usage)| (plugin.clone(), turn_usage(usage, 0)))
        .collect()
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

fn last_text(transcript: &[Message]) -> String {
    transcript
        .iter()
        .rev()
        .find_map(|message| match message {
            Message::Assistant(assistant) => Some(assistant.text()),
            _ => None,
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use tau_ai::message::AssistantBlock;

    use super::{
        seams::check_rewrite,
        toolbox::{model_only, not_found},
        *,
    };

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

    /// A tool that only has a name and an exposure.
    struct Named {
        name: String,
        exposure: crate::tool::Exposure,
        schema: Value,
    }

    #[async_trait::async_trait]
    impl AgentTool for Named {
        fn name(&self) -> &str {
            &self.name
        }
        fn description(&self) -> &str {
            "named"
        }
        fn parameters(&self) -> &Value {
            &self.schema
        }
        fn exposure(&self) -> crate::tool::Exposure {
            self.exposure
        }
        async fn call(
            &self,
            _args: Value,
            _ctx: ToolCtx,
        ) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput::text(""))
        }
    }

    /// A source with a fixed list of tools.
    struct Fixed(Vec<Arc<dyn AgentTool>>);

    impl ToolSource for Fixed {
        fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
            self.0.clone()
        }
    }

    #[hegel::composite]
    fn named(tc: &hegel::TestCase, names: Vec<&'static str>) -> (String, u8) {
        use hegel::generators as gs;
        let name: &str = tc.draw(gs::sampled_from(names));
        // 0 Direct, 1 Nested, 2 ModelOnly.
        let exposure: u8 = tc.draw(gs::integers().min_value(0).max_value(2));
        (name.to_owned(), exposure)
    }

    fn tool(name: &str, exposure: u8) -> Arc<dyn AgentTool> {
        use crate::tool::Exposure;
        Arc::new(Named {
            name: name.to_owned(),
            exposure: [Exposure::Direct, Exposure::Nested, Exposure::ModelOnly]
                [usize::from(exposure)],
            schema: serde_json::json!({"type": "object"}),
        })
    }

    /// For any run tools (names may repeat) and sources (whose names may
    /// clash with the run's and each other's), the toolbox agrees with a
    /// reference: the later run tool of a name wins in the earlier's
    /// place; the model sees and can call exactly the run's `Direct` and
    /// `ModelOnly` tools; tools can call exactly what the catalog lists,
    /// the run's `Direct` and `Nested` tools and then each name's first
    /// callable source tool that no run tool hides; and a call resolves
    /// to the very tool the catalog lists.
    #[hegel::test(test_cases = 300)]
    fn exposure_decides_who_sees_and_calls_a_tool(tc: hegel::TestCase) {
        use hegel::generators as gs;
        let run: Vec<(String, u8)> =
            tc.draw(gs::vecs(named(vec!["a", "b", "c", "d"])).max_size(6));
        let sources: Vec<Vec<(String, u8)>> = tc.draw(
            gs::vecs(gs::vecs(named(vec!["c", "d", "e", "f"])).max_size(4))
                .max_size(3),
        );
        let loop_tools = run
            .iter()
            .map(|(name, exposure)| {
                let tool = tool(name, *exposure);
                LoopTool {
                    schema: Arc::new(
                        ArgumentSchema::new(tool.parameters()).unwrap(),
                    ),
                    tool,
                    owner: None,
                }
            })
            .collect();
        let source_tools: Vec<Vec<Arc<dyn AgentTool>>> = sources
            .iter()
            .map(|tools| tools.iter().map(|(n, e)| tool(n, *e)).collect())
            .collect();
        let toolbox = Toolbox::new(
            loop_tools,
            source_tools
                .iter()
                .enumerate()
                .map(|(i, tools)| {
                    (Arc::new(Fixed(tools.clone())) as Arc<dyn ToolSource>, i)
                })
                .collect(),
        );

        // The reference.
        let mut order: Vec<String> = Vec::new();
        let mut exposure: HashMap<String, u8> = HashMap::new();
        for (name, e) in &run {
            if !order.contains(name) {
                order.push(name.clone());
            }
            exposure.insert(name.clone(), *e);
        }
        let declared: Vec<String> = order
            .iter()
            .filter(|n| exposure[*n] != 1)
            .cloned()
            .collect();
        let mut callable: Vec<String> = order
            .iter()
            .filter(|n| exposure[*n] != 2)
            .cloned()
            .collect();
        for tools in &sources {
            for (name, e) in tools {
                if *e != 2 && !order.contains(name) && !callable.contains(name)
                {
                    callable.push(name.clone());
                }
            }
        }

        let seen: Vec<String> =
            toolbox.declared().map(|t| t.name().to_owned()).collect();
        assert_eq!(seen, declared);
        let catalog = toolbox.catalog();
        let listed: Vec<String> = catalog
            .tools()
            .iter()
            .map(|t| t.name().to_owned())
            .collect();
        assert_eq!(listed, callable);
        for name in ["a", "b", "c", "d", "e", "f", "g"] {
            assert_eq!(
                toolbox.for_model(name).is_ok(),
                declared.iter().any(|n| n == name),
                "{name}"
            );
            match toolbox.for_tool(name) {
                Ok(found) => {
                    let listed = catalog.get(name).expect("listed");
                    assert!(Arc::ptr_eq(&found.tool, listed), "{name}");
                }
                Err(message) => {
                    assert!(catalog.get(name).is_none(), "{name}");
                    let known = order.iter().any(|n| n == name)
                        || sources.iter().flatten().any(|(n, _)| n == name);
                    let expected = if known {
                        model_only(name)
                    } else {
                        not_found(name)
                    };
                    assert_eq!(message, expected);
                }
            }
        }
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
