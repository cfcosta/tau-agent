//! The tests of tau's built-in Luau plugins that need Rust (ADR 0051): each
//! plugin's Luau tests as tau runs them, its fixtures against what it draws,
//! and what it does in a real loop. One crate, one binary. Everything is
//! read from the folder at run time, so a Luau edit compiles nothing.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::path::PathBuf;

mod ask;
mod binaries;
mod compaction;
mod compaction_chain;
mod constitution;
mod fast_compaction;
mod goal;
mod package;
mod reasoning;
mod schedule;
mod titles;
mod todo;
mod tree_compaction;
mod watcher;

/// The fixture `name` of the package `plugin`, as text.
pub(crate) fn fixture(plugin: &str, name: &str) -> &'static str {
    let text = tau_luau_plugins::packages::fixture(plugin, name).unwrap();
    Box::leak(text.into_boxed_str())
}

/// Where the fixture `name` of the package `plugin` is, to write it anew.
pub(crate) fn fixture_path(plugin: &str, name: &str) -> PathBuf {
    tau_luau_plugins::packages::find()
        .unwrap()
        .join(plugin)
        .join("fixtures")
        .join(format!("{name}.json"))
}
