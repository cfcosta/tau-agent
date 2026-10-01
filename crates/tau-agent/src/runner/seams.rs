//! The plugins' seams the loop calls around a turn, and what it does
//! with their failures.

use super::*;

/// Why a rewrite cannot replace `transcript`, if it cannot: it must not
/// be empty, must end with the message the transcript ends with (the one
/// the next request answers), and must keep every tool call of a
/// completed assistant message paired with its result, calls first.
pub(super) fn check_rewrite(
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

impl Runner {
    /// Asks each plugin, in order, for the effort of the turn's request,
    /// until one picks it, and sets it on the session.
    pub(super) async fn choose_effort(
        &mut self,
        transcript: &[Message],
        turn: u32,
    ) {
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
        self.report_failures(&failures).await;
    }

    /// Offers the transcript to each plugin, in order, until one rewrites
    /// it; stores the rewrite and makes it the working transcript.
    /// `Ok(Ok(()))` when a plugin rewrote it; otherwise the plugins that
    /// failed, and why (each also reported as a `PluginError`).
    pub(super) async fn rewrite_context(
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
        self.report_failures(&failures).await;
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
        self.report_failures(&failures).await;
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
    pub(super) async fn before_stop(
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
                    .push((plugin.ctx.plugin().into(), describe(&error))),
            }
        }
        self.report_failures(&failures).await;
        decision
    }

    /// Stores the usage plugins charged since the last write, if any.
    pub(super) async fn save_charged(&mut self) -> Result<(), StoreError> {
        let unsaved = {
            let charged = self.charged.lock().expect("not poisoned");
            charged.unsaved != Usage::default()
                || !charged.unsaved_by.is_empty()
        };
        if !unsaved {
            return Ok(());
        }
        self.persist_entries(Vec::new(), &Usage::default(), 0).await
    }

    /// Hands an event to every plugin, in order, then to the subscriber;
    /// what plugins charged goes first, then their reports. `RunStart`
    /// stays first: what plugins charged or reported as the run started
    /// follows it.
    /// Reports each plugin's failure at a seam as a
    /// `RunEvent::PluginError`, in order.
    pub(super) async fn report_failures(&mut self, failures: &Failures) {
        for (plugin, message) in failures {
            self.emit(RunEvent::PluginError {
                run: self.run.clone(),
                plugin: plugin.clone(),
                message: message.clone(),
            })
            .await;
        }
    }
}
