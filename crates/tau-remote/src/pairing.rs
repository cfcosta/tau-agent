//! The pairing code: what the computer shows as a QR code, and a phone
//! reads to find it, trust it and pair with it.
//!
//! It is a URI:
//!
//! ```text
//! tau-pair://100.84.12.7:7443?host=cfcosta-desk&fp=<64 hex digits>&code=K7QM-2XPA
//! ```
//!
//! - the address the computer listens on;
//! - `host`, the computer's name, to show before anything connects;
//! - `fp`, the SHA-256 fingerprint of the computer's certificate, the
//!   only thing a phone trusts it by;
//! - `code`, the one-time secret the phone trades for a device token.

use std::{fmt, str::FromStr};

pub const SCHEME: &str = "tau-pair";

/// Why a pairing code, or a part of one, did not read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("not a tau pairing code")]
    Scheme,
    #[error("the pairing code has no {0}")]
    Missing(&'static str),
    #[error("`{0}` is not an address, such as 100.84.12.7:7443")]
    Address(String),
    #[error("`{0}` is not a port")]
    Port(String),
    #[error("a certificate fingerprint is 64 hexadecimal digits")]
    Fingerprint,
    #[error("a pairing code is 8 letters and digits, such as K7QM-2XPA")]
    Secret,
    #[error("the pairing code is not escaped properly")]
    Escape,
}

/// Where a computer listens: a name or an IP address, and a port.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Address {
    /// A DNS name or an IP address; an IPv6 address without brackets.
    pub host: String,
    pub port: u16,
}

impl Address {
    /// The port tau listens on unless the person picks another.
    pub const DEFAULT_PORT: u16 = 7443;

    /// Reads an address as a person types it: `host:port`, `[v6]:port`,
    /// or a host alone, on the default port.
    pub fn typed(text: &str) -> Result<Self, ParseError> {
        let text = text.trim();
        let bare =
            text.parse::<std::net::Ipv6Addr>().is_ok() || !text.contains(':');
        if bare && !text.is_empty() {
            return Self::new(text, Self::DEFAULT_PORT, text);
        }
        text.parse()
    }

    fn new(host: &str, port: u16, text: &str) -> Result<Self, ParseError> {
        let host = host.trim_start_matches('[').trim_end_matches(']');
        let valid = !host.is_empty()
            && host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-.:_".contains(c));
        if !valid || port == 0 {
            return Err(ParseError::Address(text.to_owned()));
        }
        Ok(Self {
            host: host.to_owned(),
            port,
        })
    }
}

impl FromStr for Address {
    type Err = ParseError;

    fn from_str(text: &str) -> Result<Self, ParseError> {
        let (host, port) = text
            .rsplit_once(':')
            .ok_or_else(|| ParseError::Address(text.to_owned()))?;
        // An IPv6 address needs its brackets, or its last group would
        // read as the port.
        if host.contains(':') && !(host.starts_with('[') && host.ends_with(']'))
        {
            return Err(ParseError::Address(text.to_owned()));
        }
        let port = port
            .parse()
            .map_err(|_| ParseError::Port(port.to_owned()))?;
        Self::new(host, port, text)
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.host.contains(':') {
            write!(f, "[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{}:{}", self.host, self.port)
        }
    }
}

/// The SHA-256 of a computer's certificate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint(pub [u8; 32]);

impl Fingerprint {
    /// Lowercase hexadecimal, as the pairing code carries it.
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    pub fn from_hex(text: &str) -> Result<Self, ParseError> {
        let text = text.as_bytes();
        if text.len() != 64 {
            return Err(ParseError::Fingerprint);
        }
        let mut bytes = [0; 32];
        for (byte, pair) in bytes.iter_mut().zip(text.chunks(2)) {
            let pair = std::str::from_utf8(pair)
                .map_err(|_| ParseError::Fingerprint)?;
            *byte = u8::from_str_radix(pair, 16)
                .map_err(|_| ParseError::Fingerprint)?;
        }
        Ok(Self(bytes))
    }

    /// Enough to recognize it: `SHA-256 4F:A1:9C…E2`.
    pub fn short(&self) -> String {
        let [a, b, c, ..] = self.0;
        format!("SHA-256 {a:02X}:{b:02X}:{c:02X}…{:02X}", self.0[31])
    }

    /// All of it, in eight lines of four bytes, to compare by eye with
    /// what the computer shows.
    pub fn lines(&self) -> Vec<String> {
        self.0
            .chunks(4)
            .map(|line| {
                line.iter()
                    .map(|byte| format!("{byte:02X}"))
                    .collect::<Vec<_>>()
                    .join(":")
            })
            .collect()
    }
}

/// The one-time secret a phone trades for its device token: eight
/// letters and digits that do not read alike, shown as `K7QM-2XPA`.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct PairingSecret(String);

impl PairingSecret {
    /// No 0 or O, no 1 or I: it may be typed from a screen.
    pub const ALPHABET: &str = "23456789ABCDEFGHJKLMNPQRSTUVWXYZ";

    /// The secret as a person might type it: any case, with or without
    /// the dash and spaces.
    pub fn typed(text: &str) -> Result<Self, ParseError> {
        let secret: String = text
            .chars()
            .filter(|c| *c != '-' && !c.is_whitespace())
            .map(|c| c.to_ascii_uppercase())
            .collect();
        let valid = secret.len() == 8
            && secret.chars().all(|c| Self::ALPHABET.contains(c));
        if valid {
            Ok(Self(secret))
        } else {
            Err(ParseError::Secret)
        }
    }

    /// The eight characters, without the dash.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PairingSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", &self.0[..4], &self.0[4..])
    }
}

/// Never printed by accident.
impl fmt::Debug for PairingSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PairingSecret(…)")
    }
}

/// What a computer shows for a phone to pair with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingCode {
    pub address: Address,
    /// The computer's name, as its owner reads it.
    pub host: String,
    pub fingerprint: Fingerprint,
    pub secret: PairingSecret,
}

impl FromStr for PairingCode {
    type Err = ParseError;

    fn from_str(text: &str) -> Result<Self, ParseError> {
        let rest = text
            .trim()
            .strip_prefix(SCHEME)
            .and_then(|rest| rest.strip_prefix("://"))
            .ok_or(ParseError::Scheme)?;
        let (address, query) = rest.split_once('?').unwrap_or((rest, ""));
        let address = address.trim_end_matches('/').parse()?;
        let (mut host, mut fingerprint, mut secret) = (None, None, None);
        for pair in query.split('&').filter(|pair| !pair.is_empty()) {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let value = unescape(value)?;
            match key {
                "host" => host = Some(value),
                "fp" => fingerprint = Some(Fingerprint::from_hex(&value)?),
                "code" => secret = Some(PairingSecret::typed(&value)?),
                // Later versions may add fields.
                _ => {}
            }
        }
        Ok(Self {
            address,
            host: host
                .filter(|host| !host.is_empty())
                .ok_or(ParseError::Missing("computer name"))?,
            fingerprint: fingerprint
                .ok_or(ParseError::Missing("certificate fingerprint"))?,
            secret: secret.ok_or(ParseError::Missing("pairing secret"))?,
        })
    }
}

impl fmt::Display for PairingCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{SCHEME}://{}?host={}&fp={}&code={}",
            self.address,
            escape(&self.host),
            self.fingerprint.to_hex(),
            self.secret,
        )
    }
}

/// Percent-escapes all but unreserved characters (RFC 3986).
fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            escaped.push(byte as char);
        } else {
            escaped.push_str(&format!("%{byte:02X}"));
        }
    }
    escaped
}

fn unescape(text: &str) -> Result<String, ParseError> {
    let mut bytes = Vec::with_capacity(text.len());
    let mut rest = text.as_bytes();
    while let Some((&byte, tail)) = rest.split_first() {
        if byte == b'%' {
            let hex = tail.get(..2).ok_or(ParseError::Escape)?;
            let hex =
                std::str::from_utf8(hex).map_err(|_| ParseError::Escape)?;
            bytes.push(
                u8::from_str_radix(hex, 16).map_err(|_| ParseError::Escape)?,
            );
            rest = &tail[2..];
        } else {
            bytes.push(if byte == b'+' { b' ' } else { byte });
            rest = tail;
        }
    }
    String::from_utf8(bytes).map_err(|_| ParseError::Escape)
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;

    fn fingerprint() -> Fingerprint {
        let mut bytes = [0; 32];
        bytes[..3].copy_from_slice(&[0x4f, 0xa1, 0x9c]);
        bytes[31] = 0xe2;
        Fingerprint(bytes)
    }

    fn code() -> PairingCode {
        PairingCode {
            address: "100.84.12.7:7443".parse().unwrap(),
            host: "cfcosta-desk".into(),
            fingerprint: fingerprint(),
            secret: PairingSecret::typed("K7QM-2XPA").unwrap(),
        }
    }

    #[test]
    fn reads_what_the_computer_shows() {
        let text = format!(
            "tau-pair://100.84.12.7:7443?host=cfcosta-desk&fp={}&code=K7QM-2XPA",
            fingerprint().to_hex()
        );
        assert_eq!(text.parse(), Ok(code()));
        assert_eq!(code().to_string(), text);
    }

    #[test]
    fn a_fingerprint_reads_short_and_whole() {
        assert_eq!(fingerprint().short(), "SHA-256 4F:A1:9C…E2");
        let lines = fingerprint().lines();
        assert_eq!(lines.len(), 8);
        assert_eq!(lines[0], "4F:A1:9C:00");
        assert_eq!(lines[7], "00:00:00:E2");
    }

    #[test]
    fn a_secret_is_read_as_typed() {
        let secret = PairingSecret::typed(" k7qm 2xpa ").unwrap();
        assert_eq!(secret.to_string(), "K7QM-2XPA");
        assert_eq!(secret.as_str(), "K7QM2XPA");
        assert_eq!(format!("{secret:?}"), "PairingSecret(…)");
        // O and I are not in the alphabet; neither is a short code.
        assert_eq!(PairingSecret::typed("K7QM-2XPO"), Err(ParseError::Secret));
        assert_eq!(PairingSecret::typed("K7QM"), Err(ParseError::Secret));
    }

    #[test]
    fn addresses_read_as_typed() {
        let typed = |text| Address::typed(text).map(|a| a.to_string());
        assert_eq!(typed("100.84.12.7"), Ok("100.84.12.7:7443".into()));
        assert_eq!(
            typed(" desk.tail.ts.net:9000 "),
            Ok("desk.tail.ts.net:9000".into())
        );
        assert_eq!(typed("fd7a::1"), Ok("[fd7a::1]:7443".into()));
        assert_eq!(typed("[fd7a::1]:80"), Ok("[fd7a::1]:80".into()));
        assert_eq!(typed("desk:http"), Err(ParseError::Port("http".into())));
        assert!(typed("").is_err());
        assert!(typed("desk:0").is_err());
        assert!(typed("a b:1").is_err());
    }

    #[test]
    fn a_code_without_its_parts_is_refused() {
        let hex = fingerprint().to_hex();
        let parse = |text: &str| text.parse::<PairingCode>();
        assert_eq!(parse("https://desk:1?host=d"), Err(ParseError::Scheme));
        assert_eq!(
            parse(&format!("tau-pair://desk:1?fp={hex}&code=K7QM2XPA")),
            Err(ParseError::Missing("computer name"))
        );
        assert_eq!(
            parse("tau-pair://desk:1?host=d&code=K7QM2XPA"),
            Err(ParseError::Missing("certificate fingerprint"))
        );
        assert_eq!(
            parse(&format!("tau-pair://desk:1?host=d&fp={hex}")),
            Err(ParseError::Missing("pairing secret"))
        );
        assert_eq!(
            parse("tau-pair://desk:1?host=d&fp=4fa1&code=K7QM2XPA"),
            Err(ParseError::Fingerprint)
        );
    }

    #[hegel::test]
    fn codes_round_trip(tc: TestCase) {
        let bytes: Vec<u8> =
            tc.draw(gs::vecs(gs::integers::<u8>()).min_size(32).max_size(32));
        let alphabet: Vec<char> = PairingSecret::ALPHABET.chars().collect();
        let secret: String = tc
            .draw(gs::vecs(gs::sampled_from(alphabet)).min_size(8).max_size(8))
            .into_iter()
            .collect();
        let v6 = tc.draw(gs::booleans());
        let address = Address {
            host: if v6 {
                "fd7a:115c::1".into()
            } else {
                "desk.local".into()
            },
            port: tc.draw(gs::integers::<u16>().min_value(1)),
        };
        let code = PairingCode {
            address,
            host: tc.draw(gs::text().min_size(1)),
            fingerprint: Fingerprint(bytes.try_into().unwrap()),
            secret: PairingSecret::typed(&secret).unwrap(),
        };
        assert_eq!(code.to_string().parse(), Ok(code));
    }
}
