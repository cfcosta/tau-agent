//! A scope's selection ([`super::State`]): none, being dragged from
//! where the press was, or settled once the press ends. Generic over its
//! ends, a text and an offset in it.

use statig::{
    Outcome::{self, Handled, Super, Transition},
    blocking::{IntoStateMachineExt as _, StateMachine},
    state_machine,
};

/// What the pointer does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Input<E> {
    /// It presses on `E`: a selection starts there and follows it.
    Press(E),
    /// It picks from one end to the other, a word or a line.
    Pick(E, E),
    /// It presses on nothing selectable.
    Clear,
    /// It moves, pressed, nearest `E`; `None` over a scope that draws
    /// nothing.
    Move(Option<E>),
    /// The press ends.
    Lift,
}

pub(super) struct Selecting<E>(std::marker::PhantomData<E>);

impl<E> Default for Selecting<E> {
    fn default() -> Self {
        Self(std::marker::PhantomData)
    }
}

#[state_machine(
    initial = "State::none()",
    state(derive(Debug, Clone, Copy, PartialEq))
)]
impl<E: Copy + PartialEq + 'static> Selecting<E> {
    #[state(superstate = "pointer")]
    fn none(event: &Input<E>) -> Outcome<State<E>> {
        match event {
            Input::Move(_) | Input::Lift => Handled,
            _ => Super,
        }
    }

    /// The press goes on: moving the pointer moves `head`.
    #[state(superstate = "pointer")]
    fn dragging(
        anchor: &mut E,
        head: &mut E,
        event: &Input<E>,
    ) -> Outcome<State<E>> {
        match event {
            Input::Move(Some(to)) => {
                *head = *to;
                Handled
            }
            // Over a scope that draws nothing, the head stays.
            Input::Move(None) => Handled,
            Input::Lift => Transition(State::settled(*anchor, *head)),
            _ => Super,
        }
    }

    /// The press ended: the selection stays until the next.
    #[state(superstate = "pointer")]
    fn settled(
        anchor: &mut E,
        head: &mut E,
        event: &Input<E>,
    ) -> Outcome<State<E>> {
        // Its ends are its state's: statig reads them off this handler.
        let _ = (anchor, head);
        match event {
            Input::Move(_) | Input::Lift => Handled,
            _ => Super,
        }
    }

    /// A press starts again, wherever the selection was.
    #[superstate]
    fn pointer(event: &Input<E>) -> Outcome<State<E>> {
        match event {
            Input::Press(at) => Transition(State::dragging(*at, *at)),
            Input::Pick(anchor, head) => {
                Transition(State::settled(*anchor, *head))
            }
            Input::Clear => Transition(State::none()),
            Input::Move(_) | Input::Lift => Handled,
        }
    }
}

/// A scope's selection.
pub(super) struct Selection<E: Copy + PartialEq + 'static>(
    StateMachine<Selecting<E>>,
);

impl<E: Copy + PartialEq + 'static> Default for Selection<E> {
    fn default() -> Self {
        Self(Selecting::default().state_machine())
    }
}

impl<E: Copy + PartialEq + 'static> Selection<E> {
    pub(super) fn take(&mut self, input: Input<E>) {
        self.0.handle(&input);
    }

    /// Its ends, anchor first, when it has any.
    pub(super) fn ends(&self) -> Option<(E, E)> {
        match self.0.state() {
            State::None {} => None,
            State::Dragging { anchor, head }
            | State::Settled { anchor, head } => Some((*anchor, *head)),
        }
    }

    /// Whether the press goes on.
    pub(super) fn is_dragging(&self) -> bool {
        matches!(self.0.state(), State::Dragging { .. })
    }
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;

    type End = (u8, u8);

    /// A selection under any pointer inputs, in any order (moves with
    /// no press, a lift twice, a press or pick while dragging), against
    /// what each must do.
    struct Pointer {
        selection: Selection<End>,
        /// The ends, and whether the press goes on.
        model: Option<(End, End, bool)>,
    }

    fn end(tc: &TestCase) -> End {
        (
            tc.draw(gs::integers::<u8>().max_value(3)),
            tc.draw(gs::integers::<u8>().max_value(9)),
        )
    }

    #[hegel::state_machine]
    impl Pointer {
        #[rule]
        fn press(&mut self, tc: TestCase) {
            let at = end(&tc);
            self.selection.take(Input::Press(at));
            self.model = Some((at, at, true));
        }

        #[rule]
        fn pick(&mut self, tc: TestCase) {
            let (from, to) = (end(&tc), end(&tc));
            self.selection.take(Input::Pick(from, to));
            self.model = Some((from, to, false));
        }

        #[rule]
        fn clear(&mut self, _tc: TestCase) {
            self.selection.take(Input::Clear);
            self.model = None;
        }

        #[rule(weight = 3)]
        fn moves(&mut self, tc: TestCase) {
            let to = tc.draw(gs::booleans()).then(|| end(&tc));
            let anchor = self.selection.ends().map(|(anchor, _)| anchor);
            self.selection.take(Input::Move(to));
            assert_eq!(
                self.selection.ends().map(|(anchor, _)| anchor),
                anchor,
                "a move moved the anchor"
            );
            if let (Some((_, head, true)), Some(to)) = (&mut self.model, to) {
                *head = to;
            }
        }

        #[rule]
        fn lift(&mut self, _tc: TestCase) {
            let ends = self.selection.ends();
            self.selection.take(Input::Lift);
            assert_eq!(self.selection.ends(), ends, "a lift changed the ends");
            if let Some((_, _, dragging)) = &mut self.model {
                *dragging = false;
            }
        }

        #[invariant]
        fn the_selection_is_the_model(&mut self, _tc: TestCase) {
            assert_eq!(
                self.selection.ends(),
                self.model.map(|(anchor, head, _)| (anchor, head))
            );
            assert_eq!(
                self.selection.is_dragging(),
                self.model.is_some_and(|(_, _, dragging)| dragging)
            );
        }
    }

    #[hegel::test(test_cases = 300)]
    fn a_selection_follows_the_pointer(tc: TestCase) {
        hegel::stateful::machine(Pointer {
            selection: Selection::default(),
            model: None,
        })
        .steps(30)
        .run(tc);
    }
}
