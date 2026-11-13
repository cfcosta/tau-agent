//! One request in flight on a lane, as the driver sees it: waiting for
//! the lane to send it, sent on a connection, holding a server error
//! while the lane decides whether to resend, and over. The driver drops
//! a request once it is over, which is exactly when the lane is free
//! again: a request whose caller saw `Done` has told the lane it
//! completed, and one whose caller saw `Error` has failed there.

use serde_json::Value;
use statig::prelude::*;
use tokio::sync::mpsc;

use crate::{
    event::{Accumulator, AssistantEvent, ErrorReason},
    message::{Timestamp, Usage},
    refusal::Refusal,
    responses::{input::response_items, stream::StreamProcessor},
    retry::Class,
    ws::proto::{lane, pool::ConnectionId},
};

/// What a request keeps across its attempts.
pub(super) struct Request {
    events: mpsc::UnboundedSender<AssistantEvent>,
    model: String,
    timestamp: Timestamp,
    /// Whether the caller has seen `Start`.
    start_sent: bool,
    /// Whether the request went as Responses Lite, whose response the
    /// server can stop.
    lite: bool,
}

/// One attempt's stream, from the connection it was sent on.
pub(super) struct Attempt {
    processor: StreamProcessor,
    /// The stream as the caller saw it, so a `Done` it could not take
    /// never reaches the caller.
    accumulator: Accumulator,
    /// Whether the lane knows output has started.
    output_reported: bool,
}

/// What happens to a request.
pub(super) enum Input<'a> {
    /// The lane sent it, or sent it again, on this connection.
    Sent(ConnectionId),
    /// A frame of its response arrived.
    Frame(&'a Value),
    /// The lane gave up on it: it fails with the error it held, else
    /// with `refusal`'s, else as its stream stands.
    Failed(Option<&'a Refusal>),
    /// Its connection was refused for good: it fails with the refusal's
    /// error.
    Refused(&'a Refusal),
    /// The caller cancelled it.
    Cancel,
}

/// What the driver must do after an [`Input`].
#[derive(Default)]
pub(super) struct Effects {
    /// Events for the lane, in order.
    pub(super) lane: Vec<lane::Event>,
    /// A cancelled attempt whose response is still streaming.
    pub(super) cut: Option<Cut>,
}

/// A cancelled response still streaming on `on`: the driver stops it on
/// the server with `interrupt`, its id, or closes the connection.
pub(super) struct Cut {
    pub(super) on: ConnectionId,
    pub(super) interrupt: Option<String>,
}

impl Request {
    pub(super) fn new(
        events: mpsc::UnboundedSender<AssistantEvent>,
        model: String,
        timestamp: Timestamp,
        lite: bool,
    ) -> Self {
        Self {
            events,
            model,
            timestamp,
            start_sent: false,
            lite,
        }
    }

    fn attempt(&self) -> Attempt {
        Attempt {
            processor: StreamProcessor::new(self.model.clone(), self.timestamp),
            accumulator: Accumulator::new(),
            output_reported: false,
        }
    }

    /// Sends `event` to the caller, dropping repeated `Start`s: the
    /// processor emits `Start` before anything else, even when the first
    /// frame is an error, so every stream the caller sees begins with
    /// one, and a resent attempt's is not news.
    fn send(&mut self, event: AssistantEvent) {
        if matches!(event, AssistantEvent::Start { .. }) {
            if self.start_sent {
                return;
            }
            self.start_sent = true;
        }
        let _ = self.events.send(event);
    }

    /// Keeps `event` in `attempt`'s stream and sends it to the caller.
    fn forward(&mut self, attempt: &mut Attempt, event: AssistantEvent) {
        let _ = attempt.accumulator.push(event.clone());
        self.send(event);
    }

    /// Ends the request with `refusal`'s error.
    fn refuse(&mut self, refusal: &Refusal) {
        self.send(AssistantEvent::Start {
            model: self.model.clone(),
            response_id: None,
            timestamp: self.timestamp,
        });
        self.send(AssistantEvent::Error {
            reason: ErrorReason::Error,
            message: refusal.message.clone(),
            usage: Usage::default(),
            class: refusal.class(),
        });
    }

    /// Ends the request as `attempt`'s stream stands.
    fn close(&mut self, attempt: &mut Attempt) {
        for event in attempt.processor.close() {
            self.forward(attempt, event);
        }
    }
}

#[state_machine(initial = "State::waiting()")]
impl Request {
    /// Submitted, not sent yet.
    #[state]
    fn waiting(&mut self, event: &Input<'_>) -> Outcome<State> {
        match event {
            Input::Sent(on) => Transition(State::sent(*on, self.attempt())),
            Input::Frame(_) => Handled,
            Input::Failed(Some(refusal)) | Input::Refused(refusal) => {
                self.refuse(refusal);
                Transition(State::over())
            }
            Input::Failed(None) => {
                let mut attempt = self.attempt();
                self.close(&mut attempt);
                Transition(State::over())
            }
            Input::Cancel => Transition(State::over()),
        }
    }

    /// Sent on `on`, its response streaming or yet to start.
    #[state]
    fn sent(
        &mut self,
        on: &mut ConnectionId,
        attempt: &mut Attempt,
        context: &mut Effects,
        event: &Input<'_>,
    ) -> Outcome<State> {
        match event {
            Input::Sent(again) => {
                Transition(State::sent(*again, self.attempt()))
            }
            Input::Frame(frame) => self.frame(attempt, context, frame),
            Input::Failed(Some(refusal)) | Input::Refused(refusal) => {
                self.refuse(refusal);
                Transition(State::over())
            }
            Input::Failed(None) => {
                self.close(attempt);
                Transition(State::over())
            }
            // Its terminal frame has not come, or it would not be here.
            Input::Cancel => {
                context.cut = Some(Cut {
                    on: *on,
                    interrupt: attempt
                        .processor
                        .response_id()
                        .filter(|_| self.lite)
                        .map(str::to_owned),
                });
                Transition(State::over())
            }
        }
    }

    /// Answered on `on` with `error`, held back while the lane decides
    /// whether to send it again.
    #[state]
    fn held(
        &mut self,
        error: &mut AssistantEvent,
        event: &Input<'_>,
    ) -> Outcome<State> {
        match event {
            Input::Sent(on) => Transition(State::sent(*on, self.attempt())),
            Input::Frame(_) => Handled,
            Input::Failed(_) => {
                self.send(error.clone());
                Transition(State::over())
            }
            Input::Refused(refusal) => {
                self.refuse(refusal);
                Transition(State::over())
            }
            Input::Cancel => Transition(State::over()),
        }
    }

    /// Done: the caller has its terminal event, and the lane is free.
    #[state]
    fn over() -> Outcome<State> {
        Handled
    }
}

impl Request {
    /// Takes one frame of `attempt`'s response.
    fn frame(
        &mut self,
        attempt: &mut Attempt,
        context: &mut Effects,
        frame: &Value,
    ) -> Outcome<State> {
        for event in attempt.processor.push(frame) {
            match event {
                AssistantEvent::Error { .. } => {
                    context.lane.push(lane::Event::ServerError {
                        code: attempt.processor.error_code().map(str::to_owned),
                    });
                    return Transition(State::held(event));
                }
                AssistantEvent::Done { .. } => {
                    let mut accumulator =
                        std::mem::take(&mut attempt.accumulator);
                    let message = match accumulator.push(event.clone()) {
                        Ok(()) => accumulator.finish(),
                        Err(error) => Err(error),
                    };
                    let message = match message {
                        Ok(message) => message,
                        // The stream does not hold up, so it cannot be
                        // done: the request fails as the server's answer
                        // would.
                        Err(error) => {
                            context
                                .lane
                                .push(lane::Event::ServerError { code: None });
                            return Transition(State::held(
                                AssistantEvent::Error {
                                    reason: ErrorReason::Error,
                                    message: format!(
                                        "The response did not hold up: {error}"
                                    ),
                                    usage: Usage::default(),
                                    class: Class::Fatal,
                                },
                            ));
                        }
                    };
                    self.send(event);
                    context.lane.push(lane::Event::Completed {
                        response_id: message
                            .response_id
                            .clone()
                            .unwrap_or_default(),
                        output_items: response_items(&message),
                    });
                    return Transition(State::over());
                }
                AssistantEvent::Start { .. } => self.forward(attempt, event),
                _ => {
                    if !attempt.output_reported {
                        attempt.output_reported = true;
                        context.lane.push(lane::Event::Output);
                    }
                    self.forward(attempt, event);
                }
            }
        }
        Handled
    }
}
