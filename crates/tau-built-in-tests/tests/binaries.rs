//! What keeps each crate's integration tests in one binary
//! (docs/reference/testing.md, "One binary per crate"): with `autotests`
//! off, a file `tests/main.rs` does not name is never built, so it never
//! fails either.

use std::path::Path;

/// Every crate with more than one test file builds them as one binary,
/// `tests/main.rs`, which names each of them. A `[[test]]` of its own
/// (a `harness = false` one) stays apart.
#[test]
fn every_test_file_is_in_its_crates_one_binary() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut manifests = Vec::new();
    super::package::walk(
        &crates,
        &|path| {
            path.file_name().is_some_and(|name| name == "Cargo.toml")
                && !path.components().any(|part| part.as_os_str() == "target")
        },
        &mut manifests,
    );
    assert!(manifests.len() > 30, "{manifests:?}");
    let mut wrong = Vec::new();
    for manifest in manifests {
        let crate_dir = manifest.parent().unwrap();
        let tests = crate_dir.join("tests");
        let Ok(entries) = std::fs::read_dir(&tests) else {
            continue;
        };
        let text = std::fs::read_to_string(&manifest).unwrap();
        let apart = apart(&text);
        let mut files: Vec<String> = entries
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.ends_with(".rs"))
            .filter(|name| name != "main.rs" && !apart.contains(name))
            .collect();
        files.sort();
        let main = std::fs::read_to_string(tests.join("main.rs")).ok();
        if files.len() < 2 && main.is_none() {
            continue;
        }
        let name = crate_dir.file_name().unwrap().to_string_lossy();
        let Some(main) = main else {
            wrong.push(format!(
                "{name}: {} test files, no tests/main.rs",
                files.len()
            ));
            continue;
        };
        if !text.contains("autotests = false") {
            wrong.push(format!("{name}: tests/main.rs, but autotests on"));
        }
        for file in &files {
            let module = format!("mod {};", file.trim_end_matches(".rs"));
            if !main.lines().any(|line| line.trim() == module) {
                wrong.push(format!("{name}: tests/main.rs leaves out {file}"));
            }
        }
    }
    assert_eq!(wrong, Vec::<String>::new());
}

/// The files of the manifest's `[[test]]`s other than `tests/main.rs`:
/// each its `path`, or `tests/<name>.rs` without one.
fn apart(manifest: &str) -> Vec<String> {
    let mut apart = Vec::new();
    let (mut in_test, mut name, mut path) = (false, None, None);
    let mut end = |name: &mut Option<String>, path: &mut Option<String>| {
        let file = path.take().or(name.take().map(|n| format!("tests/{n}.rs")));
        if let Some(file) = file
            && let Some(file) = file.strip_prefix("tests/")
            && file != "main.rs"
        {
            apart.push(file.to_owned());
        }
        *name = None;
    };
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            if in_test {
                end(&mut name, &mut path);
            }
            in_test = line == "[[test]]";
            continue;
        }
        if !in_test {
            continue;
        }
        let value = |key: &str| {
            line.strip_prefix(key)
                .and_then(|rest| rest.trim_start().strip_prefix('='))
                .map(|rest| rest.trim().trim_matches('"').to_owned())
        };
        if let Some(found) = value("name") {
            name = Some(found);
        } else if let Some(found) = value("path") {
            path = Some(found);
        }
    }
    if in_test {
        end(&mut name, &mut path);
    }
    apart
}
