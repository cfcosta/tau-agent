//! `tau --check-plugins` and `tau --plugin-test` (ADR 0051), as the
//! installed package's check runs them: they answer before a host or a
//! window exists, and their exit says how it went.

use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

const TAU: &str = env!("CARGO_BIN_EXE_tau");

/// Copies `from` into `to`, and all under it.
fn copy(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            copy(&path, &to.join(entry.file_name()));
        } else {
            fs::copy(&path, to.join(entry.file_name())).unwrap();
        }
    }
}

fn source() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins")
}

fn check(dir: &Path) -> Output {
    Command::new(TAU)
        .arg("--check-plugins")
        .env("TAU_PLUGINS_DIR", dir)
        .output()
        .unwrap()
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn check_plugins_passes_on_the_source_tree() {
    let output = check(&source());
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("tau-compaction"),
        "{}",
        text(&output)
    );
}

#[test]
fn check_plugins_fails_loudly_on_a_broken_folder() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join("plugins");
    copy(&source(), &dir);

    // A listed plugin is missing.
    fs::remove_dir_all(dir.join("tau-compaction")).unwrap();
    let output = check(&dir);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("lists tau-compaction"),
        "{}",
        text(&output)
    );

    // A package the file does not list is there.
    copy(
        &source().join("tau-compaction"),
        &dir.join("tau-compaction"),
    );
    copy(&source().join("tau-titles"), &dir.join("not-listed"));
    let output = check(&dir);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("holds not-listed"),
        "{}",
        text(&output)
    );
    fs::remove_dir_all(dir.join("not-listed")).unwrap();

    // A built-in does not load.
    fs::write(dir.join("tau-goal/src/plugin.luau"), "return tau.plugin {")
        .unwrap();
    let output = check(&dir);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(text(&output).contains("tau-goal"), "{}", text(&output));

    // A folder that is not there, named over everything else.
    let output = check(&home.path().join("nowhere"));
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("TAU_PLUGINS_DIR"),
        "{}",
        text(&output)
    );
}

#[test]
fn plugin_test_says_how_a_folder_s_tests_went() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join("greet");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("tests")).unwrap();
    fs::write(
        dir.join("tau.toml"),
        "name = \"greet\"\nversion = \"0.1.0\"\ndescription = \"Greets.\"\n",
    )
    .unwrap();
    fs::write(
        dir.join("src/plugin.luau"),
        "return require(\"tau\").plugin { tools = { hello = { description = \
         \"Hello.\", call = function() return \"hi\" end } } }\n",
    )
    .unwrap();
    let case = |expected: &str| {
        format!(
            "local t = require(\"tau\").test\n\
             t.case(\"says\", function() t.equal(t.run{{}}:tool(\"hello\"), \
             \"{expected}\") end)\n"
        )
    };
    let run = || {
        Command::new(TAU)
            .arg("--plugin-test")
            .arg(&dir)
            .output()
            .unwrap()
    };

    fs::write(dir.join("tests/says.luau"), case("hi")).unwrap();
    let output = run();
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("1 of 1 tests pass"),
        "{}",
        text(&output)
    );

    fs::write(dir.join("tests/says.luau"), case("bye")).unwrap();
    let output = run();
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("0 of 1 tests pass"),
        "{}",
        text(&output)
    );

    fs::remove_file(dir.join("tau.toml")).unwrap();
    let output = run();
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(text(&output).contains("tau.toml"), "{}", text(&output));

    let missing = Command::new(TAU).arg("--plugin-test").output().unwrap();
    assert_eq!(missing.status.code(), Some(2), "{}", text(&missing));
}
