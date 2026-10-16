//! `write` (`docs/reference/tools.md`, "write"), ported from pi's
//! `write.ts` and `tools.test.ts`.

use std::sync::Arc;

use hegel::{TestCase, generators as gs};
use serde_json::json;
use tau_agent::tool::{AgentTool, ToolCtx};
use tau_tools_host::{ABORTED, path::Root, write::Write};

fn cancelled_ctx() -> ToolCtx {
    let ctx = ToolCtx::detached();
    ctx.cancel.cancel();
    ctx
}

fn new_write(dir: &std::path::Path) -> Write {
    Write::new(Root::new(dir))
}

/// `write` returns exactly `Successfully wrote to <path>` and the file
/// holds exactly the given content, for any generated text, under up to
/// three directories that did not exist, which it creates (`tools.md`,
/// "write"; round trip). Its details say it created the file, with a
/// diff that adds every line of it, for its card.
#[hegel::test(test_cases = 50)]
// A line of `++` shows as `+++` in the diff, like the `+++` header.
#[hegel::explicit_test_case(content = String::from("++"), dirs = Vec::<String>::new())]
fn write_round_trips_the_content(tc: TestCase) {
    let content: String = tc.draw(gs::text().max_size(200));
    let dirs: Vec<String> = tc.draw(
        gs::vecs(gs::text().alphabet("abcé").min_size(1).max_size(4))
            .max_size(3),
    );
    let path = dirs
        .iter()
        .map(String::as_str)
        .chain(["f.txt"])
        .collect::<Vec<_>>()
        .join("/");
    let dir = tempfile::tempdir().unwrap();
    let write = new_write(dir.path());

    let output = tau_testing::block_on(write.call(
        json!({"path": path, "content": content}),
        ToolCtx::detached(),
    ))
    .unwrap();

    assert_eq!(
        output.content,
        tau_agent::tool::ToolOutput::text(format!(
            "Successfully wrote to {path}"
        ))
        .content
    );
    let details = output.details.expect("details for the card");
    assert_eq!(details["created"], json!(true));
    let diff = details["diff"].as_str().unwrap();
    // Lines end at `\n` only, on both sides: `str::lines` would also
    // take a `\r` before it. The two file headers come first; after
    // them, a line of the file's that starts with `++` is still a line.
    let added: Vec<&str> = diff
        .split('\n')
        .skip(2)
        .filter(|line| line.starts_with('+'))
        .map(|line| &line[1..])
        .collect();
    let lines: Vec<&str> = content
        .split_inclusive('\n')
        .map(|line| line.strip_suffix('\n').unwrap_or(line))
        .collect();
    assert_eq!(added, lines, "{diff}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join(&path)).unwrap(),
        content
    );
}

/// Overwriting a file replaces its content whole and gives its card a
/// unified diff it can read: after
/// the two file headers, each line is a hunk header or one of its
/// hunk's lines, none of them empty, and each hunk holds as many old
/// and new lines as its header says. A line without its `\n`, even one
/// that ends in `\r`, is followed by `\ No newline at end of file`.
#[hegel::test(test_cases = 200)]
fn write_diffs_hold_what_their_hunks_say(tc: TestCase) {
    let text = || gs::from_regex("[ab\r\n]{0,40}").fullmatch(true);
    let old: String = tc.draw(text());
    let new: String = tc.draw(text());
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), &old).unwrap();
    let output = tau_testing::block_on(new_write(dir.path()).call(
        json!({"path": "f.txt", "content": new}),
        ToolCtx::detached(),
    ))
    .unwrap();
    // The file holds the new content whole, and it was not created.
    assert_eq!(
        std::fs::read_to_string(dir.path().join("f.txt")).unwrap(),
        new
    );
    let details = output.details.expect("details for the card");
    assert_eq!(details["created"], json!(false));
    let diff = details["diff"].as_str().unwrap();
    let lines: Vec<&str> = diff.split_terminator('\n').collect();
    if old == new {
        assert!(lines.is_empty(), "{diff:?}");
        return;
    }
    assert_eq!(lines[..2], ["--- f.txt", "+++ f.txt"], "{diff:?}");
    // The old and new lines the current hunk has left to give.
    let (mut old_left, mut new_left) = (0usize, 0usize);
    let take = |left: &mut usize| {
        *left = left.checked_sub(1).expect("a hunk past its count");
    };
    for line in &lines[2..] {
        if let Some(header) = line.strip_prefix("@@ ") {
            assert_eq!((old_left, new_left), (0, 0), "{diff:?}");
            let count = |sign: char| {
                header
                    .split_whitespace()
                    .find_map(|part| part.strip_prefix(sign))
                    .map(|range| {
                        range
                            .split_once(',')
                            .map_or(1, |(_, n)| n.parse().unwrap())
                    })
                    .unwrap()
            };
            (old_left, new_left) = (count('-'), count('+'));
            continue;
        }
        match line.chars().next() {
            Some(' ') => {
                take(&mut old_left);
                take(&mut new_left);
            }
            Some('-') => take(&mut old_left),
            Some('+') => take(&mut new_left),
            Some('\\') => {}
            _ => panic!("{line:?} is no diff line: {diff:?}"),
        }
    }
    assert_eq!((old_left, new_left), (0, 0), "{diff:?}");
}

/// Cancelled (`tools.md`, "Error strings", "all"): nothing is written.
#[test]
fn aborted_when_cancelled_before_writing() {
    let dir = tempfile::tempdir().unwrap();
    let write = new_write(dir.path());
    let result = tau_testing::block_on(
        write.call(json!({"path": "f.txt", "content": "hi"}), cancelled_ctx()),
    );
    assert_eq!(result.unwrap_err().to_string(), ABORTED);
    assert!(!dir.path().join("f.txt").exists());
}

/// Per-path lock, shared with `edit` (`tools.md`, "write": "Holds the
/// same per-path mutex as `edit`"): overlapping `write` calls to one
/// file run one after another. Each call's diff is from what the file
/// held when it ran to what it wrote, so the calls' "before" contents
/// chain from the original through every writer exactly once, and the
/// file ends as the last one left it. Without the lock, two calls read
/// the same "before" and the chain breaks.
#[hegel::test(test_cases = 50)]
fn concurrent_writes_run_one_after_another(tc: TestCase) {
    let n = tc.draw(gs::integers::<usize>().min_value(2).max_value(6));

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), "original\n").unwrap();
    let write = Arc::new(new_write(dir.path()));
    let contents: Vec<String> =
        (0..n).map(|i| format!("writer {i}\n")).collect();

    let befores: Vec<String> = tau_testing::block_on(async {
        let mut tasks = Vec::new();
        for content in &contents {
            let write = write.clone();
            let content = content.clone();
            tasks.push(tokio::spawn(async move {
                write
                    .call(
                        json!({"path": "f.txt", "content": content}),
                        ToolCtx::detached(),
                    )
                    .await
            }));
        }
        let mut befores = Vec::new();
        for task in tasks {
            let output = task.await.unwrap().unwrap();
            let details = output.details.unwrap();
            // Every content is one line: the diff's one removed line is
            // what the file held.
            let removed: Vec<&str> = details["diff"]
                .as_str()
                .unwrap()
                .lines()
                .skip(2)
                .filter_map(|line| line.strip_prefix('-'))
                .collect();
            assert_eq!(removed.len(), 1, "{details}");
            befores.push(format!("{}\n", removed[0]));
        }
        befores
    });

    // Follow the chain from the original: each step is the one call
    // that saw what the step before wrote.
    let mut current = String::from("original\n");
    let mut seen = vec![false; n];
    for _ in 0..n {
        let next: Vec<usize> = (0..n)
            .filter(|&i| !seen[i] && befores[i] == current)
            .collect();
        assert_eq!(
            next.len(),
            1,
            "calls {next:?} all wrote over {current:?}: {befores:?}"
        );
        seen[next[0]] = true;
        current.clone_from(&contents[next[0]]);
    }
    assert_eq!(
        std::fs::read_to_string(dir.path().join("f.txt")).unwrap(),
        current
    );
}

/// The tool's name, description and parameter descriptions are ported
/// from pi verbatim (`tools.md`: "Port pi's tool `description` strings
/// and parameter descriptions verbatim").
#[test]
fn tool_metadata_matches_the_ported_pi_strings() {
    let dir = tempfile::tempdir().unwrap();
    let write = new_write(dir.path());
    assert_eq!(write.name(), "write");
    assert_eq!(
        write.description(),
        "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Automatically creates parent directories."
    );
    let schema = write.parameters();
    assert_eq!(
        schema["properties"]["path"]["description"],
        "Path to the file to write (relative or absolute)"
    );
    assert_eq!(
        schema["properties"]["content"]["description"],
        "Content to write to the file"
    );
}
