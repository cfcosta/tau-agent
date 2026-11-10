//! The skills tau-skills ships, in its package (ADR 0050, gap K): each
//! one a skill the plugin offers and none a problem, every skill and
//! reference file they point to there, and none tied to one language or
//! another harness.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

/// Skills a shipped one may mention without tau shipping them: the
/// person's own, used when the run has them.
const OPTIONAL: [&str; 2] = ["diataxis", "asd-ste100"];

/// The package, whose `skills/` holds what it ships.
fn package() -> PathBuf {
    tau_luau_plugins::packages::find()
        .unwrap()
        .join(tau_luau_plugins::names::SKILLS)
}

/// Each shipped skill: its folder's name, and its files by path, with
/// their text.
fn shipped() -> Vec<(String, Vec<(String, String)>)> {
    let dir = package().join("skills");
    let mut skills: Vec<(String, Vec<(String, String)>)> = fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            (name, tree(&entry.path()))
        })
        .collect();
    skills.sort();
    skills
}

/// Every file under `dir`, with its text, by path.
fn tree(dir: &Path) -> Vec<(String, String)> {
    let mut files = Vec::new();
    let mut todo = vec![dir.to_owned()];
    while let Some(next) = todo.pop() {
        for entry in fs::read_dir(next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                todo.push(path);
            } else {
                let name =
                    path.strip_prefix(dir).unwrap().display().to_string();
                files.push((name, fs::read_to_string(&path).unwrap()));
            }
        }
    }
    files.sort();
    files
}

/// Whether `name` is one a skill may have.
fn named_so(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
        && !name.ends_with('-')
}

/// The names between backticks right before " skill" in `text`: the
/// skills it tells the model to load.
fn skills_named(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("` skill") {
        let before = &rest[..at];
        if let Some(open) = before.rfind('`') {
            let name = &before[open + 1..];
            // A pattern such as `playbook-*` names a kind, not a skill.
            if named_so(name) {
                names.push(name.to_owned());
            }
        }
        rest = &rest[at + 1..];
    }
    names
}

#[test]
fn every_shipped_skill_is_offered_and_none_is_a_problem() {
    let skills = super::read(None, Some(&package()));
    assert!(skills.problems.is_empty(), "{:?}", skills.problems);
    let found: Vec<&str> =
        skills.found.iter().map(|s| s.name.as_str()).collect();
    let shipped: Vec<String> =
        shipped().into_iter().map(|(name, _)| name).collect();
    assert_eq!(found, shipped);
    assert!(shipped.len() > 40, "{shipped:?}");
    for skill in &skills.found {
        assert!(skill.builtin, "{}", skill.name);
        assert!(
            skill.description.len() <= 400,
            "{}: {}",
            skill.name,
            skill.description
        );
    }
}

#[test]
fn every_skill_and_reference_a_skill_names_is_there() {
    let skills = shipped();
    let names: BTreeSet<&str> =
        skills.iter().map(|(name, _)| name.as_str()).collect();
    let mut missing = Vec::new();
    for (name, files) in &skills {
        for (path, text) in files {
            for named in skills_named(text) {
                if !names.contains(named.as_str())
                    && !OPTIONAL.contains(&named.as_str())
                {
                    missing.push(format!(
                        "{name}/{path} names the `{named}` skill"
                    ));
                }
            }
            let mut rest = text.as_str();
            while let Some(at) = rest.find("references/") {
                let tail = &rest[at..];
                let end = tail
                    .find(|c: char| {
                        !(c.is_ascii_alphanumeric() || "-_./".contains(c))
                    })
                    .unwrap_or(tail.len());
                let reference = tail[..end].trim_end_matches('.');
                if !package().join("skills").join(name).join(reference).exists()
                {
                    missing
                        .push(format!("{name}/{path} points to {reference}"));
                }
                rest = &tail[end..];
            }
        }
        assert!(files.iter().any(|(path, _)| path == "SKILL.md"), "{name}");
    }
    assert!(missing.is_empty(), "not shipped:\n{}", missing.join("\n"));
}

/// Words that tie a shipped skill to one language's toolchain, or to
/// the harness the skills were adapted from. tau runs in any
/// repository, with its own tools: a skill names the kind of tool and
/// leaves the choice to the repository.
const FOREIGN: [&str; 24] = [
    "cargo",
    "clippy",
    "nextest",
    "rustc",
    "pub(crate)",
    "serde",
    "tokio",
    ".unwrap()",
    "#[",
    "eslint",
    "pytest",
    "jest",
    "npm ",
    "typescript",
    "py-spy",
    "cursor",
    "poteto-mode",
    "unslop",
    "todolist",
    "task tool",
    "/loop",
    "worktree",
    "subagent_type",
    "gh pr",
];

#[test]
fn no_shipped_skill_assumes_a_language_or_another_harness() {
    let mut found = Vec::new();
    for (name, files) in shipped() {
        for (path, text) in files {
            let text = text.to_lowercase();
            for word in FOREIGN.iter().filter(|word| text.contains(*word)) {
                found.push(format!("{name}/{path} says {word:?}"));
            }
        }
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
}
