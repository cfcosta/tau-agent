//! Long-term memory for tau agents: a Zettelkasten of typed, linked
//! Markdown notes (`docs/reference/plugins.md`, `tau-memory`; the
//! research behind it is `docs/research/memory.md`). This is its
//! interface half: the notes as their pages show them, and what a run
//! recalled and saved. Keeping the notes, searching them and the
//! plugin each run gets are `tau-memory-host`'s (ADR 0030).

pub mod note;
pub mod record;
pub mod ui;

pub use record::NAME;
pub use ui::MemoryUi;
