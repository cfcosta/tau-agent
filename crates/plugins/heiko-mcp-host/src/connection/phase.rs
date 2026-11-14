//! What a connection does ([`super::Connection`]): settled, connecting,
//! or closed for good. Kept under the connection's lock with its
//! session, and its status is published under the same lock, so the two
//! always agree. Each input answers what the connection does next.

use statig::{
    Outcome::{self, Handled, Transition},
    blocking::{IntoStateMachineExt as _, StateMachine},
    state_machine,
};

/// What happens to a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Input {
    /// A connect is asked for: one with no live session.
    Begin,
    /// A sign-in or sign-out restarts it.
    Restart,
    /// The connect under way ended.
    Land,
    /// It is closed, with the plugin, or disabled.
    Close,
}

/// What the connection does after an input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Answer {
    /// Nothing more: it is where it was.
    #[default]
    Stay,
    /// A connect began: run it.
    Run,
    /// A restart came during a connect: it connects again once that
    /// one ends.
    Deferred,
    /// A restart found it settled: connect now.
    Connect,
    /// The connect landed: keep what it brought.
    Keep,
    /// The connect landed after a restart: drop what it brought and
    /// connect again.
    Again,
    /// It is closed: drop what a connect brought, and connect never.
    Closed,
}

#[derive(Default)]
pub(super) struct Phases;

#[state_machine(
    initial = "State::settled()",
    state(derive(Debug, Clone, Copy, PartialEq, Eq))
)]
impl Phases {
    /// No connect under way: it holds a session, or the status says why
    /// it holds none.
    #[state]
    fn settled(context: &mut Answer, event: &Input) -> Outcome<State> {
        match event {
            Input::Begin => {
                *context = Answer::Run;
                Transition(State::connecting(false))
            }
            Input::Restart => {
                *context = Answer::Connect;
                Handled
            }
            Input::Land => Handled,
            Input::Close => Transition(State::closed()),
        }
    }

    /// A connect is under way. `again`: a restart came meanwhile, so
    /// what it brings is dropped and it connects again.
    #[state]
    fn connecting(
        context: &mut Answer,
        again: &mut bool,
        event: &Input,
    ) -> Outcome<State> {
        match event {
            Input::Begin => Handled,
            Input::Restart => {
                *again = true;
                *context = Answer::Deferred;
                Handled
            }
            Input::Land => {
                *context = if *again { Answer::Again } else { Answer::Keep };
                Transition(State::settled())
            }
            Input::Close => Transition(State::closed()),
        }
    }

    /// Closed for good: nothing connects it again.
    #[state]
    fn closed(context: &mut Answer, event: &Input) -> Outcome<State> {
        let _ = event;
        *context = Answer::Closed;
        Handled
    }
}

/// A connection's phase.
pub(super) struct Phase(StateMachine<Phases>);

impl std::fmt::Debug for Phase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.state().fmt(f)
    }
}

impl Default for Phase {
    fn default() -> Self {
        Self(Phases.state_machine())
    }
}

impl Phase {
    /// A phase closed from the start: a disabled server's.
    pub(super) fn closed() -> Self {
        let mut phase = Self::default();
        phase.take(Input::Close);
        phase
    }

    pub(super) fn take(&mut self, input: Input) -> Answer {
        let mut answer = Answer::Stay;
        self.0.handle_with_context(&input, &mut answer);
        answer
    }

    pub(super) fn is_closed(&self) -> bool {
        matches!(self.0.state(), State::Closed {})
    }
}

#[cfg(test)]
mod tests {
    use hegel::TestCase;

    use super::*;

    /// A phase under any inputs, in any order (a land with no connect, a
    /// restart or begin after the close, several restarts during one
    /// connect), against what each must answer.
    #[derive(Default)]
    struct Connecting {
        phase: Phase,
        closed: bool,
        /// The connect under way, and whether a restart came during it.
        under_way: Option<bool>,
    }

    #[hegel::state_machine]
    impl Connecting {
        #[rule(weight = 2)]
        fn begin(&mut self, _tc: TestCase) {
            let answer = self.phase.take(Input::Begin);
            let want = if self.closed {
                Answer::Closed
            } else if self.under_way.is_some() {
                Answer::Stay
            } else {
                self.under_way = Some(false);
                Answer::Run
            };
            assert_eq!(answer, want, "one connect at a time, none once closed");
        }

        #[rule(weight = 2)]
        fn restart(&mut self, _tc: TestCase) {
            let answer = self.phase.take(Input::Restart);
            let want = match (self.closed, &mut self.under_way) {
                (true, _) => Answer::Closed,
                (false, Some(again)) => {
                    *again = true;
                    Answer::Deferred
                }
                (false, None) => Answer::Connect,
            };
            assert_eq!(answer, want);
        }

        #[rule(weight = 2)]
        fn land(&mut self, _tc: TestCase) {
            let answer = self.phase.take(Input::Land);
            let want = match (self.closed, self.under_way.take()) {
                (true, _) => Answer::Closed,
                // A restart during the connect is never lost.
                (false, Some(true)) => Answer::Again,
                (false, Some(false)) => Answer::Keep,
                (false, None) => Answer::Stay,
            };
            assert_eq!(answer, want);
        }

        #[rule]
        fn close(&mut self, _tc: TestCase) {
            self.phase.take(Input::Close);
            self.closed = true;
            self.under_way = None;
        }

        #[invariant]
        fn closed_is_final(&mut self, _tc: TestCase) {
            assert_eq!(self.phase.is_closed(), self.closed);
        }
    }

    #[hegel::test(test_cases = 300)]
    fn a_connection_phase_loses_no_restart(tc: TestCase) {
        hegel::stateful::machine(Connecting::default())
            .steps(30)
            .run(tc);
    }
}
