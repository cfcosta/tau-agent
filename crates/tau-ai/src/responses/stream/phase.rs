//! Where a response is ([`super::StreamProcessor`]): `Start` goes out
//! with the first known frame, and nothing goes out after the terminal
//! event.

use statig::{
    Outcome::{self, Handled, Transition},
    blocking::{IntoStateMachineExt as _, StateMachine},
    state_machine,
};

/// What happens to the response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// A frame tau-ai knows came: the first starts the stream.
    Frame,
    /// A function call was opened: a `stop` is then `ToolUse`.
    ToolCall,
    /// The terminal event went out, or the stream closed without one.
    Terminal,
}

#[derive(Default)]
pub(crate) struct Phases;

#[state_machine(
    initial = "State::waiting()",
    state(derive(Debug, Clone, Copy, PartialEq, Eq))
)]
impl Phases {
    /// Nothing known came yet. Whatever comes starts the stream: the
    /// machine says so in `context`, and `Start` goes out first.
    #[state]
    fn waiting(context: &mut bool, event: &Step) -> Outcome<State> {
        *context = true;
        match event {
            Step::Frame => Transition(State::streaming(false)),
            Step::ToolCall => Transition(State::streaming(true)),
            Step::Terminal => Transition(State::finished()),
        }
    }

    /// Started. `tool_call`: a function call was opened.
    #[state]
    fn streaming(tool_call: &mut bool, event: &Step) -> Outcome<State> {
        match event {
            Step::Frame => Handled,
            Step::ToolCall => {
                *tool_call = true;
                Handled
            }
            Step::Terminal => Transition(State::finished()),
        }
    }

    /// The terminal event went out: nothing after it counts.
    #[state]
    fn finished() -> Outcome<State> {
        Handled
    }
}

/// A response's phase.
pub(crate) struct Phase(StateMachine<Phases>);

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
    /// Takes `step`. True when the stream starts with it: `Start` is to
    /// go out before anything it brings.
    pub(crate) fn take(&mut self, step: Step) -> bool {
        let mut starts = false;
        self.0.handle_with_context(&step, &mut starts);
        starts
    }

    pub(crate) fn is_finished(&self) -> bool {
        matches!(self.0.state(), State::Finished {})
    }

    /// Whether a function call was opened, while it streams.
    pub(crate) fn tool_call(&self) -> bool {
        matches!(self.0.state(), State::Streaming { tool_call: true })
    }
}
