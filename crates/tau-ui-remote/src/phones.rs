//! Phones, as the computer shows them (decision 0013): whether they may
//! connect, where tau listens, the pairing code, and the phones paired.
//!
//! tau-ui's phone server keeps it and hands it to the
//! Workspace with [`Workspace::set_phones`](crate::Workspace::set_phones).

use std::time::Instant;

use serde::{Deserialize, Serialize};
use tau_remote::{Address, Device, Fingerprint, PairingCode};

use crate::workspace::WorkspaceEvent;

/// An address of this computer a phone could reach it at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalAddress {
    pub ip: String,
    /// Where it is, in words: `Tailscale`, `wlan0`.
    pub label: String,
}

/// A pairing code on screen, until it expires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShownCode {
    pub code: PairingCode,
    pub expires: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Phones {
    /// "Allow phones": off until the person turns it on.
    pub allowed: bool,
    pub addresses: Vec<LocalAddress>,
    /// The address picked to listen on, as an IP.
    pub listen: Option<String>,
    pub port: u16,
    /// Where tau listens, once it does.
    pub listening: Option<Address>,
    pub fingerprint: Option<Fingerprint>,
    pub code: Option<ShownCode>,
    pub paired: Vec<Device>,
    /// Why tau could not listen, or what last went wrong.
    pub error: Option<String>,
}

/// What the person asked of the Phones screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhonesRequest {
    Allow(bool),
    /// Listen on this IP.
    ListenOn(String),
    ShowCode,
    HideCode,
    /// Refuse this phone's token and close its connection.
    Revoke(String),
}

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
        !matches!(
            self,
            Self::Pair(_)
                | Self::Phones(_)
                | Self::OpenRepos(_)
                | Self::JevKey { .. }
                | Self::GitHubSignIn
                | Self::GitHubCheck
                | Self::GitHubToken { .. }
                | Self::GitHubSignOut
                | Self::ChatGptSignIn { .. }
                | Self::ChatGptCallback { .. }
                | Self::SwitchChatGpt { .. }
                | Self::SignOut
        )
    }
}

/// How a phone reaches `ip`, in words, for the paired screen and the
/// address list.
pub fn reach(ip: &str) -> Option<&'static str> {
    let octets: Vec<u8> =
        ip.split('.').filter_map(|part| part.parse().ok()).collect();
    match octets[..] {
        // Tailscale's CGNAT range, 100.64.0.0/10.
        [100, b, _, _] if (64..128).contains(&b) => Some("over Tailscale"),
        [10, ..] | [192, 168, ..] => Some("on your network"),
        [172, b, ..] if (16..32).contains(&b) => Some("on your network"),
        _ if ip.starts_with("fd7a:115c:a1e0") => Some("over Tailscale"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_say_how_they_are_reached() {
        assert_eq!(reach("100.84.12.7"), Some("over Tailscale"));
        assert_eq!(reach("192.168.1.20"), Some("on your network"));
        assert_eq!(reach("172.20.0.3"), Some("on your network"));
        assert_eq!(reach("8.8.8.8"), None);
    }

    /// Signing in and out, keys, pairing, the phones' own settings and
    /// paths on the computer stay with the computer; what a person does
    /// with chats and repositories a phone may ask for. Every event the
    /// computer keeps is listed, with some a phone may send.
    #[test]
    fn a_phone_asks_for_what_it_may_and_nothing_the_computer_keeps() {
        use crate::pairing::PairRequest;
        let run = || tau_agent::tool::RunId("r".into());
        let kept = [
            WorkspaceEvent::Pair(PairRequest::Scan),
            WorkspaceEvent::Phones(PhonesRequest::ShowCode),
            WorkspaceEvent::OpenRepos(vec!["tau".into()]),
            WorkspaceEvent::JevKey {
                key: Some("ts-x".into()),
            },
            WorkspaceEvent::GitHubSignIn,
            WorkspaceEvent::GitHubCheck,
            WorkspaceEvent::GitHubToken {
                token: "github_pat_x".into(),
            },
            WorkspaceEvent::GitHubSignOut,
            WorkspaceEvent::ChatGptSignIn {
                account: None,
                consent: false,
            },
            WorkspaceEvent::ChatGptCallback {
                url: "http://localhost/cb".into(),
            },
            WorkspaceEvent::SwitchChatGpt {
                account: "a".into(),
            },
            WorkspaceEvent::SignOut,
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
            WorkspaceEvent::ChatGptCancel,
        ];
        for event in &asked {
            assert!(event.from_phone(), "{event:?} is a phone's to ask");
        }
    }
}
