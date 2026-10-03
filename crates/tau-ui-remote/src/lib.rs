//! tau's interface in GPUI: a desktop layout with the run list, the
//! transcript and an inspector, and a one-column phone layout below
//! 720 px, with every plugin's views.
//!
//! The crate draws runs; it does not start them. A host wires it to
//! agents in two directions:
//!
//! - **In:** every [`HostUpdate`](update::HostUpdate) a host applies goes
//!   to [`Workspace::apply`]: the events runs stream, the catalog, what
//!   plugins answered.
//! - **Out:** the workspace emits a [`WorkspaceEvent`] when the user
//!   starts, steers, cancels or forks a run. Subscribe to it and call the
//!   matching `Agent` or `Run` method.
//!
//! ```ignore
//! let workspace = cx.new(|cx| Workspace::new("tau-agent", vec![], catalog, window, cx));
//! cx.subscribe(&workspace, move |workspace, event, cx| match event {
//!     WorkspaceEvent::NewRun { prompt, .. } => {
//!         let run = agent.start(prompt, &store);
//!         // Apply what `run` streams as `HostUpdate::Event`s.
//!     }
//!     WorkspaceEvent::Cancel { run } => runs[run].cancel(),
//!     _ => {}
//! })
//! .detach();
//! ```
//!
//! tau-ui's host is that wiring for a real coding agent, on the computer.
//! On a phone, [`remote`] is: it pairs with the tau on a computer and
//! applies what that tau's host applies. Onboarding ([`setup`]) and pull
//! requests ([`pull_request`]) follow the same pattern: the workspace
//! emits a request, the host answers with a [`HostUpdate`](update::HostUpdate).
//! So does a phone's pairing ([`pairing`]): the workspace emits a
//! [`PairRequest`](pairing::PairRequest), and the phone's remote answers
//! with [`Workspace::update_pairing`].

pub mod attach;
pub mod attention;
pub mod catalog;
pub mod models;
pub mod motion;
pub mod pairing;
pub mod phones;
pub mod picker;
pub mod plan_usage;
pub mod plugins;
pub mod pull_request;
pub mod push;
pub mod queue;
pub mod remote;
pub mod repos;
pub mod route;
pub mod search;
pub mod setup;
pub mod slash;
pub mod titles;
pub mod ui;
pub mod update;
pub mod view;
pub mod workspace;

use gpui::App;
pub use tau_ui_kit::{assets, input, markdown, theme};
pub use workspace::{Workspace, WorkspaceEvent};

/// Sets up what the interface needs once per app: fonts, the theme and
/// the text field's keys.
pub fn init(cx: &mut App) {
    tau_ui_kit::init(cx);
    workspace::bind_keys(cx);
    tau_terminal::view::bind_keys(cx);
}
