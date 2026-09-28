//! The writing tools refuse an immutable working-copy commit. A tag
//! makes a commit immutable; the tools cannot make one, so the test
//! sets it with jj-lib directly, as the host would.

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
use pollster::block_on;
use serde_json::json;
use tau_agent::{
    plugin::Plugin,
    tool::{RunId, ToolCtx, ToolUpdates},
};
use tau_vcs::{Identity, Vcs, VcsPlugin};
use tokio_util::sync::CancellationToken;

fn ctx() -> ToolCtx {
    let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
    ToolCtx::new(
        CancellationToken::new(),
        ToolUpdates::for_tests("call_1", sender),
        RunId("run_1".into()),
    )
}

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
    let vcs = Vcs::init(dir.path(), Identity::default()).unwrap();
    let tools = VcsPlugin::new(vcs).tools();
    let find = |name: &str| {
        tools
            .iter()
            .find(|tool| tool.name() == name)
            .unwrap()
            .clone()
    };
    tag_working_copy(dir.path());

    let describe = find("vcs_describe");
    let err =
        tau_testing::block_on(describe.call(json!({"message": "nope"}), ctx()))
            .unwrap_err()
            .to_string();
    assert!(err.starts_with("The working-copy commit "), "{err}");
    assert!(err.ends_with(" is immutable"), "{err}");

    // Reading still works, and shows the flag.
    let log = find("vcs_log");
    let output = tau_testing::block_on(log.call(json!({}), ctx())).unwrap();
    let details = output.details.unwrap();
    assert_eq!(details["changes"][0]["immutable"], json!(true));
}
