//! A GPUI interface for tau agents: a desktop layout with the run list,
//! the transcript and an inspector, and a one-column phone layout below
//! 720 px.
//!
//! The crate draws runs; it does not start them. A host wires it to
//! agents in two directions:
//!
//! - **In:** every [`RunEvent`](tau_agent::event::RunEvent) a run streams
//!   goes to [`Workspace::apply_event`]. What plugins decide but no event
//!   carries yet (the chosen reasoning effort, a blocked call's rule, the
//!   notes memory suggests) goes to [`Workspace::update_run`] as a
//!   [`RunUpdate`](view::RunUpdate).
//! - **Out:** the workspace emits a [`WorkspaceEvent`] when the user
//!   starts, steers, cancels or forks a run. Subscribe to it and call the
//!   matching `Agent` or `Run` method.
//!
//! ```ignore
//! let workspace = cx.new(|cx| Workspace::new("tau-agent", vec![], window, cx));
//! cx.subscribe(&workspace, move |workspace, event, cx| match event {
//!     WorkspaceEvent::NewRun { prompt } => {
//!         let run = agent.start(prompt, &store);
//!         // Stream `run.events()` into `Workspace::apply_event`.
//!     }
//!     WorkspaceEvent::Steer { run, text } => runs[run].steer(text),
//!     WorkspaceEvent::Cancel { run } => runs[run].cancel(),
//!     _ => {}
//! })
//! .detach();
//! ```
//!
//! [`host::Host`] is that wiring for a real coding agent: a tokio runtime
//! beside GPUI, `tau-tools` and `tau-compaction` as plugins, a ChatGPT
//! (Codex) sign-in or an API key, and the run store. `cargo run -p
//! tau-ui` uses it when a model is configured, and opens onboarding to
//! set one up when none is ([`host::onboard`]). With `--demo`, [`demo`]
//! replays a scripted session.
//!
//! Onboarding ([`setup`]) and pull requests ([`pull_request`]) follow
//! the same pattern: the workspace emits a request, the host answers
//! with [`Workspace::update_setup`] or [`Workspace::set_pull_request`].

pub mod accounts;
pub mod assets;
pub mod catalog;
pub mod demo;
pub mod github;
pub mod host;
pub mod input;
pub mod models;
pub mod picker;
pub mod pull_request;
pub mod repos;
pub mod route;
pub mod search;
pub mod setup;
pub mod theme;
pub mod ui;
pub mod view;
pub mod workspace;

use gpui::App;
pub use workspace::{Workspace, WorkspaceEvent};

/// Sets up what the interface needs once per app: fonts, the theme and
/// the text field's keys.
pub fn init(cx: &mut App) {
    if let Err(error) = assets::load_fonts(cx) {
        eprintln!("tau-ui: could not load the bundled fonts: {error}");
    }
    cx.set_global(theme::Theme::graphite());
    input::bind_keys(cx);
    workspace::bind_keys(cx);
}
