//! Private files, sync and async: whatever is written, in whatever
//! order, a file holds the last contents written to it, readable by its
//! owner only, in an owner-only directory, with no temporary left behind.
//! The two writers are checked against each other and against a map.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::PermissionsExt as _,
    path::Path,
};

use hegel::{TestCase, generators as gs};
use tau_ai::files::{write_private, write_private_async};

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// Names a user's files could have, dots and odd characters included.
#[hegel::composite]
fn name(tc: &TestCase) -> String {
    tc.draw(gs::sampled_from(vec![
        "a".to_owned(),
        "key".to_owned(),
        ".hidden".to_owned(),
        "with space".to_owned(),
        "é雪".to_owned(),
        "dots.in.name.json".to_owned(),
    ]))
}

#[hegel::composite]
fn write(tc: &TestCase) -> (String, Vec<u8>, bool) {
    (
        tc.draw(name()),
        tc.draw(gs::vecs(gs::integers::<u8>()).max_size(40)),
        tc.draw(gs::booleans()),
    )
}

#[hegel::test(test_cases = 100)]
fn files_hold_their_last_write_privately_with_nothing_left_over(tc: TestCase) {
    let writes = tc.draw(gs::vecs(write()).min_size(1).max_size(12));
    let nested = tc.draw(gs::booleans());
    let root = tempfile::tempdir().unwrap();
    let dir = if nested {
        root.path().join("one").join("two")
    } else {
        root.path().to_owned()
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut model: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for (file, bytes, sync) in &writes {
        let path = dir.join(file);
        if *sync {
            write_private(&path, bytes).unwrap();
        } else {
            runtime.block_on(write_private_async(&path, bytes)).unwrap();
        }
        model.insert(file.clone(), bytes.clone());
        // Right after each write, not only at the end.
        assert_eq!(&fs::read(&path).unwrap(), bytes);
    }
    let mut listed: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    listed.sort();
    assert_eq!(listed, model.keys().cloned().collect::<Vec<_>>());
    for (file, bytes) in &model {
        let path = dir.join(file);
        assert_eq!(&fs::read(&path).unwrap(), bytes);
        assert_eq!(mode(&path), 0o600, "{file}");
    }
    if nested {
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(dir.parent().unwrap()), 0o700);
    }
}
