//! `ls` (`tau_tools_host::ls::Ls`), against `docs/reference/tools.md` ("ls")
//! and pi's `ls.ts`, on a real directory (`docs/reference/testing.md`,
//! "tau-tools").

use hegel::{TestCase, generators as gs};
use serde_json::json;
use tau_agent::{
    error::ToolError,
    tool::{AgentTool, ToolCtx, ToolOutput},
    validation::ArgumentSchema,
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
}

/// A listing cut at the entry limit holds only what the model got,
/// and says it was cut.
#[test]
fn details_stop_where_the_text_does() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(dir.path().join(name), "x").unwrap();
    }
    let root = Root::new(dir.path());
    let listing = listing(&root, json!({"limit": 2}));
    assert_eq!(listing.entries.len(), 2);
    assert!(listing.truncated);
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

/// The exact entry limit is complete; only finding an additional name
/// marks the bounded result incomplete.
#[test]
fn structured_entry_limit_distinguishes_exact_from_exceeded() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(dir.path().join(name), "x").unwrap();
    }
    let root = Root::new(dir.path());

    let exact = structured(&root, json!({"limit": 3}));
    assert_eq!(exact["entries"].as_array().unwrap().len(), 3);
    assert_eq!(exact["truncated"], false);
    assert_eq!(exact["complete"], true);

    let exceeded = structured(&root, json!({"limit": 2}));
    let names: Vec<&str> = exceeded["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["a.txt", "b.txt"]);
    assert_eq!(exceeded["truncated"], true);
    assert_eq!(exceeded["complete"], false);
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

/// Symlink records preserve Entry's followed kind and link target.
#[cfg(unix)]
#[test]
fn structured_symlink_metadata_matches_details_entries() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("target")).unwrap();
    std::os::unix::fs::symlink("target", dir.path().join("shortcut")).unwrap();
    let root = Root::new(dir.path());

    let value = structured(&root, json!({}));
    let shortcut = value["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "shortcut")
        .unwrap();
    assert_eq!(shortcut["kind"], "symlink_dir");
    assert_eq!(shortcut["target"], "target");
    assert_eq!(shortcut["items"], 0);
}

/// The schema advertised by AgentTool accepts the structured success
/// value through tau-agent's existing schema validation API.
#[test]
fn structured_output_validates_against_its_schema() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "hello").unwrap();
    let tool = Ls::new(Root::new(dir.path()));
    let schema = ArgumentSchema::new(
        tool.output_schema().expect("ls has an output schema"),
    )
    .expect("the output schema compiles");
    let output =
        block_on(tool.call(json!({}), ToolCtx::detached())).expect("ls runs");
    let value = output.structured.expect("a structured listing");

    schema
        .validate(&value)
        .expect("the listing matches its schema");
}

/// Property inventory: structured names equal the independent, sorted names
/// read back from the temporary filesystem. This checks record identity
/// without using the text renderer as an oracle.
///
/// The generator uses safe ASCII fragments and appends the drawn index, so
/// every generated name is valid and unique without filtering or rejection;
/// shrinking shortens fragments and the index still prevents collisions.
/// The test uses 100 cases locally and in CI. Workspace hegel.toml already
/// selects the fixed-seed, derandomized CI profile; change suite-wide counts
/// there, and use a per-test count only for a deliberately different cost.
#[hegel::composite]
fn safe_ls_names(tc: &TestCase) -> Vec<String> {
    let fragments: Vec<String> = tc.draw(
        gs::vecs(
            gs::text()
                .alphabet("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-")
                .min_size(1)
                .max_size(12),
        )
        .max_size(12),
    );
    fragments
        .into_iter()
        .enumerate()
        .map(|(index, fragment)| format!("{fragment}-{index}"))
        .collect()
}

#[hegel::test(test_cases = 100)]
fn structured_names_match_sorted_filesystem_oracle(tc: TestCase) {
    let names = tc.draw(safe_ls_names());
    let dir = tempfile::tempdir().unwrap();
    for name in &names {
        std::fs::write(dir.path().join(name), "x").unwrap();
    }

    let mut expected: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| {
            entry
                .unwrap()
                .file_name()
                .into_string()
                .expect("generated filenames are valid UTF-8")
        })
        .collect();
    expected.sort_by(|a, b| {
        a.to_lowercase()
            .cmp(&b.to_lowercase())
            .then_with(|| a.cmp(b))
    });

    let actual = structured(&Root::new(dir.path()), json!({}));
    let actual_names: Vec<&str> = actual["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap())
        .collect();
    assert_eq!(actual_names, expected);
    assert_eq!(actual["complete"], true);
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
