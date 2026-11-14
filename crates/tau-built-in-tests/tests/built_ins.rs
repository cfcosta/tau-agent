//! Every plugin tau comes with passes `tau-plugin test` (ADR 0053): its
//! cases, properties and loops, and its fixtures as they are in its folder.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use tau_luau_plugins_host::packages::{self, Fixtures, Package};
use tau_testing::block_on_io;
use tokio_util::sync::CancellationToken;

#[test]
fn every_built_in_passes_tau_plugin_test() {
    let elsewhere = tempfile::tempdir().unwrap();
    let built_ins = packages::chosen(&[], elsewhere.path()).unwrap();
    assert!(built_ins.len() > 10, "{} built-ins", built_ins.len());
    let mut failed = Vec::new();
    for Package { folder, files } in built_ins {
        let name = files.manifest.name.clone();
        let cancel = CancellationToken::new();
        match block_on_io(packages::test(
            files,
            &folder,
            Fixtures::Check,
            cancel,
            &Default::default(),
        )) {
            Ok(results) => {
                assert!(!results.is_empty(), "{name} has no tests");
                failed.extend(
                    results
                        .into_iter()
                        .filter(|result| !result.passed)
                        .map(|result| (name.clone(), result)),
                );
            }
            Err(error) => failed.push((
                name.clone(),
                tau_luau_plugins::TestResult {
                    file: String::new(),
                    name: "loads".into(),
                    passed: false,
                    error: Some(error),
                    kind: tau_luau_plugins::TestKind::File,
                    ..Default::default()
                },
            )),
        }
    }
    assert!(failed.is_empty(), "{failed:#?}");
}
