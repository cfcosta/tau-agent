//! tau on a phone connects to a tau running on a computer (decision
//! 0013). This crate carries what crosses between the two, with no GPUI
//! in it; so far, the pairing code a phone scans.

pub mod pairing;

pub use pairing::{
    Address,
    Fingerprint,
    PairingCode,
    PairingSecret,
    ParseError,
};
