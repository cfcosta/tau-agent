//! `ls` (`tau_tools::ls::Ls`), against `docs/reference/tools.md` ("ls")
//! and pi's `ls.ts`, on a real directory (`docs/reference/testing.md`,
//! "tau-tools").

use hegel::{TestCase, generators as gs};
use serde_json::json;
use tau_agent::tool::{AgentTool, RunId, ToolCtx, ToolOutput, ToolUpdates};
use tau_ai::message::InputBlock;
use tau_testing::block_on;
use tau_tools::{
    ls::Ls,
    path::Root,
    truncate::{MAX_BYTES, format_size, truncate_head},
};
use tokio_util::sync::CancellationToken;

fn ctx() -> ToolCtx {
    let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
    ToolCtx::new(
        CancellationToken::new(),
        ToolUpdates::for_tests("call_1", sender),
        RunId("run_1".into()),
    )
}

fn text_of(output: &ToolOutput) -> &str {
    match &output.content[0] {
        InputBlock::Text(t) => &t.text,
        other => panic!("expected a text block, got {other:?}"),
    }
}

fn call(root: &Root, args: serde_json::Value) -> anyhow::Result<String> {
    block_on(async {
        Ls::new(root.clone())
            .call(args, ctx())
            .await
            .map(|out| text_of(&out).to_owned())
    })
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
    let actual = call(&root, json!({"limit": limit})).unwrap();

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
}

/// Dotfiles and directories are listed, with a trailing `/` on
/// directories (pi, `tools.test.ts:936`, "should list dotfiles and
/// directories").
#[test]
fn dotfiles_and_directories_are_listed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".hidden-file"), "secret").unwrap();
    std::fs::create_dir(dir.path().join(".hidden-dir")).unwrap();
    let root = Root::new(dir.path());

    let output = call(&root, json!({})).unwrap();
    assert!(output.contains(".hidden-file"), "{output}");
    assert!(output.contains(".hidden-dir/"), "{output}");
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

/// The entry limit notice, pinned exactly (`docs/reference/tools.md`,
/// "ls"; the "limit reached" wording is part of the spec).
#[test]
fn entry_limit_notice() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(dir.path().join(name), "x").unwrap();
    }
    let root = Root::new(dir.path());
    let output = call(&root, json!({"limit": 2})).unwrap();
    assert!(output.contains("a.txt"), "{output}");
    assert!(output.contains("b.txt"), "{output}");
    assert!(!output.contains("c.txt"), "{output}");
    assert!(
        output.contains("[2 entries limit reached. Use limit=4 for more]"),
        "{output}"
    );
}

/// Names equal but for case list in byte order, whatever order the
/// filesystem returns them in (a regression: the tie used to follow
/// `read_dir`'s order; pi's comes from libuv's sorted `scandir`).
#[test]
fn names_equal_but_for_case_list_in_byte_order() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["n", "é", "N", "É", "nN", "Nn"] {
        std::fs::write(dir.path().join(name), "x").unwrap();
    }
    let root = Root::new(dir.path());
    let output = call(&root, json!({})).unwrap();
    assert_eq!(output, "N\nn\nNn\nnN\nÉ\né");
}
