//! What a phone sends the computer (decision 0013), and which of it a
//! phone may ask. The computer's side of phones is the tau-phones
//! plugin's.

use serde::{Deserialize, Serialize};

use crate::workspace::WorkspaceEvent;

/// What a phone sends up, inside the wire's frames.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PhoneUp {
    /// Something the person did on the phone, for the host.
    Event(WorkspaceEvent),
    /// The phone's name, as the computer lists it.
    Name(String),
}

impl WorkspaceEvent {
    /// Whether a phone may ask for this. Signing in and out, keys,
    /// pairing and paths on the computer stay with the computer.
    pub fn from_phone(&self) -> bool {
        !matches!(self, Self::Pair(_) | Self::OpenRepos(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Signing in and out, keys, pairing and paths on the computer stay
    /// with the computer; what a person does
    /// with chats and repositories a phone may ask for. Every event the
    /// computer keeps is listed, with some a phone may send.
    #[test]
    fn a_phone_asks_for_what_it_may_and_nothing_the_computer_keeps() {
        use crate::pairing::PairRequest;
        let run = || tau_agent::tool::RunId("r".into());
        let kept = [
            WorkspaceEvent::Pair(PairRequest::Scan),
            WorkspaceEvent::OpenRepos(vec!["tau".into()]),
        ];
        for event in &kept {
            assert!(!event.from_phone(), "{event:?} stays on the computer");
        }
        let asked = [
            WorkspaceEvent::Say {
                run: run(),
                text: "go".into(),
                model: Default::default(),
                images: Vec::new(),
            },
            WorkspaceEvent::Cancel { run: run() },
            WorkspaceEvent::CloseRun { run: run() },
            WorkspaceEvent::Land { run: run() },
            WorkspaceEvent::Push {
                repo: "tau".into(),
                fetch: true,
            },
        ];
        for event in &asked {
            assert!(event.from_phone(), "{event:?} is a phone's to ask");
        }
    }
}
