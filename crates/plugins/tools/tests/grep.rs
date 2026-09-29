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

/// The pieces a generated line is built from: the search tokens in
/// either case, the look-alike `NEEXLE` (matched by the regex `NEE.LE`
/// but not by the literal), runs of `a`/`b` for `a+b`, a multi-byte
/// letter in both cases, and U+2028, which pi's `rg --json` parsing
/// drops the match line for (`grep.ts:169`) but a native scan must not
/// (`docs/reference/tools.md`, "grep").
const TOKENS: &[&str] = &[
    "NEEDLE", "needle", "NEE.LE", "nee.le", "NEEXLE", "a", "b", "ab", "aab",
    "X", "é", "É", " ", "\t", "\u{2028}", "\u{2029}",
];

/// The patterns searched for. Each is tried both as a regex and as a
/// literal, which differ for `NEE.LE` and `a+b`.
const PATTERNS: &[&str] = &["NEEDLE", "NEE.LE", "a+b", "é"];

/// One line: a few tokens, sometimes followed by a run of several
/// hundred characters (one or two bytes each), so a line can go past
/// the 500-character cut and the output past 50 KB.
#[hegel::composite]
fn grep_line(tc: &TestCase, long_lines: bool) -> String {
    let tokens: Vec<&str> =
        tc.draw(gs::vecs(gs::sampled_from(TOKENS)).max_size(6));
    let mut line = tokens.concat();
    let p = if long_lines { 0.8 } else { 0.05 };
    if tc.draw(gs::weighted_booleans(p)) {
        let filler = tc.draw(gs::sampled_from(vec!["a", "é", "x"]));
        let count =
            tc.draw(gs::integers::<usize>().min_value(400).max_value(700));
        line.push_str(&filler.repeat(count));
    }
    line
}

/// A file's content: up to a dozen lines, any of which may match, so
/// matches share a file, context windows meet and the limit can be
/// reached in the middle of a file.
#[hegel::composite]
fn grep_file(tc: &TestCase, long_lines: bool) -> String {
    let lines: Vec<String> =
        tc.draw(gs::vecs(grep_line(long_lines)).max_size(12));
    let mut content = lines.join("\n");
    if !content.is_empty() && tc.draw(gs::booleans()) {
        content.push('\n');
    }
    content
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

/// How a line shows up in `grep`'s output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shown {
    Match,
    /// Within `context` lines after an earlier match.
    After,
    /// Within `context` lines before a later match, and not after one.
    Before,
}

/// Each shown line of a file, by index, from the definition of a
/// context window: a matching line is a match; any other line is shown
/// when a match lies within `context` lines of it, as after-context when
/// that match comes first, else as before-context. Every line is shown
/// at most once, so overlapping windows merge.
fn shown_lines(matches: &[bool], context: usize) -> Vec<(usize, Shown)> {
    let n = matches.len();
    (0..n)
        .filter_map(|k| {
            if matches[k] {
                return Some((k, Shown::Match));
            }
            let before = k.saturating_sub(context)..k;
            if matches[before].iter().any(|&m| m) {
                return Some((k, Shown::After));
            }
            let after = (k + 1).min(n)..(k + context + 1).min(n);
            matches[after]
                .iter()
                .any(|&m| m)
                .then_some((k, Shown::Before))
        })
        .collect()
}

/// A line cut to 500 characters, marked, and whether it was cut.
fn cut_line(line: &str) -> (String, bool) {
    if line.chars().count() <= 500 {
        (line.to_owned(), false)
    } else {
        let kept: String = line.chars().take(500).collect();
        (format!("{kept}... [truncated]"), true)
    }
}

/// `grep` over a generated tree (files at several depths) equals a
/// naive regex scan of each file's lines, in path order: the same
/// matches, context windows merged where they meet, the match limit
/// reached possibly in the middle of a file (a rejected match shows
/// none of its own before-context), lines cut at 500 characters, the
/// output cut at 50 KB, and the exact `path:N:`/`path-N-` formatting and
/// notices (`docs/reference/tools.md`, "grep";
/// `docs/reference/testing.md`, "tau-tools").
#[hegel::test(test_cases = 80)]
fn grep_matches_a_naive_scan(tc: TestCase) {
    // Sometimes a bulk case: many files of mostly long lines, wide
    // context and a high limit, so the output can pass 50 KB.
    let bulk = tc.draw(gs::weighted_booleans(0.2));
    // Enough files that the search workers finish them out of order.
    let file_count = tc.draw(
        gs::integers::<usize>()
            .min_value(if bulk { 20 } else { 1 })
            .max_value(40),
    );
    let long_lines = bulk;
    let files: Vec<String> = (0..file_count)
        .map(|_| tc.draw(grep_file(long_lines)))
        .collect();
    let dirs: Vec<&str> = (0..file_count)
        .map(|_| {
            tc.draw(gs::sampled_from(vec!["", "d/", "d/e/", "d.x/", "D/"]))
        })
        .collect();
    let pattern = tc.draw(gs::sampled_from(PATTERNS));
    let ignore_case = tc.draw(gs::booleans());
    let literal = tc.draw(gs::booleans());
    let (context, limit) = if bulk {
        (3, 200)
    } else {
        (
            tc.draw(gs::integers::<usize>().min_value(0).max_value(3)),
            tc.draw(gs::one_of(vec![
                gs::integers::<u32>().min_value(1).max_value(6),
                gs::integers::<u32>().min_value(1).max_value(200),
            ])),
        )
    };
    let only_first = tc.draw(gs::booleans());

    let dir = tempfile::tempdir().unwrap();
    let names: Vec<String> = (0..file_count)
        .map(|i| format!("{}f{i:02}.txt", dirs[i]))
        .collect();
    for (name, content) in names.iter().zip(&files) {
        let path = dir.path().join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    let root = Root::new(dir.path());

    let mut args = json!({
        "pattern": pattern,
        "ignoreCase": ignore_case,
        "literal": literal,
        "context": context,
        "limit": limit,
    });
    if only_first {
        // A bare name matches at any depth; the names are unique.
        args["glob"] = json!("f00.txt");
    }
    let actual = call(&root, args).unwrap();

    // Oracle: an independent scan, using `regex` (not `grep-regex`),
    // over the files in path order (component by component).
    let re = RegexBuilder::new(&if literal {
        regex::escape(pattern)
    } else {
        pattern.to_owned()
    })
    .case_insensitive(ignore_case)
    .build()
    .unwrap();
    let mut order: Vec<usize> = if only_first {
        vec![0]
    } else {
        (0..file_count).collect()
    };
    order.sort_by_key(|&i| names[i].split('/').collect::<Vec<_>>());

    let mut out_lines: Vec<String> = Vec::new();
    let mut match_count = 0u32;
    let mut limit_reached = false;
    let mut lines_cut = false;
    'files: for &fi in &order {
        let lines = lines_of(&files[fi]);
        let matches: Vec<bool> =
            lines.iter().map(|line| re.is_match(line)).collect();
        if matches.iter().filter(|&&m| m).count() > 1 {
            tc.event("several matches in a file");
        }
        let shown = shown_lines(&matches, context);
        if context > 0
            && shown.windows(2).any(|w| {
                w[0].1 == Shown::After
                    && w[1].0 == w[0].0 + 1
                    && matches!(w[1].1, Shown::Before | Shown::Match)
            })
        {
            tc.event("context windows merged");
        }
        let mut push = |k: usize, sep: char, out: &mut Vec<String>| {
            let (text, cut) = cut_line(lines[k]);
            lines_cut |= cut;
            out.push(format!("{}{sep}{}{sep} {text}", names[fi], k + 1));
        };
        let mut pending = Vec::new();
        for (k, kind) in shown {
            match kind {
                Shown::Before => pending.push(k),
                Shown::Match => {
                    if match_count >= limit {
                        limit_reached = true;
                        break 'files;
                    }
                    match_count += 1;
                    for before in pending.drain(..) {
                        push(before, '-', &mut out_lines);
                    }
                    push(k, ':', &mut out_lines);
                }
                Shown::After => push(k, '-', &mut out_lines),
            }
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
            tc.event("match limit reached");
            notices.push(format!(
                "{limit} matches limit reached. Use limit={} for more, or refine pattern",
                limit * 2
            ));
        }
        if truncated {
            tc.event("output cut at 50 KB");
            notices.push(format!("{} limit reached", format_size(MAX_BYTES)));
        }
        if lines_cut {
            tc.event("a line cut at 500 characters");
            notices.push(
                "Some lines truncated to 500 chars. Use read tool to see full lines"
                    .to_owned(),
            );
        }
        if !notices.is_empty() {
            text.push_str(&format!("\n\n[{}]", notices.join(". ")));
        }
        text
    };

    assert_eq!(
        actual, expected,
        "pattern = {pattern:?}, literal = {literal}, context = {context}, limit = {limit}"
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
