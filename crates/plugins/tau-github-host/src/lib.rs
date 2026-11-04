//! GitHub through tau's GitHub App, as where tau's repositories are
//! hosted: tau-github's host half (ADR 0030, 0045). It signs in with a
//! device code (or a personal access token), keeps the sign-in in tau's
//! config directory, lists the repositories the app reaches, opens pull
//! requests and reads their checks; [`GitHub`] is the `tau.forge` slot's
//! filler the host clones and pushes through.
//!
//! A device sign-in needs no secret, so none ships with tau. Tokens from
//! the app expire after eight hours, and renewing one needs the app's
//! secret, so an expired token counts as signed out: sign in again.

pub mod api;
mod forge;
mod half;
mod token;
pub mod url;

pub use api::{Api, CLIENT_ID, DeviceStart, Endpoints, PER_PAGE, Poll};
pub use forge::GitHub;
pub use half::GitHubHost;
pub use token::Token;
