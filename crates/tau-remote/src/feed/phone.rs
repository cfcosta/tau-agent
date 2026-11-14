//! Where a phone's connection is in the [`super::Feed`]: waiting for its
//! snapshot, which holds the messages until then, or taking messages.

use statig::{
    Outcome::{self, Handled, Transition},
    blocking::{IntoStateMachineExt as _, StateMachine},
    state_machine,
};

/// What happens to a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Input {
    /// It had what it missed replayed.
    Replayed,
    /// It had its snapshot.
    Snapshot,
}

#[derive(Default)]
pub(super) struct Phones;

#[state_machine(
    initial = "State::waiting()",
    state(derive(Debug, Clone, Copy, PartialEq, Eq))
)]
impl Phones {
    /// It joined with nothing the feed could replay: it takes no
    /// message until its snapshot, which holds them.
    #[state]
    fn waiting(event: &Input) -> Outcome<State> {
        match event {
            Input::Replayed | Input::Snapshot => Transition(State::taking()),
        }
    }

    /// It takes new messages. A snapshot again resets what it has.
    #[state]
    fn taking() -> Outcome<State> {
        Handled
    }
}

/// One connection's place in the feed.
pub(super) struct Phone(StateMachine<Phones>);

impl Phone {
    /// A connection that joined: replayed when the feed had what it
    /// missed, waiting for its snapshot otherwise.
    pub(super) fn joined(replayed: bool) -> Self {
        let mut phone = Self(Phones.state_machine());
        if replayed {
            phone.0.handle(&Input::Replayed);
        }
        phone
    }

    pub(super) fn snapshot(&mut self) {
        self.0.handle(&Input::Snapshot);
    }

    pub(super) fn is_taking(&self) -> bool {
        matches!(self.0.state(), State::Taking {})
    }
}

impl Clone for Phone {
    fn clone(&self) -> Self {
        Self::joined(self.is_taking())
    }
}

impl std::fmt::Debug for Phone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.state().fmt(f)
    }
}
