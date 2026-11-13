//! One lane: the requests of one run on one connection, in order.
//!
//! The lane owns its continuation (see [`continuation`]) and applies the
//! recovery ladder from `docs/reference/openai-websocket.md`:
//!
//! | Condition                                  | Action                                   |
//! | ------------------------------------------ | ---------------------------------------- |
//! | `previous_response_not_found` on a delta   | clear the continuation, resend in full   |
//! | `websocket_connection_limit_reached`, before any output | reconnect, then resend in full |
//! | connection lost before any output          | reconnect, then resend in full           |
//! | connection lost after output started       | fail the request                         |
//! | any other server error                     | clear the continuation, fail the request |
//! | cancel                                     | clear the continuation, drop the request |
//!
//! Each request gets at most one transparent recovery. A second one fails
//! the request instead, so a flapping server cannot keep a request looping;
//! the agent's retry policy decides what happens next.
//!
//! The lane never does I/O. It returns [`Action`]s for the driver to carry
//! out, and it ignores frames that arrive for a request it no longer has,
//! such as the tail of a cancelled response.
//!
//! [`continuation`]: super::continuation

use serde_json::Value;
use statig::{
    Outcome::{self, Handled, Super, Transition},
    blocking::StateMachine,
    state_machine,
};

use super::continuation::{Body, Continuation, RequestKind, prepare};

/// The server error that means the connection no longer holds the
/// previous response.
pub const PREVIOUS_RESPONSE_NOT_FOUND: &str = "previous_response_not_found";

/// The server error that means the connection reached its maximum age.
pub const CONNECTION_LIMIT_REACHED: &str = "websocket_connection_limit_reached";

/// Something that happened to the lane's current request or connection.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A `response.*` event for the current request arrived.
    Output,
    /// The current request's response completed.
    Completed {
        response_id: String,
        /// The response's output items, converted as the next request's
        /// input will convert them, without tool outputs.
        output_items: Vec<Value>,
    },
    /// The server answered the current request with an `error`.
    ServerError { code: Option<String> },
    /// The connection closed or failed.
    ConnectionLost,
    /// A new connection is ready, after [`Action::Reconnect`].
    Reconnected,
    /// The run cancelled its request.
    Cancel,
}

/// What the driver must do.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Send this `response.create` body.
    Send(Body),
    /// Open a new connection for this lane, then report
    /// [`Event::Reconnected`].
    Reconnect,
    /// Give up on the current request and report this to the run.
    Fail(Failure),
}

/// Why a request failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    Server { code: Option<String> },
    ConnectionLost { before_first_event: bool },
}

/// Misuse of the lane by its caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LaneError {
    /// A request was submitted while another is in flight.
    #[error("a request is already in flight on this lane")]
    Busy,
}

/// A recovery-ladder step, for [`LaneStats`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Recovery {
    PreviousResponseNotFound,
    ConnectionLimitReached,
    ConnectionLost,
}

/// Counters for one lane. Part of `PoolStats`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaneStats {
    pub full_requests: u64,
    pub delta_requests: u64,
    /// Input items in the last delta request.
    pub last_delta_items: u64,
    pub previous_response_not_found: u64,
    pub connection_limit_reached: u64,
    pub connection_lost: u64,
}

impl LaneStats {
    fn count(&mut self, recovery: Recovery) {
        match recovery {
            Recovery::PreviousResponseNotFound => {
                self.previous_response_not_found += 1
            }
            Recovery::ConnectionLimitReached => {
                self.connection_limit_reached += 1
            }
            Recovery::ConnectionLost => self.connection_lost += 1,
        }
    }
}

/// One lane: what it continues from, its counters, and where its
/// request is.
#[derive(Default)]
pub struct Lane {
    continuation: Option<Continuation>,
    stats: LaneStats,
    request: StateMachine<Requests>,
}

impl std::fmt::Debug for Lane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lane")
            .field("continuation", &self.continuation)
            .field("stats", &self.stats)
            .field("request", self.request.state())
            .finish()
    }
}

/// A request, as the run gave it and as it went out.
#[derive(Debug, Clone)]
pub(crate) struct Request {
    full_body: Body,
    kind: RequestKind,
}

impl Lane {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn stats(&self) -> &LaneStats {
        &self.stats
    }

    /// Whether a request is in flight.
    pub fn is_busy(&self) -> bool {
        !matches!(self.request.state(), State::Idle {})
    }

    /// Takes the lane's continuation, as it leaves its connection: the
    /// connection keeps it for the next lane of the same conversation.
    pub fn take_continuation(&mut self) -> Option<Continuation> {
        self.continuation.take()
    }

    /// Sets what the lane continues from, as it comes onto a connection:
    /// what that connection's last lane left, or nothing.
    pub fn set_continuation(&mut self, continuation: Option<Continuation>) {
        self.continuation = continuation;
    }

    /// Submits the run's next request, given as it would be sent in full.
    pub fn submit(&mut self, full_body: Body) -> Result<Action, LaneError> {
        if self.is_busy() {
            return Err(LaneError::Busy);
        }
        self.give(&Input::Submit(&full_body)).ok_or(LaneError::Busy)
    }

    /// Applies one event and returns what the driver must do, if anything.
    pub fn handle(&mut self, event: Event) -> Option<Action> {
        self.give(&Input::Event(&event))
    }

    fn give(&mut self, input: &Input<'_>) -> Option<Action> {
        let mut context = Context {
            continuation: &mut self.continuation,
            stats: &mut self.stats,
            action: None,
        };
        self.request.handle_with_context(input, &mut context);
        context.action
    }
}

/// What a request's state answers with: the lane's continuation and
/// counters, which it changes, and what the driver must do.
struct Context<'a> {
    continuation: &'a mut Option<Continuation>,
    stats: &'a mut LaneStats,
    action: Option<Action>,
}

impl Context<'_> {
    /// Prepares `full_body` against the continuation, counts it, and
    /// sends it: the request it is now.
    fn send(&mut self, full_body: Body) -> Request {
        let prepared = prepare(self.continuation.as_ref(), &full_body);
        match prepared.kind {
            RequestKind::Full => self.stats.full_requests += 1,
            RequestKind::Delta => {
                self.stats.delta_requests += 1;
                self.stats.last_delta_items = prepared.body.input.len() as u64;
            }
        }
        self.action = Some(Action::Send(prepared.body));
        Request {
            full_body,
            kind: prepared.kind,
        }
    }

    /// Opens a new connection for the request; its cache goes.
    fn reconnect(&mut self, recovery: Recovery) {
        self.stats.count(recovery);
        *self.continuation = None;
        self.action = Some(Action::Reconnect);
    }

    fn fail(&mut self, failure: Failure) {
        self.action = Some(Action::Fail(failure));
    }
}

/// What reaches a request: the run's next one, or an [`Event`].
enum Input<'a> {
    Submit(&'a Body),
    Event(&'a Event),
}

/// Where the lane's request is. Each gets at most one transparent
/// recovery: `retried` once it had it.
#[derive(Debug, Default)]
struct Requests;

#[state_machine(
    initial = "State::idle()",
    state(derive(Debug)),
    visibility = "pub(crate)"
)]
impl Requests {
    /// No request in flight.
    #[state]
    fn idle(context: &mut Context<'_>, event: &Input<'_>) -> Outcome<State> {
        match event {
            Input::Submit(full_body) => Transition(State::awaiting(
                context.send((*full_body).clone()),
                false,
            )),
            Input::Event(Event::ServerError { .. } | Event::ConnectionLost) => {
                // The server evicts its cache after an error.
                *context.continuation = None;
                Handled
            }
            Input::Event(Event::Reconnected | Event::Cancel) => {
                *context.continuation = None;
                Handled
            }
            Input::Event(Event::Output | Event::Completed { .. }) => Handled,
        }
    }

    /// Sent, with no output yet: a failure here can still be recovered
    /// from unseen, once.
    #[state(superstate = "in_flight")]
    fn awaiting(
        request: &mut Request,
        retried: &mut bool,
        context: &mut Context<'_>,
        event: &Input<'_>,
    ) -> Outcome<State> {
        let Input::Event(event) = event else {
            return Super;
        };
        match event {
            Event::Output => Transition(State::streaming(request.clone())),
            Event::ServerError { code } if !*retried => match code.as_deref() {
                Some(PREVIOUS_RESPONSE_NOT_FOUND)
                    if request.kind == RequestKind::Delta =>
                {
                    context.stats.count(Recovery::PreviousResponseNotFound);
                    *context.continuation = None;
                    // The continuation is gone, so this resend is full.
                    let resent = context.send(request.full_body.clone());
                    Transition(State::awaiting(resent, true))
                }
                Some(CONNECTION_LIMIT_REACHED) => {
                    context.reconnect(Recovery::ConnectionLimitReached);
                    Transition(State::reconnecting(request.clone()))
                }
                _ => Super,
            },
            Event::ConnectionLost if !*retried => {
                context.reconnect(Recovery::ConnectionLost);
                Transition(State::reconnecting(request.clone()))
            }
            Event::ConnectionLost => {
                *context.continuation = None;
                context.fail(Failure::ConnectionLost {
                    before_first_event: true,
                });
                Transition(State::idle())
            }
            _ => Super,
        }
    }

    /// Its output has started: the run sees it, so nothing is sent again.
    #[state(superstate = "in_flight")]
    fn streaming(
        request: &mut Request,
        context: &mut Context<'_>,
        event: &Input<'_>,
    ) -> Outcome<State> {
        let _ = request;
        match event {
            Input::Event(Event::Output) => Handled,
            Input::Event(Event::ConnectionLost) => {
                *context.continuation = None;
                context.fail(Failure::ConnectionLost {
                    before_first_event: false,
                });
                Transition(State::idle())
            }
            _ => Super,
        }
    }

    /// Waiting for a new connection, then sent again in full: its
    /// recovery.
    #[state(superstate = "in_flight")]
    fn reconnecting(
        request: &mut Request,
        context: &mut Context<'_>,
        event: &Input<'_>,
    ) -> Outcome<State> {
        match event {
            Input::Event(Event::Reconnected) => {
                *context.continuation = None;
                let resent = context.send(request.full_body.clone());
                Transition(State::awaiting(resent, true))
            }
            // The old connection is gone; a late completion from it
            // completes nothing.
            Input::Event(Event::Output | Event::Completed { .. }) => Handled,
            Input::Event(Event::ConnectionLost) => {
                // The reconnect itself failed.
                *context.continuation = None;
                context.fail(Failure::ConnectionLost {
                    before_first_event: true,
                });
                Transition(State::idle())
            }
            _ => Super,
        }
    }

    /// A request in flight: it completes, fails with the server's error,
    /// or is cancelled.
    #[superstate]
    fn in_flight(
        request: &mut Request,
        context: &mut Context<'_>,
        event: &Input<'_>,
    ) -> Outcome<State> {
        match event {
            // One request at a time: [`Lane::submit`] refuses it first.
            Input::Submit(_) => Handled,
            Input::Event(Event::Completed {
                response_id,
                output_items,
            }) => {
                *context.continuation = Some(Continuation::record(
                    &request.full_body,
                    output_items.clone(),
                    response_id.clone(),
                ));
                Transition(State::idle())
            }
            Input::Event(Event::ServerError { code }) => {
                *context.continuation = None;
                context.fail(Failure::Server { code: code.clone() });
                Transition(State::idle())
            }
            Input::Event(Event::Cancel) => {
                *context.continuation = None;
                Transition(State::idle())
            }
            // A reconnect notice for a request not reconnecting: the cache
            // is gone all the same.
            Input::Event(Event::Reconnected) => {
                *context.continuation = None;
                Handled
            }
            Input::Event(Event::Output | Event::ConnectionLost) => Handled,
        }
    }
}
