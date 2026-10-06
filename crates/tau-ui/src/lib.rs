//! tau on the computer: the host that runs agents in the repositories
//! cloned from GitHub and drives tau-ui-remote's interface, and serves
//! the phones paired with it.
//!
//! [`host::Host`] is the wiring for a real coding agent: a tokio runtime
//! beside GPUI, every plugin through the registry with its host half
//! ([`hosted::halves`]), a ChatGPT plan through Sign in with ChatGPT,
//! and the run store. The `tau` app (`cargo run -p tau`) uses it when a
//! ChatGPT account with plan use is signed in, and opens onboarding to
//! sign one in when none is ([`host::onboard`]). With `--demo`, [`demo`]
//! replays a scripted session, its plugins answering through their real
//! host halves.

pub mod accounts;
pub mod demo;
pub mod github;
pub mod host;
pub mod hosted;
pub mod interface_runtime;
pub mod metered;
pub mod notify;
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
    push,
    queue,
    route,
    setup,
    titles,
    update,
    view,
    workspace,
};
