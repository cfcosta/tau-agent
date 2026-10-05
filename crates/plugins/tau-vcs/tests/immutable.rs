//! The writing tools refuse an immutable working-copy commit. A tag
//! makes a commit immutable; the tools cannot make one, so the test
//! sets it with jj-lib directly, as the host would.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0027)"
)]

use std::path::Path;

use jj_lib::{
    config::{ConfigLayer, ConfigSource, StackedConfig},
    default_backend_factories::{
        default_backend_factories,
        default_working_copy_factories,
    },
    op_store::RefTarget,
    ref_name::{RefName, WorkspaceName},
    settings::UserSettings,
    workspace::Workspace,
};
use serde_json::json;
use tau_agent::{plugin::Plugin, tool::ToolCtx};
use tau_testing::block_on_io as block_on;
use tau_vcs::{Identity, VcsPlugin};

/// Tags the working-copy commit of the workspace at `dir`.
fn tag_working_copy(dir: &Path) {
    let mut config = StackedConfig::with_defaults();
    let mut user = ConfigLayer::empty(ConfigSource::User);
    user.set_value("user.name", "host").unwrap();
    user.set_value("user.email", "host@localhost").unwrap();
    config.add_layer(user);
    let settings = UserSettings::from_config(config).unwrap();
    let workspace = Workspace::load(
        &settings,
        dir,
        &default_backend_factories(),
        &default_working_copy_factories(),
    )
    .unwrap();
    let repo = block_on(workspace.repo_loader().load_at_head()).unwrap();
    let wc = repo
        .view()
        .get_wc_commit_id(WorkspaceName::DEFAULT)
        .unwrap()
        .clone();
    let mut tx = repo.start_transaction();
    tx.repo_mut()
        .set_local_tag_target(RefName::new("v1"), RefTarget::normal(wc));
    block_on(tx.commit("tag the working copy")).unwrap();
}

#[test]
fn writes_refuse_an_immutable_working_copy() {
    let dir = tempfile::tempdir().unwrap();
    let vcs = tau_testing::block_on_io(tau_vcs::Vcs::init(
        dir.path(),
        Identity::default(),
    ))
    .unwrap();
    let tools = VcsPlugin::new(vcs).tools();
    let find = |name: &str| {
        tools
            .iter()
            .find(|tool| tool.name() == name)
            .unwrap()
            .clone()
    };
    tag_working_copy(dir.path());

    for (name, args) in [
        ("vcs_describe", json!({"message": "nope"})),
        ("vcs_commit", json!({"message": "nope"})),
        ("vcs_new", json!({})),
        ("vcs_restore", json!({"paths": ["file.txt"]})),
        ("vcs_resolve", json!({"paths": ["file.txt"]})),
        ("vcs_undo", json!({})),
    ] {
        let err =
            tau_testing::block_on(find(name).call(args, ToolCtx::detached()))
                .unwrap_err()
                .to_string();
        assert!(err.starts_with("The working-copy commit "), "{name}: {err}");
        assert!(err.ends_with(" is immutable"), "{name}: {err}");
    }

    // Reading still works, and shows the flag.
    let log = find("vcs_log");
    let output =
        tau_testing::block_on(log.call(json!({}), ToolCtx::detached()))
            .unwrap();
    let details = output.details.unwrap();
    assert_eq!(details["changes"][0]["immutable"], json!(true));
}
