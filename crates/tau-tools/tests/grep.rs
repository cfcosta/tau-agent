//! `grep` (`tau_tools::grep::Grep`), against `docs/reference/tools.md`
//! ("grep") and pi's `grep.ts`, on a real directory
//! (`docs/reference/testing.md`, "tau-tools").

use hegel::{TestCase, generators as gs};
use regex::RegexBuilder;
use serde_json::json;
use tau_agent::tool::{AgentTool, RunId, ToolCtx, ToolOutput, ToolUpdates};
use tau_ai::message::InputBlock;
use tau_testing::block_on;
use tau_tools::{
    grep::Grep,
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
        Grep::new(root.clone())
            .call(args, ctx())
            .await
            .map(|out| text_of(&out).to_owned())
    })
}

/// The tool's identity and pinned description string, verbatim from
/// pi's `grep.ts` except for the ported limits
/// (`docs/reference/tools.md`, "grep").
#[test]
fn name_description_and_schema() {
    let dir = tempfile::tempdir().unwrap();
    let tool = Grep::new(Root::new(dir.path()));
    assert_eq!(tool.name(), "grep");
    assert_eq!(
        tool.description(),
        "Search file contents for a pattern. Returns matching lines with \
file paths and line numbers. Respects .gitignore. Output is truncated to \
100 matches or 50KB (whichever is hit first). Long lines are truncated to \
500 chars."
    );
    let schema = tool.parameters();
    assert_eq!(schema["required"], json!(["pattern"]));
    assert_eq!(
        schema["properties"]["ignoreCase"]["description"],
        json!("Case-insensitive search (default: false)")
    );
}

/// A file's content: a handful of short lines built from a narrow
/// alphabet (so "NEEDLE"/"needle" can never appear by accident), with
/// at most one line carrying the search token -- in either case, and
/// sometimes split by a U+2028 or U+2029 character rather than a
/// newline, which pi's `rg --json` parsing drops the match line for
/// (`grep.ts:169`) but a native scan must not
/// (`docs/reference/tools.md`, "grep").
#[hegel::composite]
fn grep_file(tc: TestCase) -> String {
    let line_count = tc.draw(gs::integers::<usize>().min_value(0).max_value(5));
    let needle_at = if line_count > 0 && tc.draw(gs::booleans()) {
        Some(
            tc.draw(
                gs::integers::<usize>()
                    .min_value(0)
                    .max_value(line_count - 1),
            ),
        )
    } else {
        None
    };
    let mut lines = Vec::new();
    for i in 0..line_count {
        let base = tc
            .draw(gs::text().alphabet("ab \tXY\u{2028}\u{2029}").max_size(10));
        let line = if Some(i) == needle_at {
            let upper = tc.draw(gs::booleans());
            let sep =
                tc.draw(gs::sampled_from(vec!["", "\u{2028}", "\u{2029}"]));
            format!("{base}{sep}{}", if upper { "NEEDLE" } else { "needle" })
        } else {
            base
        };
        lines.push(line);
    }
    lines.join("\n")
}

/// `content`'s lines the way line-oriented tools count them: none for
/// an empty string, and no empty line after a trailing `\n` (matching
/// `tau_tools::truncate::lines`, and how `grep-searcher` itself splits
/// a file).
fn lines_of(content: &str) -> Vec<&str> {
    if content.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<&str> = content.split('\n').collect();
    if content.ends_with('\n') {
        lines.pop();
    }
    lines
}

/// A naive, independent scan of `content`'s lines for the literal
/// "NEEDLE" (case-insensitively when `ignore_case`), returning the
/// 0-based index of the one line that can hold it (the generator never
/// places more than one).
fn find_needle(content: &str, ignore_case: bool) -> Option<usize> {
    let re = RegexBuilder::new(&regex::escape("NEEDLE"))
        .case_insensitive(ignore_case)
        .build()
        .unwrap();
    lines_of(content)
        .into_iter()
        .position(|line| re.is_match(line))
}

/// `grep` over a generated tree equals a naive regex scan of each
/// file's lines, including the match limit, the context window and the
/// exact `path:N:`/`path-N-` formatting (`docs/reference/tools.md`,
/// "grep"; `docs/reference/testing.md`, "tau-tools").
#[hegel::test(test_cases = 50)]
fn grep_matches_a_naive_scan(tc: TestCase) {
    let file_count = tc.draw(gs::integers::<usize>().min_value(1).max_value(3));
    let files: Vec<String> =
        (0..file_count).map(|_| tc.draw(grep_file())).collect();
    let ignore_case = tc.draw(gs::booleans());
    let literal = tc.draw(gs::booleans());
    let context = tc.draw(gs::integers::<usize>().min_value(0).max_value(2));
    let limit = tc.draw(gs::integers::<u32>().min_value(1).max_value(6));
    let only_first = tc.draw(gs::booleans());

    let dir = tempfile::tempdir().unwrap();
    let names: Vec<String> =
        (0..file_count).map(|i| format!("f{i}.txt")).collect();
    for (name, content) in names.iter().zip(&files) {
        std::fs::write(dir.path().join(name), content).unwrap();
    }
    let root = Root::new(dir.path());

    let mut args = json!({
        "pattern": "NEEDLE",
        "ignoreCase": ignore_case,
        "literal": literal,
        "context": context,
        "limit": limit,
    });
    if only_first {
        args["glob"] = json!(names[0]);
    }
    let actual = call(&root, args).unwrap();

    // Oracle: an independent scan, using `regex` (not `grep-regex`).
    let mut out_lines: Vec<String> = Vec::new();
    let mut match_count = 0u32;
    let mut limit_reached = false;
    let candidates: Vec<usize> = if only_first {
        vec![0]
    } else {
        (0..file_count).collect()
    };
    'files: for &fi in &candidates {
        let content = &files[fi];
        let Some(i) = find_needle(content, ignore_case) else {
            continue;
        };
        if match_count >= limit {
            limit_reached = true;
            break 'files;
        }
        match_count += 1;
        let lines = lines_of(content);
        let start = i.saturating_sub(context);
        let end = (i + context).min(lines.len() - 1);
        for (k, line) in lines.iter().enumerate().take(end + 1).skip(start) {
            let sep = if k == i { ':' } else { '-' };
            out_lines.push(format!(
                "{}{sep}{}{sep} {}",
                names[fi],
                k + 1,
                line
            ));
        }
    }

    let expected = if match_count == 0 {
        "No matches found".to_owned()
    } else {
        let raw = out_lines.join("\n");
        let truncation = truncate_head(&raw, usize::MAX, MAX_BYTES);
        let truncated = truncation.truncated();
        let mut text = truncation.content;
        let mut notices = Vec::new();
        if limit_reached {
            notices.push(format!(
                "{limit} matches limit reached. Use limit={} for more, or refine pattern",
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

    assert_eq!(
        actual, expected,
        "files = {files:?}, context = {context}, limit = {limit}"
    );
}

/// A single file passed as `path` is searched directly, and reported
/// under its own name rather than a path relative to some directory
/// (pi, `tools.test.ts:828`, "should include filename when searching a
/// single file").
#[test]
fn single_file_path_reports_its_own_name() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("example.txt");
    std::fs::write(&file, "first line\nmatch line\nlast line").unwrap();
    let root = Root::new(dir.path());

    let output = call(
        &root,
        json!({"pattern": "match", "path": file.to_string_lossy()}),
    )
    .unwrap();
    assert!(output.contains("example.txt:2: match line"), "{output}");
}

/// The match limit and context window, pinned exactly as pi's own test
/// expects them (`tools.test.ts:842`, "should respect global limit and
/// include context lines"): once the limit is reached mid-file, the
/// rejected match's own context (here, "middle") never appears either.
#[test]
fn limit_reached_notice_and_context_lines() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("context.txt");
    std::fs::write(
        &file,
        "before\nmatch one\nafter\nmiddle\nmatch two\nafter two",
    )
    .unwrap();
    let root = Root::new(dir.path());

    let output = call(
        &root,
        json!({
            "pattern": "match",
            "path": file.to_string_lossy(),
            "limit": 1,
            "context": 1,
        }),
    )
    .unwrap();
    assert!(output.contains("context.txt-1- before"), "{output}");
    assert!(output.contains("context.txt:2: match one"), "{output}");
    assert!(output.contains("context.txt-3- after"), "{output}");
    assert!(
        output.contains(
            "[1 matches limit reached. Use limit=2 for more, or refine pattern]"
        ),
        "{output}"
    );
    assert!(!output.contains("middle"), "{output}");
    assert!(!output.contains("match two"), "{output}");
}

/// No matches at all is reported with a fixed string, not an empty
/// result (`grep.ts`).
#[test]
fn no_matches_found() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "nothing here").unwrap();
    let root = Root::new(dir.path());
    let output = call(&root, json!({"pattern": "zzz"})).unwrap();
    assert_eq!(output, "No matches found");
}

/// A path that does not exist is an error, not an empty result.
#[test]
fn missing_path_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let root = Root::new(dir.path());
    let error =
        call(&root, json!({"pattern": "x", "path": "missing"})).unwrap_err();
    assert!(error.to_string().contains("Path not found"), "{error}");
}

/// Hidden files are searched, and `.gitignore` is respected
/// (`docs/reference/tools.md`, "grep").
#[test]
fn hidden_files_included_gitignore_respected() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".gitignore"), "ignored.txt\n").unwrap();
    std::fs::write(dir.path().join(".hidden.txt"), "needle here").unwrap();
    std::fs::write(dir.path().join("ignored.txt"), "needle here").unwrap();
    std::fs::write(dir.path().join("kept.txt"), "needle here").unwrap();
    let root = Root::new(dir.path());

    let output = call(&root, json!({"pattern": "needle"})).unwrap();
    assert!(output.contains(".hidden.txt"), "{output}");
    assert!(output.contains("kept.txt"), "{output}");
    assert!(!output.contains("ignored.txt"), "{output}");
}

/// The user's own global gitignore (`core.excludesFile`) is not part
/// of the spec and must not leak into results just because the search
/// tree has no `.git` directory of its own.
#[test]
fn global_gitignore_is_not_consulted() {
    let dir = tempfile::tempdir().unwrap();
    // A name commonly listed in a personal global gitignore.
    std::fs::write(dir.path().join(".env"), "needle").unwrap();
    let root = Root::new(dir.path());
    let output = call(&root, json!({"pattern": "needle"})).unwrap();
    assert!(output.contains(".env"), "{output}");
}

/// The `glob` argument filters which files are searched, matching a
/// bare name at any depth (`docs/reference/tools.md`, "grep").
#[test]
fn glob_filters_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("keep.rs"), "needle").unwrap();
    std::fs::write(dir.path().join("skip.txt"), "needle").unwrap();
    let root = Root::new(dir.path());

    let output =
        call(&root, json!({"pattern": "needle", "glob": "*.rs"})).unwrap();
    assert!(output.contains("keep.rs"), "{output}");
    assert!(!output.contains("skip.txt"), "{output}");
}
