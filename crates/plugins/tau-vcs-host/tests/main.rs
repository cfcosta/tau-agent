//! Every integration test of tau-vcs-host, in one binary: linking one
//! rather than one per file is most of what building the tests
//! costs (docs/reference/testing.md, "One binary per crate").

mod common;

mod bookmarks;
mod clone_trunk_config;
mod concurrent_runs;
mod conflict_snapshots;
mod fork_model;
mod git_lock;
mod ids;
mod immutable;
mod immutable_config;
mod import_model;
mod init;
mod inspect;
mod jj_cli_repo_format;
mod jj_config;
mod jj_config_model;
mod land;
mod local_setup;
mod main_workspace;
mod merged_operations;
mod model;
mod operations_jj_cli;
mod project;
mod push_config;
mod push_model;
mod run_heads;
mod run_workspace;
mod runs_model;
mod snapshot;
mod snapshot_config_model;
mod sub_agent_finalization;
mod sub_agent_model;
mod sub_agent_restart;
mod sub_agents;
mod sweep;
mod tools;
mod turns_model;
mod update_model;
mod working_copy_config;
mod workspaces_model;
mod worktrees;
