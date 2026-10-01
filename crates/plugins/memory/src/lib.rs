//! Long-term memory for tau agents: a Zettelkasten of typed, linked
//! Markdown notes (`docs/reference/plugins.md`, `tau-memory`; the
//! research behind it is `docs/research/memory.md`).

pub mod colbert;
#[cfg(feature = "docbert")]
pub mod docbert;
pub mod eval;
pub mod index;
pub mod memory;
pub mod note;
pub mod plugin;
pub mod recall;
pub mod safety;
pub mod store;
pub mod ui;

pub use memory::Memory;
pub use plugin::{MemoryPlugin, Scopes};
