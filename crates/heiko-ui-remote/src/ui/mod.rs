//! The pieces screens are built from. The shared components and
//! marked-up text come from `heiko-ui-kit`; this module adds the ones that
//! know heiko's own types, and how a run's status reads. The workspace
//! decides where screens go for the window's width.

pub mod chrome;
pub mod components;
pub mod crew;
pub mod ending;
pub mod inspector;
pub mod interrupted;
pub mod landing;
pub mod push;
pub mod queue;
pub mod screens;
pub mod transcript;

pub use components::*;
use gpui::{Hsla, SharedString};
use heiko_agent::event::StopReason;
pub use heiko_ui_kit::prose::*;

use crate::{
    attention::Attention,
    theme::Theme,
    view::{RunStatus, RunView},
};

/// The mark a run's status is shown with: working while it plans or
/// runs, failing on an error, the limit it stopped at, and idle while it
/// waits for the person.
pub fn status_mark_state(status: &RunStatus) -> State {
    match status {
        RunStatus::Planning | RunStatus::Running => State::Working,
        RunStatus::Finished(StopReason::Limit(_)) => State::Limit,
        RunStatus::Finished(StopReason::Error(_)) => State::Failing,
        // Waiting for the person, whether it stopped or was stopped.
        RunStatus::Finished(StopReason::Stop | StopReason::Cancelled)
        | RunStatus::Interrupted => State::Idle,
    }
}

/// The color and words a run's status reads as.
pub fn status_look(status: &RunStatus, t: &Theme) -> (Hsla, SharedString) {
    match status {
        RunStatus::Planning => (t.green, "planning".into()),
        RunStatus::Running => (t.green, "running".into()),
        RunStatus::Finished(stop) => stop_look(stop, t),
        RunStatus::Interrupted => (t.muted, "interrupted".into()),
    }
}

/// The color and words `run` reads as: its status, or read-only for a
/// chat that landed or was dropped.
pub fn run_look(run: &RunView, t: &Theme) -> (Hsla, SharedString) {
    match run.ending {
        Some(_) => (t.dim, ending::READ_ONLY.into()),
        None => status_look(&run.status, t),
    }
}

pub fn stop_look(stop: &StopReason, t: &Theme) -> (Hsla, SharedString) {
    match stop {
        // A conversation that stopped is not over: it waits for the user.
        StopReason::Stop => (t.muted, "your turn".into()),
        StopReason::Limit(kind) => {
            (t.red, format!("limit · {kind:?}").to_lowercase().into())
        }
        StopReason::Cancelled => (t.muted, "cancelled".into()),
        StopReason::Error(_) => (t.red, "error".into()),
    }
}

/// The color of a run's line for what it needs: its mark's.
pub fn attention_color(attention: &Attention, t: &Theme) -> Hsla {
    match attention {
        Attention::Asks { .. } => t.roles.live,
        Attention::ReadyToLand { .. } => t.green,
        Attention::WouldConflict { .. }
        | Attention::ConflictsOnMain { .. }
        | Attention::Failed => t.red,
        Attention::Queued(_) => t.accent,
        Attention::Landed | Attention::Dropped => t.dim,
        Attention::Working { .. }
        | Attention::Interrupted
        | Attention::Idle => t.muted,
    }
}
