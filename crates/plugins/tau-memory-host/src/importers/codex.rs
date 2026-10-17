//! Codex's memory, `memories/`: `MEMORY.md`, grouped by working
//! directory under `# Task Group: <cwd> — <what>` headings; a summary
//! of the person, `memory_summary.md`; a summary per task,
//! `rollout_summaries/*.md`, each starting with `key: value` lines
//! (`thread_id`, `updated_at`, `cwd`); and `raw_memories.md`, a section
//! per thread (`## Thread `<id>``). A task summary `MEMORY.md` lists, and
//! a raw thread that has a task summary, are read once, as what holds
//! them. Its chats are `sessions/**/*.jsonl`, whose first line, the
//! `session_meta`, says where and when it ran.
//!
//! Codex can import Claude Code's chats itself
//! (`external_agent_session_imports.json`): those copies are skipped, as
//! Claude Code's own are read. So are sessions a sub-agent ran.

use std::{
    collections::HashSet,
    fs,
    io::{BufRead as _, BufReader},
    path::{Path, PathBuf},
};

use serde_json::Value;
use tau_memory::import::{Material, Tool};

use super::{
    Fingerprint,
    Item,
    Read,
    ReadError,
    Remotes,
    Scan,
    claude::{lines_from, said_by},
    cut,
    day,
};

/// Codex's record of the Claude Code chats it imported.
pub const IMPORTS: &str = "external_agent_session_imports.json";

const CALL_CHARS: usize = 300;
const RESULT_CHARS: usize = 400;

/// Everything under `root` (`~/.codex`).
pub fn scan(root: &Path, remotes: &mut Remotes) -> Scan {
    let mut scan = Scan::default();
    memories(&root.join("memories"), remotes, &mut scan);
    let imported = imported(root);
    let mut sessions = Vec::new();
    jsonl_under(&root.join("sessions"), &mut sessions);
    sessions.sort();
    for path in sessions {
        let Ok((fingerprint, size)) = Fingerprint::of_file(&path) else {
            scan.failed.push(path);
            continue;
        };
        let Some(meta) = session_meta(&path) else {
            scan.failed.push(path);
            continue;
        };
        if meta.subagent
            || meta.id.as_ref().is_some_and(|id| imported.contains(id))
        {
            continue;
        }
        scan.items.push(Item {
            tool: Tool::Codex,
            material: Material::Chat,
            key: path.display().to_string(),
            path,
            place: meta.cwd.map(|cwd| remotes.place(Path::new(&cwd))),
            about_you: false,
            fingerprint,
            size,
            session: meta.id,
            date: meta.date,
            section: None,
        });
    }
    scan
}

/// The ids of the sessions Codex made of Claude Code's chats.
fn imported(root: &Path) -> HashSet<String> {
    fs::read_to_string(root.join(IMPORTS))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .map(|imports| {
            imports["records"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|record| record["imported_thread_id"].as_str())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn jsonl_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            jsonl_under(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "jsonl") {
            out.push(path);
        }
    }
}

/// What a session's first line says.
struct Meta {
    id: Option<String>,
    cwd: Option<String>,
    date: Option<String>,
    subagent: bool,
}

fn session_meta(path: &Path) -> Option<Meta> {
    let file = fs::File::open(path).ok()?;
    let line = BufReader::new(file).lines().next()?.ok()?;
    let event: Value = serde_json::from_str(&line).ok()?;
    if event["type"] != "session_meta" {
        return None;
    }
    let meta = &event["payload"];
    Some(Meta {
        id: meta["id"].as_str().map(str::to_owned),
        cwd: meta["cwd"].as_str().map(str::to_owned),
        date: meta["timestamp"]
            .as_str()
            .or(event["timestamp"].as_str())
            .and_then(day),
        subagent: meta["thread_source"] == "subagent"
            || meta["source"].get("subagent").is_some(),
    })
}

/// The items of the memory folder.
fn memories(dir: &Path, remotes: &mut Remotes, scan: &mut Scan) {
    let index = dir.join("MEMORY.md");
    let mut listed: HashSet<String> = HashSet::new();
    if let Ok(text) = fs::read_to_string(&index) {
        for (n, (heading, section)) in
            sections(&text, "# Task Group:").into_iter().enumerate()
        {
            for word in section.split_whitespace() {
                if let Some(at) = word.find("rollout_summaries/") {
                    let name = &word[at + "rollout_summaries/".len()..];
                    let name = name.trim_end_matches([',', ')', ';']);
                    listed.insert(name.to_owned());
                }
            }
            let cwd = heading
                .split(" — ")
                .next()
                .unwrap_or(&heading)
                .trim()
                .to_owned();
            scan.items.push(Item {
                tool: Tool::Codex,
                material: Material::Saved,
                key: format!("{}#{n}", index.display()),
                path: index.clone(),
                place: cwd
                    .starts_with('/')
                    .then(|| remotes.place(Path::new(&cwd))),
                about_you: false,
                fingerprint: Fingerprint::of_text(&section),
                size: section.len() as u64,
                session: None,
                date: None,
                section: Some(section),
            });
        }
    }
    let summary = dir.join("memory_summary.md");
    if let Ok((fingerprint, size)) = Fingerprint::of_file(&summary) {
        scan.items.push(Item {
            tool: Tool::Codex,
            material: Material::Saved,
            key: summary.display().to_string(),
            path: summary,
            place: None,
            about_you: true,
            fingerprint,
            size,
            session: None,
            date: None,
            section: None,
        });
    }
    let mut threads: HashSet<String> = HashSet::new();
    let mut rollouts: Vec<PathBuf> =
        fs::read_dir(dir.join("rollout_summaries"))
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
            .collect();
    rollouts.sort();
    for path in rollouts {
        let Ok(text) = fs::read_to_string(&path) else {
            scan.failed.push(path);
            continue;
        };
        let header = header(&text);
        if let Some(thread) = header.get("thread_id") {
            threads.insert(thread.clone());
        }
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if listed.contains(&name) {
            continue;
        }
        let Ok((fingerprint, size)) = Fingerprint::of_file(&path) else {
            scan.failed.push(path);
            continue;
        };
        scan.items.push(Item {
            tool: Tool::Codex,
            material: Material::Saved,
            key: path.display().to_string(),
            place: header.get("cwd").map(|cwd| remotes.place(Path::new(cwd))),
            about_you: false,
            fingerprint,
            size,
            session: header.get("thread_id").cloned(),
            date: header.get("updated_at").and_then(|at| day(at)),
            section: None,
            path,
        });
    }
    let raw = dir.join("raw_memories.md");
    if let Ok(text) = fs::read_to_string(&raw) {
        for (heading, section) in sections(&text, "## Thread") {
            let thread = heading.trim().trim_matches('`').to_owned();
            if threads.contains(&thread) {
                continue;
            }
            let header =
                header(section.split_once('\n').map_or("", |(_, rest)| rest));
            scan.items.push(Item {
                tool: Tool::Codex,
                material: Material::Saved,
                key: format!("{}#{thread}", raw.display()),
                path: raw.clone(),
                place: header
                    .get("cwd")
                    .map(|cwd| remotes.place(Path::new(cwd))),
                about_you: false,
                fingerprint: Fingerprint::of_text(&section),
                size: section.len() as u64,
                session: Some(thread),
                date: header.get("updated_at").and_then(|at| day(at)),
                section: Some(section),
            });
        }
    }
}

/// The sections of `text` under lines starting with `marker`: what
/// follows the marker on its line, and the section, that line included.
fn sections(text: &str, marker: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        if let Some(heading) = line.strip_prefix(marker) {
            out.push((heading.trim().to_owned(), String::new()));
        }
        if let Some((_, section)) = out.last_mut() {
            section.push_str(line);
            section.push('\n');
        }
    }
    out
}

/// The `key: value` lines at the start of `text`, up to the first line
/// that is not one.
fn header(text: &str) -> std::collections::HashMap<String, String> {
    let mut found = std::collections::HashMap::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            break;
        };
        let key = key.trim();
        if key.is_empty()
            || !key.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
        {
            break;
        }
        found.insert(key.to_owned(), value.trim().to_owned());
    }
    found
}

/// A session from `offset` on, as text for the model.
pub fn read_chat(path: &Path, offset: u64) -> Result<Read, ReadError> {
    let (lines, offset) = lines_from(path, offset)?;
    let mut text = String::new();
    let mut events = 0;
    for line in lines.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        events += 1;
        if event["type"] != "response_item" {
            continue;
        }
        let item = &event["payload"];
        match item["type"].as_str() {
            Some("message") => {
                let who = match item["role"].as_str() {
                    Some("user") => "user",
                    Some("assistant") => "assistant",
                    _ => continue,
                };
                for part in item["content"].as_array().into_iter().flatten() {
                    let said = part["text"].as_str().unwrap_or("");
                    if who == "user" && injected(said) {
                        continue;
                    }
                    said_by(&mut text, who, said);
                }
            }
            Some("function_call") => text.push_str(&format!(
                "[call {}] {}\n\n",
                item["name"].as_str().unwrap_or("tool"),
                cut(item["arguments"].as_str().unwrap_or(""), CALL_CHARS)
            )),
            Some("custom_tool_call") => text.push_str(&format!(
                "[call {}] {}\n\n",
                item["name"].as_str().unwrap_or("tool"),
                cut(item["input"].as_str().unwrap_or(""), CALL_CHARS)
            )),
            Some("function_call_output" | "custom_tool_call_output") => {
                let output = match &item["output"] {
                    Value::String(output) => output.clone(),
                    other => other.to_string(),
                };
                text.push_str(&format!(
                    "[result]\n{}\n\n",
                    cut(output.trim(), RESULT_CHARS)
                ));
            }
            _ => {}
        }
    }
    if events == 0 && !lines.trim().is_empty() {
        return Err(ReadError::Format("not a Codex session".into()));
    }
    Ok(Read::Text { text, offset })
}

/// Whether a user message is what Codex put in for the person: its
/// environment, the instructions files, permissions.
fn injected(text: &str) -> bool {
    let text = text.trim_start();
    if text.starts_with("# AGENTS.md") {
        return true;
    }
    let Some(tag) = text.strip_prefix('<') else {
        return false;
    };
    tag.split('>').next().is_some_and(|name| {
        !name.is_empty()
            && name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
    })
}
