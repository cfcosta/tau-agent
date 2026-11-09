//! Every integration test of tau-ui, in one binary: linking one
//! rather than one per file is most of what building the tests
//! costs (docs/reference/testing.md, "One binary per crate").

mod common;
mod support;
#[path = "common/sync.rs"]
mod sync;

mod ask;
mod canvas;
mod compaction;
mod constitution;
mod crash_model;
mod direnv;
mod direnv_ui;
mod forecast;
mod goal;
mod host;
mod kernel;
mod landing;
mod local;
mod notify;
mod offline_model;
mod phone_model;
mod phones;
mod plugin_services;
mod plugins_model;
mod push;
mod queue;
mod queue_model;
mod reasoning;
mod restart;
mod sync_model;
mod todo_model;
mod todo_sync_model;
mod update;
mod views;
mod watcher_ui;
mod workspace;
