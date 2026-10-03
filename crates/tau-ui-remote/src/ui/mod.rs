//! The pieces screens are built from. The shared components and
//! marked-up text come from `tau-ui-kit`; this module adds the ones that
//! know tau's own types, and how a run's status reads. The workspace
//! decides where screens go for the window's width.

pub mod chrome;
pub mod components;
pub mod ending;
pub mod inspector;
pub mod interrupted;
pub mod landing;
pub mod push;
pub mod queue;
pub mod screens;
pub mod transcript;

pub use components::*;
use gpui::{Hsla, IntoElement, SharedString, div, prelude::*, px};
use tau_agent::event::StopReason;
pub use tau_ui_kit::prose::*;

use crate::{
    assets::Icon,
    attention::Attention,
    theme::{IconSize, Theme},
    view::{RunStatus, RunView},
};

/// The dot color and words a run's status reads as.
pub fn status_look(status: &RunStatus, t: &Theme) -> (Hsla, SharedString) {
    match status {
        RunStatus::Planning => (t.blue, "planning".into()),
        RunStatus::Running => (t.accent, "running".into()),
        RunStatus::Finished(stop) => stop_look(stop, t),
        RunStatus::Interrupted => (t.muted, "interrupted".into()),
    }
}

/// The dot color and words `run` reads as: its status, or read-only
/// for a chat that landed or was dropped.
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
        RunStatus::Interrupted => {
            icon(Icon::Pause, size, t.muted).into_any_element()
        }
    }
}

/// The icon a run's row shows for what it needs: its state's own, or,
/// when it needs nothing in particular, its status's (`nested` rows
/// under a run show the fork mark then).
pub fn attention_icon(
    attention: &Attention,
    run: &RunView,
    nested: bool,
    t: &Theme,
    size: IconSize,
) -> gpui::AnyElement {
    let glyph = |glyph, color| icon(glyph, size, color).into_any_element();
    match attention {
        Attention::Asks { .. } => glyph(Icon::Question, t.blue),
        Attention::ReadyToLand { .. } => glyph(Icon::Landable, t.green),
        Attention::WouldConflict { .. } => glyph(Icon::Warning, t.red),
        Attention::Interrupted => glyph(Icon::Interrupted, t.muted),
        Attention::Landed => glyph(Icon::Check, t.dim),
        Attention::Idle if nested => glyph(Icon::Fork, t.blue),
        Attention::Working { .. } | Attention::Failed | Attention::Idle => {
            status_icon(run, t, size)
        }
    }
}

/// The color of a run's line for what it needs.
pub fn attention_color(attention: &Attention, t: &Theme) -> Hsla {
    match attention {
        Attention::Asks { .. } => t.blue,
        Attention::ReadyToLand { .. } => t.green,
        Attention::WouldConflict { .. } | Attention::Failed => t.red,
        Attention::Landed => t.dim,
        Attention::Working { .. }
        | Attention::Interrupted
        | Attention::Idle => t.muted,
    }
}

/// The tint behind a run's row while it waits on the person to answer
/// or to land it.
pub fn attention_tint(attention: &Attention, t: &Theme) -> Option<Hsla> {
    match attention {
        Attention::Asks { .. } => Some(t.blue.opacity(0.06)),
        Attention::ReadyToLand { .. } => Some(t.green.opacity(0.06)),
        _ => None,
    }
}
