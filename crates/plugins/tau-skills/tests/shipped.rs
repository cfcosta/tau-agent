//! The skills tau-skills ships, as the model gets them: installed into
//! tau's data directory, each one a skill the scan offers, every skill
//! and reference file they point to there, and an install that leaves
//! only what this build ships.

use std::{collections::BTreeSet, fs, path::Path};

use tau_skills::{
    SKILL_FILE,
    scan,
    shipped::{self, MANIFEST, SKILLS},
};

/// Skills a shipped one may mention without tau shipping them: the
/// person's own, used when the run has them.
const OPTIONAL: [&str; 2] = ["diataxis", "asd-ste100"];

fn installed() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    shipped::install(dir.path()).unwrap();
    dir
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
            if scan::check_name(name).is_ok() {
                names.push(name.to_owned());
            }
        }
        rest = &rest[at + 1..];
    }
    names
}

#[test]
fn every_shipped_skill_is_offered_and_none_is_a_problem() {
    let dir = installed();
    let skills = scan::scan(dir.path());
    assert!(skills.problems.is_empty(), "{:?}", skills.problems);
    let found: Vec<&str> =
        skills.found.iter().map(|s| s.name.as_str()).collect();
    let shipped: Vec<&str> = SKILLS.iter().map(|(name, _)| *name).collect();
    assert_eq!(found, shipped);
    assert!(shipped.len() > 40, "{shipped:?}");
    for skill in &skills.found {
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
    let dir = installed();
    let shipped: BTreeSet<&str> =
        SKILLS.iter().map(|(name, _)| *name).collect();
    let mut missing = Vec::new();
    for (name, files) in SKILLS {
        for (path, text) in *files {
            for named in skills_named(text) {
                if !shipped.contains(named.as_str())
                    && !OPTIONAL.contains(&named.as_str())
                {
                    missing.push(format!(
                        "{name}/{path} names the `{named}` skill"
                    ));
                }
            }
            let mut rest = *text;
            while let Some(at) = rest.find("references/") {
                let tail = &rest[at..];
                let end = tail
                    .find(|c: char| {
                        !(c.is_ascii_alphanumeric() || "-_./".contains(c))
                    })
                    .unwrap_or(tail.len());
                let reference = tail[..end].trim_end_matches('.');
                if !dir.path().join(name).join(reference).exists() {
                    missing
                        .push(format!("{name}/{path} points to {reference}"));
                }
                rest = &tail[end..];
            }
        }
        assert!(files.iter().any(|(path, _)| *path == SKILL_FILE), "{name}");
    }
    assert!(missing.is_empty(), "not shipped:\n{}", missing.join("\n"));
}

#[test]
fn installing_again_changes_nothing() {
    let dir = installed();
    let once = tree(dir.path());
    shipped::install(dir.path()).unwrap();
    assert_eq!(tree(dir.path()), once);
}

/// A skill an earlier build shipped is removed; another plugin's, and
/// edits to a shipped one, do not survive or are not touched as theirs.
#[test]
fn an_install_leaves_only_what_this_build_ships() {
    let dir = tempfile::tempdir().unwrap();
    let (first, _) = SKILLS[0];
    for name in ["retired", "tau-agents", first] {
        fs::create_dir_all(dir.path().join(name)).unwrap();
        fs::write(dir.path().join(name).join("stale.md"), "old").unwrap();
    }
    fs::write(dir.path().join(MANIFEST), format!("retired\n{first}\n"))
        .unwrap();
    shipped::install(dir.path()).unwrap();
    assert!(!dir.path().join("retired").exists());
    assert!(dir.path().join("tau-agents/stale.md").exists());
    assert!(!dir.path().join(first).join("stale.md").exists());
    assert!(dir.path().join(first).join(SKILL_FILE).exists());
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
    for (name, files) in SKILLS {
        for (path, text) in *files {
            let text = text.to_lowercase();
            for word in FOREIGN.iter().filter(|word| text.contains(*word)) {
                found.push(format!("{name}/{path} says {word:?}"));
            }
        }
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
}
