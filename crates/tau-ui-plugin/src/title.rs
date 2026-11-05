//! The `tau.title` slot (ADR 0045): a run's title, written from what it
//! is about. Until it is written, and when nothing fills the slot, the
//! run shows the first line of its prompt.

use std::{future::Future, pin::Pin};

use crate::slots::Slot;

/// The slot.
pub const TITLE: Slot<dyn Title> = Slot::new("tau.title");

/// The longest title, in characters.
pub const MAX_CHARS: usize = 36;

/// What a title is written with.
pub type Written<'a> =
    Pin<Box<dyn Future<Output = anyhow::Result<Option<String>>> + Send + 'a>>;

/// Writes titles.
pub trait Title: Send + Sync {
    /// The title of a run about `task` (its prompt, or what a plugin
    /// reads it as, such as a goal's condition) that runs on
    /// `run_model`: at most [`MAX_CHARS`]. None when there is nothing to
    /// write it with now, such as no model while the person is signed
    /// out.
    fn write<'a>(&'a self, task: &'a str, run_model: &'a str) -> Written<'a>;
}
