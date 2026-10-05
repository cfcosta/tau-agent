//! `find` (`tau_tools_host::find::Find`), against `docs/reference/tools.md`
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
use tau_tools_host::{
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

fn call_output(
    root: &Root,
    args: serde_json::Value,
) -> Result<ToolOutput, ToolError> {
    block_on(async {
        Find::new(root.clone())
            .call(args, ToolCtx::detached())
            .await
    })
}

fn call(root: &Root, args: serde_json::Value) -> Result<String, ToolError> {
    call_output(root, args).map(|out| text_of(&out).to_owned())
}

fn structured_of(output: &ToolOutput) -> &serde_json::Value {
    output
        .structured
        .as_ref()
        .expect("find returns structured output")
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

    let output_schema = tool.output_schema().unwrap();
    assert_eq!(output_schema["type"], "object");
    assert_eq!(
        output_schema["required"],
        json!([
            "root",
            "paths",
            "truncated",
            "complete",
            "limit_reached",
            "bytes_truncated",
            "skipped"
        ])
    );
    let properties = &output_schema["properties"];
    assert_eq!(properties["root"]["type"], "string");
    assert_eq!(properties["paths"]["type"], "array");
    assert_eq!(properties["paths"]["items"]["type"], "string");
    for field in ["truncated", "complete", "limit_reached", "bytes_truncated"] {
        assert_eq!(properties[field]["type"], "boolean", "{field}");
    }
    assert_eq!(properties["skipped"]["type"], "integer");
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

/// Alphabet used to generate portable, single-component file names and glob
/// prefixes. It excludes path separators, control characters, and glob syntax.
const SAFE_STEM_CHARS: &[char] = &[
    'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o',
    'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z', '0', '1', '2', '3',
    '4', '5', '6', '7', '8', '9', '_', '-', '.',
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

#[hegel::composite]
fn safe_stem(tc: &TestCase) -> String {
    let chars = tc.draw(
        gs::vecs(gs::sampled_from(SAFE_STEM_CHARS.to_vec()))
            .min_size(1)
            .max_size(10),
    );
    chars.into_iter().collect()
}

#[derive(Debug, hegel::PrettyPrintable)]
struct GeneratedFindCase {
    filenames: Vec<String>,
    prefix: String,
    use_prefix: bool,
}

#[hegel::composite]
fn generated_find_case(tc: &TestCase) -> GeneratedFindCase {
    let stems = tc.draw(gs::vecs(safe_stem()).max_size(12));
    let mut filenames: Vec<String> = stems
        .into_iter()
        .map(|stem| format!("{stem}.txt"))
        .collect();
    filenames.sort();
    filenames.dedup();

    let prefix_chars = tc
        .draw(gs::vecs(gs::sampled_from(SAFE_STEM_CHARS.to_vec())).max_size(4));
    let prefix = prefix_chars.into_iter().collect();
    let use_prefix = tc.draw(gs::booleans());
    GeneratedFindCase {
        filenames,
        prefix,
        use_prefix,
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
// Property inventory: find_matches_a_naive_walk is differential against
// an independent recursive filesystem walk plus a narrow .gitignore model;
// it checks display text, selected paths, ordering, and entry-limit reporting.
// generated_file_names_match_the_naive_prefix_oracle independently checks
// generated filename sets against a plain starts_with oracle for the safe
// prefix*.txt / *.txt glob subset. These laws catch path selection and
// ordering regressions without reusing the production walker or glob matcher.
//
// Generator plan: generated stems use a bounded ASCII alphabet with no path
// separators, control characters, or glob metacharacters. A vector is sorted
// and deduplicated after drawing, so every case is a valid filesystem tree and
// shrinking keeps it valid without rejection. Prefixes use the same alphabet.
//
// CI: Hegel auto-selects its ci profile (derandomized, with database use
// disabled); workspace-wide case counts belong in hegel.toml. This property
// runs 50 cases because each one creates a temporary directory and files.
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

    let output = call_output(
        &root,
        json!({"pattern": case.pattern, "limit": case.limit}),
    )
    .unwrap();
    let actual = text_of(&output);

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
    let structured = structured_of(&output);
    assert_eq!(structured["paths"], json!(matched));
    assert_eq!(structured["limit_reached"], limit_reached);
    assert_eq!(structured["bytes_truncated"], false);
    assert_eq!(structured["truncated"], limit_reached);
    assert_eq!(structured["complete"], !limit_reached);
    assert_eq!(structured["skipped"], 0);
}

/// Compares real results against a minimal naive oracle for generated file
/// names and a safe subset of prefix globs. The oracle enumerates with
/// read_dir and checks string prefixes directly, independently of ignore and
/// globset.
#[hegel::test(test_cases = 50)]
fn generated_file_names_match_the_naive_prefix_oracle(tc: TestCase) {
    let case = tc.draw(generated_find_case());
    let dir = tempfile::tempdir().unwrap();
    for filename in &case.filenames {
        std::fs::write(dir.path().join(filename), "x").unwrap();
    }

    let prefix = if case.use_prefix {
        case.prefix.as_str()
    } else {
        ""
    };
    let pattern = if case.use_prefix {
        format!("{prefix}*.txt")
    } else {
        "*.txt".to_owned()
    };
    let root = Root::new(dir.path());
    let output = call_output(&root, json!({"pattern": pattern})).unwrap();

    let mut expected: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|filename| {
            filename.starts_with(prefix) && filename.ends_with(".txt")
        })
        .collect();
    expected.sort();

    assert_eq!(structured_of(&output)["paths"], json!(expected));
    let expected_text = if expected.is_empty() {
        "No files found matching pattern".to_owned()
    } else {
        expected.join("\n")
    };
    assert_eq!(text_of(&output), expected_text);
    assert_eq!(structured_of(&output)["complete"], true);
    assert_eq!(structured_of(&output)["truncated"], false);
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
    let output = call_output(&root, json!({"pattern": "*.zzz"})).unwrap();
    assert_eq!(text_of(&output), "No files found matching pattern");
    assert_eq!(
        structured_of(&output),
        &json!({
            "root": dir.path().to_string_lossy(),
            "paths": [],
            "truncated": false,
            "complete": true,
            "limit_reached": false,
            "bytes_truncated": false,
            "skipped": 0
        })
    );
}

/// Zero, exact, and exceeded entry limits keep their existing display text and
/// report the selected records and completeness separately.
#[test]
fn zero_exact_and_exceeded_limits_report_structured_paths() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(dir.path().join(name), "x").unwrap();
    }
    let root = Root::new(dir.path());

    let zero =
        call_output(&root, json!({"pattern": "*.txt", "limit": 0})).unwrap();
    assert_eq!(
        text_of(&zero),
        "\n\n[0 results limit reached. Use limit=0 for more, or refine pattern]"
    );
    assert_eq!(structured_of(&zero)["paths"], json!([]));
    assert_eq!(structured_of(&zero)["limit_reached"], true);
    assert_eq!(structured_of(&zero)["truncated"], true);
    assert_eq!(structured_of(&zero)["complete"], false);

    let exact =
        call_output(&root, json!({"pattern": "*.txt", "limit": 3})).unwrap();
    assert_eq!(text_of(&exact), "a.txt\nb.txt\nc.txt");
    assert_eq!(
        structured_of(&exact)["paths"],
        json!(["a.txt", "b.txt", "c.txt"])
    );
    assert_eq!(structured_of(&exact)["limit_reached"], false);
    assert_eq!(structured_of(&exact)["truncated"], false);
    assert_eq!(structured_of(&exact)["complete"], true);

    let exceeded =
        call_output(&root, json!({"pattern": "*.txt", "limit": 2})).unwrap();
    assert_eq!(
        text_of(&exceeded),
        "a.txt\nb.txt\n\n[2 results limit reached. Use limit=4 for more, or refine pattern]"
    );
    assert_eq!(structured_of(&exceeded)["paths"], json!(["a.txt", "b.txt"]));
    assert_eq!(structured_of(&exceeded)["limit_reached"], true);
    assert_eq!(structured_of(&exceeded)["truncated"], true);
    assert_eq!(structured_of(&exceeded)["complete"], false);
}

/// Complete path records survive display formatting even when a valid path
/// contains colons, punctuation, Unicode, or a newline.
#[test]
fn structured_paths_preserve_punctuation_unicode_and_newlines() {
    let dir = tempfile::tempdir().unwrap();
    let mut expected = vec![
        "colon:é.txt".to_owned(),
        "line\nbreak.txt".to_owned(),
        "space (x)! [y].txt".to_owned(),
    ];
    expected.sort();
    for name in &expected {
        std::fs::write(dir.path().join(name), "x").unwrap();
    }
    let root = Root::new(dir.path());

    let output = call_output(&root, json!({"pattern": "*"})).unwrap();
    assert_eq!(text_of(&output), expected.join("\n"));
    assert_eq!(structured_of(&output)["paths"], json!(expected));
    assert_eq!(structured_of(&output)["complete"], true);
}

/// The byte cut applies only to the display text. Structured paths retain
/// every complete record selected before that presentation cut.
#[test]
fn byte_truncation_keeps_complete_structured_paths() {
    let dir = tempfile::tempdir().unwrap();
    let file_count = 220usize;
    for index in 0..file_count {
        let name = format!("{index:03}_{}.txt", "x".repeat(242));
        std::fs::write(dir.path().join(name), "x").unwrap();
    }
    let root = Root::new(dir.path());

    let output = call_output(&root, json!({"pattern": "*.txt"})).unwrap();
    let structured = structured_of(&output);
    assert!(text_of(&output).len() <= MAX_BYTES + 64);
    assert_eq!(structured["paths"].as_array().unwrap().len(), file_count);
    assert_eq!(structured["bytes_truncated"], true);
    assert_eq!(structured["truncated"], true);
    assert_eq!(structured["limit_reached"], false);
    assert_eq!(structured["complete"], false);
    assert_eq!(structured["skipped"], 0);
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
