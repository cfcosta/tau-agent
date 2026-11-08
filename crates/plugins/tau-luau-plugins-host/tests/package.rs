//! A package's `tau.toml` and layout (ADR 0051): what is refused and why,
//! that a folder and a commit read alike, and what a version's digest
//! covers.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{fs, path::Path};

use hegel::generators as gs;
use tau_luau_plugins_host::runtime::{Files, MANIFEST, Manifest, PLUGIN, load};
use tau_testing::block_on_io;

const GOOD: &str = "name = \"greet\"\nversion = \"0.1.0\"\n\
                    description = \"Says hello.\"\n";
const ENTRY: &str = "return require(\"tau\").plugin {}\n";

#[test]
fn a_manifest_is_refused_for_what_is_wrong_with_it() {
    for (text, why) in [
        ("version = \"0.1.0\"\ndescription = \"d\"\n", "name"),
        ("name = \"a\"\ndescription = \"d\"\n", "version"),
        ("name = \"a\"\nversion = \"0.1.0\"\n", "description"),
        (
            "name = \"a\"\nversion = \"0.1.0\"\ndescription = \"d\"\nuses = 1\n",
            "uses",
        ),
        (
            "name = \"a/b\"\nversion = \"0.1.0\"\ndescription = \"d\"\n",
            "folder's name",
        ),
        (
            "name = \".a\"\nversion = \"0.1.0\"\ndescription = \"d\"\n",
            "folder's name",
        ),
        (
            "name = \"a\"\nversion = \"1.2\"\ndescription = \"d\"\n",
            "three numbers",
        ),
        (
            "name = \"a\"\nversion = \"1.2.x\"\ndescription = \"d\"\n",
            "three numbers",
        ),
        (
            "name = \"a\"\nversion = \"0.1.0\"\ndescription = \"\"\n",
            "one line",
        ),
        (
            "name = \"a\"\nversion = \"0.1.0\"\ndescription = \"a\\nb\"\n",
            "one line",
        ),
        (GOOD_WITH_ENTRY_OUTSIDE, "src/<name>.luau"),
        (GOOD_WITH_ENTRY_UP, "src/<name>.luau"),
        (GOOD_WITH_ENTRY_DEEP, "src/<name>.luau"),
        (GOOD_WITH_ENTRY_TAU, "src/<name>.luau"),
    ] {
        let error = Manifest::parse(text).unwrap_err();
        assert!(error.contains(why), "{text}: {error}");
        assert!(error.starts_with(MANIFEST), "{error}");
    }
    assert_eq!(Manifest::parse(GOOD).unwrap().entry, "");
    assert_eq!(
        Manifest::parse(&Manifest::text("greet", "Says \"hello\".")).unwrap(),
        Manifest {
            name: "greet".into(),
            version: "0.1.0".into(),
            description: "Says \"hello\".".into(),
            entry: String::new(),
        }
    );
}

const GOOD_WITH_ENTRY_OUTSIDE: &str = "name = \"a\"\nversion = \"0.1.0\"\n\
    description = \"d\"\nentry = \"plugin.luau\"\n";
const GOOD_WITH_ENTRY_UP: &str = "name = \"a\"\nversion = \"0.1.0\"\n\
    description = \"d\"\nentry = \"src/../x.luau\"\n";
const GOOD_WITH_ENTRY_DEEP: &str = "name = \"a\"\nversion = \"0.1.0\"\n\
    description = \"d\"\nentry = \"src/a/b.luau\"\n";
const GOOD_WITH_ENTRY_TAU: &str = "name = \"a\"\nversion = \"0.1.0\"\n\
    description = \"d\"\nentry = \"src/tau.luau\"\n";

fn paths(files: &[(&str, &str)]) -> Vec<(String, Vec<u8>)> {
    files
        .iter()
        .map(|(path, text)| ((*path).to_owned(), text.as_bytes().to_vec()))
        .collect()
}

#[test]
fn a_folder_without_what_it_needs_says_so() {
    let error =
        Files::from_paths(&paths(&[("src/plugin.luau", ENTRY)]), PLUGIN)
            .unwrap_err();
    assert!(error.contains("no tau.toml"), "{error}");
    let error = Files::from_paths(
        &paths(&[("plugin.luau", ENTRY), (MANIFEST, GOOD)]),
        PLUGIN,
    )
    .unwrap_err();
    assert!(error.contains("no src/plugin.luau"), "{error}");
    for nested in ["src/a/b.luau", "tests/a/b.luau"] {
        let error = Files::from_paths(
            &paths(&[
                (MANIFEST, GOOD),
                ("src/plugin.luau", ENTRY),
                (nested, ENTRY),
            ]),
            PLUGIN,
        )
        .unwrap_err();
        assert!(error.contains(nested), "{error}");
    }
    // A manifest's entry is the entry, whatever it is called, and the
    // default is not read as one.
    let named = format!("{GOOD}entry = \"src/main.luau\"\n");
    let files = Files::from_paths(
        &paths(&[
            (MANIFEST, &named),
            ("src/main.luau", ENTRY),
            ("src/helper.luau", "return {}"),
        ]),
        PLUGIN,
    )
    .unwrap();
    assert_eq!(files.entry(), "src/main.luau");
    assert_eq!(files.libs.keys().collect::<Vec<_>>(), ["helper"]);
    block_on_io(load("greet", files)).unwrap();
}

/// `tau.plugin {}` takes neither of what `tau.toml` says.
#[test]
fn a_plugin_saying_its_name_or_description_is_refused() {
    for said in ["name = \"greet\"", "description = \"Says hello.\""] {
        let files = Files {
            source: format!("return require(\"tau\").plugin {{ {said} }}\n"),
            ..Files::named("greet")
        };
        let error = block_on_io(load("greet", files)).unwrap_err();
        assert!(error.contains("in its tau.toml"), "{said}: {error}");
    }
}

/// `tau.toml`'s word for the declaration.
#[test]
fn the_declaration_says_what_the_manifest_says() {
    let source = "return require(\"tau\").plugin { nav = { icon = \"list\" } }";
    let files = Files::from_paths(
        &paths(&[(MANIFEST, GOOD), ("src/plugin.luau", source)]),
        PLUGIN,
    )
    .unwrap();
    let loaded = block_on_io(load("greet", files)).unwrap();
    assert_eq!(loaded.declaration.name, "greet");
    assert_eq!(loaded.declaration.description, "Says hello.");
    assert_eq!(loaded.declaration.nav.unwrap().label, "greet");
}

fn write(dir: &Path, path: &str, text: &str) {
    let path = dir.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// A folder and a commit of it read alike, and the digest moves with
/// anything tau reads, never with `fixtures/`.
#[hegel::test(test_cases = 50)]
fn a_folder_and_its_commit_are_one_version(tc: hegel::TestCase) {
    let word = || gs::sampled_from(vec!["alpha", "beta", "gamma", "delta"]);
    let libs = tc.draw(gs::vecs(word()).max_size(4));
    let tests = tc.draw(gs::vecs(word()).max_size(3));
    let body = tc.draw(gs::text().max_size(40));
    let mut files: Vec<(String, String)> = vec![
        (MANIFEST.to_owned(), GOOD.to_owned()),
        ("src/plugin.luau".to_owned(), ENTRY.to_owned()),
    ];
    for name in libs {
        files.push((format!("src/{name}.luau"), body.clone()));
    }
    for name in tests {
        files.push((format!("tests/{name}.luau"), body.clone()));
    }
    files.push(("fixtures/view.json".to_owned(), body.clone()));
    let dir = tempfile::tempdir().unwrap();
    for (path, text) in &files {
        write(dir.path(), path, text);
    }
    let as_paths: Vec<(String, Vec<u8>)> = files
        .iter()
        .map(|(path, text)| (path.clone(), text.as_bytes().to_vec()))
        .collect();
    let read = Files::read(dir.path(), PLUGIN).unwrap();
    let committed = Files::from_paths(&as_paths, PLUGIN).unwrap();
    assert_eq!(read.digest(), committed.digest());

    // Fixtures are no new version; anything else is.
    let digest = read.digest();
    write(dir.path(), "fixtures/view.json", &format!("{body}!"));
    assert_eq!(Files::read(dir.path(), PLUGIN).unwrap().digest(), digest);
    write(dir.path(), "README.md", "# Greet");
    assert_ne!(Files::read(dir.path(), PLUGIN).unwrap().digest(), digest);
    fs::remove_file(dir.path().join("README.md")).unwrap();
    write(
        dir.path(),
        MANIFEST,
        "name = \"greet\"\nversion = \"0.2.0\"\ndescription = \"Says hello.\"\n",
    );
    assert_ne!(Files::read(dir.path(), PLUGIN).unwrap().digest(), digest);
}
