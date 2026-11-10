//! Every integration test of tau-ui-remote, in one binary: linking one
//! rather than one per file is most of what building the tests
//! costs (docs/reference/testing.md, "One binary per crate").

mod agents;
mod attention;
mod core_names_no_plugin;
mod design;
mod fresh_main;
mod landed_itself;
mod markdown;
mod own_repo;
mod phone_without_plugins;
mod plugin_keys;
mod plugins_sidebar;
mod remote_pairing;
mod remote_window;
mod repo_actions;
mod repo_model;
mod setup_steps;
mod sidebar;
mod status_bar;
mod table_layout;
mod terminal;
