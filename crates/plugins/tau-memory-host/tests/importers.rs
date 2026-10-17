//! Reading what Claude Code and Codex kept, from synthetic copies of
//! their files in a directory of the test's own: notes, memories, task
//! summaries and chats; the folder each was about, and the remote of
//! the repository there; what is skipped, and what does not read.

use std::{
    fs,
    path::{Path, PathBuf},
};

use hegel::{TestCase, generators as gs};
use serde_json::json;
use tau_memory::import::{Material, Place, Tool, normalize_remote};
use tau_memory_host::{
    importers::{
        self,
        Item,
        Read,
        Remotes,
        Roots,
        chunks,
        claude::{self, decode, encode, parse_note},
        codex,
        remote,
    },
    note::NoteType,
};

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn lines(events: &[serde_json::Value]) -> String {
    events
        .iter()
        .map(|event| format!("{event}\n"))
        .collect::<String>()
}

/// A Git checkout at `dir` whose origin is `url`.
fn git_checkout(dir: &Path, url: &str) {
    write(
        &dir.join(".git/config"),
        &format!(
            "[core]\n\tbare = false\n[remote \"upstream\"]\n\turl = https://example.com/other.git\n[remote \"origin\"]\n\turl = {url}\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n"
        ),
    );
}

fn items_of(items: &[Item], tool: Tool, material: Material) -> Vec<&Item> {
    items
        .iter()
        .filter(|item| item.tool == tool && item.material == material)
        .collect()
}

/// Every directory Claude Code names after a folder reads back as that
/// folder, while it exists, whatever letters, digits, dots, dashes and
/// underscores its names hold.
#[hegel::test(test_cases = 60)]
fn a_folder_name_decodes_to_the_folder(tc: TestCase) {
    let base = tempfile::tempdir().unwrap();
    let parts: Vec<String> = tc.draw(
        gs::vecs(
            gs::from_regex("[a-zA-Z0-9][a-zA-Z0-9._-]{0,8}").fullmatch(true),
        )
        .min_size(1)
        .max_size(4),
    );
    let mut path = base.path().canonicalize().unwrap();
    for part in &parts {
        path.push(part);
    }
    fs::create_dir_all(&path).unwrap();
    assert_eq!(decode(&encode(&path)), Some(path));
}

/// A folder that is gone decodes to nothing.
#[test]
fn a_folder_that_is_gone_decodes_to_nothing() {
    let base = tempfile::tempdir().unwrap();
    let gone = base.path().join("was-here").join("once");
    assert_eq!(decode(&encode(&gone)), None);
}

/// A note as Claude Code writes it, with its type under `metadata` or at
/// the top.
fn note_file(
    name: &str,
    description: &str,
    kind: &str,
    body: &str,
    nested: bool,
) -> String {
    let kind = if nested {
        format!(
            "metadata:\n  node_type: memory\n  type: {kind}\n  originSessionId: s-1\n"
        )
    } else {
        format!("type: {kind}\n")
    };
    format!(
        "---\nname: {name}\ndescription: {description}\n{kind}---\n\n{body}\n"
    )
}

/// Any well-formed note reads as itself: its name the title, its type
/// mapped (about the person: a preference; else a fact), its body kept.
#[hegel::test(test_cases = 100)]
fn any_well_formed_note_reads_as_itself(tc: TestCase) {
    let word = || {
        gs::from_regex("[A-Za-z0-9][A-Za-z0-9 ,.()/`-]{0,30}[A-Za-z0-9]")
            .fullmatch(true)
    };
    let name: String = tc.draw(word());
    let description: String = tc.draw(word());
    let body: String =
        tc.draw(gs::from_regex("[A-Za-z0-9 .,\n-]{1,120}").fullmatch(true));
    let kind: &str = tc.draw(gs::sampled_from(vec![
        "user",
        "feedback",
        "project",
        "reference",
    ]));
    let nested: bool = tc.draw(gs::booleans());
    let text = note_file(&name, &description, kind, &body, nested);
    let note = parse_note(&text, Path::new("memory/x.md")).unwrap();
    let one_line =
        |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    assert_eq!(note.title, one_line(&name));
    assert_eq!(note.description, one_line(&description));
    assert_eq!(note.body, body.trim());
    let about_you = matches!(kind, "user" | "feedback");
    assert_eq!(note.about_you, about_you);
    assert_eq!(
        note.kind,
        if about_you {
            NoteType::Preference
        } else {
            NoteType::Fact
        }
    );
}

/// A note without front matter does not read; one without a name takes
/// its file's; quoted and folded values read as their text.
#[test]
fn notes_read_what_they_can() {
    assert!(parse_note("just text\n", Path::new("a.md")).is_err());
    assert!(parse_note("---\nname: x\n", Path::new("a.md")).is_err());
    let note = parse_note(
        "---\ndescription: \"Quoted: with a colon\"\nmetadata:\n  type: project\n---\nBody.\n",
        Path::new("memory/keep-target-dirs_apart.md"),
    )
    .unwrap();
    assert_eq!(note.title, "keep target dirs apart");
    assert_eq!(note.description, "Quoted: with a colon");
    let folded = parse_note(
        "---\nname: Folded\ndescription: >-\n  one line\n  and another\ntype: reference\n---\nB\n",
        Path::new("a.md"),
    )
    .unwrap();
    assert_eq!(folded.description, "one line and another");
    assert_eq!(folded.kind, NoteType::Fact);
}

/// A home with Claude Code's files: two projects, one whose chats say
/// where it ran and one only its name says.
struct ClaudeHome {
    home: tempfile::TempDir,
    repo: PathBuf,
    other: PathBuf,
}

fn claude_home() -> ClaudeHome {
    let home = tempfile::tempdir().unwrap();
    let work = home.path().canonicalize().unwrap().join("work");
    let repo = work.join("shop-api");
    let other = work.join("notes.site");
    fs::create_dir_all(&repo).unwrap();
    fs::create_dir_all(&other).unwrap();
    git_checkout(&repo, "git@github.com:acme/shop-api.git");
    let projects = home.path().join(".claude/projects");
    // The directory is named after the folder the chats ran in.
    let first = projects.join(encode(&repo));
    write(
        &first.join("memory/run-tests-with-make.md"),
        &note_file(
            "Tests run with make check",
            "Not cargo test",
            "project",
            "Run `make check`.",
            true,
        ),
    );
    write(
        &first.join("memory/terse-answers.md"),
        &note_file(
            "Answers are short",
            "The person wants terse replies",
            "feedback",
            "Keep it short.",
            false,
        ),
    );
    write(
        &first.join("memory/MEMORY.md"),
        "- [Tests](run-tests-with-make.md)\n",
    );
    write(&first.join("memory/broken.md"), "no front matter here\n");
    write(&first.join("memory/data/table.tsv"), "a\tb\n");
    write(
        &first.join("0001.jsonl"),
        &lines(&[
            json!({"type": "permission-mode", "sessionId": "0001"}),
            json!({"type": "user", "cwd": repo.join("src"), "timestamp": "2026-10-04T10:00:00Z",
                   "message": {"role": "user", "content": "Why do the tests hang?"}}),
        ]),
    );
    // A sub-agent's chat sits deeper, and is not one of the folder's.
    write(&first.join("0001/subagents/a.jsonl"), "{}\n");
    // The other folder has notes and no chat: its name says where.
    write(
        &projects
            .join(encode(&other))
            .join("memory/site-builds-with-zola.md"),
        &note_file(
            "The site builds with zola",
            "zola build",
            "reference",
            "zola build",
            true,
        ),
    );
    ClaudeHome { home, repo, other }
}

/// A scan lists each note but the index, with the folder it is about and
/// that folder's remote; it names what does not read, and leaves out
/// what is not a note or a chat.
#[test]
fn claude_code_scan_finds_notes_and_chats_and_their_folders() {
    let fixture = claude_home();
    let roots = Roots::new(fixture.home.path());
    let scan = importers::scan(&roots);
    let saved = items_of(&scan.items, Tool::ClaudeCode, Material::Saved);
    let names: Vec<String> = saved
        .iter()
        .map(|item| {
            item.path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    // By folder: `notes.site` before `shop-api`.
    assert_eq!(
        names,
        [
            "site-builds-with-zola.md",
            "run-tests-with-make.md",
            "terse-answers.md"
        ]
    );
    let saved = [saved[1], saved[2], saved[0]];
    // The chat ran in a folder inside the repository; the project is
    // named after where it started.
    let place = saved[0].place.clone().unwrap();
    assert_eq!(
        place.remote.as_deref(),
        Some("git@github.com:acme/shop-api.git")
    );
    assert!(place.matches(Some(&fixture.repo), None));
    assert!(saved[1].about_you);
    assert!(!saved[0].about_you);
    assert_eq!(
        saved[2].place,
        Some(Place {
            folder: Some(fixture.other.display().to_string()),
            remote: None
        })
    );
    assert_eq!(scan.failed.len(), 1);
    assert!(scan.failed[0].ends_with("memory/broken.md"));
    let chats = items_of(&scan.items, Tool::ClaudeCode, Material::Chat);
    assert_eq!(chats.len(), 1);
    assert_eq!(chats[0].date.as_deref(), Some("2026-10-04"));
    assert_eq!(chats[0].session.as_deref(), Some("0001"));
    assert_eq!(
        roots.show(&chats[0].path),
        format!("~/.claude/projects/{}/0001.jsonl", encode(&fixture.repo))
    );
}

/// A chat reads as who said what, the calls made and the start of what
/// they returned; what tools added for the model and side chains stay
/// out. Read again from where it stopped, it gives only what came since.
#[test]
fn a_claude_code_chat_reads_as_text_and_again_from_where_it_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.jsonl");
    let first = lines(&[
        json!({"type": "user", "message": {"role": "user",
               "content": "Pin the toolchain.<system-reminder>internal</system-reminder>"}}),
        json!({"type": "assistant", "message": {"content": [
            {"type": "thinking", "thinking": "hidden"},
            {"type": "text", "text": "Pinning it to 1.92."},
            {"type": "tool_use", "name": "edit", "input": {"path": "rust-toolchain.toml"}}]}}),
        json!({"type": "user", "message": {"content": [
            {"type": "tool_result", "content": [{"type": "text", "text": "ok"}]}]}}),
        json!({"type": "user", "isMeta": true, "message": {"content": "meta"}}),
        json!({"type": "assistant", "isSidechain": true, "message": {"content": [
            {"type": "text", "text": "side"}]}}),
    ]);
    // A line still being written is not read.
    fs::write(&path, format!("{first}{{\"type\": \"us")).unwrap();
    let Read::Text { text, offset } = claude::read_chat(&path, 0).unwrap()
    else {
        panic!("a chat reads as text")
    };
    assert_eq!(
        text,
        "[user]\nPin the toolchain.\n\n[assistant]\nPinning it to 1.92.\n\n\
         [call edit] {\"path\":\"rust-toolchain.toml\"}\n\n[result]\nok\n\n"
    );
    assert_eq!(offset, first.len() as u64);
    let more = lines(&[
        json!({"type": "user", "message": {"content": "And the lints."}}),
    ]);
    fs::write(&path, format!("{first}{more}")).unwrap();
    let Read::Text {
        text,
        offset: after,
    } = claude::read_chat(&path, offset).unwrap()
    else {
        panic!("a chat reads as text")
    };
    assert_eq!(text, "[user]\nAnd the lints.\n\n");
    assert_eq!(after, (first.len() + more.len()) as u64);
    fs::write(&path, "not json\n").unwrap();
    assert!(claude::read_chat(&path, 0).is_err());
}

/// A home with Codex's files.
fn codex_home() -> (tempfile::TempDir, PathBuf) {
    let home = tempfile::tempdir().unwrap();
    let repo = home.path().canonicalize().unwrap().join("work/billing");
    fs::create_dir_all(&repo).unwrap();
    // A jj repository: its Git store holds the remote.
    write(&repo.join(".jj/repo/store/git_target"), "git");
    write(
        &repo.join(".jj/repo/store/git/config"),
        "[remote \"origin\"]\n\turl = https://github.com/acme/billing\n",
    );
    let root = home.path().join(".codex");
    let memories = root.join("memories");
    write(
        &memories.join("MEMORY.md"),
        &format!(
            "# Task Group: {} — invoices and their rounding\n\nscope: billing\n\n## Task 1: Round once\n\n### rollout_summary_files\n\n- rollout_summaries/2026-10-01-a.md (cwd={})\n\n## User preferences\n\n- Keep money in cents.\n\n# Task Group: /nowhere/else — another folder\n\n## Reusable knowledge\n\n- Something else.\n",
            repo.display(),
            repo.display()
        ),
    );
    write(
        &memories.join("memory_summary.md"),
        "## User Profile\n\nPrefers small commits.\n",
    );
    write(
        &memories.join("rollout_summaries/2026-10-01-a.md"),
        &format!(
            "thread_id: t-a\nupdated_at: 2026-10-01T10:00:00Z\ncwd: {}\n\n# Round once\n",
            repo.display()
        ),
    );
    write(
        &memories.join("rollout_summaries/2026-10-02-b.md"),
        &format!(
            "thread_id: t-b\nupdated_at: 2026-10-02T10:00:00Z\ncwd: {}\n\n# Taxes\n",
            repo.display()
        ),
    );
    write(
        &memories.join("raw_memories.md"),
        &format!(
            "# Raw Memories\n\n## Thread `t-a`\nupdated_at: 2026-10-01T10:00:00Z\ncwd: {0}\n\nsame as its summary\n\n## Thread `t-c`\nupdated_at: 2026-10-03T10:00:00Z\ncwd: {0}\n\nonly here\n",
            repo.display()
        ),
    );
    let meta = |id: &str, extra: serde_json::Value| {
        let mut payload = json!({"id": id, "cwd": repo, "timestamp": "2026-10-05T09:00:00Z", "thread_source": "user"});
        payload
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        json!({"timestamp": "2026-10-05T09:00:00Z", "type": "session_meta", "payload": payload})
    };
    let sessions = root.join("sessions/2026/10/05");
    write(
        &sessions.join("rollout-own.jsonl"),
        &lines(&[meta("s-own", json!({}))]),
    );
    write(
        &sessions.join("rollout-copy.jsonl"),
        &lines(&[meta("s-copy", json!({}))]),
    );
    write(
        &sessions.join("rollout-sub.jsonl"),
        &lines(&[meta("s-sub", json!({"thread_source": "subagent"}))]),
    );
    write(&sessions.join("rollout-bad.jsonl"), "not json\n");
    write(
        &root.join(codex::IMPORTS),
        &json!({"records": [{"imported_thread_id": "s-copy", "source_path": "/x.jsonl"}]}).to_string(),
    );
    (home, repo)
}

/// A scan reads each task group of `MEMORY.md`, the summary of the
/// person, the task summaries `MEMORY.md` does not hold and the raw
/// threads no summary holds; and the sessions but Codex's copies of
/// Claude Code's and its sub-agents'.
#[test]
fn codex_scan_reads_each_thing_once() {
    let (home, repo) = codex_home();
    let roots = Roots::new(home.path());
    let scan = importers::scan(&roots);
    let saved = items_of(&scan.items, Tool::Codex, Material::Saved);
    let keys: Vec<String> = saved
        .iter()
        .map(|item| roots.show(Path::new(&item.key)))
        .collect();
    assert_eq!(
        keys,
        [
            "~/.codex/memories/MEMORY.md#0",
            "~/.codex/memories/MEMORY.md#1",
            "~/.codex/memories/memory_summary.md",
            "~/.codex/memories/rollout_summaries/2026-10-02-b.md",
            "~/.codex/memories/raw_memories.md#t-c",
        ]
    );
    let billing = Some(repo.as_path());
    assert!(saved[0].place.as_ref().unwrap().matches(billing, None));
    assert!(
        saved[0]
            .section
            .as_deref()
            .unwrap()
            .contains("Keep money in cents")
    );
    assert_eq!(
        saved[1].place.as_ref().unwrap().folder.as_deref(),
        Some("/nowhere/else")
    );
    assert_eq!(saved[2].place, None);
    assert!(saved[2].about_you);
    assert_eq!(saved[3].session.as_deref(), Some("t-b"));
    assert_eq!(saved[3].date.as_deref(), Some("2026-10-02"));
    // The jj repository's Git store says where it pushes.
    assert!(
        saved[3]
            .place
            .as_ref()
            .unwrap()
            .matches(None, Some("git@github.com:acme/billing.git"))
    );
    let chats = items_of(&scan.items, Tool::Codex, Material::Chat);
    let sessions: Vec<Option<&str>> =
        chats.iter().map(|item| item.session.as_deref()).collect();
    assert_eq!(sessions, [Some("s-own")]);
    assert_eq!(chats[0].date.as_deref(), Some("2026-10-05"));
    assert_eq!(scan.failed.len(), 1);
    assert!(scan.failed[0].ends_with("rollout-bad.jsonl"));
}

/// A session reads as what the person and Codex said and the calls it
/// made; what Codex put in for the person stays out.
#[test]
fn a_codex_session_reads_as_text() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.jsonl");
    let message = |role: &str, kind: &str, text: &str| {
        json!({"type": "response_item", "payload": {"type": "message", "role": role,
               "content": [{"type": kind, "text": text}]}})
    };
    write(
        &path,
        &lines(&[
            json!({"type": "session_meta", "payload": {"id": "s"}}),
            message("developer", "input_text", "rules"),
            message(
                "user",
                "input_text",
                "<environment_context>\n<cwd>/x</cwd>\n</environment_context>",
            ),
            message("user", "input_text", "# AGENTS.md instructions for /x"),
            message("user", "input_text", "Make rounding happen once."),
            json!({"type": "response_item", "payload": {"type": "function_call", "name": "shell", "arguments": "{\"cmd\":\"ls\"}"}}),
            json!({"type": "response_item", "payload": {"type": "function_call_output", "output": "src\n"}}),
            json!({"type": "response_item", "payload": {"type": "reasoning", "summary": []}}),
            message(
                "assistant",
                "output_text",
                "Done: rounding is in one place.",
            ),
            json!({"type": "event_msg", "payload": {"type": "token_count"}}),
        ]),
    );
    let Read::Text { text, .. } = codex::read_chat(&path, 0).unwrap() else {
        panic!("a session reads as text")
    };
    assert_eq!(
        text,
        "[user]\nMake rounding happen once.\n\n[call shell] {\"cmd\":\"ls\"}\n\n\
         [result]\nsrc\n\n[assistant]\nDone: rounding is in one place.\n\n"
    );
}

/// Neither tool on this computer: nothing to read.
#[test]
fn an_empty_home_has_nothing() {
    let home = tempfile::tempdir().unwrap();
    let scan = importers::scan(&Roots::new(home.path()));
    assert!(scan.items.is_empty());
    assert!(scan.failed.is_empty());
    let mut remotes = Remotes::default();
    assert!(
        claude::scan(&home.path().join(".claude"), &mut remotes)
            .items
            .is_empty()
    );
}

/// However a remote is written (SSH, scp-like, HTTPS, with or without
/// `.git`, a user or a port, the host in any case), it is one remote.
#[hegel::test(test_cases = 100)]
fn a_remote_is_one_remote_however_written(tc: TestCase) {
    let owner: String =
        tc.draw(gs::from_regex("[a-z0-9][a-z0-9-]{0,8}").fullmatch(true));
    let name: String = tc.draw(
        gs::from_regex("[a-z0-9][a-z0-9._-]{0,8}[a-z0-9]").fullmatch(true),
    );
    let host: &str = tc.draw(gs::sampled_from(vec![
        "github.com",
        "GitHub.com",
        "gitlab.example.org",
    ]));
    let suffix: &str =
        tc.draw(gs::sampled_from(vec!["", ".git", "/", ".git/"]));
    let forms = [
        format!("git@{host}:{owner}/{name}{suffix}"),
        format!("ssh://git@{host}/{owner}/{name}{suffix}"),
        format!("ssh://git@{host}:22/{owner}/{name}{suffix}"),
        format!("https://{host}/{owner}/{name}{suffix}"),
        format!("https://someone@{host}/{owner}/{name}{suffix}"),
    ];
    let want = format!("{}/{owner}/{name}", host.to_ascii_lowercase());
    for form in &forms {
        assert_eq!(
            normalize_remote(form).as_deref(),
            Some(want.as_str()),
            "{form}"
        );
        let place = Place {
            folder: None,
            remote: Some(form.clone()),
        };
        assert!(
            place
                .matches(None, Some(&format!("https://{host}/{owner}/{name}")))
        );
    }
    let other = format!("https://{host}/{owner}/{name}-fork");
    assert!(
        !Place {
            folder: None,
            remote: Some(forms[0].clone())
        }
        .matches(None, Some(&other))
    );
}

/// A place is a repository's when it is the repository's folder or
/// inside it, or pushes where the repository does.
#[test]
fn a_place_belongs_to_the_repository_holding_it() {
    let place = Place {
        folder: Some("/work/shop/api".into()),
        remote: None,
    };
    assert!(place.matches(Some(Path::new("/work/shop")), None));
    assert!(place.matches(Some(Path::new("/work/shop/api")), None));
    assert!(!place.matches(Some(Path::new("/work/sho")), None));
    assert!(!place.matches(None, Some("https://github.com/a/b")));
    assert_eq!(normalize_remote(""), None);
    assert_eq!(
        normalize_remote("/srv/git/app.git").as_deref(),
        Some("/srv/git/app")
    );
}

/// A remote comes from the config of the repository a folder is in: a
/// checkout's, a worktree's shared one, or a jj repository's Git store.
#[test]
fn a_folder_remote_comes_from_its_repository_config() {
    let dir = tempfile::tempdir().unwrap();
    let plain = dir.path().join("plain");
    git_checkout(&plain, "git@example.com:a/plain.git");
    fs::create_dir_all(plain.join("deep/er")).unwrap();
    assert_eq!(
        remote::of_folder(&plain.join("deep/er")).as_deref(),
        Some("git@example.com:a/plain.git")
    );
    // A worktree's `.git` names its directory, which names the shared one.
    let tree = dir.path().join("tree");
    write(
        &tree.join(".git"),
        &format!("gitdir: {}\n", plain.join(".git/worktrees/tree").display()),
    );
    write(&plain.join(".git/worktrees/tree/commondir"), "../..\n");
    assert_eq!(
        remote::of_folder(&tree).as_deref(),
        Some("git@example.com:a/plain.git")
    );
    let none = dir.path().join("none");
    fs::create_dir_all(&none).unwrap();
    assert_eq!(remote::from_config("[core]\n\tbare = false\n"), None);
    assert_eq!(
        remote::from_config("[remote \"fork\"]\n\turl = https://e.com/f\n"),
        Some("https://e.com/f".into())
    );
}

/// Chunks join back into the text, and none is longer than asked (or
/// than one character, when less is asked).
#[hegel::test(test_cases = 200)]
fn chunks_join_into_the_text(tc: TestCase) {
    let text: String = tc.draw(gs::text().max_size(400));
    let max: usize =
        tc.draw(gs::integers::<usize>().min_value(1).max_value(80));
    let parts = chunks(&text, max);
    assert_eq!(parts.concat(), text);
    for part in &parts {
        assert!(part.len() <= max.max(4), "{part:?} over {max}");
        assert!(!part.is_empty());
    }
}
