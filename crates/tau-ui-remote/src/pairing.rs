//! What a phone shows while it pairs with the tau on a computer, and
//! when it cannot reach it (decision 0013).
//!
//! The phone's remote fills it in as it connects, through
//! [`Workspace::update_pairing`](crate::Workspace::update_pairing).

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
    pub progress: Progress,
    /// The computer this phone is paired with, once it is.
    pub computer: Option<Computer>,
    /// Times in a row the paired computer did not answer.
    pub tries: u32,
}

/// What the phone's remote learned while pairing or connecting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingUpdate {
    Progress(Progress),
    /// Paired: the phone has a device token for this computer.
    Paired(Computer),
    /// The paired computer answered again.
    Connected(Computer),
    /// The paired computer did not answer after `tries` tries.
    Unreachable {
        computer: Computer,
        tries: u32,
    },
}

impl Pairing {
    pub fn update(&mut self, update: PairingUpdate) {
        match update {
            PairingUpdate::Progress(progress) => self.progress = progress,
            PairingUpdate::Paired(computer)
            | PairingUpdate::Connected(computer) => {
                self.progress = Progress::Idle;
                self.computer = Some(computer);
                self.tries = 0;
            }
            PairingUpdate::Unreachable { computer, tries } => {
                self.progress = Progress::Idle;
                self.computer = Some(computer);
                self.tries = tries;
            }
        }
    }

    /// Whether the phone is waiting on the remote.
    pub fn busy(&self) -> bool {
        matches!(
            self.progress,
            Progress::Scanning
                | Progress::Connecting { .. }
                | Progress::Pairing { .. }
        )
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
        pairing.update(PairingUpdate::Progress(Progress::Connecting {
            address: computer().address,
        }));
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
}
