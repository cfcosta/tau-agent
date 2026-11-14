//! A run going on in this session ([`super::Host`]): working, then, once
//! its `RunEnd` went by, ending while its outcome is stored. Generic
//! over how it is steered and cancelled, which it keeps throughout.

use statig::{
    Outcome::{self, Handled, Transition},
    blocking::{IntoStateMachineExt as _, StateMachine},
    state_machine,
};

/// What happens to a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Input {
    /// Its `RunEnd` went by.
    End,
}

/// How to steer and cancel a run, kept whichever state it is in.
pub(crate) struct Runs<C> {
    control: C,
}

#[state_machine(initial = "State::live()")]
impl<C: 'static> Runs<C> {
    /// Working: it reads what it is steered with.
    #[state]
    fn live(event: &Input) -> Outcome<State> {
        match event {
            Input::End => Transition(State::ending()),
        }
    }

    /// It stops in a moment: it reads nothing more, and can still be
    /// cancelled.
    #[state]
    fn ending(event: &Input) -> Outcome<State> {
        match event {
            Input::End => Handled,
        }
    }
}

/// One run going on.
pub(crate) struct Run<C: 'static>(StateMachine<Runs<C>>);

impl<C: 'static> Run<C> {
    pub(crate) fn live(control: C) -> Self {
        Self(Runs { control }.state_machine())
    }

    /// Its end went by.
    pub(crate) fn end(&mut self) {
        self.0.handle(&Input::End);
    }

    /// How to cancel it.
    pub(crate) fn control(&self) -> &C {
        &self.0.inner().control
    }

    /// How to steer it, while it works.
    pub(crate) fn steerable(&self) -> Option<&C> {
        matches!(self.0.state(), State::Live {}).then(|| self.control())
    }

    pub(crate) fn is_ending(&self) -> bool {
        matches!(self.0.state(), State::Ending {})
    }
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;

    /// A run's ends and steers in any order (an end twice, a steer
    /// after the end), against what each must do.
    struct Going {
        run: Run<u32>,
        control: u32,
        ended: bool,
    }

    #[hegel::state_machine]
    impl Going {
        #[rule]
        fn end(&mut self, _tc: TestCase) {
            self.run.end();
            self.ended = true;
        }

        #[rule(weight = 2)]
        fn steer(&mut self, _tc: TestCase) {
            let steered = self.run.steerable().copied();
            assert_eq!(
                steered,
                (!self.ended).then_some(self.control),
                "only a working run reads a steer"
            );
        }

        #[invariant]
        fn the_run_is_the_model(&mut self, _tc: TestCase) {
            assert_eq!(
                *self.run.control(),
                self.control,
                "it can be cancelled"
            );
            assert_eq!(self.run.is_ending(), self.ended);
        }
    }

    #[hegel::test(test_cases = 100)]
    fn a_run_reads_steers_only_while_it_works(tc: TestCase) {
        let control = tc.draw(gs::integers::<u32>());
        hegel::stateful::machine(Going {
            run: Run::live(control),
            control,
            ended: false,
        })
        .steps(10)
        .run(tc);
    }
}
