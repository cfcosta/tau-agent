//! Asking the model for a response, with retries: the loop's turns and
//! a plugin's own requests (`PluginCtx::ask`) read a response the same
//! way.

use std::time::Duration;

use tau_ai::llm::EventStream;

use super::*;

/// How reading a response stream ended.
pub(crate) enum Streamed {
    /// With a terminal event: the message, and the class of its error
    /// (`Fatal` when it has none).
    Finished(AssistantMessage, Class),
    /// Cancelled first, with what had come.
    Cancelled(Accumulator),
    /// An event out of the stream's grammar, with what had come.
    BrokeGrammar(Accumulator),
    /// Without a terminal event, with what had come.
    NoTerminal(Accumulator),
}

/// A response stream being read: each event is accumulated as it comes.
pub(crate) struct Reading {
    stream: EventStream,
    accumulator: Accumulator,
    class: Class,
    stopped: Option<Stop>,
}

/// Why reading stopped before the stream's end.
#[derive(Clone, Copy)]
enum Stop {
    Cancelled,
    BrokeGrammar,
}

impl Reading {
    pub(crate) fn new(stream: EventStream) -> Self {
        Self {
            stream,
            accumulator: Accumulator::new(),
            class: Class::Fatal,
            stopped: None,
        }
    }

    /// The next event, accumulated. `None` once the stream ends, an
    /// event breaks its grammar, or `cancel` fires: [`Self::end`] says
    /// which.
    pub(crate) async fn next(
        &mut self,
        cancel: &CancellationToken,
    ) -> Option<AssistantEvent> {
        if self.stopped.is_some() {
            return None;
        }
        let event = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                self.stopped = Some(Stop::Cancelled);
                return None;
            }
            event = self.stream.next() => event,
        }?;
        if let AssistantEvent::Error { class, .. } = &event {
            self.class = *class;
        }
        if self.accumulator.push(event.clone()).is_err() {
            self.stopped = Some(Stop::BrokeGrammar);
            return None;
        }
        Some(event)
    }

    /// How reading ended, once [`Self::next`] gave `None`.
    pub(crate) fn end(self) -> Streamed {
        match self.stopped {
            Some(Stop::Cancelled) => Streamed::Cancelled(self.accumulator),
            Some(Stop::BrokeGrammar) => {
                Streamed::BrokeGrammar(self.accumulator)
            }
            None if self.accumulator.is_finished() => Streamed::Finished(
                self.accumulator.finish().expect("finished"),
                self.class,
            ),
            None => Streamed::NoTerminal(self.accumulator),
        }
    }

    /// Reads to the end, unless `cancel` fires first.
    pub(crate) async fn read_all(
        mut self,
        cancel: &CancellationToken,
    ) -> Streamed {
        while self.next(cancel).await.is_some() {}
        self.end()
    }
}

/// Waits `delay` before a retry. False when `cancel` fires first.
pub(crate) async fn wait(cancel: &CancellationToken, delay: Duration) -> bool {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => false,
        _ = tokio::time::sleep(delay) => true,
    }
}

/// What a response cut short by a steering message leaves: what had
/// finished in it, or an empty message that stops. Dropping the stream
/// stops it; the next request goes in full.
fn cut(
    reading: Reading,
    model: &str,
    timestamp: Timestamp,
) -> AssistantMessage {
    reading
        .accumulator
        .cut()
        .unwrap_or_else(|| AssistantMessage {
            content: Vec::new(),
            model: model.to_owned(),
            response_id: None,
            usage: Usage::default(),
            stop_reason: MessageStop::Stop,
            error_message: None,
            timestamp,
        })
}

/// Ends a partial message with an error of `reason`.
pub(super) fn finish(
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

impl Runner {
    /// Asks for a response, retrying failures classified as retryable
    /// with the run's policy. Returns the last response and its class.
    pub(super) async fn respond(
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
            if !wait(&self.cancel, delay).await {
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
        }
    }

    /// Streams one response, emitting its deltas. A cancel drops the
    /// stream and ends the message as aborted.
    pub(super) async fn respond_once(
        &mut self,
        transcript: &[Message],
    ) -> (AssistantMessage, Class) {
        let timestamp = (self.clock)();
        let stream = self.session.respond(transcript, timestamp);
        let model = self.session.settings().model.clone();
        let mut reading = Reading::new(stream);
        let mut call_id = String::new();
        loop {
            // A steering message cuts the response short: the person
            // should not wait for the rest of an answer they moved on
            // from (Codex's instant interrupt).
            let event = tokio::select! {
                biased;
                event = reading.next(&self.cancel) => event,
                Some(text) = self.steering.recv() => {
                    self.preempted = Some(text);
                    return (cut(reading, &model, timestamp), Class::Fatal);
                }
            };
            let Some(event) = event else {
                break;
            };
            match event {
                AssistantEvent::TextDelta { delta, .. } => {
                    self.emit(RunEvent::TextDelta {
                        run: self.run.clone(),
                        parent: self.parent.clone(),
                        delta,
                    })
                    .await
                }
                AssistantEvent::ThinkingDelta { delta, .. } => {
                    self.emit(RunEvent::ThinkingDelta {
                        run: self.run.clone(),
                        delta,
                    })
                    .await
                }
                AssistantEvent::ToolCallStart { id, .. } => call_id = id,
                AssistantEvent::ToolCallDelta { delta, .. } => {
                    self.emit(RunEvent::ToolCallDelta {
                        run: self.run.clone(),
                        call_id: call_id.clone(),
                        json_fragment: delta,
                    })
                    .await
                }
                _ => {}
            }
        }
        let streamed = reading.end();
        let (accumulator, reason, text) = match streamed {
            Streamed::Finished(message, class) => return (message, class),
            Streamed::Cancelled(accumulator) => {
                (accumulator, ErrorReason::Aborted, CANCELLED)
            }
            Streamed::BrokeGrammar(accumulator) => (
                accumulator,
                ErrorReason::Error,
                "the model's response broke the event grammar",
            ),
            Streamed::NoTerminal(accumulator) => (
                accumulator,
                ErrorReason::Error,
                "the model's response ended without a terminal event",
            ),
        };
        (
            finish(accumulator, &model, timestamp, reason, text),
            Class::Fatal,
        )
    }

    /// Fails every call of a response cut off by the output limit.
    pub(super) async fn fail_truncated(
        &mut self,
        calls: &[MessageToolCall],
    ) -> Vec<ToolResultMessage> {
        let mut results = Vec::new();
        for call in calls {
            let args = Value::Object(call.arguments.clone());
            self.emit_start(&call.id, &call.name, args, None).await;
            let output = ToolOutput::text(format!(
                "Tool call \"{}\" {TRUNCATED}",
                call.name
            ));
            self.emit_end(&call.id, &output, true, None).await;
            results.push(self.result_message(call, output, true));
        }
        results
    }
}
