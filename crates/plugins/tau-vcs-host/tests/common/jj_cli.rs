//! The jj CLI the differential tests compare tau with: the one of
//! jj-lib's own version, since another reads config and revsets by
//! other rules. The dev shell pins it in `TAU_TEST_JJ` (`flake.nix`),
//! leaving the `jj` on `PATH` the person's own.

use std::{path::PathBuf, process::Command};

/// What `jj --version` prints for jj-lib's version, from `Cargo.lock`.
const VERSION: &str = "jj 0.45.1";

/// The jj CLI at jj-lib's version, by its full path, since some tests
/// run it with no `PATH`: `TAU_TEST_JJ` when it is set, else the `jj`
/// on `PATH` when it is that version, else none, and the caller says it
/// did not compare. A `TAU_TEST_JJ` that is not that version fails the
/// test: the pin and jj-lib must move together.
pub fn matching_jj() -> Option<PathBuf> {
    if let Some(pinned) = std::env::var_os("TAU_TEST_JJ") {
        let pinned = PathBuf::from(pinned);
        let version = version(&pinned);
        assert_eq!(
            version.as_deref(),
            Some(VERSION),
            "TAU_TEST_JJ ({}) is not jj-lib's version",
            pinned.display()
        );
        return Some(pinned);
    }
    let jj = std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join("jj"))
        .find(|path| path.is_file())?;
    (version(&jj).as_deref() == Some(VERSION)).then_some(jj)
}

/// What `jj --version` prints, trimmed.
fn version(jj: &std::path::Path) -> Option<String> {
    let output = Command::new(jj).arg("--version").output().ok()?;
    Some(String::from_utf8(output.stdout).ok()?.trim().to_owned())
}
