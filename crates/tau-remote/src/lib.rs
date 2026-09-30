//! tau on a phone connects to a tau running on a computer (decision
//! 0013). This crate carries what crosses between the two, with no GPUI
//! in it:
//!
//! - [`pairing`]: the code the computer shows as a QR code;
//! - [`tls`]: the computer's self-signed certificate, trusted by its
//!   fingerprint;
//! - [`devices`]: the phones paired with the computer, and their tokens;
//! - [`wire`]: the frames on the socket, with the app's JSON inside;
//! - [`server`]: the computer's side: pairing, tokens, and messages
//!   numbered so a phone that comes back gets what it missed;
//! - [`client`]: the phone's side.

pub mod client;
pub mod devices;
pub mod pairing;
pub mod server;
pub mod time;
pub mod tls;
pub mod wire;

pub use client::{ClientError, Connection, Credentials, Sender};
pub use devices::Device;
pub use pairing::{
    Address,
    Fingerprint,
    PairingCode,
    PairingSecret,
    ParseError,
};
pub use server::{ConnId, Server, ServerConfig, ServerEvent, ServerHandle};
pub use wire::{Down, Refusal};

/// Fills `bytes` from the system's random source.
fn random(bytes: &mut [u8]) {
    getrandom::fill(bytes).expect("the system has a random source");
}
