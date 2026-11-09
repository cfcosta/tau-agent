//! Where tau finds its packages (ADR 0051), in order: what names the folder,
//! the folder beside the executable, then the source tree.

use std::{
    fs,
    path::{Path, PathBuf},
};

use hegel::generators as gs;
use tau_luau_plugins::packages::{
    EXAMPLES,
    INSTALLED,
    OVERRIDE,
    example_from,
    find,
    find_from,
    fixture,
};

struct Places {
    _dir: tempfile::TempDir,
    wanted: PathBuf,
    exe: PathBuf,
    installed: PathBuf,
    source: PathBuf,
}

fn same(found: &Path, place: &Path) -> bool {
    found.canonicalize().unwrap() == place.canonicalize().unwrap()
}

#[hegel::test(test_cases = 100)]
fn the_folder_is_found_in_order(tc: hegel::TestCase) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let places = Places {
        wanted: root.join("wanted"),
        exe: root.join("out/bin/.tau-wrapped"),
        installed: root.join("out/share/tau/plugins"),
        source: root.join("source/plugins"),
        _dir: dir,
    };
    let (wanted, installed, source, named) = (
        tc.draw(gs::booleans()),
        tc.draw(gs::booleans()),
        tc.draw(gs::booleans()),
        tc.draw(gs::booleans()),
    );
    fs::create_dir_all(places.exe.parent().unwrap()).unwrap();
    for (exists, place) in [
        (wanted, &places.wanted),
        (installed, &places.installed),
        (source, &places.source),
    ] {
        if exists {
            fs::create_dir_all(place).unwrap();
        }
    }
    let found = find_from(
        named.then(|| places.wanted.clone().into_os_string()),
        Some(&places.exe),
        &places.source,
    );
    if named {
        // It never falls through, whatever else exists.
        if wanted {
            assert!(same(&found.unwrap(), &places.wanted));
        } else {
            let error = found.unwrap_err();
            assert!(error.contains(OVERRIDE), "{error}");
        }
    } else if installed {
        assert!(same(&found.unwrap(), &places.installed));
    } else if source {
        assert!(same(&found.unwrap(), &places.source));
    } else {
        // It says everywhere it looked.
        let error = found.unwrap_err();
        assert!(error.contains(INSTALLED), "{error}");
        assert!(error.contains("source/plugins"), "{error}");
        assert!(error.contains(OVERRIDE), "{error}");
    }
}

/// An empty override is no override, as unset.
#[test]
fn an_empty_override_is_none() {
    let source = tempfile::tempdir().unwrap();
    let found = find_from(Some("".into()), None, source.path()).unwrap();
    assert!(same(&found, source.path()));
}

/// This tree's tests find this tree's folder, and fixtures in it.
#[test]
fn tests_read_the_source_tree() {
    let found = find().unwrap();
    assert!(found.join("built-in.toml").is_file(), "{found:?}");
    let error = fixture("tau-goal", "no-such-fixture").unwrap_err();
    assert!(error.contains("no-such-fixture.json"), "{error}");
    assert!(fixture("tau-goal", "working").unwrap().starts_with('{'));
}

#[test]
fn an_example_is_its_folders_files_with_the_manifest_first() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join(EXAMPLES).join("one");
    fs::create_dir_all(folder.join("src")).unwrap();
    fs::write(folder.join("tau.toml"), "manifest").unwrap();
    fs::write(folder.join("src/plugin.luau"), "entry").unwrap();
    fs::write(folder.join("README.md"), "readme").unwrap();

    let example = example_from(dir.path(), "one").unwrap();
    assert_eq!(example.name, "one");
    assert_eq!(
        example.files,
        [
            ("tau.toml".to_owned(), "manifest".to_owned()),
            ("README.md".to_owned(), "readme".to_owned()),
            ("src/plugin.luau".to_owned(), "entry".to_owned()),
        ]
    );
    let missing = example_from(dir.path(), "two").unwrap_err();
    assert!(missing.contains("two"), "{missing}");
}
