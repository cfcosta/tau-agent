//! Claude Code's memory: `projects/<folder>/memory/*.md`, one note per
//! file with YAML front matter (`name`, `description`, and `type` under
//! `metadata` or at the top: `user`, `feedback`, `project` or
//! `reference`), beside a `MEMORY.md` index, which is skipped; and its
//! chats, `projects/<folder>/*.jsonl`, one event per line.
//!
//! `<folder>` is the working directory with every character but a
//! letter or a digit written `-`, so `-` in a name reads the same as
//! `/`. The folder is the `cwd` a chat recorded, else the existing path
//! the name spells ([`decode`]).

use std::{
    fs,
    io::{BufRead as _, BufReader, Read as _, Seek as _, SeekFrom},
    path::{Path, PathBuf},
};

use serde_json::Value;
use tau_memory::{
    import::{Material, Tool},
    note::NoteType,
};

use super::{
    Fingerprint,
    Item,
    Read,
    ReadError,
    Remotes,
    SavedNote,
    Scan,
    cut,
    day,
};

/// The index Claude Code keeps beside its notes: not a note.
pub const INDEX: &str = "MEMORY.md";

/// How much of a tool call's arguments, and of its result, a chat shows
/// the model.
const CALL_CHARS: usize = 300;
const RESULT_CHARS: usize = 400;

/// The name Claude Code gives the directory of `path`'s chats.
pub fn encode(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// The existing directory whose [`encode`] is `name`, found by walking
/// from `/` along directories whose names fit; `None` when none does.
pub fn decode(name: &str) -> Option<PathBuf> {
    walk(Path::new("/"), name)
}

fn walk(dir: &Path, rest: &str) -> Option<PathBuf> {
    if rest.is_empty() {
        return Some(dir.to_owned());
    }
    // Every component starts with what `/` became.
    let rest = rest.strip_prefix('-')?;
    // The names that fit, the longest first: `tau-agent` before `tau`.
    let mut fits: Vec<(String, usize)> = fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let encoded = encode(Path::new(&name));
            let after = rest.strip_prefix(encoded.as_str())?;
            (after.is_empty() || after.starts_with('-'))
                .then_some((name, encoded.len()))
        })
        .collect();
    fits.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    fits.into_iter().find_map(|(name, len)| {
        let path = dir.join(&name);
        if path.is_dir() {
            walk(&path, &rest[len..])
        } else {
            None
        }
    })
}

/// Every note and chat under `root` (`~/.claude`).
pub fn scan(root: &Path, remotes: &mut Remotes) -> Scan {
    let mut scan = Scan::default();
    let Ok(entries) = fs::read_dir(root.join("projects")) else {
        return scan;
    };
    let mut projects: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    projects.sort();
    for project in projects {
        let name = project
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut chats: Vec<(PathBuf, Head)> = files(&project, "jsonl")
            .into_iter()
            .map(|path| {
                let head = head(&path);
                (path, head)
            })
            .collect();
        chats.sort_by(|a, b| a.0.cmp(&b.0));
        let folder = chats
            .iter()
            .find_map(|(_, head)| head.cwd.clone())
            .map(PathBuf::from)
            .or_else(|| decode(&name));
        let place = folder.map(|folder| remotes.place(&folder));
        for path in files(&project.join("memory"), "md") {
            if path.file_name().is_some_and(|file| file == INDEX) {
                continue;
            }
            let read = fs::read_to_string(&path)
                .map_err(ReadError::from)
                .and_then(|text| parse_note(&text, &path));
            let (Ok(note), Ok((fingerprint, size))) =
                (read, Fingerprint::of_file(&path))
            else {
                scan.failed.push(path);
                continue;
            };
            scan.items.push(Item {
                tool: Tool::ClaudeCode,
                material: Material::Saved,
                key: path.display().to_string(),
                path,
                place: place.clone(),
                about_you: note.about_you,
                fingerprint,
                size,
                session: None,
                date: None,
                section: None,
            });
        }
        for (path, head) in chats {
            let Ok((fingerprint, size)) = Fingerprint::of_file(&path) else {
                scan.failed.push(path);
                continue;
            };
            scan.items.push(Item {
                tool: Tool::ClaudeCode,
                material: Material::Chat,
                key: path.display().to_string(),
                session: path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned()),
                path,
                place: place.clone(),
                about_you: false,
                fingerprint,
                size,
                date: head.date,
                section: None,
            });
        }
    }
    scan
}

/// The files in `dir` ending in `.ext`, sorted.
fn files(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file() && path.extension().is_some_and(|e| e == ext)
        })
        .collect();
    found.sort();
    found
}

/// What the start of a chat says: where it ran, and when.
#[derive(Debug, Default)]
struct Head {
    cwd: Option<String>,
    date: Option<String>,
}

/// How many of a chat's first lines are looked at for [`Head`].
const HEAD_LINES: usize = 64;

fn head(path: &Path) -> Head {
    let mut head = Head::default();
    let Ok(file) = fs::File::open(path) else {
        return head;
    };
    for line in BufReader::new(file).lines().take(HEAD_LINES) {
        let Ok(line) = line else { break };
        let Ok(event) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if head.cwd.is_none() {
            head.cwd = event["cwd"].as_str().map(str::to_owned);
        }
        if head.date.is_none() {
            head.date = event["timestamp"].as_str().and_then(day);
        }
        if head.cwd.is_some() && head.date.is_some() {
            break;
        }
    }
    head
}

/// A memory note file: its front matter between `---` lines, then its
/// body. `path` names it when it has no `name`.
pub fn parse_note(text: &str, path: &Path) -> Result<SavedNote, ReadError> {
    let text = text.replace("\r\n", "\n");
    let rest = text
        .strip_prefix("---\n")
        .ok_or_else(|| ReadError::Format("no front matter".into()))?;
    let (front, body) = match rest.find("\n---") {
        Some(at) => {
            let after = &rest[at + 4..];
            let body = after.split_once('\n').map_or("", |(_, body)| body);
            (&rest[..at], body)
        }
        None if rest.starts_with("---") => ("", rest.trim_start_matches('-')),
        None => {
            return Err(ReadError::Format("front matter never ends".into()));
        }
    };
    let fields = front_matter(front);
    let get = |key: &str| {
        fields
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.is_empty())
    };
    let kind = get("metadata.type").or_else(|| get("type")).unwrap_or("");
    let about_you = matches!(kind, "user" | "feedback");
    let body = body.trim().to_owned();
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().replace(['-', '_'], " "))
        .unwrap_or_default();
    let title = one_line(get("name").unwrap_or(&stem));
    if title.is_empty() {
        return Err(ReadError::Format("no name".into()));
    }
    let description = get("description")
        .map(one_line)
        .filter(|line| !line.is_empty())
        .or_else(|| body.lines().find(|l| !l.trim().is_empty()).map(one_line))
        .unwrap_or_else(|| title.clone());
    Ok(SavedNote {
        kind: match kind {
            "user" | "feedback" => NoteType::Preference,
            _ => NoteType::Fact,
        },
        title,
        description,
        body,
        about_you,
    })
}

/// `text` on one line, its runs of white space one space.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The keys of simple YAML front matter, a nested key as
/// `parent.child`, with their values unquoted; a block scalar (`|`,
/// `>`) is its lines joined.
fn front_matter(front: &str) -> Vec<(String, String)> {
    let mut fields: Vec<(String, String)> = Vec::new();
    let mut parent: Option<String> = None;
    let mut block: Option<(String, usize)> = None;
    for line in front.lines() {
        let indent = line.len() - line.trim_start().len();
        if let Some((key, at)) = &block {
            if indent > *at && !line.trim().is_empty() {
                let field = fields
                    .iter_mut()
                    .find(|(k, _)| k == key)
                    .expect("the block's key is kept");
                if !field.1.is_empty() {
                    field.1.push(' ');
                }
                field.1.push_str(line.trim());
                continue;
            }
            block = None;
        }
        let Some((key, value)) = line.trim().split_once(':') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || key.starts_with('#') || key.starts_with('-') {
            continue;
        }
        let value = value.trim();
        let full = match (&parent, indent) {
            (Some(parent), 1..) => format!("{parent}.{key}"),
            _ => key.to_owned(),
        };
        if indent == 0 {
            parent = value.is_empty().then(|| key.to_owned());
        }
        if value.starts_with('|') || value.starts_with('>') {
            block = Some((full.clone(), indent));
            fields.push((full, String::new()));
            continue;
        }
        let value = unquote(value);
        fields.push((full, value));
    }
    fields
}

fn unquote(value: &str) -> String {
    let quoted = |q: char| {
        value.len() >= 2 && value.starts_with(q) && value.ends_with(q)
    };
    if quoted('"') {
        value[1..value.len() - 1].replace("\\\"", "\"")
    } else if quoted('\'') {
        value[1..value.len() - 1].replace("''", "'")
    } else {
        value.to_owned()
    }
}

/// A chat's events from `offset` on, as text for the model: what the
/// person and the agent said, the calls it made and the start of what
/// they returned. Only whole lines are read; the offset returned is
/// after the last.
pub fn read_chat(path: &Path, offset: u64) -> Result<Read, ReadError> {
    let (lines, offset) = lines_from(path, offset)?;
    let mut text = String::new();
    let mut events = 0;
    for line in lines.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        events += 1;
        if event["isSidechain"] == true || event["isMeta"] == true {
            continue;
        }
        let content = &event["message"]["content"];
        match event["type"].as_str() {
            Some("user") => match content {
                Value::String(said) => said_by(&mut text, "user", said),
                Value::Array(blocks) => {
                    for block in blocks {
                        match block["type"].as_str() {
                            Some("text") => said_by(
                                &mut text,
                                "user",
                                block["text"].as_str().unwrap_or(""),
                            ),
                            Some("tool_result") => {
                                let result = match &block["content"] {
                                    Value::String(result) => result.clone(),
                                    Value::Array(parts) => parts
                                        .iter()
                                        .filter_map(|part| {
                                            part["text"].as_str()
                                        })
                                        .collect::<Vec<_>>()
                                        .join("\n"),
                                    _ => String::new(),
                                };
                                text.push_str(&format!(
                                    "[result]\n{}\n\n",
                                    cut(result.trim(), RESULT_CHARS)
                                ));
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            },
            Some("assistant") => {
                for block in content.as_array().into_iter().flatten() {
                    match block["type"].as_str() {
                        Some("text") => said_by(
                            &mut text,
                            "assistant",
                            block["text"].as_str().unwrap_or(""),
                        ),
                        Some("tool_use") => text.push_str(&format!(
                            "[call {}] {}\n\n",
                            block["name"].as_str().unwrap_or("tool"),
                            cut(&block["input"].to_string(), CALL_CHARS)
                        )),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    if events == 0 && !lines.trim().is_empty() {
        return Err(ReadError::Format("not a Claude Code chat".into()));
    }
    Ok(Read::Text { text, offset })
}

/// Adds what `who` said, without the reminders tools add to it.
pub(super) fn said_by(text: &mut String, who: &str, said: &str) {
    let said = without_reminders(said);
    let said = said.trim();
    if !said.is_empty() {
        text.push_str(&format!("[{who}]\n{said}\n\n"));
    }
}

/// `text` without `<system-reminder>` blocks.
fn without_reminders(text: &str) -> String {
    const OPEN: &str = "<system-reminder>";
    const CLOSE: &str = "</system-reminder>";
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find(OPEN) {
        out.push_str(&rest[..at]);
        match rest[at..].find(CLOSE) {
            Some(end) => rest = &rest[at + end + CLOSE.len()..],
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// The whole lines of `path` from `offset`, and the offset after them.
pub(super) fn lines_from(
    path: &Path,
    offset: u64,
) -> Result<(String, u64), ReadError> {
    let mut file = fs::File::open(path)?;
    let size = file.metadata()?.len();
    // A file that shrank was written again: read it all.
    let offset = if offset > size { 0 } else { offset };
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let whole = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |at| at + 1);
    bytes.truncate(whole);
    Ok((
        String::from_utf8_lossy(&bytes).into_owned(),
        offset + whole as u64,
    ))
}
