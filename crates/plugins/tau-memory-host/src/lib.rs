//! Long-term memory for tau agents: a Zettelkasten of typed, linked
//! Markdown notes (`docs/reference/plugins.md`, `tau-memory`; the
//! research behind it is `docs/research/memory.md`): tau-memory's host
//! half. The notes' shapes, its records and its pages are
//! `tau-memory`'s (ADR 0030).

pub mod colbert;
#[cfg(feature = "demo")]
pub mod demo;
#[cfg(feature = "docbert")]
pub mod docbert;
pub mod eval;
mod half;
pub mod index;
pub mod memory;
pub mod plugin;
pub mod recall;
pub mod safety;
pub mod store;

pub use half::{
    Host,
    Memories,
    MemoryHost,
    Search,
    notebook,
    now,
    stale_on_turn,
};
pub use memory::Memory;
pub use plugin::{MemoryPlugin, Scopes};
// What it keeps and publishes, from the interface half.
pub use tau_memory::{NAME, note, record};
