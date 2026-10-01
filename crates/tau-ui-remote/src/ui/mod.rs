//! The pieces screens are built from. The shared components and
//! marked-up text come from `tau-ui-kit`; this module adds the ones that
//! know tau's own types, and how a run's status reads. The workspace
//! decides where screens go for the window's width.

pub mod chrome;
pub mod components;
pub mod inspector;
pub mod landing;
pub mod screens;
pub mod transcript;

pub use components::*;
use gpui::{Hsla, IntoElement, SharedString, div, prelude::*, px};
use tau_agent::event::StopReason;
pub use tau_ui_kit::prose::*;

use crate::{
    assets::Icon,
    theme::{IconSize, Theme},
    view::{RunStatus, RunView},
};

/// The dot color and words a run's status reads as.
pub fn status_look(status: &RunStatus, t: &Theme) -> (Hsla, SharedString) {
    match status {
        RunStatus::Planning => (t.blue, "planning".into()),
        RunStatus::Running => (t.accent, "running".into()),
        RunStatus::Finished(stop) => stop_look(stop, t),
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

/// The icon a run shows in lists.
pub fn status_icon(
    run: &RunView,
    t: &Theme,
    size: IconSize,
) -> gpui::AnyElement {
    match &run.status {
        RunStatus::Planning | RunStatus::Running => {
            live_dot(t.accent, 8.).into_any_element()
        }
        // Waiting for the user, whether it stopped or was stopped.
        RunStatus::Finished(StopReason::Stop | StopReason::Cancelled) => div()
            .size(px(size.0))
            .flex()
            .items_center()
            .justify_center()
            .child(dot(t.border_strong, 6.))
            .into_any_element(),
        RunStatus::Finished(_) => {
            icon(Icon::Warning, size, t.red).into_any_element()
        }
    }
}
