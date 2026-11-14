//! Every plugin tau comes with passes `tau-plugin test` (ADR 0053): its
//! cases, properties and loops, and its fixtures as they are in its
//! folder. Each package is a test of its own, so nextest runs them at
//! once and a filter takes one (`test(tau-memory)`); a package's tests
//! run at once too, in the pool (`tau_luau_plugins_host::pool`).

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::process::ExitCode;

use tau_luau_plugins_host::packages::{self, Fixtures, Package};
use tokio_util::sync::CancellationToken;

/// Speaks enough of libtest's command line for `cargo test` and nextest:
/// `--list` names a test a package (nextest asks with `--format terse`,
/// and asks for ignored ones apart, of which there are none), and a name
/// runs the packages that match it, exactly with `--exact`; none runs
/// them all.
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().any(|arg| arg == name);
    let elsewhere = tempfile::tempdir().unwrap();
    let built_ins = packages::chosen(&[], elsewhere.path()).unwrap();
    assert!(built_ins.len() > 10, "{} built-ins", built_ins.len());
    if flag("--list") {
        if !flag("--ignored") {
            for Package { files, .. } in &built_ins {
                println!("{}: test", files.manifest.name);
            }
        }
        return ExitCode::SUCCESS;
    }
    if flag("--ignored") {
        return ExitCode::SUCCESS;
    }
    // nextest runs as many packages at once as there are cores: each
    // takes a share of them for its own tests.
    let cores = std::thread::available_parallelism().map_or(1, usize::from);
    tau_luau_plugins_host::pool::set_jobs((cores / 4).max(2));
    let filter = args.iter().find(|arg| !arg.starts_with('-'));
    let chosen = built_ins.into_iter().filter(|Package { files, .. }| {
        let name = &files.manifest.name;
        match filter {
            None => true,
            Some(filter) if flag("--exact") => name == filter,
            Some(filter) => name.contains(filter.as_str()),
        }
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut failed = false;
    for Package { folder, files } in chosen {
        let name = files.manifest.name.clone();
        let tested = runtime.block_on(packages::test(
            files,
            &folder,
            Fixtures::Check,
            CancellationToken::new(),
            &Default::default(),
        ));
        match tested {
            Ok(results) if results.is_empty() => {
                eprintln!("{name}: it has no tests");
                failed = true;
            }
            Ok(results) => {
                let failures: Vec<_> =
                    results.iter().filter(|result| !result.passed).collect();
                if failures.is_empty() {
                    println!("{name}: ok");
                } else {
                    eprintln!("{name}: {failures:#?}");
                    failed = true;
                }
            }
            Err(error) => {
                eprintln!("{name}: it does not load: {error}");
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
