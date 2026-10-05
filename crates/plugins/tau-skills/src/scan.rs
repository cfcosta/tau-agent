//! Reading the skills folder: each folder in it with a `SKILL.md`, its
//! frontmatter, and what a run's instructions say of them.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use serde_yaml_ng::Value;

use crate::{Problem, SKILL_FILE, Skill, Skills, TOOL};

/// A name the model loads a skill by: lower-case letters, digits and
/// hyphens, as other agents' skills are named, at most this long.
pub const NAME_LIMIT: usize = 64;

/// How much of a description the instructions take, in characters.
pub const DESCRIPTION_LIMIT: usize = 1024;

/// How many files a skill's folder is counted up to: more say "many".
pub const FILES_LIMIT: usize = 1000;

/// The heading the list goes under in a run's instructions.
pub const HEADING: &str = "# Skills";

/// What `SKILL.md`'s frontmatter says.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Front {
    pub name: Option<String>,
    pub description: Option<String>,
    /// The other keys, in their order.
    pub other: Vec<String>,
}

/// Reads `dir`'s skills: every folder in it, by name, with a `SKILL.md`
/// that has a description. A folder without one, or with a name another
/// skill has, is a [`Problem`]. A missing folder holds nothing.
pub fn scan(dir: &Path) -> Skills {
    let mut skills = Skills {
        dir: Some(dir.to_owned()),
        ..Skills::default()
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return skills;
    };
    let mut folders: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        // A link to a folder counts: skills are often linked in.
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.path())
        .collect();
    folders.sort();
    let mut names = BTreeSet::new();
    for folder in folders {
        match read(&folder) {
            Ok(skill) if names.insert(skill.name.clone()) => {
                skills.found.push(skill)
            }
            Ok(skill) => skills.problems.push(Problem {
                dir: folder,
                reason: format!(
                    "another skill is named {}; this one is not offered",
                    skill.name
                ),
            }),
            Err(reason) => skills.problems.push(Problem {
                dir: folder,
                reason,
            }),
        }
    }
    skills.found.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// The person's skills in `dir`, and those tau ships in `builtin`
/// ([`crate::BUILTIN_DIR`]) that the person has none of the same name
/// of.
pub fn scan_all(dir: Option<&Path>, builtin: &Path) -> Skills {
    let mut skills = dir.map(scan).unwrap_or_default();
    let shipped = scan(builtin);
    for skill in shipped.found {
        if skills.get(&skill.name).is_none() {
            skills.found.push(Skill {
                builtin: true,
                ..skill
            });
        }
    }
    skills.problems.extend(shipped.problems);
    skills.found.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// One skill's folder, or why it is not one.
fn read(folder: &Path) -> Result<Skill, String> {
    let file = folder.join(SKILL_FILE);
    let text = fs::read_to_string(&file)
        .map_err(|error| format!("cannot read {SKILL_FILE}: {error}"))?;
    let (front, _) = split(&text)?;
    let front = parse(front)?;
    let folder_name = folder
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = front.name.unwrap_or(folder_name);
    check_name(&name)?;
    let description = front
        .description
        .map(|text| one_line(&text))
        .filter(|text| !text.is_empty())
        .ok_or_else(|| {
            format!(
                "{SKILL_FILE} has no description in its frontmatter; the \
                 model picks skills by their description"
            )
        })?;
    Ok(Skill {
        name,
        description,
        dir: folder.to_owned(),
        files: count_files(folder),
        ignored: front.other,
        builtin: false,
    })
}

/// Whether `name` is one a skill can have.
pub fn check_name(name: &str) -> Result<(), String> {
    let fits = !name.is_empty()
        && name.len() <= NAME_LIMIT
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
        && !name.ends_with('-');
    if fits {
        Ok(())
    } else {
        Err(format!(
            "its name, {name:?}, is not lower-case letters, digits and \
             hyphens, up to {NAME_LIMIT}"
        ))
    }
}

/// `text` split into its frontmatter, between `---` lines at its start,
/// and the instructions after it.
pub fn split(text: &str) -> Result<(&str, &str), String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let no_front =
        || format!("{SKILL_FILE} does not start with frontmatter (---)");
    let rest = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
        .ok_or_else(no_front)?;
    let mut at = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return Ok((&rest[..at], &rest[at + line.len()..]));
        }
        at += line.len();
    }
    // A file that is all frontmatter has no instructions after it.
    if rest[at..].trim_end() == "---" {
        return Ok((&rest[..at], ""));
    }
    Err(format!("{SKILL_FILE}'s frontmatter has no closing ---"))
}

/// What the frontmatter `front` says: a YAML mapping.
pub fn parse(front: &str) -> Result<Front, String> {
    let value: Value = serde_yaml_ng::from_str(front)
        .map_err(|error| format!("its frontmatter is not YAML: {error}"))?;
    let Value::Mapping(map) = value else {
        return if value.is_null() {
            Ok(Front::default())
        } else {
            Err("its frontmatter is not a list of keys and values".into())
        };
    };
    let mut parsed = Front::default();
    for (key, value) in map {
        let Value::String(key) = key else { continue };
        match key.as_str() {
            "name" => parsed.name = text_of(&value, "name")?,
            "description" => {
                parsed.description = text_of(&value, "description")?
            }
            _ => parsed.other.push(key),
        }
    }
    Ok(parsed)
}

/// `value` as text: YAML reads `name: 404` as a number and `name: true`
/// as a flag, which are their text here; `name: null` or `~` is none
/// given.
fn text_of(value: &Value, key: &str) -> Result<Option<String>, String> {
    match value {
        Value::Null => Ok(None),
        Value::String(text) => Ok(Some(text.clone())),
        Value::Number(number) => Ok(Some(number.to_string())),
        Value::Bool(flag) => Ok(Some(flag.to_string())),
        _ => Err(format!("its {key} is not text")),
    }
}

/// `text` on one line, its runs of white space one space, cut to
/// [`DESCRIPTION_LIMIT`] characters.
pub fn one_line(text: &str) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match joined.char_indices().nth(DESCRIPTION_LIMIT) {
        Some((at, _)) => format!("{}…", &joined[..at]),
        None => joined,
    }
}

/// The files under `folder`, counted up to [`FILES_LIMIT`].
fn count_files(folder: &Path) -> usize {
    let mut count = 0;
    let mut todo = vec![folder.to_owned()];
    while let Some(dir) = todo.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                todo.push(entry.path());
            } else {
                count += 1;
                if count >= FILES_LIMIT {
                    return count;
                }
            }
        }
    }
    count
}

/// What a run's instructions say of `skills`: how to load one, then each
/// by name and description. None when there are none.
pub fn section(skills: &Skills) -> Option<String> {
    if skills.found.is_empty() {
        return None;
    }
    let mut text = format!(
        "{HEADING}\n\n\
         Skills are instructions for particular tasks, kept in folders \
         outside the workspace. When a task fits a skill's description, \
         call `{TOOL}` with its name before you start, and follow what it \
         says. Its folder may hold scripts and references it names: read \
         and run them from there. A message that starts with `/name` \
         asks for that skill: load it first, then do what the message \
         says after the name.\n"
    );
    for skill in &skills.found {
        text.push_str(&format!("\n- {}: {}", skill.name, skill.description));
    }
    Some(text)
}
