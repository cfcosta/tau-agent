//! `ls` (`tau_tools_host::ls::Ls`), against `docs/reference/tools.md` ("ls")
//! and pi's `ls.ts`, on a real directory (`docs/reference/testing.md`,
//! "tau-tools").

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

mod common;

use hegel::{TestCase, generators as gs};
use serde_json::json;
use tau_agent::{
    error::ToolError,
    tool::{AgentTool, ToolCtx, ToolOutput},
};
use tau_ai::message::InputBlock;
use tau_testing::block_on;
use tau_tools_host::{
    details::{EntryKind, Listing},
    ls::Ls,
    path::Root,
    truncate::{MAX_BYTES, format_size, truncate_head},
};

fn text_of(output: &ToolOutput) -> &str {
    match &output.content[0] {
        InputBlock::Text(t) => &t.text,
        other => panic!("expected a text block, got {other:?}"),
    }
}

fn call(root: &Root, args: serde_json::Value) -> Result<String, ToolError> {
    block_on(async {
        Ls::new(root.clone())
            .call(args, ToolCtx::detached())
            .await
            .map(|out| text_of(&out).to_owned())
    })
}

fn listing(root: &Root, args: serde_json::Value) -> Listing {
    let output =
        block_on(Ls::new(root.clone()).call(args, ToolCtx::detached()))
            .expect("ls runs");
    serde_json::from_value(output.details.expect("a listing"))
        .expect("the details are a listing")
}

fn structured(root: &Root, args: serde_json::Value) -> serde_json::Value {
    block_on(Ls::new(root.clone()).call(args, ToolCtx::detached()))
        .expect("ls runs")
        .structured
        .expect("a structured listing")
}

/// The tool's identity and pinned description string, verbatim from
/// pi's `ls.ts` except for the ported limits (`docs/reference/tools.md`,
/// "ls").
#[test]
fn name_description_and_schema() {
    let dir = tempfile::tempdir().unwrap();
    let tool = Ls::new(Root::new(dir.path()));
    assert_eq!(tool.name(), "ls");
    assert_eq!(
        tool.description(),
        "List directory contents. Returns entries sorted alphabetically, \
with '/' suffix for directories. Includes dotfiles. Output is truncated \
to 500 entries or 50KB (whichever is hit first)."
    );
    let schema = tool.parameters();
    assert_eq!(
        schema["properties"]["path"]["description"],
        json!("Directory to list (default: current directory)")
    );
}

/// A set of directory entries whose names come from a small alphabet
/// of letters in both cases, multi-byte ones included, and a dot, so
/// names that differ only in case (`n` and `N`, `é` and `É`) come up
/// often, as do dotfiles (`docs/reference/tools.md`, "ls").
#[hegel::composite]
fn ls_entries(tc: &TestCase) -> Vec<(String, bool)> {
    let names: Vec<String> = tc.draw(
        gs::vecs(gs::text().alphabet("nNéÉ.").min_size(1).max_size(3))
            .unique(true)
            .min_size(1)
            .max_size(6),
    );
    names
        .into_iter()
        // `.` and `..` are not names a file can have.
        .filter(|name| name != "." && name != "..")
        .map(|name| (name, tc.draw(gs::booleans())))
        .collect()
}

/// `ls` output is sorted case-insensitively, names equal but for case
/// in byte order (so the listing does not depend on the order the
/// filesystem returns them in), with a trailing `/` on directories and
/// dotfiles included (`docs/reference/tools.md`, "ls";
/// `docs/reference/testing.md`, "tau-tools").
#[hegel::test(test_cases = 100)]
fn ls_matches_a_naive_listing(tc: TestCase) {
    let entries = tc.draw(ls_entries());
    let limit = tc.draw(gs::integers::<u32>().min_value(1).max_value(6));

    let dir = tempfile::tempdir().unwrap();
    for (name, is_dir) in &entries {
        let path = dir.path().join(name);
        if *is_dir {
            std::fs::create_dir(&path).unwrap();
        } else {
            std::fs::write(&path, "x").unwrap();
        }
    }
    let root = Root::new(dir.path());
    let output = block_on(
        Ls::new(root.clone())
            .call(json!({"limit": limit}), ToolCtx::detached()),
    )
    .unwrap();
    let actual = text_of(&output).to_owned();
    let structured = output.structured.as_ref().expect("a structured listing");
    common::assert_matches_output_schema(&Ls::new(root.clone()), structured);

    let mut sorted = entries.clone();
    sorted.sort_by(|(a, _), (b, _)| {
        a.to_lowercase()
            .cmp(&b.to_lowercase())
            .then_with(|| a.cmp(b))
    });
    if sorted
        .windows(2)
        .any(|w| w[0].0.to_lowercase() == w[1].0.to_lowercase())
    {
        tc.event("names equal but for case");
    }
    let mut results = Vec::new();
    let mut limit_reached = false;
    for (name, is_dir) in &sorted {
        if results.len() >= limit as usize {
            limit_reached = true;
            break;
        }
        results.push(format!("{name}{}", if *is_dir { "/" } else { "" }));
    }

    let expected = if results.is_empty() {
        "(empty directory)".to_owned()
    } else {
        let raw = results.join("\n");
        let truncation = truncate_head(&raw, usize::MAX, MAX_BYTES);
        let truncated = truncation.truncated();
        let mut text = truncation.content;
        let mut notices = Vec::new();
        if limit_reached {
            notices.push(format!(
                "{limit} entries limit reached. Use limit={} for more",
                limit * 2
            ));
        }
        if truncated {
            notices.push(format!("{} limit reached", format_size(MAX_BYTES)));
        }
        if !notices.is_empty() {
            text.push_str(&format!("\n\n[{}]", notices.join(". ")));
        }
        text
    };

    assert_eq!(actual, expected, "entries = {entries:?}, limit = {limit}");
    // The structured entries are the ones listed, by name, in order.
    let names: Vec<&str> = structured["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap())
        .collect();
    let listed: Vec<&str> = results
        .iter()
        .map(|line| line.strip_suffix('/').unwrap_or(line))
        .collect();
    assert_eq!(names, listed);
    assert_eq!(structured["truncated"], limit_reached);
    assert_eq!(structured["complete"], !limit_reached);
    // The card's details stop where the text does, and say so; an empty
    // directory (every drawn name was `.` or `..`) has no card.
    match &output.details {
        None => assert!(entries.is_empty(), "no listing of {entries:?}"),
        Some(details) => {
            let details: Listing = serde_json::from_value(details.clone())
                .expect("the details are a listing");
            let shown: Vec<&str> =
                details.entries.iter().map(|e| e.name.as_str()).collect();
            assert_eq!(shown, listed);
            assert_eq!(details.truncated, limit_reached);
        }
    }
}

/// An empty directory is reported with a fixed string (`ls.ts`).
#[test]
fn empty_directory() {
    let dir = tempfile::tempdir().unwrap();
    let root = Root::new(dir.path());
    let output = call(&root, json!({})).unwrap();
    assert_eq!(output, "(empty directory)");
}

/// A path that does not exist is an error.
#[test]
fn missing_path_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let root = Root::new(dir.path());
    let error = call(&root, json!({"path": "missing"})).unwrap_err();
    assert!(error.to_string().contains("Path not found"), "{error}");
}

/// A path that names a file, not a directory, is an error (`ls.ts`).
#[test]
fn file_path_is_not_a_directory() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f.txt");
    std::fs::write(&file, "x").unwrap();
    let root = Root::new(dir.path());
    let error =
        call(&root, json!({"path": file.to_string_lossy()})).unwrap_err();
    assert!(error.to_string().contains("Not a directory"), "{error}");
}

/// The details carry each entry the model got, in its order, with its
/// kind, a file's size, a directory's entry count, a symlink's target
/// and whether `.gitignore` leaves it out (`docs/reference/tools.md`,
/// "ls").
#[cfg(unix)]
#[test]
fn details_describe_each_entry() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".gitignore"), "build/\n").unwrap();
    std::fs::write(dir.path().join("notes.md"), "hello").unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/a.rs"), "").unwrap();
    std::fs::write(dir.path().join("src/b.rs"), "").unwrap();
    std::fs::create_dir(dir.path().join("build")).unwrap();
    std::os::unix::fs::symlink("notes.md", dir.path().join("link")).unwrap();
    std::os::unix::fs::symlink("src", dir.path().join("code")).unwrap();
    let root = Root::new(dir.path());

    let listing = listing(&root, json!({}));
    let names: Vec<&str> =
        listing.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        [".gitignore", "build", "code", "link", "notes.md", "src"]
    );
    let get =
        |name: &str| listing.entries.iter().find(|e| e.name == name).unwrap();
    assert_eq!(get("notes.md").kind, EntryKind::File);
    assert_eq!(get("notes.md").size, Some(5));
    assert!(get("notes.md").modified.is_some());
    assert_eq!(get("src").kind, EntryKind::Dir);
    assert_eq!(get("src").items, Some(2));
    assert_eq!(get("src").size, None);
    assert!(get("build").ignored);
    assert!(!get("src").ignored);
    assert_eq!(get("link").kind, EntryKind::Symlink);
    assert_eq!(get("link").target.as_deref(), Some("notes.md"));
    assert_eq!(get("code").kind, EntryKind::SymlinkDir);
    assert!(!listing.truncated);
    // The structured result holds the same records.
    assert_eq!(
        structured(&root, json!({}))["entries"],
        serde_json::to_value(&listing.entries).unwrap()
    );
}

/// Structured results always retain the empty-directory value, including
/// when a zero limit prevents listing entries.
#[test]
fn structured_empty_and_zero_limit_results_are_objects() {
    let empty = tempfile::tempdir().unwrap();
    let empty_root = Root::new(empty.path());
    let empty_value = structured(&empty_root, json!({}));
    assert_eq!(empty_value["dir"], empty.path().to_string_lossy().as_ref());
    assert_eq!(empty_value["entries"], json!([]));
    assert_eq!(empty_value["truncated"], false);
    assert_eq!(empty_value["complete"], true);

    let populated = tempfile::tempdir().unwrap();
    std::fs::write(populated.path().join("a.txt"), "x").unwrap();
    let populated_root = Root::new(populated.path());
    let zero_limit = structured(&populated_root, json!({"limit": 0}));
    assert_eq!(zero_limit["entries"], json!([]));
    assert_eq!(zero_limit["truncated"], true);
    assert_eq!(zero_limit["complete"], false);
}

/// Structured names come from filesystem records, not rendered lines;
/// newline, colon, and multibyte characters remain part of one name.
#[test]
fn structured_names_preserve_newlines_punctuation_and_unicode() {
    let dir = tempfile::tempdir().unwrap();
    let name = "line\nbreak: café.txt";
    std::fs::write(dir.path().join(name), "x").unwrap();
    let root = Root::new(dir.path());

    let value = structured(&root, json!({}));
    assert_eq!(value["entries"].as_array().unwrap().len(), 1);
    assert_eq!(value["entries"][0]["name"], name);
    assert_eq!(value["complete"], true);
}

/// The structured inventory is gathered before the presentation's byte cap,
/// so byte truncation cannot discard records that fit the entry limit.
#[test]
fn structured_entries_survive_presentation_byte_limit() {
    let dir = tempfile::tempdir().unwrap();
    const COUNT: usize = 270;
    for index in 0..COUNT {
        let name = format!("{index:03}-{}", "x".repeat(200));
        std::fs::write(dir.path().join(name), "x").unwrap();
    }
    let root = Root::new(dir.path());
    let value = structured(&root, json!({"limit": 500}));

    assert_eq!(value["entries"].as_array().unwrap().len(), COUNT);
    assert_eq!(value["truncated"], true);
    assert_eq!(value["complete"], false);
}

/// tau-tools' card reads a listing only from `ls`'s details: what the
/// other coding tools put in theirs is no listing (`tau_tools::ui`).
#[tokio::test(flavor = "multi_thread")]
async fn only_ls_details_read_as_a_listing() {
    use tau_tools::ui::listing::DirListing;
    use tau_tools_host::{
        edit::Edit,
        exec::{ExecCommand, Processes},
        find::Find,
        grep::Grep,
        read::Read,
        write::Write,
    };
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "needle\n").unwrap();
    let root = Root::new(dir.path());
    let calls: Vec<(Box<dyn AgentTool>, serde_json::Value)> = vec![
        (Box::new(Read::new(root.clone())), json!({"path": "a.txt"})),
        (
            Box::new(Edit::new(root.clone())),
            json!({"path": "a.txt", "edits": [{"oldText": "needle", "newText": "pin"}]}),
        ),
        (
            Box::new(Write::new(root.clone())),
            json!({"path": "b.txt", "content": "pin\n"}),
        ),
        (Box::new(Grep::new(root.clone())), json!({"pattern": "pin"})),
        (
            Box::new(Find::new(root.clone())),
            json!({"pattern": "*.txt"}),
        ),
        (
            Box::new(ExecCommand::new(root.clone(), Processes::new())),
            json!({"cmd": "echo hi"}),
        ),
    ];
    for (tool, args) in calls {
        let output = tool.call(args, ToolCtx::detached()).await.unwrap();
        let details = output.details.unwrap_or_default();
        assert!(
            DirListing::parse(&details).is_none(),
            "{}'s details read as a listing: {details}",
            tool.name()
        );
    }
    let output = Ls::new(root)
        .call(json!({}), ToolCtx::detached())
        .await
        .unwrap();
    let listing =
        DirListing::parse(&output.details.unwrap()).expect("a listing");
    assert_eq!(listing.entries.len(), 2);
}
