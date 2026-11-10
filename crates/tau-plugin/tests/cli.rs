//! `tau-plugin check` and `tau-plugin test` (ADR 0053), as the installed
//! package's check runs them: their exit says how it went, and what they
//! print what they found.

use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

const TAU_PLUGIN: &str = env!("CARGO_BIN_EXE_tau-plugin");

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

/// `tau-plugin` with `args`, run where no package is, so with no folder
/// it takes tau's own.
fn tau_plugin(args: &[&str], plugins: &Path) -> Output {
    let elsewhere = tempfile::tempdir().unwrap();
    Command::new(TAU_PLUGIN)
        .args(args)
        .current_dir(elsewhere.path())
        .env("TAU_PLUGINS_DIR", plugins)
        .output()
        .unwrap()
}

fn check(dir: &Path) -> Output {
    tau_plugin(&["check"], dir)
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn check_passes_on_the_source_tree_and_says_what_each_reaches() {
    let output = check(&source());
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("tau-compaction"),
        "{}",
        text(&output)
    );
    // A folder named: what it declares and reaches, in the Allow's words.
    let example = source().join("examples/word-count");
    let output = tau_plugin(&["check", example.to_str().unwrap()], &source());
    assert!(output.status.success(), "{}", text(&output));
    let said = text(&output);
    assert!(said.contains("tools: word_count"), "{said}");
    assert!(said.contains("reaches: the `read` tool"), "{said}");
    let output =
        tau_plugin(&["check", "--json", example.to_str().unwrap()], &source());
    let found: serde_json::Value =
        serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(found[0]["name"], "word-count");
    assert_eq!(found[0]["declaration"]["uses"]["tools"][0], "read");
}

#[test]
fn check_fails_loudly_on_a_broken_folder() {
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
fn test_says_how_a_folder_s_tests_went() {
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
    let run = || tau_plugin(&["test", dir.to_str().unwrap()], &source());

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

    // Run in the package's folder, it takes that one.
    fs::write(
        dir.join("tau.toml"),
        "name = \"greet\"\nversion = \"0.1.0\"\ndescription = \"Greets.\"\n",
    )
    .unwrap();
    let here = Command::new(TAU_PLUGIN)
        .args(["test", "--verbose"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(here.status.code(), Some(1), "{}", text(&here));
    assert!(text(&here).contains("FAIL  says › says"), "{}", text(&here));
    // A filter keeps only what it names: here, not the failing case.
    let none = tau_plugin(
        &["test", "--filter", "nothing", dir.to_str().unwrap()],
        &source(),
    );
    assert!(none.status.success(), "{}", text(&none));
    assert!(text(&none).contains("greet: no tests"), "{}", text(&none));

    for wrong in [
        &[][..],
        &["frob"],
        &["test", "--frob"],
        &["test", "--filter"],
    ] {
        let output = tau_plugin(wrong, &source());
        assert_eq!(
            output.status.code(),
            Some(2),
            "{wrong:?}: {}",
            text(&output)
        );
    }
}

/// `new` writes a package that passes its tests, and `install` commits it
/// to the plugins repository under the data directory it is given.
#[test]
fn a_new_package_passes_and_installs() {
    let home = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        Command::new(TAU_PLUGIN)
            .args(args)
            .current_dir(home.path())
            .env("XDG_DATA_HOME", home.path().join("data"))
            .env("TAU_PLUGINS_DIR", source())
            .output()
            .unwrap()
    };
    let made = run(&["new", "greeter"]);
    assert!(made.status.success(), "{}", text(&made));
    assert!(
        text(&made).contains("greeter: 2 of 2 tests pass"),
        "{}",
        text(&made)
    );
    let again = run(&["new", "greeter"]);
    assert_eq!(again.status.code(), Some(1), "{}", text(&again));
    let wrong = run(&["new", "Greeter"]);
    assert_eq!(wrong.status.code(), Some(2), "{}", text(&wrong));

    let installed = run(&["install", "greeter", "--json"]);
    assert!(installed.status.success(), "{}", text(&installed));
    let said: serde_json::Value =
        serde_json::from_slice(&installed.stdout).unwrap();
    assert_eq!(said["name"], "greeter");
    assert_eq!(
        std::path::PathBuf::from(said["repository"].as_str().unwrap()),
        home.path().join("data/tau/luau-plugins")
    );
    let theirs =
        run(&["install", source().join("tau-titles").to_str().unwrap()]);
    assert_eq!(theirs.status.code(), Some(1), "{}", text(&theirs));
    assert!(
        text(&theirs).contains("one of tau's own"),
        "{}",
        text(&theirs)
    );
}
