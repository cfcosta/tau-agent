//! A change carries its bookmarks and says when it is divergent. The
//! tools cannot make either, so the test does it with jj-lib directly,
//! as the host (or the user's own `jj`) would.

use std::path::Path;

use jj_lib::{
    backend::CommitId,
    config::{ConfigLayer, ConfigSource, StackedConfig},
    default_backend_factories::{
        default_backend_factories,
        default_working_copy_factories,
    },
    op_store::RefTarget,
    ref_name::RefName,
    repo::Repo as _,
    settings::UserSettings,
    workspace::Workspace,
};
use pollster::block_on;
use serde_json::{Value, json};
use tau_agent::{plugin::Plugin, tool::ToolCtx};
use tau_vcs::{Identity, Vcs, VcsPlugin};

/// Puts `main` on the commit `hex`, and writes a second commit with the
/// same change id on the root commit, so the change is divergent.
fn bookmark_and_diverge(dir: &Path, hex: &str) {
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
    let id = CommitId::try_from_hex(hex).unwrap();
    let commit = repo.store().get_commit(&id).unwrap();
    let mut tx = repo.start_transaction();
    tx.repo_mut()
        .set_local_bookmark_target(RefName::new("main"), RefTarget::normal(id));
    let root = repo.store().root_commit_id().clone();
    block_on(
        tx.repo_mut()
            .new_commit(vec![root], commit.tree())
            .set_change_id(commit.change_id().clone())
            .set_description("the twin")
            .write(),
    )
    .unwrap();
    block_on(tx.commit("bookmark and diverge")).unwrap();
}

#[test]
fn a_change_carries_its_bookmarks_and_divergence() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "a\n").unwrap();
    let vcs = Vcs::init(dir.path(), Identity::default()).unwrap();
    let tools = VcsPlugin::new(vcs).tools();
    let call = |name: &str, args: Value| {
        let tool = tools.iter().find(|tool| tool.name() == name).unwrap();
        let output =
            tau_testing::block_on(tool.call(args, ToolCtx::detached()))
                .expect(name);
        let text = output
            .content
            .iter()
            .filter_map(|block| match block {
                tau_ai::message::InputBlock::Text(text) => {
                    Some(text.text.clone())
                }
                _ => None,
            })
            .collect::<String>();
        (text, output.details.unwrap())
    };
    let (_, committed) = call("vcs_commit", json!({"message": "Add a"}));
    let hex = committed["committed"]["commit_id"].as_str().unwrap();
    bookmark_and_diverge(dir.path(), hex);

    let (text, details) = call("vcs_log", json!({}));
    let row = text.lines().nth(1).unwrap();
    assert!(row.ends_with(" (divergent) [main] Add a"), "{text}");
    let change = &details["changes"][1];
    assert_eq!(change["bookmarks"], json!(["main"]));
    assert_eq!(change["divergent"], json!(true));
    assert_eq!(details["changes"][0]["divergent"], json!(false));
    assert_eq!(details["changes"][0]["bookmarks"], json!([]));
}
