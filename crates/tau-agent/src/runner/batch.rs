//! A batch of tool calls: each call through the plugins' `before_tool`,
//! started, its nested calls, and its result through
//! `after_tool_result`.

use super::{
    toolbox::{Ready, schedule},
    *,
};

pub(super) enum Prepared {
    /// Answered without running the tool.
    Immediate(ToolOutput),
    Ready {
        tool: LoopTool,
        call: ToolCall,
    },
}

/// Where a call's result goes once its tool returns.
pub(super) enum Origin {
    /// Into the batch's results, at this index.
    Batch(usize),
    /// Back to the tool that made the call.
    Nested {
        /// The scope of the call that made it.
        scope: u64,
        parent: Arc<str>,
        reply: oneshot::Sender<Result<ToolOutput, ToolError>>,
    },
}

impl Origin {
    pub(super) fn parent(&self) -> Option<String> {
        match self {
            Self::Batch(_) => None,
            Self::Nested { parent, .. } => Some(parent.to_string()),
        }
    }
}

/// A call whose tool has returned.
pub(super) struct Finished {
    origin: Origin,
    call: ToolCall,
    /// The scope of the nested calls this call made.
    scope: u64,
    result: Result<ToolOutput, ToolError>,
}

/// A running call's nested calls.
pub(super) struct Scope {
    /// The running call's id.
    id: Arc<str>,
    /// Closed when the call's tool returns.
    open: Arc<AtomicBool>,
    /// The token of the nested calls, cancelled when the call ends.
    children: CancellationToken,
    /// How many nested calls it made: the last one's number.
    made: u32,
    running: usize,
    /// A sequential tool's call is running, alone.
    exclusive: bool,
    /// Prepared calls waiting for a sequential one, or to be one.
    waiting: VecDeque<Queued>,
    /// The call has ended; the scope goes once its calls are done.
    ended: bool,
}

/// A prepared nested call that has not started.
pub(super) struct Queued {
    tool: LoopTool,
    call: ToolCall,
    reply: oneshot::Sender<Result<ToolOutput, ToolError>>,
}

/// One batch's running calls, the model's and the nested ones.
pub(super) struct Batch {
    pending: FuturesUnordered<ToolFuture>,
    scopes: HashMap<u64, Scope>,
    next_scope: u64,
    updates: mpsc::UnboundedSender<(Arc<str>, ToolOutput)>,
    requests: mpsc::UnboundedSender<NestedRequest>,
    /// Each nested call's parent, for its updates' events.
    parents: HashMap<Arc<str>, Arc<str>>,
}

impl Batch {
    /// Drops a scope that has ended and has no calls left.
    pub(super) fn forget(&mut self, key: u64) {
        if self.scopes.get(&key).is_some_and(|scope| {
            scope.ended && scope.running == 0 && scope.waiting.is_empty()
        }) {
            self.scopes.remove(&key);
        }
    }
}

/// The id of the `n`th nested call made by the call `parent`.
pub(crate) fn nested_id(parent: &str, n: u32) -> String {
    format!("{parent}/{n}")
}

/// A running tool call.
pub(super) type ToolFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Finished> + Send>>;

impl Runner {
    /// Prepares the calls in order, runs them, and returns their result
    /// messages in source order. Nested calls the running tools make are
    /// prepared and run as they come.
    pub(super) async fn execute(
        &mut self,
        calls: &[MessageToolCall],
        transcript: &[Message],
        message: &AssistantMessage,
    ) -> Vec<ToolResultMessage> {
        self.pending_turn = Some(Arc::new(message.clone()));
        let mut outcomes: Vec<Option<(ToolOutput, bool)>> =
            vec![None; calls.len()];
        let mut ready: Vec<Ready> = Vec::new();
        for (index, call) in calls.iter().enumerate() {
            let args = Value::Object(call.arguments.clone());
            self.emit_start(&call.id, &call.name, args.clone(), None)
                .await;
            let tool = self.tools.for_model(&call.name);
            match self.prepare(tool, &call.id, &call.name, args, None).await {
                Prepared::Immediate(output) => {
                    self.emit_end(&call.id, &output, true, None).await;
                    outcomes[index] = Some((output, true));
                }
                Prepared::Ready { tool, call } => {
                    ready.push((index, tool, call))
                }
            }
        }

        let (mut queue, mut groups) = schedule(ready);
        let (updates_tx, mut updates_rx) = mpsc::unbounded_channel();
        let (requests_tx, mut requests_rx) = mpsc::unbounded_channel();
        let mut batch = Batch {
            pending: FuturesUnordered::new(),
            scopes: HashMap::new(),
            next_scope: 0,
            updates: updates_tx,
            requests: requests_tx,
            parents: HashMap::new(),
        };
        let mut skipped = Vec::new();
        if let Some(size) = groups.pop_front() {
            self.launch(&mut queue, &mut batch, &mut skipped, size);
        }
        while !batch.pending.is_empty() {
            tokio::select! {
                Some((call_id, partial)) = updates_rx.recv() => {
                    let parent = batch.parents.get(&call_id).cloned();
                    self.emit_update(call_id, partial, parent).await;
                }
                Some(request) = requests_rx.recv() => {
                    self.nested(&mut batch, request).await;
                }
                Some(finished) = batch.pending.next() => {
                    // Updates sent before the tool resolved come first.
                    while let Ok((call_id, partial)) = updates_rx.try_recv() {
                        let parent = batch.parents.get(&call_id).cloned();
                        self.emit_update(call_id, partial, parent).await;
                    }
                    if let Some((index, output, is_error)) = self
                        .finished(&mut batch, finished, transcript, message)
                        .await
                    {
                        outcomes[index] = Some((output, is_error));
                    }
                    // A group done, the next one starts.
                    if batch.pending.is_empty()
                        && let Some(size) = groups.pop_front()
                    {
                        self.launch(&mut queue, &mut batch, &mut skipped, size);
                    }
                }
            }
        }
        for (index, call) in skipped {
            let output = ToolOutput::text(CANCELLED);
            self.emit_end(&call.id, &output, true, None).await;
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

    /// Takes a nested call from a running tool: numbers it, prepares it
    /// as a model's call is prepared, and starts it when its scope lets
    /// it. A call whose caller has ended fails at once, without events.
    pub(super) async fn nested(
        &mut self,
        batch: &mut Batch,
        request: NestedRequest,
    ) {
        let NestedRequest {
            scope: key,
            name,
            args,
            reply,
        } = request;
        let Some(scope) = batch
            .scopes
            .get_mut(&key)
            .filter(|scope| scope.open.load(Ordering::SeqCst))
        else {
            let _ = reply.send(Err(ENDED.into()));
            return;
        };
        scope.made += 1;
        let parent = scope.id.clone();
        let id = nested_id(&parent, scope.made);
        batch.parents.insert(id.as_str().into(), parent.clone());
        self.emit_start(&id, &name, args.clone(), Some(parent.to_string()))
            .await;
        let tool = self.tools.for_tool(&name);
        match self.prepare(tool, &id, &name, args, Some(&parent)).await {
            Prepared::Immediate(output) => {
                self.emit_end(&id, &output, true, Some(parent.to_string()))
                    .await;
                let _ = reply.send(Err(ToolError::output(output)));
            }
            Prepared::Ready { tool, call } => {
                // Nothing ran while it was prepared: the scope is there.
                if let Some(scope) = batch.scopes.get_mut(&key) {
                    scope.waiting.push_back(Queued { tool, call, reply });
                }
                self.start_waiting(batch, key);
            }
        }
    }

    /// Starts the scope's waiting calls in order, while its calls are
    /// running: a sequential tool's call starts alone and runs alone.
    pub(super) fn start_waiting(&self, batch: &mut Batch, key: u64) {
        loop {
            let Some(scope) = batch.scopes.get_mut(&key) else {
                return;
            };
            if !scope.open.load(Ordering::SeqCst) {
                // The caller has returned; its end cancels these.
                return;
            }
            let Some(front) = scope.waiting.front() else {
                return;
            };
            let sequential =
                front.tool.tool.execution_mode() == ExecutionMode::Sequential;
            if scope.exclusive || (sequential && scope.running > 0) {
                return;
            }
            let Queued { tool, call, reply } =
                scope.waiting.pop_front().expect("a front");
            scope.running += 1;
            scope.exclusive = sequential;
            let cancel = scope.children.clone();
            let origin = Origin::Nested {
                scope: key,
                parent: scope.id.clone(),
                reply,
            };
            self.start_call(batch, tool, call, cancel, origin);
        }
    }

    /// Ends a call's scope: cancels the nested calls it left running,
    /// and fails the ones still waiting.
    pub(super) async fn end_scope(&mut self, batch: &mut Batch, key: u64) {
        let Some(scope) = batch.scopes.get_mut(&key) else {
            return;
        };
        scope.ended = true;
        scope.open.store(false, Ordering::SeqCst);
        scope.children.cancel();
        let parent = scope.id.to_string();
        let waiting = std::mem::take(&mut scope.waiting);
        for queued in waiting {
            let output = ToolOutput::text(CANCELLED);
            self.emit_end(&queued.call.id, &output, true, Some(parent.clone()))
                .await;
            let _ = queued.reply.send(Err(ToolError::output(output)));
        }
        batch.forget(key);
    }

    /// Handles a call whose tool returned: ends its scope, runs the
    /// plugins' `after_tool_result`, emits `ToolEnd`, and hands the
    /// result on. Returns a batch call's index and result.
    pub(super) async fn finished(
        &mut self,
        batch: &mut Batch,
        finished: Finished,
        transcript: &[Message],
        message: &AssistantMessage,
    ) -> Option<(usize, ToolOutput, bool)> {
        let Finished {
            origin,
            call,
            scope,
            result,
        } = finished;
        self.end_scope(batch, scope).await;
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
            if let Err(error) = plugin
                .run
                .after_tool_result(&view, &mut output, &plugin.ctx)
                .await
            {
                failures.push((plugin.ctx.plugin().into(), describe(&error)));
            }
        }
        self.report_failures(&failures).await;
        self.emit_end(&call.id, &output, is_error, origin.parent())
            .await;
        match origin {
            Origin::Batch(index) => Some((index, output, is_error)),
            Origin::Nested { scope, reply, .. } => {
                let _ = reply.send(if is_error {
                    Err(ToolError::output(output))
                } else {
                    Ok(output)
                });
                if let Some(parent) = batch.scopes.get_mut(&scope) {
                    parent.running -= 1;
                    parent.exclusive = false;
                }
                self.start_waiting(batch, scope);
                batch.forget(scope);
                None
            }
        }
    }

    /// Repairs and validates the arguments of a call to `tool` (or
    /// answers with why there is no tool), and runs the `before_tool`
    /// hooks.
    pub(super) async fn prepare(
        &mut self,
        tool: Result<LoopTool, String>,
        id: &str,
        name: &str,
        args: Value,
        parent: Option<&str>,
    ) -> Prepared {
        if self.cancel.is_cancelled() {
            return Prepared::Immediate(ToolOutput::text(CANCELLED));
        }
        let tool = match tool {
            Ok(tool) => tool,
            Err(message) => {
                return Prepared::Immediate(ToolOutput::text(message));
            }
        };
        let raw = tool.tool.prepare_arguments(args);
        let args = match tool.schema.validate(&raw) {
            Ok(args) => args,
            Err(error) => {
                return Prepared::Immediate(ToolOutput::text(
                    error.to_string(),
                ));
            }
        };
        let mut call_seen = ToolCall {
            id: id.to_owned(),
            name: name.to_owned(),
            args,
            parent: parent.map(str::to_owned),
        };
        for plugin in &mut self.plugins {
            let before = call_seen.args.clone();
            match plugin.run.before_tool(&mut call_seen, &plugin.ctx).await {
                Ok(Decision::Allow) => {}
                Ok(Decision::Block(reason)) => {
                    return Prepared::Immediate(ToolOutput::text(reason));
                }
                Err(error) => {
                    return Prepared::Immediate(ToolOutput::text(describe(
                        &error,
                    )));
                }
            }
            if call_seen.args != before {
                // Changed arguments must still satisfy the tool's schema.
                match tool.schema.validate(&call_seen.args) {
                    Ok(args) => call_seen.args = args,
                    Err(error) => {
                        return Prepared::Immediate(ToolOutput::text(
                            error.to_string(),
                        ));
                    }
                }
            }
        }
        Prepared::Ready {
            tool,
            call: call_seen,
        }
    }

    /// Starts up to `count` queued calls. Once the run is cancelled,
    /// queued calls are skipped instead of started.
    pub(super) fn launch(
        &self,
        queue: &mut VecDeque<Ready>,
        batch: &mut Batch,
        skipped: &mut Vec<(usize, ToolCall)>,
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
            self.start_call(
                batch,
                tool,
                call,
                self.cancel.clone(),
                Origin::Batch(index),
            );
            started += 1;
        }
    }

    /// Starts a call with `cancel` as its token, and opens the scope of
    /// the nested calls it makes.
    pub(super) fn start_call(
        &self,
        batch: &mut Batch,
        tool: LoopTool,
        call: ToolCall,
        cancel: CancellationToken,
        origin: Origin,
    ) {
        let key = batch.next_scope;
        batch.next_scope += 1;
        let open = Arc::new(AtomicBool::new(true));
        let id: Arc<str> = call.id.as_str().into();
        batch.scopes.insert(
            key,
            Scope {
                id: id.clone(),
                open: open.clone(),
                children: cancel.child_token(),
                made: 0,
                running: 0,
                exclusive: false,
                waiting: VecDeque::new(),
                ended: false,
            },
        );
        let ctx = ToolCtx {
            cancel,
            updates: ToolUpdates::new(id.clone(), batch.updates.clone()),
            run: self.run.clone(),
            scope: self.pending_turn.clone().map(|turn| RunScope {
                store: self.store.clone(),
                workflow: self.workflow.clone(),
                events: self.events.clone(),
                children: self.children.clone(),
                call: id,
                turn,
                stored: self.last_seq.load(Ordering::SeqCst),
            }),
            plugin: tool
                .owner
                .and_then(|owner| self.plugins.get(owner))
                .map(|plugin| plugin.ctx.clone()),
            nesting: Some(Nesting {
                scope: key,
                open: open.clone(),
                requests: batch.requests.clone(),
                tools: self.tools.clone(),
            }),
        };
        let args = call.args.clone();
        let tool = tool.tool;
        batch.pending.push(Box::pin(async move {
            let updates = ctx.updates.clone();
            let result = tool.call(args, ctx).await;
            updates.close();
            open.store(false, Ordering::SeqCst);
            Finished {
                origin,
                call,
                scope: key,
                result,
            }
        }));
    }
}
