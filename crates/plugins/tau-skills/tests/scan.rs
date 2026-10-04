//! Reading the skills folder against what was written to it: every
//! folder is a skill or a problem, never both and never lost; a skill
//! has the name and description its frontmatter gives, however the YAML
//! writes them; and the instructions list exactly the skills.

use std::{collections::BTreeSet, fs, path::Path};

use hegel::{TestCase, generators as gs};
use tau_skills::{
    SKILL_FILE,
    scan::{self, HEADING},
};

/// One folder as a test writes it.
#[derive(Debug, Clone)]
enum Folder {
    /// A skill: its name in the frontmatter or only as the folder's, its
    /// description written as `style` says, and other keys.
    Skill {
        name: String,
        named: bool,
        description: String,
        style: u8,
        other: Vec<String>,
    },
    /// A `SKILL.md` with no description.
    NoDescription,
    /// No `SKILL.md` at all.
    Empty,
    /// A name a skill cannot have.
    BadName,
}

hegel::pretty_print_as_debug!(Folder);

fn name(tc: &TestCase) -> String {
    tc.draw(
        gs::text()
            .alphabet("abcdefghijklmnopqrstuvwxyz0123456789-")
            .min_size(1)
            .max_size(12),
    )
    .trim_matches('-')
    .to_owned()
}

#[hegel::composite]
fn folder(tc: &hegel::TestCase) -> Folder {
    match tc.draw(gs::integers::<u8>().max_value(5)) {
        0 => Folder::NoDescription,
        1 => Folder::Empty,
        2 => Folder::BadName,
        _ => Folder::Skill {
            name: name(tc),
            named: tc.draw(gs::booleans()),
            // Words, colons, quotes and lines: what YAML must escape.
            description: tc.draw(
                gs::text()
                    .alphabet("ab :#'\"\n-")
                    .min_size(1)
                    .max_size(40),
            ),
            style: tc.draw(gs::integers::<u8>().max_value(2)),
            other: tc.draw(
                gs::vecs(gs::sampled_from(vec![
                    "allowed-tools".to_owned(),
                    "license".to_owned(),
                    "metadata".to_owned(),
                ]))
                .unique(true)
                .max_size(3),
            ),
        },
    }
}

/// The frontmatter's description in one of three YAML styles: a
/// literal block, a folded block, or a quoted string.
fn yaml_description(text: &str, style: u8) -> String {
    // A block's lines at one indent, blank ones left out: the
    // description reads the same, its white space collapsed.
    let indented = |marker: &str| {
        let lines: Vec<String> = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| format!("  {line}"))
            .collect();
        format!("description: {marker}\n{}\n", lines.join("\n"))
    };
    match style {
        0 => indented("|"),
        1 => indented(">"),
        _ => format!(
            "description: {}\n",
            serde_json::to_string(text).unwrap()
        ),
    }
}

fn write(dir: &Path, folder_name: &str, folder: &Folder) {
    let path = dir.join(folder_name);
    fs::create_dir_all(&path).unwrap();
    let front = match folder {
        Folder::Empty => return,
        Folder::NoDescription => "name: quiet\n".to_owned(),
        Folder::BadName => "name: Not A Name\ndescription: x\n".to_owned(),
        Folder::Skill {
            name,
            named,
            description,
            style,
            other,
        } => {
            let mut front = String::new();
            if *named {
                // Quoted: unquoted, YAML reads some names as other
                // things (a test below pins how).
                let quoted = serde_json::to_string(name).unwrap();
                front.push_str(&format!("name: {quoted}\n"));
            }
            front.push_str(&yaml_description(description, *style));
            for key in other {
                front.push_str(&format!("{key}: [read, bash]\n"));
            }
            front
        }
    };
    fs::write(
        path.join(SKILL_FILE),
        format!("---\n{front}---\n\n# Steps\n\nDo it.\n"),
    )
    .unwrap();
}

#[hegel::test(test_cases = 200)]
fn every_folder_is_a_skill_or_a_problem(tc: TestCase) {
    let folders: Vec<Folder> =
        tc.draw(gs::vecs(folder()).max_size(6));
    let home = tempfile::tempdir().unwrap();
    let mut expected = Vec::new();
    let mut used = BTreeSet::new();
    for (n, folder) in folders.iter().enumerate() {
        // Unnamed skills take their folder's name; folders are unique.
        let folder_name = match folder {
            Folder::Skill {
                name, named: false, ..
            } if !name.is_empty() && !used.contains(name) => name.clone(),
            _ => format!("folder-{n}"),
        };
        used.insert(folder_name.clone());
        write(home.path(), &folder_name, folder);
        expected.push((folder_name, folder.clone()));
    }
    let skills = scan::scan(home.path());

    // Each folder once, as a skill or a problem.
    let found: BTreeSet<_> =
        skills.found.iter().map(|skill| skill.dir.clone()).collect();
    let problems: BTreeSet<_> =
        skills.problems.iter().map(|problem| problem.dir.clone()).collect();
    assert!(found.is_disjoint(&problems));
    assert_eq!(found.len() + problems.len(), folders.len());

    // Of skills with one name, the folder first by path is the one.
    expected.sort_by(|a, b| a.0.cmp(&b.0));
    let mut names = BTreeSet::new();
    for (folder_name, folder) in &expected {
        let dir = home.path().join(folder_name);
        let skill = skills.found.iter().find(|skill| skill.dir == dir);
        match folder {
            Folder::Skill {
                name,
                named,
                description,
                other,
                ..
            } => {
                let want = if *named { name.clone() } else { folder_name.clone() };
                let valid = scan::check_name(&want).is_ok();
                let words = scan::one_line(description);
                // Only a skill claims its name.
                let fresh = valid && !words.is_empty() && names.insert(want.clone());
                if !fresh {
                    assert!(skill.is_none(), "{folder_name}: {folder:?}");
                    continue;
                }
                let skill = skill.unwrap_or_else(|| {
                    panic!("{folder_name} is not a skill: {:?}", skills.problems)
                });
                assert_eq!(skill.name, want);
                assert_eq!(skill.description, words);
                assert_eq!(&skill.ignored, other);
                assert_eq!(skill.files, 1);
            }
            _ => assert!(skill.is_none(), "{folder_name}: {folder:?}"),
        }
    }

    // The instructions list exactly the skills, a line each.
    match scan::section(&skills) {
        None => assert!(skills.found.is_empty()),
        Some(section) => {
            assert!(section.starts_with(HEADING));
            let lines: Vec<&str> =
                section.lines().filter(|line| line.starts_with("- ")).collect();
            assert_eq!(lines.len(), skills.found.len());
            for skill in &skills.found {
                let line = format!("- {}: {}", skill.name, skill.description);
                assert!(lines.contains(&line.as_str()), "{line}");
            }
        }
    }
}

/// Frontmatter is the file's start, between `---` lines, whatever the
/// line ends; anything else is not a skill's.
#[test]
fn frontmatter_is_found_at_the_start_only() {
    let (front, body) = scan::split("---\nname: a\n---\nbody\n").unwrap();
    assert_eq!((front, body), ("name: a\n", "body\n"));
    let (front, body) = scan::split("---\r\nname: a\r\n---\r\nbody").unwrap();
    assert_eq!((front, body), ("name: a\r\n", "body"));
    assert_eq!(scan::split("\u{feff}---\nx: 1\n---").unwrap().1, "");
    assert!(scan::split("# no frontmatter\n").is_err());
    assert!(scan::split("---\nname: a\n").is_err());
}

/// A missing folder holds no skills, and says nothing is wrong.
#[test]
fn a_missing_folder_holds_nothing() {
    let home = tempfile::tempdir().unwrap();
    let skills = scan::scan(&home.path().join("nowhere"));
    assert!(skills.found.is_empty() && skills.problems.is_empty());
    assert_eq!(scan::section(&skills), None);
}

/// Unquoted, YAML reads a name like `404` as a number and `true` as a
/// flag: they are their text. `null` and `~` are no name, so the folder
/// names the skill.
#[test]
fn unquoted_names_read_as_written() {
    let home = tempfile::tempdir().unwrap();
    for (folder, name) in
        [("a", "404"), ("b", "true"), ("c", "null"), ("d", "~")]
    {
        fs::create_dir_all(home.path().join(folder)).unwrap();
        fs::write(
            home.path().join(folder).join(SKILL_FILE),
            format!("---\nname: {name}\ndescription: x\n---\n"),
        )
        .unwrap();
    }
    let skills = scan::scan(home.path());
    let names: Vec<&str> =
        skills.found.iter().map(|skill| skill.name.as_str()).collect();
    assert_eq!(names, ["404", "c", "d", "true"], "{:?}", skills.problems);
}
