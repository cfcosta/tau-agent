//! Compaction into a tree of one-line summaries the agent can zoom
//! into, after OptChat
//! (<https://gist.github.com/VictorTaelin/91837951a5ce5b38f341ec1ba1df6449>;
//! `docs/reference/tree-compaction.md`).
//!
//! tau-compaction replaces the older messages with one summary, and
//! what the summary leaves out is gone. This plugin folds them into a
//! history kept word for word instead, grows a binary tree of
//! summaries over it, and puts the view first in their place: lines
//! that tile the whole history, recent ones a message each, older ones
//! many, within a byte budget. The `zoom` tool opens any line into the
//! two it was made from, down to a message whole.
//!
//! - [`tree`]: the history, the tree and the view, without I/O.
//! - [`build`]: building lines with a model, many at once.
//! - [`TreeCompaction`]: the plugin, and the `zoom` tool it adds.
//! - [`ui`]: its UI and its host half, off by default.
//!
//! ## Deviations from OptChat
//!
//! - **Within a run.** OptChat starts every user message fresh, on the
//!   view alone. A tau run is one conversation whose tool loop needs
//!   its latest messages whole, so the view replaces only what falls
//!   before tau-compaction's cut, and the kept tail stays as it was.
//! - **Built when compacting.** OptChat builds lines in the background
//!   as messages come, since every turn reads the view. Most tau runs
//!   never compact, so lines are built only when a compaction is due,
//!   all at once: level-0 lines `jobs` at a time, then merges a level
//!   at a time. A level-0 line's context is the view before the batch
//!   plus the batch's messages just before its own, cut short, instead
//!   of the view up to it, which would build them one by one.
//! - **A smaller view.** 64,000 bytes by default, not 128,000: the view
//!   shares the window with the kept tail.
//! - **No `date` tool.** A run is short; its messages' times rarely
//!   matter.

pub mod build;
mod plugin;
pub mod tree;
pub mod ui;

pub use plugin::{
    Details,
    Record,
    TreeCompaction,
    VIEW_PREFIX,
    VIEW_SUFFIX,
    Zoom,
    ZoomArgs,
    view_message,
};

/// The name tree compaction goes by in events and stored rewrites.
pub const NAME: &str = "tau-tree-compaction";
