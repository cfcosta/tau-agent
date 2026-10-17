//! Whose a merge of a forked operation log is (`docs/reference/vcs.md`,
//! "Tagged operations"): two operations written from the same one, as
//! tau and a `jj` command can write them at once, each tau's or not,
//! and the merge jj-lib writes when it next loads the repository. The
//! merge carries no one's tag, and `is_taus` says it is tau's exactly
//! when both sides are; the root operation is never tau's.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

#[path = "common/ops.rs"]
mod ops;

use std::sync::Arc;

use hegel::{TestCase, generators as gs};
use jj_lib::{
    backend::CommitId,
    repo::{ReadonlyRepo, Repo as _},
};
use pollster::block_on;
use tau_vcs_host::{Identity, ProjectRepo, TAU_OP_ATTRIBUTE, is_taus};

/// The repository at its head, loaded as another program loads it,
/// which merges a forked log.
fn load(project: &ProjectRepo) -> Arc<ReadonlyRepo> {
    use jj_lib::{
        config::{ConfigLayer, ConfigSource, StackedConfig},
        default_backend_factories::{
            default_backend_factories,
            default_working_copy_factories,
        },
        settings::UserSettings,
        workspace::Workspace,
    };
    let mut config = StackedConfig::with_defaults();
    let mut user = ConfigLayer::empty(ConfigSource::User);
    user.set_value("user.name", "person").unwrap();
    user.set_value("user.email", "person@localhost").unwrap();
    config.add_layer(user);
    let settings =
        UserSettings::from_config_and_home_dir(config, None).unwrap();
    let workspace = Workspace::load(
        &settings,
        project.main_dir(),
        &default_backend_factories(),
        &default_working_copy_factories(),
    )
    .unwrap();
    block_on(workspace.repo_loader().load_at_head()).unwrap()
}

/// An operation from `base` that adds a commit on the root, tagged as
/// tau's when `taus`.
fn write_op(base: &Arc<ReadonlyRepo>, taus: bool, description: &str) {
    let mut tx = base.start_transaction();
    let root: CommitId = base.store().root_commit_id().clone();
    let tree = base.store().empty_merged_tree();
    block_on(tx.repo_mut().new_commit(vec![root], tree).write()).unwrap();
    if taus {
        tx.set_attribute(TAU_OP_ATTRIBUTE.to_owned(), "test".to_owned());
    }
    block_on(tx.commit(description)).unwrap();
}

#[hegel::test(test_cases = 20)]
fn a_merge_is_taus_when_every_side_is(tc: TestCase) {
    let left = tc.draw(gs::booleans());
    let right = tc.draw(gs::booleans());
    let home = tempfile::tempdir().unwrap();
    let project = ProjectRepo::init(
        home.path().join("p"),
        Identity::default(),
        &[("a.txt".into(), "one\n".into())],
    )
    .unwrap();
    let base = load(&project);
    write_op(&base, left, "tau: left");
    write_op(&base, right, "right");

    let merged = load(&project);
    let merge = merged.operation();
    assert_eq!(merge.parent_ids().len(), 2, "the log forked and merged");
    assert!(
        !merge.metadata().attributes.contains_key(TAU_OP_ATTRIBUTE),
        "jj-lib's merge carries no tag"
    );
    assert_eq!(is_taus(merge).unwrap(), left && right);
    let root = block_on(merged.loader().root_operation());
    assert!(ops::is_root(&root));
    assert!(!is_taus(&root).unwrap());
}
