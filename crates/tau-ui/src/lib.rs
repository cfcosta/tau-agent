//! tau on the computer: the host that runs agents in the repositories
//! cloned from the forge or added in place, drives tau-ui-remote's
//! interface, and mirrors it to phones ([`mirror`]) for the plugin that
//! carries it (tau-phones).
//!
//! [`host::Host`] is the wiring for a real coding agent: a tokio runtime
//! beside GPUI, every plugin through the registry with its host half
//! ([`hosted::halves`]), models through what fills `tau.provider`, and
//! the run store. The `tau` app (`cargo run -p tau`) always starts it,
//! and opens setup to sign in while there is no model to run on
//! ([`host::Host::signed_in`]). With `--demo`, [`demo`]
//! replays a scripted session, its plugins answering through their real
//! host halves.

pub mod accounts;
pub mod host;
pub mod hosted;
pub mod mirror;
pub mod notify;

// The interface's modules, by the paths the host has always used.
use tau_ui_remote::{
    Workspace,
    catalog,
    models,
    phones,
    plugins,
    pull_request,
    push,
    route,
    setup,
    titles,
    update,
    view,
    workspace,
};
