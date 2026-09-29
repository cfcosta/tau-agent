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

use super::continuation::{Body, Continuation, RequestKind, prepare};

/// The server error that means the connection no longer holds the
/// previous response.
pub const PREVIOUS_RESPONSE_NOT_FOUND: &str = "previous_response_not_found";

/// The server error that means the connection reached its maximum age.
pub const CONNECTION_LIMIT_REACHED: &str = "websocket_connection_limit_reached";

/// The server error that means the connection has as many named streams
/// as it takes. Handled like [`CONNECTION_LIMIT_REACHED`]: whether the
/// server counts streams in use or every stream id it has seen, another
/// connection has room.
pub const STREAM_LIMIT_REACHED: &str = "websocket_stream_limit_reached";

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
    StreamLimitReached,
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
    pub stream_limit_reached: u64,
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
            Recovery::StreamLimitReached => self.stream_limit_reached += 1,
            Recovery::ConnectionLost => self.connection_lost += 1,
        }
    }
}

#[derive(Debug, Default)]
pub struct Lane {
    continuation: Option<Continuation>,
    request: Option<Request>,
    stats: LaneStats,
}

#[derive(Debug)]
struct Request {
    full_body: Body,
    kind: RequestKind,
    output_started: bool,
    recovered: bool,
    /// Waiting for [`Event::Reconnected`] before resending.
    reconnecting: bool,
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
        self.request.is_some()
    }

    /// Submits the run's next request, given as it would be sent in full.
    pub fn submit(&mut self, full_body: Body) -> Result<Action, LaneError> {
        if self.request.is_some() {
            return Err(LaneError::Busy);
        }
        Ok(self.send(full_body))
    }

    /// Applies one event and returns what the driver must do, if anything.
    pub fn handle(&mut self, event: Event) -> Option<Action> {
        match event {
            Event::Output => {
                if let Some(request) = &mut self.request {
                    request.output_started = true;
                }
                None
            }
            Event::Completed {
                response_id,
                output_items,
            } => {
                // While reconnecting, the old connection is gone; a late
                // completion from it must not complete the resend.
                if self.request.as_ref().is_some_and(|r| r.reconnecting) {
                    return None;
                }
                if let Some(request) = self.request.take() {
                    self.continuation = Some(Continuation::record(
                        &request.full_body,
                        output_items,
                        response_id,
                    ));
                }
                None
            }
            Event::ServerError { code } => {
                // The server evicts its cache after an error.
                self.continuation = None;
                let request = self.request.as_ref()?;
                match code.as_deref() {
                    Some(PREVIOUS_RESPONSE_NOT_FOUND)
                        if request.kind == RequestKind::Delta
                            && !request.recovered =>
                    {
                        self.stats.count(Recovery::PreviousResponseNotFound);
                        let full_body = self.request.take()?.full_body;
                        // The continuation is gone, so this resend is full.
                        let action = self.send(full_body);
                        self.mark_recovered();
                        Some(action)
                    }
                    Some(CONNECTION_LIMIT_REACHED)
                        if !request.output_started && !request.recovered =>
                    {
                        self.stats.count(Recovery::ConnectionLimitReached);
                        Some(self.start_reconnect())
                    }
                    Some(STREAM_LIMIT_REACHED)
                        if !request.output_started && !request.recovered =>
                    {
                        self.stats.count(Recovery::StreamLimitReached);
                        Some(self.start_reconnect())
                    }
                    _ => {
                        self.request = None;
                        Some(Action::Fail(Failure::Server { code }))
                    }
                }
            }
            Event::ConnectionLost => {
                self.continuation = None;
                let request = self.request.as_ref()?;
                if request.reconnecting {
                    // The reconnect itself failed.
                    self.request = None;
                    Some(Action::Fail(Failure::ConnectionLost {
                        before_first_event: true,
                    }))
                } else if !request.output_started && !request.recovered {
                    self.stats.count(Recovery::ConnectionLost);
                    Some(self.start_reconnect())
                } else {
                    let before_first_event = !request.output_started;
                    self.request = None;
                    Some(Action::Fail(Failure::ConnectionLost {
                        before_first_event,
                    }))
                }
            }
            Event::Reconnected => {
                self.continuation = None;
                match self.request.take() {
                    Some(request) if request.reconnecting => {
                        let action = self.send(request.full_body);
                        self.mark_recovered();
                        Some(action)
                    }
                    other => {
                        self.request = other;
                        None
                    }
                }
            }
            Event::Cancel => {
                self.continuation = None;
                self.request = None;
                None
            }
        }
    }

    /// Prepares and records a request; returns the send action.
    fn send(&mut self, full_body: Body) -> Action {
        let prepared = prepare(self.continuation.as_ref(), &full_body);
        match prepared.kind {
            RequestKind::Full => self.stats.full_requests += 1,
            RequestKind::Delta => {
                self.stats.delta_requests += 1;
                self.stats.last_delta_items = prepared.body.input.len() as u64;
            }
        }
        self.request = Some(Request {
            full_body,
            kind: prepared.kind,
            output_started: false,
            recovered: false,
            reconnecting: false,
        });
        Action::Send(prepared.body)
    }

    fn mark_recovered(&mut self) {
        if let Some(request) = &mut self.request {
            request.recovered = true;
        }
    }

    fn start_reconnect(&mut self) -> Action {
        self.continuation = None;
        if let Some(request) = &mut self.request {
            request.reconnecting = true;
            request.recovered = true;
        }
        Action::Reconnect
    }
}
