//! The `tau.notify` slot (ADR 0045): where the host's notices go while
//! tau's window is not in front. The kernel decides when to notify (a
//! chat asks, is ready to land, failed, or something waits on the
//! person); the plugin that fills the slot shows the notice, and says
//! when the person opens it.

use crate::slots::Slot;

/// The slot.
pub const NOTIFY: Slot<dyn Notify> = Slot::new("tau.notify");

/// One notice: its words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
}

/// Where notices go.
pub trait Notify: Send + Sync {
    /// Shows `notice`; `opened` runs once the person opens it, and the
    /// kernel brings its chat up. A filler that is switched off shows
    /// nothing.
    fn show(&self, notice: Notice, opened: Box<dyn FnOnce() + Send>);
}
