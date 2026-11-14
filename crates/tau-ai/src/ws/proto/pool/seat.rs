//! Where a pool's lane is ([`super::Pool`]): with no connection, waiting
//! a while for a busy one of its path or its parent's, or holding one.
//! What moves in and out (the request a waiting lane keeps, the
//! connection a lane gives up) passes through the machine's [`Hand`],
//! so none is lost to an input that comes out of order.

use std::time::Duration;

use statig::{
    Outcome::{self, Handled, Transition},
    blocking::{IntoStateMachineExt as _, StateMachine},
    state_machine,
};

use super::ConnectionId;
use crate::ws::proto::continuation::Body;

/// What happens to a lane's seat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Input {
    /// It waits until then, with the request in the hand, if any.
    Wait(Duration),
    /// A request, in the hand, is sent while it waits: it keeps it.
    Submit,
    /// Its waiting request is cancelled: it goes back in the hand.
    Withdraw,
    /// It takes this connection; the request it waited with, and a
    /// connection it held, go back in the hand.
    Attach(ConnectionId),
    /// It gives up the connection it holds, into the hand.
    Leave,
}

/// What an input hands the machine, and what it hands back.
#[derive(Debug, Default)]
pub(super) struct Hand {
    pub(super) body: Option<Body>,
    pub(super) connection: Option<ConnectionId>,
}

#[derive(Default)]
pub(super) struct Seats;

#[state_machine(
    initial = "State::unseated()",
    state(derive(Debug, Clone, PartialEq))
)]
impl Seats {
    /// Without a connection: before it took one, and after it lost one.
    /// A request it is handed stays in the hand: it waits for none.
    #[state]
    fn unseated(context: &mut Hand, event: &Input) -> Outcome<State> {
        match event {
            Input::Wait(until) => {
                Transition(State::waiting(*until, context.body.take()))
            }
            Input::Attach(connection) => Transition(State::held(*connection)),
            Input::Submit | Input::Withdraw | Input::Leave => Handled,
        }
    }

    /// Waiting for a busy connection of its path, or its parent's, until
    /// `until`, with the request sent meanwhile.
    #[state]
    fn waiting(
        context: &mut Hand,
        until: &mut Duration,
        pending: &mut Option<Body>,
        event: &Input,
    ) -> Outcome<State> {
        match event {
            Input::Wait(later) => {
                *until = *later;
                Handled
            }
            Input::Submit if pending.is_none() => {
                *pending = context.body.take();
                Handled
            }
            Input::Withdraw => {
                context.body = pending.take();
                Handled
            }
            Input::Attach(connection) => {
                context.body = pending.take();
                Transition(State::held(*connection))
            }
            Input::Submit | Input::Leave => Handled,
        }
    }

    /// Holding `connection`.
    #[state]
    fn held(
        context: &mut Hand,
        connection: &mut ConnectionId,
        event: &Input,
    ) -> Outcome<State> {
        match event {
            Input::Leave => {
                context.connection = Some(*connection);
                Transition(State::unseated())
            }
            Input::Attach(other) => {
                context.connection = Some(*connection);
                Transition(State::held(*other))
            }
            Input::Wait(_) | Input::Submit | Input::Withdraw => Handled,
        }
    }
}

/// A lane's seat.
pub(super) struct Seat(StateMachine<Seats>);

impl std::fmt::Debug for Seat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.state().fmt(f)
    }
}

impl Default for Seat {
    fn default() -> Self {
        Self(Seats.state_machine())
    }
}

impl Seat {
    fn take(&mut self, input: Input, mut hand: Hand) -> Hand {
        self.0.handle_with_context(&input, &mut hand);
        hand
    }

    /// Has it wait until `until`, with `pending`. A lane already waiting
    /// waits until then with what it kept, and hands `pending` back; one
    /// holding a connection waits for none, and hands it back too.
    pub(super) fn wait(
        &mut self,
        until: Duration,
        pending: Option<Body>,
    ) -> Option<Body> {
        self.take(
            Input::Wait(until),
            Hand {
                body: pending,
                connection: None,
            },
        )
        .body
    }

    /// Keeps `body` while it waits; hands it back otherwise, or when it
    /// keeps one already.
    pub(super) fn submit(&mut self, body: Body) -> Option<Body> {
        self.take(
            Input::Submit,
            Hand {
                body: Some(body),
                connection: None,
            },
        )
        .body
    }

    /// The request it waited with, cancelled.
    pub(super) fn withdraw(&mut self) -> Option<Body> {
        self.take(Input::Withdraw, Hand::default()).body
    }

    /// Takes `connection`. Returns the request it waited with, and the
    /// connection it held before, if any.
    pub(super) fn attach(
        &mut self,
        connection: ConnectionId,
    ) -> (Option<Body>, Option<ConnectionId>) {
        let hand = self.take(Input::Attach(connection), Hand::default());
        (hand.body, hand.connection)
    }

    /// Gives up the connection it holds, if it holds one.
    pub(super) fn leave(&mut self) -> Option<ConnectionId> {
        self.take(Input::Leave, Hand::default()).connection
    }

    pub(super) fn connection(&self) -> Option<ConnectionId> {
        match self.0.state() {
            State::Held { connection } => Some(*connection),
            _ => None,
        }
    }

    pub(super) fn is_unseated(&self) -> bool {
        matches!(self.0.state(), State::Unseated {})
    }

    /// Until when it waits, while it does.
    pub(super) fn waiting_until(&self) -> Option<Duration> {
        match self.0.state() {
            State::Waiting { until, .. } => Some(*until),
            _ => None,
        }
    }

    /// Whether it waits with a request.
    pub(super) fn waits_with_request(&self) -> bool {
        matches!(
            self.0.state(),
            State::Waiting {
                pending: Some(_),
                ..
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};
    use serde_json::{Map, Value};

    use super::*;

    fn body(n: u64) -> Body {
        let mut fields = Map::new();
        fields.insert("n".into(), Value::from(n));
        Body::from(fields)
    }

    fn number(body: &Body) -> u64 {
        body.fields.get("n").and_then(Value::as_u64).unwrap()
    }

    /// A seat given any inputs, in any order: what the pool never sends
    /// (a request to a lane with no connection to wait for, a connection
    /// to a lane holding one) among what it does.
    struct Seating {
        seat: Seat,
        next: u64,
        /// The requests handed in and not handed back.
        kept: Vec<u64>,
        /// The connections handed in and not handed back.
        holding: Vec<ConnectionId>,
    }

    impl Seating {
        fn back_body(&mut self, body: Option<Body>) {
            if let Some(body) = body {
                let n = number(&body);
                let at = self.kept.iter().position(|kept| *kept == n);
                let at =
                    at.unwrap_or_else(|| panic!("request {n} came back twice"));
                self.kept.remove(at);
            }
        }

        fn back_connection(&mut self, connection: Option<ConnectionId>) {
            if let Some(connection) = connection {
                let at =
                    self.holding.iter().position(|held| *held == connection);
                let at = at.unwrap_or_else(|| {
                    panic!("connection {connection} came back twice")
                });
                self.holding.remove(at);
            }
        }

        fn fresh(&mut self) -> u64 {
            self.next += 1;
            self.next
        }
    }

    #[hegel::state_machine]
    impl Seating {
        #[rule]
        fn wait(&mut self, tc: TestCase) {
            let until = Duration::from_secs(
                tc.draw(gs::integers::<u64>().max_value(9)),
            );
            let pending = tc.draw(gs::booleans()).then(|| self.fresh());
            self.kept.extend(pending);
            let back = self.seat.wait(until, pending.map(body));
            self.back_body(back);
        }

        #[rule]
        fn submit(&mut self, _tc: TestCase) {
            let n = self.fresh();
            self.kept.push(n);
            let refused = self.seat.submit(body(n));
            self.back_body(refused);
        }

        #[rule]
        fn withdraw(&mut self, _tc: TestCase) {
            let withdrawn = self.seat.withdraw();
            self.back_body(withdrawn);
        }

        #[rule]
        fn attach(&mut self, tc: TestCase) {
            let connection =
                tc.draw(gs::integers::<ConnectionId>().max_value(4));
            self.holding.push(connection);
            let (pending, before) = self.seat.attach(connection);
            self.back_body(pending);
            self.back_connection(before);
        }

        #[rule]
        fn leave(&mut self, _tc: TestCase) {
            let held = self.seat.connection();
            let left = self.seat.leave();
            assert_eq!(left, held, "it gives up the connection it holds");
            self.back_connection(left);
        }

        #[invariant]
        fn nothing_is_lost(&mut self, _tc: TestCase) {
            // What was handed in and not back is what the seat keeps.
            let keeps = usize::from(self.seat.waits_with_request());
            assert_eq!(
                self.kept.len(),
                keeps,
                "requests kept: {:?}",
                self.kept
            );
            let holds = usize::from(self.seat.connection().is_some());
            assert_eq!(
                self.holding.len(),
                holds,
                "connections: {:?}",
                self.holding
            );
            let states = usize::from(self.seat.is_unseated())
                + usize::from(self.seat.waiting_until().is_some())
                + holds;
            assert_eq!(states, 1, "{:?}", self.seat);
        }
    }

    #[hegel::test(test_cases = 300)]
    fn a_seat_loses_no_request_and_no_connection(tc: TestCase) {
        hegel::stateful::machine(Seating {
            seat: Seat::default(),
            next: 0,
            kept: Vec::new(),
            holding: Vec::new(),
        })
        .steps(30)
        .run(tc);
    }
}
