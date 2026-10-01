//! `find` (`tau_tools::find::Find`), against `docs/reference/tools.md`
//! ("find") and pi's `find.ts`, on a real directory
//! (`docs/reference/testing.md`, "tau-tools").

use std::path::Path;

use globset::GlobBuilder;
use hegel::{TestCase, generators as gs};
use serde_json::json;
use tau_agent::{
    error::ToolError,
    tool::{AgentTool, ToolCtx, ToolOutput},
};
use tau_ai::message::InputBlock;
use tau_testing::block_on;
use tau_tools::{
    find::Find,
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
        Find::new(root.clone())
            .call(args, ToolCtx::detached())
            .await
            .map(|out| text_of(&out).to_owned())
    })
}

/// The tool's identity and pinned description string, verbatim from
/// pi's `find.ts` except for the ported limits
/// (`docs/reference/tools.md`, "find").
#[test]
fn name_description_and_schema() {
    let dir = tempfile::tempdir().unwrap();
    let tool = Find::new(Root::new(dir.path()));
    assert_eq!(tool.name(), "find");
    assert_eq!(
        tool.description(),
        "Search for files by glob pattern. Returns matching file paths \
relative to the search directory. Respects .gitignore. Output is \
truncated to 1000 results or 50KB (whichever is hit first)."
    );
    let schema = tool.parameters();
    assert_eq!(schema["required"], json!(["pattern"]));
    assert_eq!(
        schema["properties"]["limit"]["description"],
        json!("Maximum number of results (default: 1000)")
    );
}

/// The candidate files a generated tree draws from: a mix of root and
/// nested files, extensions and a dotfile, so both basename globs and
/// full-path globs (with an implicit `**/` prefix) have something to
/// match (`docs/reference/tools.md`, "find").
const CANDIDATES: &[&str] = &[
    "a.txt",
    "b.txt",
    "c.rs",
    "sub/d.txt",
    "sub/e.rs",
    "sub/f.md",
    ".hidden.txt",
];

/// `find`'s glob patterns exercised by the property: a bare basename
/// pattern, an extension pattern, an anchored `**/` pattern, a
/// slash-pattern that needs the implicit `**/` prefix
/// (`docs/reference/tools.md`, "find", and the known case "a glob with
/// `/` matches the full path", pi issue #3302), an exact name and a
/// dotfile.
const PATTERNS: &[&str] = &[
    "*.txt",
    "*.rs",
    "**/*.md",
    "sub/*.rs",
    "d.txt",
    ".hidden.txt",
];

/// A hand-written `.gitignore` subset: either no `.gitignore`, one
/// that excludes an extension (`*.md`), or one that excludes an exact
/// name (`b.txt`) -- at any depth, as a slash-free gitignore line does.
#[derive(Debug, Clone, Copy, hegel::PrettyPrintable)]
enum GitignoreCase {
    None,
    ByExt,
    ByName,
}

#[derive(Debug, hegel::PrettyPrintable)]
struct FindCase {
    present: Vec<&'static str>,
    gitignore: GitignoreCase,
    pattern: &'static str,
    limit: u32,
}

#[hegel::composite]
fn find_case(tc: &TestCase) -> FindCase {
    let present = tc.draw(gs::subsequences(CANDIDATES));
    let gitignore = tc.draw(gs::sampled_from(vec![
        GitignoreCase::None,
        GitignoreCase::ByExt,
        GitignoreCase::ByName,
    ]));
    let pattern = tc.draw(gs::sampled_from(PATTERNS.to_vec()));
    let limit = tc.draw(gs::integers::<u32>().min_value(1).max_value(6));
    FindCase {
        present,
        gitignore,
        pattern,
        limit,
    }
}

/// A naive recursive walk with `std::fs::read_dir`, sorted per
/// directory -- comparing full relative paths component-wise gives the
/// same order, so this matches `find`'s sorted file list without using
/// `ignore` (`docs/reference/testing.md`, "Choosing a property").
fn naive_walk(dir: &Path, rel_prefix: &str, out: &mut Vec<String>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = if rel_prefix.is_empty() {
            name
        } else {
            format!("{rel_prefix}/{name}")
        };
        let file_type = entry.file_type().unwrap();
        if file_type.is_dir() {
            naive_walk(&entry.path(), &rel, out);
        } else if file_type.is_file() {
            out.push(rel);
        }
    }
}

/// Whether a hand-written `.gitignore` subset excludes `rel` (a
/// slash-free pattern matches a basename at any depth, as a real
/// `.gitignore` line without `/` does).
fn gitignore_excludes(case: GitignoreCase, rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    match case {
        GitignoreCase::None => false,
        GitignoreCase::ByExt => name.ends_with(".md"),
        GitignoreCase::ByName => name == "b.txt",
    }
}

/// `find`'s own `pattern` -> (full_path_mode, compiled globset pattern)
/// transform (`docs/reference/tools.md`, "find"): shared with
/// production, since the property is about the walk, not the glob
/// syntax (`docs/reference/testing.md`, "tau-tools").
fn effective_pattern(pattern: &str) -> (bool, String) {
    if !pattern.contains('/') {
        return (false, pattern.to_owned());
    }
    let full = if pattern.starts_with('/')
        || pattern.starts_with("**/")
        || pattern == "**"
    {
        pattern.to_owned()
    } else {
        format!("**/{pattern}")
    };
    (true, full)
}

/// `find` equals `globset` matching over a naive walk that respects a
/// hand-written `.gitignore` subset; "limit reached" appears only when
/// more results existed (`docs/reference/tools.md`, "find";
/// `docs/reference/testing.md`, "tau-tools").
#[hegel::test(test_cases = 50)]
fn find_matches_a_naive_walk(tc: TestCase) {
    let case = tc.draw(find_case());

    let dir = tempfile::tempdir().unwrap();
    for candidate in &case.present {
        let path = dir.path().join(candidate);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, "x").unwrap();
    }
    match case.gitignore {
        GitignoreCase::None => {}
        GitignoreCase::ByExt => {
            std::fs::write(dir.path().join(".gitignore"), "*.md\n").unwrap();
        }
        GitignoreCase::ByName => {
            std::fs::write(dir.path().join(".gitignore"), "b.txt\n").unwrap();
        }
    }
    let root = Root::new(dir.path());

    let actual =
        call(&root, json!({"pattern": case.pattern, "limit": case.limit}))
            .unwrap();

    // Oracle: a naive walk plus the same globset pattern, not `ignore`.
    let mut all_files = Vec::new();
    naive_walk(dir.path(), "", &mut all_files);
    let (full_path_mode, glob_pattern) = effective_pattern(case.pattern);
    let glob = GlobBuilder::new(&glob_pattern)
        .literal_separator(true)
        .build()
        .unwrap()
        .compile_matcher();

    let mut matched: Vec<String> = Vec::new();
    let mut limit_reached = false;
    for rel in &all_files {
        if rel == ".gitignore" || gitignore_excludes(case.gitignore, rel) {
            continue;
        }
        let candidate = if full_path_mode {
            rel.clone()
        } else {
            rel.rsplit('/').next().unwrap_or(rel).to_owned()
        };
        if !glob.is_match(&candidate) {
            continue;
        }
        if matched.len() >= case.limit as usize {
            limit_reached = true;
            break;
        }
        matched.push(rel.clone());
    }

    let expected = if matched.is_empty() && !limit_reached {
        "No files found matching pattern".to_owned()
    } else {
        let raw = matched.join("\n");
        let truncation = truncate_head(&raw, usize::MAX, MAX_BYTES);
        let truncated = truncation.truncated();
        let mut text = truncation.content;
        let mut notices = Vec::new();
        if limit_reached {
            notices.push(format!(
                "{} results limit reached. Use limit={} for more, or refine pattern",
                case.limit,
                case.limit * 2
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
        "pattern = {:?}, present = {:?}, gitignore = {:?}, limit = {}",
        case.pattern, case.present, case.gitignore, case.limit
    );
}

/// A nested `.gitignore` applies only to its own subtree, not to
/// sibling directories or the parent (pi issue #3303).
#[test]
fn nested_gitignore_applies_only_to_its_subtree() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("sub")).unwrap();
    std::fs::create_dir_all(dir.path().join("other")).unwrap();
    // The nested .gitignore excludes "skip.txt", but only inside "sub".
    std::fs::write(dir.path().join("sub").join(".gitignore"), "skip.txt\n")
        .unwrap();
    std::fs::write(dir.path().join("sub").join("skip.txt"), "x").unwrap();
    std::fs::write(dir.path().join("other").join("skip.txt"), "x").unwrap();
    let root = Root::new(dir.path());

    let output = call(&root, json!({"pattern": "**/skip.txt"})).unwrap();
    assert!(output.contains("other/skip.txt"), "{output}");
    assert!(!output.contains("sub/skip.txt"), "{output}");
}

/// A glob containing `/` matches the full path, with an implicit `**/`
/// prefix (`docs/reference/tools.md`, "find"; pi issue #3302).
#[test]
fn slash_pattern_matches_the_full_path() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src").join("lib.spec.ts"), "x").unwrap();
    std::fs::write(dir.path().join("lib.spec.ts"), "x").unwrap();
    let root = Root::new(dir.path());

    let output = call(&root, json!({"pattern": "src/**/*.spec.ts"})).unwrap();
    assert!(output.contains("src/lib.spec.ts"), "{output}");
    assert!(!output.contains("\nlib.spec.ts"), "{output}");
}

/// Searching from an absolute path outside the tool's own root works
/// (pi issue #6104, "search from /"): the resolved path is honored as
/// given, not joined onto the root a second time.
#[test]
fn search_from_an_absolute_path() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("found.txt"), "x").unwrap();
    // The tool's own root is unrelated; the model passed an absolute path.
    let elsewhere = tempfile::tempdir().unwrap();
    let root = Root::new(elsewhere.path());

    let output = call(
        &root,
        json!({"pattern": "*.txt", "path": dir.path().to_string_lossy()}),
    )
    .unwrap();
    assert!(output.contains("found.txt"), "{output}");
}

/// Hidden files are included in the walk (`docs/reference/tools.md`,
/// "find").
#[test]
fn hidden_files_are_found() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".env"), "x").unwrap();
    let root = Root::new(dir.path());
    let output = call(&root, json!({"pattern": ".env"})).unwrap();
    assert_eq!(output, ".env");
}

/// The user's own global gitignore (`core.excludesFile`) is not part
/// of the spec and must not leak into results just because the search
/// tree has no `.git` directory of its own.
#[test]
fn global_gitignore_is_not_consulted() {
    let dir = tempfile::tempdir().unwrap();
    // A name commonly listed in a personal global gitignore.
    std::fs::write(dir.path().join(".env"), "x").unwrap();
    let root = Root::new(dir.path());
    let output = call(&root, json!({"pattern": "*"})).unwrap();
    assert_eq!(output, ".env");
}

/// No matches at all is a fixed string (`find.ts`).
#[test]
fn no_files_found() {
    let dir = tempfile::tempdir().unwrap();
    let root = Root::new(dir.path());
    let output = call(&root, json!({"pattern": "*.zzz"})).unwrap();
    assert_eq!(output, "No files found matching pattern");
}

/// A path that does not exist is an error.
#[test]
fn missing_path_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let root = Root::new(dir.path());
    let error =
        call(&root, json!({"pattern": "*", "path": "missing"})).unwrap_err();
    assert!(error.to_string().contains("Path not found"), "{error}");
}
