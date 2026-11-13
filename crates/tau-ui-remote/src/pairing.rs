//! What a phone shows while it pairs with the tau on a computer, and
//! when it cannot reach it (decision 0013).
//!
//! The phone's remote fills it in as it connects, through
//! [`Workspace::update_pairing`](crate::Workspace::update_pairing).

use statig::{
    Outcome::{self, Handled, Super, Transition},
    blocking::StateMachine,
    state_machine,
};
use tau_remote::{Address, Fingerprint, PairingSecret};

/// The screens of pairing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairStep {
    /// What pairing needs, on the computer and here.
    Welcome,
    /// The camera, reading the computer's pairing code.
    Scan,
    /// The address and the code, typed; the certificate compared by eye.
    Address,
    Paired,
    /// A paired computer did not answer.
    Unreachable,
}

impl PairStep {
    /// The screen's name in the phone's header.
    pub fn title(self) -> &'static str {
        match self {
            Self::Welcome => "Connect to tau",
            Self::Scan => "Scan pairing code",
            Self::Address => "Enter the address",
            Self::Paired => "Paired",
            Self::Unreachable => "Can't reach tau",
        }
    }
}

/// A computer the phone pairs with, or is paired with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Computer {
    /// Its name, as its owner reads it: `cfcosta-desk`.
    pub name: String,
    pub address: Address,
    pub fingerprint: Fingerprint,
    /// How the phone reaches it, in words: "over Tailscale", "on this
    /// network". None when the remote cannot tell.
    pub via: Option<String>,
}

/// How far a pairing has come.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Progress {
    #[default]
    Idle,
    /// Waiting for the camera to read a code.
    Scanning,
    /// Looking for tau at the address.
    Connecting {
        address: Address,
    },
    /// tau answered with this certificate, and nothing said which to
    /// expect: the person compares it with the computer's.
    Compare {
        address: Address,
        fingerprint: Fingerprint,
    },
    /// The certificate is the one expected; trading the code for a
    /// device token.
    Pairing {
        address: Address,
        fingerprint: Fingerprint,
    },
    Failed(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pairing {
    /// How far a pairing has come, or the phone's reach for the
    /// computer it paired with.
    flow: Flow,
    /// The computer this phone is paired with, once it is.
    pub computer: Option<Computer>,
    /// Times in a row the paired computer did not answer.
    pub tries: u32,
    /// Requests made here the computer has not taken yet.
    pub unsent: usize,
}

/// What the phone's remote learned while pairing or connecting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingUpdate {
    Progress(Progress),
    /// The phone reaches for the computer it paired with, at `address`.
    Reaching {
        address: Address,
    },
    /// Paired: the phone has a device token for this computer.
    Paired(Computer),
    /// The paired computer answered again.
    Connected(Computer),
    /// The paired computer did not answer after `tries` tries.
    Unreachable {
        computer: Computer,
        tries: u32,
    },
    /// This many requests wait for the computer to take them.
    Unsent(usize),
}

impl Pairing {
    /// Records `update`. Whether the screen may follow it: what the
    /// session hears of its computer moves no pairing under way.
    pub fn update(&mut self, update: PairingUpdate) -> bool {
        match update {
            PairingUpdate::Progress(progress) => {
                self.flow.tell(&Input::Progress(&progress));
                true
            }
            PairingUpdate::Reaching { address } => {
                self.flow.tell(&Input::Reach(&address));
                true
            }
            PairingUpdate::Paired(computer) => {
                self.flow.tell(&Input::Paired);
                self.computer = Some(computer);
                self.tries = 0;
                true
            }
            PairingUpdate::Connected(computer) => {
                self.computer = Some(computer);
                self.tries = 0;
                self.flow.tell(&Input::Heard).moves
            }
            PairingUpdate::Unsent(unsent) => {
                self.unsent = unsent;
                true
            }
            PairingUpdate::Unreachable { computer, tries } => {
                self.computer = Some(computer);
                self.tries = tries;
                self.flow.tell(&Input::Heard).moves
            }
        }
    }

    /// How far a pairing has come, as the screens show it.
    pub fn progress(&self) -> Progress {
        self.flow.0.state().progress()
    }

    /// The person left pairing's screens: whether a pairing under way
    /// stops.
    pub fn leave(&mut self) -> bool {
        self.flow.tell(&Input::Leave).cancel
    }

    /// Whether the phone is waiting on the remote.
    pub fn busy(&self) -> bool {
        matches!(
            self.progress(),
            Progress::Scanning
                | Progress::Connecting { .. }
                | Progress::Pairing { .. }
        )
    }
}

/// Where pairing is: the phone's reach for its own computer, kept apart
/// from a pairing under way with any computer, which only the person
/// and that pairing move.
#[derive(Clone, Default)]
struct Flow(StateMachine<Steps>);

impl Flow {
    fn tell(&mut self, input: &Input<'_>) -> Answer {
        let mut answer = Answer::default();
        self.0.handle_with_context(input, &mut answer);
        answer
    }
}

impl PartialEq for Flow {
    fn eq(&self, other: &Self) -> bool {
        self.0.state() == other.0.state()
    }
}

impl Eq for Flow {}

impl std::fmt::Debug for Flow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.state().fmt(f)
    }
}

/// What moves pairing on.
enum Input<'a> {
    /// The person or the pairing under way got this far.
    Progress(&'a Progress),
    /// The phone reaches for its paired computer at this address.
    Reach(&'a Address),
    /// The session heard from its computer, or that it does not answer.
    Heard,
    /// Paired.
    Paired,
    /// The person left pairing's screens.
    Leave,
}

/// What [`Steps`] answers an [`Input`] with.
#[derive(Default)]
struct Answer {
    /// The screen may follow what the session heard.
    moves: bool,
    /// The pairing under way is to stop.
    cancel: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Steps;

#[state_machine(
    initial = "State::idle()",
    state(derive(Debug, Clone, PartialEq, Eq))
)]
impl Steps {
    /// Nothing under way.
    #[state(superstate = "session")]
    fn idle(event: &Input<'_>) -> Outcome<State> {
        let _ = event;
        Super
    }

    /// Reaching for the paired computer.
    #[state(superstate = "session")]
    fn reaching(address: &mut Address, event: &Input<'_>) -> Outcome<State> {
        let _ = address;
        match event {
            Input::Leave => Transition(State::idle()),
            _ => Super,
        }
    }

    /// The session's own: what it hears moves the screen.
    #[superstate(superstate = "steps")]
    fn session(context: &mut Answer, event: &Input<'_>) -> Outcome<State> {
        match event {
            Input::Heard => {
                context.moves = true;
                Transition(State::idle())
            }
            _ => Super,
        }
    }

    /// Waiting for the camera to read a code.
    #[state(superstate = "pairing")]
    fn scanning(event: &Input<'_>) -> Outcome<State> {
        let _ = event;
        Super
    }

    /// Looking for tau at the address.
    #[state(superstate = "pairing")]
    fn probing(address: &mut Address, event: &Input<'_>) -> Outcome<State> {
        let _ = (address, event);
        Super
    }

    /// The person compares the certificate with the computer's.
    #[state(superstate = "pairing")]
    fn comparing(
        address: &mut Address,
        fingerprint: &mut Fingerprint,
        event: &Input<'_>,
    ) -> Outcome<State> {
        let _ = (address, fingerprint, event);
        Super
    }

    /// Trading the code for a device token.
    #[state(superstate = "pairing")]
    fn trading(
        address: &mut Address,
        fingerprint: &mut Fingerprint,
        event: &Input<'_>,
    ) -> Outcome<State> {
        let _ = (address, fingerprint, event);
        Super
    }

    /// A pairing under way: the person left, or the session heard
    /// something, which it keeps to itself.
    #[superstate(superstate = "steps")]
    fn pairing(context: &mut Answer, event: &Input<'_>) -> Outcome<State> {
        match event {
            Input::Leave => {
                context.cancel = true;
                Transition(State::idle())
            }
            Input::Heard | Input::Reach(_) => Handled,
            _ => Super,
        }
    }

    /// The pairing failed, so: shown until the person tries again or
    /// leaves pairing's screens.
    #[state(superstate = "steps")]
    fn failed(message: &mut String, event: &Input<'_>) -> Outcome<State> {
        let _ = message;
        match event {
            Input::Leave => Transition(State::idle()),
            Input::Heard | Input::Reach(_) => Handled,
            _ => Super,
        }
    }

    /// What any step does with where pairing went.
    #[superstate]
    fn steps(event: &Input<'_>) -> Outcome<State> {
        match event {
            Input::Progress(progress) => Transition(State::of(progress)),
            Input::Reach(address) => {
                Transition(State::reaching((*address).clone()))
            }
            Input::Paired => Transition(State::idle()),
            Input::Heard | Input::Leave => Handled,
        }
    }
}

impl State {
    /// The step a pairing at `progress` is at.
    fn of(progress: &Progress) -> Self {
        match progress {
            Progress::Idle => Self::idle(),
            Progress::Scanning => Self::scanning(),
            Progress::Connecting { address } => Self::probing(address.clone()),
            Progress::Compare {
                address,
                fingerprint,
            } => Self::comparing(address.clone(), *fingerprint),
            Progress::Pairing {
                address,
                fingerprint,
            } => Self::trading(address.clone(), *fingerprint),
            Progress::Failed(message) => Self::failed(message.clone()),
        }
    }

    /// The step as the screens show it.
    fn progress(&self) -> Progress {
        match self {
            Self::Idle {} => Progress::Idle,
            Self::Scanning {} => Progress::Scanning,
            Self::Reaching { address } | Self::Probing { address } => {
                Progress::Connecting {
                    address: address.clone(),
                }
            }
            Self::Comparing {
                address,
                fingerprint,
            } => Progress::Compare {
                address: address.clone(),
                fingerprint: *fingerprint,
            },
            Self::Trading {
                address,
                fingerprint,
            } => Progress::Pairing {
                address: address.clone(),
                fingerprint: *fingerprint,
            },
            Self::Failed { message } => Progress::Failed(message.clone()),
        }
    }
}

/// What the person asked of the phone's remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairRequest {
    /// Open the camera and pair with the code it reads.
    Scan,
    /// Pair with a typed address and code; the certificate is compared
    /// by eye before the code is sent.
    Typed {
        address: Address,
        secret: PairingSecret,
    },
    /// The person compared the certificate with the computer's, and it
    /// is the same.
    Trust(Fingerprint),
    /// The phone's name, as the computer lists it.
    Name(String),
    /// Connect to the paired computer again.
    Retry,
    /// Stop pairing.
    Cancel,
}

#[cfg(test)]
mod tests {
    use hegel::{
        TestCase,
        generators::{self as gs, Generator as _},
    };

    use super::*;

    fn computer() -> Computer {
        Computer {
            name: "cfcosta-desk".into(),
            address: "100.84.12.7:7443".parse().unwrap(),
            fingerprint: Fingerprint([7; 32]),
            via: Some("over Tailscale".into()),
        }
    }

    #[test]
    fn pairing_keeps_the_computer_and_resets_the_tries() {
        let mut pairing = Pairing::default();
        pairing.update(PairingUpdate::Reaching {
            address: computer().address,
        });
        assert!(pairing.busy());
        pairing.update(PairingUpdate::Unreachable {
            computer: computer(),
            tries: 3,
        });
        assert!(!pairing.busy());
        assert_eq!(pairing.tries, 3);
        pairing.update(PairingUpdate::Paired(computer()));
        assert_eq!(pairing.tries, 0);
        assert_eq!(pairing.computer, Some(computer()));
    }

    fn compare() -> Progress {
        Progress::Compare {
            address: "192.168.1.9:7443".parse().unwrap(),
            fingerprint: Fingerprint([9; 32]),
        }
    }

    /// The paired computer goes on not answering while the person
    /// compares another's certificate: the pairing stays, and the screen
    /// does not follow the session.
    #[test]
    fn the_session_moves_no_pairing_under_way() {
        let mut pairing = Pairing::default();
        pairing.update(PairingUpdate::Progress(compare()));
        let moves = pairing.update(PairingUpdate::Unreachable {
            computer: computer(),
            tries: 4,
        });
        assert!(!moves);
        assert_eq!(pairing.progress(), compare());
        assert!(!pairing.update(PairingUpdate::Connected(computer())));
        assert_eq!(pairing.progress(), compare());
    }

    /// A pairing that failed is left when the person leaves pairing's
    /// screens: the session's news moves the screen again after.
    #[test]
    fn a_failed_pairing_left_lets_the_session_move_the_screen() {
        let mut pairing = Pairing::default();
        pairing.update(PairingUpdate::Progress(Progress::Failed("no".into())));
        assert!(!pairing.leave(), "nothing under way to stop");
        assert_eq!(pairing.progress(), Progress::Idle);
        assert!(pairing.update(PairingUpdate::Connected(computer())));
    }

    /// The phone reaching for its own computer is the session's: what it
    /// hears ends the reach and moves the screen.
    #[test]
    fn a_reach_ends_with_what_the_session_hears() {
        let mut pairing = Pairing::default();
        pairing.update(PairingUpdate::Reaching {
            address: computer().address,
        });
        assert!(pairing.busy());
        assert!(pairing.update(PairingUpdate::Unreachable {
            computer: computer(),
            tries: 3,
        }));
        assert_eq!(pairing.progress(), Progress::Idle);
    }

    /// What may happen to pairing.
    #[derive(Debug, Clone)]
    enum Happens {
        /// The person or the pairing under way got this far.
        Step(Progress),
        Reach,
        Connected,
        Unreachable,
        Paired,
        Leave,
    }

    fn happens(tc: &TestCase) -> Happens {
        let address: Address = "192.168.1.9:7443".parse().unwrap();
        let fingerprint = Fingerprint([9; 32]);
        let progress = tc.draw(
            gs::sampled_from(vec![
                Progress::Idle,
                Progress::Scanning,
                Progress::Connecting {
                    address: address.clone(),
                },
                Progress::Compare {
                    address: address.clone(),
                    fingerprint,
                },
                Progress::Pairing {
                    address,
                    fingerprint,
                },
                Progress::Failed("it failed".into()),
            ])
            .print_as_debug(),
        );
        tc.draw(
            gs::sampled_from(vec![
                Happens::Step(progress),
                Happens::Reach,
                Happens::Connected,
                Happens::Unreachable,
                Happens::Paired,
                Happens::Leave,
            ])
            .print_as_debug(),
        )
    }

    /// Whether `progress` is a pairing under way with some computer.
    fn under_way(progress: &Progress) -> bool {
        !matches!(progress, Progress::Idle | Progress::Failed(_))
    }

    /// Whatever happens, in any order: what the session hears never moves
    /// a pairing under way, nor the screen with it; with none under way,
    /// it ends any reach and moves the screen; a pairing goes only where
    /// it is told, or ends when the person leaves or it pairs.
    #[hegel::test(test_cases = 1000)]
    fn the_session_and_a_pairing_keep_apart(tc: TestCase) {
        let mut pairing = Pairing::default();
        // What the model says the pairing shows.
        let mut shown = Progress::Idle;
        let mut reaching = false;
        let steps = tc.draw(gs::integers::<usize>().max_value(30));
        for _ in 0..steps {
            let happens = happens(&tc);
            let pairing_under_way = !reaching && under_way(&shown);
            match happens {
                Happens::Step(progress) => {
                    assert!(
                        pairing
                            .update(PairingUpdate::Progress(progress.clone()))
                    );
                    shown = progress;
                    reaching = false;
                }
                Happens::Reach => {
                    pairing.update(PairingUpdate::Reaching {
                        address: computer().address,
                    });
                    if !pairing_under_way
                        && !matches!(shown, Progress::Failed(_))
                    {
                        shown = Progress::Connecting {
                            address: computer().address,
                        };
                        reaching = true;
                    }
                }
                Happens::Connected | Happens::Unreachable => {
                    let update = match happens {
                        Happens::Connected => {
                            PairingUpdate::Connected(computer())
                        }
                        _ => PairingUpdate::Unreachable {
                            computer: computer(),
                            tries: 3,
                        },
                    };
                    let quiet = !pairing_under_way
                        && !matches!(shown, Progress::Failed(_));
                    assert_eq!(pairing.update(update), quiet, "{shown:?}");
                    if quiet {
                        shown = Progress::Idle;
                        reaching = false;
                    }
                }
                Happens::Paired => {
                    assert!(pairing.update(PairingUpdate::Paired(computer())));
                    shown = Progress::Idle;
                    reaching = false;
                }
                Happens::Leave => {
                    assert_eq!(pairing.leave(), pairing_under_way);
                    shown = Progress::Idle;
                    reaching = false;
                }
            }
            assert_eq!(pairing.progress(), shown);
        }
    }
}
