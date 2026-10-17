//! A repository's operation log, read through jj-lib as another program
//! would read it: which operations are new since the last look, and
//! whether each is tau's (`docs/reference/vcs.md`, "Tagged
//! operations").

// Each test crate uses only some of these.
#![allow(dead_code)]

use std::{collections::HashSet, path::Path};

use jj_lib::{
    config::{ConfigLayer, ConfigSource, StackedConfig},
    default_backend_factories::default_backend_factories,
    op_store::OperationId,
    operation::Operation,
    repo::RepoLoader,
    settings::UserSettings,
};
use pollster::block_on;
use tau_vcs_host::{DESCRIPTION_PREFIX, TAU_OP_ATTRIBUTE, is_taus};

/// The operation log of the repository at `repo_path` (a workspace's
/// `.jj/repo`), and the operations already looked at.
pub struct OpLog {
    loader: RepoLoader,
    seen: HashSet<OperationId>,
}

impl OpLog {
    pub fn open(repo_path: &Path) -> Self {
        let mut config = StackedConfig::with_defaults();
        let mut user = ConfigLayer::empty(ConfigSource::User);
        user.set_value("user.name", "reader").unwrap();
        user.set_value("user.email", "reader@localhost").unwrap();
        config.add_layer(user);
        let settings =
            UserSettings::from_config_and_home_dir(config, None).unwrap();
        let loader = RepoLoader::init_from_file_system(
            &settings,
            repo_path,
            &default_backend_factories(),
        )
        .unwrap();
        Self {
            loader,
            seen: HashSet::new(),
        }
    }

    /// The one head of the log. Two would be a fork, which tau never
    /// makes; they are not merged here, as loading the repository
    /// would, since that writes an operation.
    pub fn head(&self) -> Operation {
        let heads =
            block_on(self.loader.op_heads_store().get_op_heads()).unwrap();
        let [head] = heads.as_slice() else {
            panic!("the operation log forked: {heads:?}");
        };
        block_on(self.loader.load_operation(head)).unwrap()
    }

    /// The operations written since the last call, all of them the
    /// first time, the root's included.
    pub fn new_ops(&mut self) -> Vec<Operation> {
        let mut new = Vec::new();
        let mut next = vec![self.head()];
        while let Some(op) = next.pop() {
            if !self.seen.insert(op.id().clone()) {
                continue;
            }
            next.extend(block_on(op.parents()).unwrap());
            new.push(op);
        }
        new
    }
}

/// Whether `op` is the root operation, which every log starts from.
pub fn is_root(op: &Operation) -> bool {
    op.parent_ids().is_empty()
}

/// Asserts `op` is tau's, as another program tells: tagged, described
/// `tau: …`, and [`is_taus`].
pub fn assert_taus(op: &Operation, what: &str) {
    let metadata = op.metadata();
    assert!(
        metadata.attributes.contains_key(TAU_OP_ATTRIBUTE),
        "{what}: operation {:?} carries no {TAU_OP_ATTRIBUTE}",
        metadata.description
    );
    assert!(
        metadata.description.starts_with(DESCRIPTION_PREFIX),
        "{what}: operation {:?} is not described `tau: …`",
        metadata.description
    );
    assert!(is_taus(op).unwrap(), "{what}: {:?}", metadata.description);
}

/// Asserts every operation written since the last look is tau's, but
/// the root, which is no one's.
pub fn assert_all_taus(log: &mut OpLog, what: &str) {
    for op in log.new_ops() {
        if is_root(&op) {
            assert!(!is_taus(&op).unwrap(), "{what}: the root is not tau's");
        } else {
            assert_taus(&op, what);
        }
    }
}
