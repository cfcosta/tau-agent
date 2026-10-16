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
/// address list: Tailscale's ranges, or a private IPv4 range on the
/// local network. Anything that is not an IP address (a host name, even
/// one whose labels are numbers) says nothing.
pub fn reach(ip: &str) -> Option<&'static str> {
    let ip = ip.trim_start_matches('[').trim_end_matches(']');
    match ip.parse::<std::net::IpAddr>().ok()? {
        std::net::IpAddr::V4(v4) => match v4.octets() {
            // Tailscale's CGNAT range, 100.64.0.0/10.
            [100, 64..=127, ..] => Some("over Tailscale"),
            [10, ..] | [172, 16..=31, ..] | [192, 168, ..] => {
                Some("on your network")
            }
            _ => None,
        },
        // Tailscale's IPv6 range, fd7a:115c:a1e0::/48.
        std::net::IpAddr::V6(v6) => (v6.segments()[..3]
            == [0xfd7a, 0x115c, 0xa1e0])
        .then_some("over Tailscale"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An address as a phone's connection names it: dotted parts, some
    /// of them past a byte or not numbers, or an IPv6 address written in
    /// either case.
    #[derive(Debug, Clone, hegel::PrettyPrintable)]
    enum Written {
        Dotted(Vec<String>),
        V6(std::net::Ipv6Addr, bool),
    }

    #[hegel::composite]
    fn written_address(tc: &hegel::TestCase) -> Written {
        use hegel::generators as gs;
        if tc.draw(gs::weighted_booleans(0.2)) {
            let first: u16 = tc.draw(gs::sampled_from(vec![
                0xfd7a, 0xfd7a, 0xfd7b, 0xfc00, 0x2001,
            ]));
            let second: u16 = tc.draw(gs::sampled_from(vec![0x115c, 0x115d]));
            let third: u16 = tc.draw(gs::sampled_from(vec![0xa1e0, 0xa1e1]));
            let rest: u16 = tc.draw(gs::integers::<u16>());
            let address =
                std::net::Ipv6Addr::new(first, second, third, 0, 0, 0, 0, rest);
            return Written::V6(address, tc.draw(gs::booleans()));
        }
        // Mostly a range's first octet, or one either side of it, with a
        // second at one of that range's edges or just past one; sometimes
        // any two.
        let (base, edges): (u16, Vec<u16>) =
            match tc.draw(gs::integers::<u8>().max_value(4)) {
                0 => (100, vec![0, 63, 64, 127, 128, 255]),
                1 => (172, vec![0, 15, 16, 31, 32, 255]),
                2 => (192, vec![0, 167, 168, 169, 255]),
                3 => (10, vec![0, 255]),
                _ => (
                    tc.draw(gs::integers::<u16>().max_value(255)),
                    vec![tc.draw(gs::integers::<u16>().max_value(255))],
                ),
            };
        let first = match tc.draw(gs::integers::<u8>().max_value(7)) {
            0 => base.saturating_sub(1),
            1 => base + 1,
            _ => base,
        };
        let second = tc.draw(hegel::one_of!(
            gs::sampled_from(edges),
            gs::integers::<u16>().max_value(255),
        ));
        let rest: Vec<u16> = tc.draw(
            gs::vecs(gs::integers::<u16>().max_value(255))
                .min_size(2)
                .max_size(2),
        );
        let mut parts: Vec<String> = [first, second, rest[0], rest[1]]
            .iter()
            .map(u16::to_string)
            .collect();
        // Sometimes not an address: fewer or more parts, a part past a
        // byte, or a word, as a host name has.
        match tc.draw(gs::integers::<u8>().max_value(11)) {
            0 => parts.truncate(
                tc.draw(gs::integers::<usize>().min_value(1).max_value(3)),
            ),
            1 => parts.push(
                tc.draw(gs::integers::<u16>().max_value(255)).to_string(),
            ),
            2 => {
                let at = tc.draw(gs::integers::<usize>().max_value(3));
                parts[at] = tc
                    .draw(gs::integers::<u16>().min_value(256).max_value(999))
                    .to_string();
            }
            3 => {
                let at =
                    tc.draw(gs::integers::<usize>().min_value(1).max_value(3));
                parts.truncate(at);
                parts.push(tc.draw(gs::sampled_from(vec![
                    "lan".to_owned(),
                    "example".to_owned(),
                    "local".to_owned(),
                ])));
            }
            _ => {}
        }
        Written::Dotted(parts)
    }

    /// What dotted `parts` say of how a phone reaches them: an IPv4
    /// address in Tailscale's 100.64.0.0/10, or in 10.0.0.0/8,
    /// 172.16.0.0/12 or 192.168.0.0/16 on the local network; anything
    /// but four numbers of a byte each says nothing. The ranges are masks
    /// over the address as a number, apart from how `reach` reads it.
    fn by_mask(parts: &[String]) -> Option<&'static str> {
        let numbers: Vec<u32> = parts
            .iter()
            .map_while(|part| part.parse::<u32>().ok())
            .filter(|n| *n <= 255)
            .collect();
        if parts.len() != 4 || numbers.len() != 4 {
            return None;
        }
        let n = numbers.iter().fold(0u32, |n, octet| n << 8 | octet);
        let within =
            |base: u32, bits: u32| n >> (32 - bits) == base >> (32 - bits);
        if within(0x6440_0000, 10) {
            Some("over Tailscale")
        } else if within(0x0a00_0000, 8)
            || within(0xac10_0000, 12)
            || within(0xc0a8_0000, 16)
        {
            Some("on your network")
        } else {
            None
        }
    }

    /// Which range an IPv4 address is in rests on its first two octets
    /// alone: every pair of them reads as the masks say.
    #[test]
    fn every_first_two_octets_read_as_their_range() {
        for first in 0..=255u32 {
            for second in 0..=255u32 {
                let parts: Vec<String> = [first, second, 7, 200]
                    .iter()
                    .map(u32::to_string)
                    .collect();
                let text = parts.join(".");
                assert_eq!(reach(&text), by_mask(&parts), "{text}");
            }
        }
    }

    /// An address says how a phone reaches it exactly when it is one: an
    /// IPv4 address in one of the ranges, as [`by_mask`] reads them, or an
    /// IPv6 address in Tailscale's fd7a:115c:a1e0::/48, whatever its
    /// case. A host name whose labels are numbers in part
    /// ("10.example.com", "192.168.lan"), or fewer or more than four
    /// numbers, or one past a byte, says nothing.
    #[hegel::test(test_cases = 500)]
    #[hegel::explicit_test_case(written = Written::Dotted(vec!["10".into(), "example".into(), "com".into()]))]
    #[hegel::explicit_test_case(written = Written::Dotted(vec!["10".into()]))]
    #[hegel::explicit_test_case(written = Written::Dotted(vec!["192".into(), "168".into(), "lan".into()]))]
    fn an_address_says_how_it_is_reached_by_its_range(tc: hegel::TestCase) {
        let written = tc.draw(written_address());
        let (text, expected) = match &written {
            Written::V6(address, upper) => {
                let text = address.to_string();
                let text = if *upper { text.to_uppercase() } else { text };
                let tailscale =
                    address.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0];
                (text, tailscale.then_some("over Tailscale"))
            }
            Written::Dotted(parts) => (parts.join("."), by_mask(parts)),
        };
        assert_eq!(reach(&text), expected, "{text}");
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
