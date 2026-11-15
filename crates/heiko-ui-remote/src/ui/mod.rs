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
use gpui::{Hsla, IntoElement, SharedString, prelude::*, rems};
use heiko_agent::event::{LimitKind, StopReason};
pub use heiko_ui_kit::prose::*;

use crate::{
    assets::Icon,
    attention::Attention,
    theme::{IconSize, Theme},
    view::{RunStatus, RunView},
};

/// The mark a run's status is shown with: working while it plans or
/// runs, failing on an error, the limit it stopped at (a cap, when it is
/// the one the person set on its cost), and idle while it waits for the
/// person.
pub fn status_mark_state(status: &RunStatus) -> State {
    match status {
        RunStatus::Planning | RunStatus::Running => State::Working,
        RunStatus::Finished(StopReason::Limit(LimitKind::Usd)) => State::Cap,
        RunStatus::Finished(StopReason::Limit(_)) => State::Limit,
        RunStatus::Finished(StopReason::Error(_)) => State::Failing,
        // Waiting for the person, whether it stopped or was stopped.
        RunStatus::Finished(StopReason::Stop | StopReason::Cancelled)
        | RunStatus::Interrupted => State::Idle,
    }
}

/// The mark a run is shown with for what it needs: its state's own, or,
/// when it needs nothing in particular, its `status`'s
/// ([`status_mark_state`]).
pub fn attention_mark_state(
    attention: &Attention,
    status: &RunStatus,
) -> State {
    match attention {
        Attention::Working { .. } => State::Working,
        Attention::Asks { .. } => State::Asks,
        Attention::ReadyToLand { .. } | Attention::Landed => State::Met,
        Attention::WouldConflict { .. } | Attention::ConflictsOnMain { .. } => {
            State::Failing
        }
        Attention::Queued(_) | Attention::Interrupted | Attention::Dropped => {
            State::Idle
        }
        Attention::Failed | Attention::Idle => status_mark_state(status),
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
        StopReason::Limit(kind) => (
            // The cap the person set reads in the cap's own color.
            if *kind == LimitKind::Usd {
                t.roles.cost
            } else {
                t.red
            },
            format!("limit · {kind:?}").to_lowercase().into(),
        ),
        StopReason::Cancelled => (t.muted, "cancelled".into()),
        StopReason::Error(_) => (t.red, "error".into()),
    }
}

/// The mark a run shows in lists, `size` px wide.
pub fn status_icon(
    run: &RunView,
    t: &Theme,
    size: IconSize,
) -> gpui::AnyElement {
    mark(status_mark_state(&run.status), t)
        .size(rems(size.0 / 16.))
        .into_any_element()
}

/// The mark a run's row shows for what it needs
/// ([`attention_mark_state`]), dimmed once the run is closed; a `nested`
/// row under a run that needs nothing shows the fork icon.
pub fn attention_icon(
    attention: &Attention,
    run: &RunView,
    nested: bool,
    t: &Theme,
    size: IconSize,
) -> gpui::AnyElement {
    if nested && *attention == Attention::Idle {
        return icon(Icon::Fork, size, t.blue).into_any_element();
    }
    let drawn = mark(attention_mark_state(attention, &run.status), t)
        .size(rems(size.0 / 16.));
    match attention {
        Attention::Landed | Attention::Dropped => drawn.text_color(t.dim),
        Attention::Queued(_) => drawn.text_color(t.accent),
        _ => drawn,
    }
    .into_any_element()
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

/// The tint behind a run's row while it waits on the person to answer
/// or to land it.
pub fn attention_tint(attention: &Attention, t: &Theme) -> Option<Hsla> {
    match attention {
        Attention::Asks { .. } => Some(t.blue.opacity(0.06)),
        Attention::ReadyToLand { .. } => Some(t.green.opacity(0.06)),
        _ => None,
    }
}
