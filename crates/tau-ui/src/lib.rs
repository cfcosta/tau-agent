//! tau on the computer: the host that runs agents in the repositories
//! cloned from GitHub and drives tau-ui-remote's interface, and serves
//! the phones paired with it.
//!
//! [`host::Host`] is the wiring for a real coding agent: a tokio runtime
//! beside GPUI, `tau-tools` and `tau-compaction` as plugins, a ChatGPT
//! plan through Sign in with ChatGPT, and the run store. `cargo run -p
//! tau-ui` uses it when a ChatGPT account with plan use is signed in,
//! and opens onboarding to sign one in when none is
//! ([`host::onboard`]). With `--demo`, [`demo`] replays a scripted
//! session, its plugins answering through their real host halves.

pub mod accounts;
pub mod demo;
pub mod github;
pub mod host;
pub mod hosted;
pub mod metered;
pub mod phone_server;

// The interface's modules, by the paths the host has always used.
use tau_ui_remote::{
    Workspace,
    WorkspaceEvent,
    catalog,
    models,
    pairing,
    phones,
    plugins,
    pull_request,
    route,
    setup,
    titles,
    update,
    view,
    workspace,
};
