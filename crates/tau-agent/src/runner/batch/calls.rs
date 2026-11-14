//! What a running call's nested calls are doing ([`super::Scope`]):
//! open, they run as their tools allow, a sequential one alone, and
//! wait in order otherwise; once the call ends, what is left running is
//! cancelled, and the scope goes when those are done. What moves in and
//! out (a call to queue, the calls to start or fail) passes through the
//! machine's [`Hand`].

use std::collections::VecDeque;

use statig::{
    Outcome::{self, Handled, Transition},
    blocking::{IntoStateMachineExt as _, StateMachine},
    state_machine,
};

/// What happens to a scope's calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Input {
    /// A call, in the hand, is prepared: it waits its turn.
    Queue,
    /// The calls that may start now go into the hand.
    Start,
    /// One of its calls returned.
    Returned,
    /// The calling tool returned: the calls still waiting go into the
    /// hand, to fail.
    End,
}

/// A call waiting in a scope, with whether its tool runs alone.
#[derive(Debug)]
pub(super) struct Waiting<T> {
    pub(super) call: T,
    pub(super) sequential: bool,
}

/// What an input hands the machine, and what it hands back.
#[derive(Debug)]
pub(super) struct Hand<T> {
    pub(super) calls: VecDeque<Waiting<T>>,
}

impl<T> Default for Hand<T> {
    fn default() -> Self {
        Self {
            calls: VecDeque::new(),
        }
    }
}

pub(super) struct Scoped<T>(std::marker::PhantomData<T>);

impl<T> Default for Scoped<T> {
    fn default() -> Self {
        Self(std::marker::PhantomData)
    }
}

#[state_machine(initial = "State::open(0, false, VecDeque::new())")]
impl<T: 'static> Scoped<T> {
    /// The call runs: `running` of its calls run, alone when
    /// `exclusive`, and `waiting` wait their turn.
    #[state]
    fn open(
        context: &mut Hand<T>,
        running: &mut usize,
        exclusive: &mut bool,
        waiting: &mut VecDeque<Waiting<T>>,
        event: &Input,
    ) -> Outcome<State<T>> {
        match event {
            Input::Queue => {
                waiting.extend(context.calls.drain(..));
                Handled
            }
            Input::Start => {
                while let Some(front) = waiting.front() {
                    if *exclusive || (front.sequential && *running > 0) {
                        break;
                    }
                    let next = waiting.pop_front().expect("a front");
                    *running += 1;
                    *exclusive = next.sequential;
                    context.calls.push_back(next);
                }
                Handled
            }
            Input::Returned => {
                *running = running.saturating_sub(1);
                *exclusive = false;
                Handled
            }
            Input::End => {
                context.calls.extend(waiting.drain(..));
                Transition(State::ending(*running))
            }
        }
    }

    /// The call ended: `running` of its calls are left, cancelled. A
    /// call queued now is handed back, to fail.
    #[state]
    fn ending(running: &mut usize, event: &Input) -> Outcome<State<T>> {
        match event {
            Input::Returned => {
                *running = running.saturating_sub(1);
                Handled
            }
            Input::Queue | Input::Start | Input::End => Handled,
        }
    }
}

/// A running call's nested calls.
pub(super) struct Calls<T: 'static>(StateMachine<Scoped<T>>);

impl<T: 'static> Default for Calls<T> {
    fn default() -> Self {
        Self(Scoped::default().state_machine())
    }
}

impl<T: 'static> Calls<T> {
    fn take(&mut self, input: Input, mut hand: Hand<T>) -> Hand<T> {
        self.0.handle_with_context(&input, &mut hand);
        hand
    }

    /// Queues `call`; hands it back when the scope ended, to fail.
    pub(super) fn queue(&mut self, call: T, sequential: bool) -> Option<T> {
        let mut hand = Hand::default();
        hand.calls.push_back(Waiting { call, sequential });
        self.take(Input::Queue, hand)
            .calls
            .pop_front()
            .map(|waiting| waiting.call)
    }

    /// The calls to start now, in order.
    pub(super) fn start(&mut self) -> Vec<T> {
        self.take(Input::Start, Hand::default())
            .calls
            .into_iter()
            .map(|waiting| waiting.call)
            .collect()
    }

    /// One of its calls returned.
    pub(super) fn returned(&mut self) {
        self.take(Input::Returned, Hand::default());
    }

    /// The calling tool returned: the calls that waited, to fail.
    pub(super) fn end(&mut self) -> Vec<T> {
        self.take(Input::End, Hand::default())
            .calls
            .into_iter()
            .map(|waiting| waiting.call)
            .collect()
    }

    /// Whether it ended and has no call left: the scope can go.
    pub(super) fn is_done(&self) -> bool {
        matches!(self.0.state(), State::Ending { running: 0 })
    }

    /// Whether the calling tool still runs.
    #[cfg(test)]
    fn is_open(&self) -> bool {
        matches!(self.0.state(), State::Open { .. })
    }
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;

    /// A scope's calls under any inputs, in any order, against a model:
    /// calls queued, started, returning and the scope ending, with
    /// sequential calls among them.
    struct Scope {
        calls: Calls<u64>,
        next: u64,
        /// Queued and not started, in order, with whether each is
        /// sequential.
        waiting: VecDeque<(u64, bool)>,
        /// Started and not returned, with whether each is sequential.
        running: Vec<(u64, bool)>,
        ended: bool,
    }

    #[hegel::state_machine]
    impl Scope {
        #[rule(weight = 3)]
        fn queue(&mut self, tc: TestCase) {
            self.next += 1;
            let sequential = tc.draw(gs::booleans());
            let refused = self.calls.queue(self.next, sequential);
            if self.ended {
                assert_eq!(
                    refused,
                    Some(self.next),
                    "an ended scope takes none"
                );
            } else {
                assert_eq!(refused, None);
                self.waiting.push_back((self.next, sequential));
            }
        }

        #[rule(weight = 3)]
        fn start(&mut self, _tc: TestCase) {
            let started = self.calls.start();
            for call in started {
                let front = self.waiting.pop_front();
                assert_eq!(front.map(|(n, _)| n), Some(call), "out of order");
                self.running.push(front.unwrap());
            }
            if self.ended {
                return;
            }
            // It started all it could: the next waits for a reason.
            if let Some((_, sequential)) = self.waiting.front() {
                let alone = self.running.iter().any(|(_, s)| *s);
                assert!(
                    alone || (*sequential && !self.running.is_empty()),
                    "{:?} could start beside {:?}",
                    self.waiting.front(),
                    self.running
                );
            }
        }

        #[rule(weight = 2)]
        fn returned(&mut self, tc: TestCase) {
            if self.running.is_empty() {
                tc.reject();
            }
            let at = tc.draw(
                gs::integers::<usize>().max_value(self.running.len() - 1),
            );
            self.running.remove(at);
            self.calls.returned();
        }

        #[rule]
        fn end(&mut self, _tc: TestCase) {
            let failed = self.calls.end();
            if self.ended {
                assert!(failed.is_empty());
                return;
            }
            let waited: Vec<u64> =
                self.waiting.drain(..).map(|(n, _)| n).collect();
            assert_eq!(failed, waited, "the calls that waited fail, in order");
            self.ended = true;
        }

        #[invariant]
        fn the_scope_is_the_model(&mut self, _tc: TestCase) {
            if self.running.iter().any(|(_, sequential)| *sequential) {
                assert_eq!(
                    self.running.len(),
                    1,
                    "a sequential call runs alone"
                );
            }
            assert_eq!(self.calls.is_open(), !self.ended);
            assert_eq!(
                self.calls.is_done(),
                self.ended && self.running.is_empty()
            );
        }
    }

    #[hegel::test(test_cases = 300)]
    fn a_scope_runs_its_calls_as_their_tools_allow(tc: TestCase) {
        hegel::stateful::machine(Scope {
            calls: Calls::default(),
            next: 0,
            waiting: VecDeque::new(),
            running: Vec::new(),
            ended: false,
        })
        .steps(30)
        .run(tc);
    }
}
