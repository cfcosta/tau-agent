//! `edit` (`docs/reference/tools.md`, "edit"), ported from pi's
//! `edit.ts`, `edit-diff.ts` and `tools.test.ts`.

use std::os::unix::fs::{PermissionsExt, symlink};

use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_agent::tool::{AgentTool, RunId, ToolCtx, ToolUpdates};
use tau_tools::{ABORTED, edit::Edit, path::Root};

fn ctx() -> ToolCtx {
    let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
    ToolCtx::new(
        Default::default(),
        ToolUpdates::for_tests("call_1", sender),
        RunId("run_1".into()),
    )
}

fn cancelled_ctx() -> ToolCtx {
    let token = tokio_util::sync::CancellationToken::new();
    token.cancel();
    let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
    ToolCtx::new(
        token,
        ToolUpdates::for_tests("call_1", sender),
        RunId("run_1".into()),
    )
}

fn new_edit(dir: &std::path::Path) -> Edit {
    Edit::new(Root::new(dir))
}

fn call(
    edit: &Edit,
    args: Value,
) -> anyhow::Result<tau_agent::tool::ToolOutput> {
    let args = edit.prepare_arguments(args);
    tau_testing::block_on(edit.call(args, ctx()))
}

/// A naive reference: splices each `(old, new)` pair into `original` at
/// the position `old` starts, in position order. Used only when every
/// `old` is known to occur exactly once and the ranges do not overlap.
fn naive_apply(original: &str, edits: &[(String, String)]) -> String {
    let mut positions: Vec<(usize, usize, &str)> = edits
        .iter()
        .map(|(old, new)| {
            let start =
                original.find(old.as_str()).expect("old text is present");
            (start, start + old.len(), new.as_str())
        })
        .collect();
    positions.sort_by_key(|p| p.0);
    let mut result = String::new();
    let mut cursor = 0;
    for (start, end, new) in positions {
        result.push_str(&original[cursor..start]);
        result.push_str(new);
        cursor = end;
    }
    result.push_str(&original[cursor..]);
    result
}

/// Splits `s` into lines that each keep their own trailing `\n`. Used
/// only by [`apply_unified_diff`], as an oracle independent of the
/// splitter `edit` uses internally.
fn lines_with_newlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, b) in s.bytes().enumerate() {
        if b == b'\n' {
            out.push(&s[start..=i]);
            start = i + 1;
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// A minimal unified-diff applier, independent of `edit`'s diff
/// generation, used only as the oracle for the diff-consistency
/// property. Assumes every hunk line is newline-terminated (the test
/// generators always produce content ending in `\n`).
fn apply_unified_diff(original: &str, patch: &str) -> String {
    let original_lines = lines_with_newlines(original);
    let mut result = String::new();
    let mut orig_idx = 0usize;
    for line in patch.lines() {
        if line.starts_with("--- ") || line.starts_with("+++ ") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("@@ -") {
            let range =
                rest.split(' ').next().expect("a range before the space");
            let start: usize = range
                .split(',')
                .next()
                .expect("a start number")
                .parse()
                .expect("a number");
            let target = start.saturating_sub(1);
            while orig_idx < target {
                result.push_str(original_lines[orig_idx]);
                orig_idx += 1;
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix(' ') {
            result.push_str(rest);
            result.push('\n');
            orig_idx += 1;
        } else if line.starts_with('-') {
            orig_idx += 1;
        } else if let Some(rest) = line.strip_prefix('+') {
            result.push_str(rest);
            result.push('\n');
        }
    }
    while orig_idx < original_lines.len() {
        result.push_str(original_lines[orig_idx]);
        orig_idx += 1;
    }
    result
}

/// `n` lines, each with a distinct marker (`line-{i}-...`) so no line's
/// text can occur anywhere else in the content, and each ends in `\n`.
#[hegel::composite]
fn unique_lines(tc: TestCase, n: usize) -> Vec<String> {
    (0..n)
        .map(|i| {
            let suffix: String = tc.draw(
                gs::text()
                    .alphabet("abcdefghijklmnopqrstuvwxyz")
                    .max_size(6),
            );
            format!("line-{i}-{suffix}\n")
        })
        .collect()
}

/// A non-empty, sorted, duplicate-free subset of `0..n`.
fn pick_indices(tc: &TestCase, n: usize) -> Vec<usize> {
    let k = tc.draw(gs::integers::<usize>().min_value(1).max_value(n));
    let mut indices: Vec<usize> = tc.draw(
        gs::vecs(gs::integers::<usize>().min_value(0).max_value(n - 1))
            .min_size(k)
            .max_size(k)
            .unique(true),
    );
    indices.sort_unstable();
    indices
}

/// An exact-match case: unique lines, a subset replaced by exact
/// `oldText`. Returns the original content and the `edits` JSON.
fn exact_case(tc: &TestCase) -> (String, Vec<Value>) {
    let n = tc.draw(gs::integers::<usize>().min_value(2).max_value(8));
    let lines = tc.draw(unique_lines(n));
    let original: String = lines.concat();
    let indices = pick_indices(tc, n);
    let edits = indices
        .iter()
        .map(|&i| json!({"oldText": lines[i], "newText": format!("REPLACED-{i}\n")}))
        .collect();
    (original, edits)
}

/// A fuzzy-match case: every line has trailing spaces the model didn't
/// type, so every edit needs fuzzy matching; the untouched lines keep
/// their padding.
fn fuzzy_case(tc: &TestCase) -> (String, Vec<String>, Vec<usize>, Vec<Value>) {
    let n = tc.draw(gs::integers::<usize>().min_value(2).max_value(6));
    let mut full_lines = Vec::with_capacity(n);
    let mut trimmed_lines = Vec::with_capacity(n);
    for i in 0..n {
        let pad = tc.draw(gs::integers::<usize>().min_value(1).max_value(3));
        full_lines.push(format!("line-{i}{}\n", " ".repeat(pad)));
        trimmed_lines.push(format!("line-{i}\n"));
    }
    let original: String = full_lines.concat();
    let indices = pick_indices(tc, n);
    let edits = indices
        .iter()
        .map(|&i| json!({"oldText": trimmed_lines[i], "newText": format!("REPLACED-{i}\n")}))
        .collect();
    (original, full_lines, indices, edits)
}

/// `edit` with exact, unique, non-overlapping edits equals a naive
/// reference that splices them into the original string
/// (`testing.md`, "tau-tools" property table).
#[hegel::test(test_cases = 50)]
fn exact_edits_equal_a_naive_reference(tc: TestCase) {
    let (original, edits) = exact_case(&tc);
    let naive_edits: Vec<(String, String)> = edits
        .iter()
        .map(|e| {
            (
                e["oldText"].as_str().unwrap().to_owned(),
                e["newText"].as_str().unwrap().to_owned(),
            )
        })
        .collect();

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f.txt");
    std::fs::write(&file, &original).unwrap();
    let edit = new_edit(dir.path());
    call(&edit, json!({"path": "f.txt", "edits": edits})).unwrap();

    let written = std::fs::read_to_string(&file).unwrap();
    assert_eq!(written, naive_apply(&original, &naive_edits));
}

/// An edit on a CRLF file equals the LF edit with CRLF restored
/// (`tools.md`, "edit", step 7; metamorphic).
#[hegel::test(test_cases = 50)]
fn crlf_edit_equals_the_lf_edit_with_endings_restored(tc: TestCase) {
    let (lf_content, edits) = exact_case(&tc);
    let crlf_content = lf_content.replace('\n', "\r\n");

    let dir = tempfile::tempdir().unwrap();
    let lf_path = dir.path().join("lf.txt");
    let crlf_path = dir.path().join("crlf.txt");
    std::fs::write(&lf_path, &lf_content).unwrap();
    std::fs::write(&crlf_path, &crlf_content).unwrap();

    let edit = new_edit(dir.path());
    call(&edit, json!({"path": "lf.txt", "edits": edits.clone()})).unwrap();
    call(&edit, json!({"path": "crlf.txt", "edits": edits})).unwrap();

    let lf_result = std::fs::read_to_string(&lf_path).unwrap();
    let crlf_result = std::fs::read_to_string(&crlf_path).unwrap();
    assert_eq!(crlf_result, lf_result.replace('\n', "\r\n"));
}

/// An edit on a file with a BOM equals the same edit without one, with
/// the BOM restored (`tools.md`, "edit", step 7; metamorphic).
#[hegel::test(test_cases = 50)]
fn bom_is_preserved_across_the_edit(tc: TestCase) {
    let (plain_content, edits) = exact_case(&tc);
    let bom_content = format!("\u{FEFF}{plain_content}");

    let dir = tempfile::tempdir().unwrap();
    let plain_path = dir.path().join("plain.txt");
    let bom_path = dir.path().join("bom.txt");
    std::fs::write(&plain_path, &plain_content).unwrap();
    std::fs::write(&bom_path, &bom_content).unwrap();

    let edit = new_edit(dir.path());
    call(&edit, json!({"path": "plain.txt", "edits": edits.clone()})).unwrap();
    call(&edit, json!({"path": "bom.txt", "edits": edits})).unwrap();

    let plain_result = std::fs::read_to_string(&plain_path).unwrap();
    let bom_result = std::fs::read_to_string(&bom_path).unwrap();
    assert_eq!(bom_result, format!("\u{FEFF}{plain_result}"));
}

/// In fuzzy mode, lines no edit touches keep their original bytes
/// (`tools.md`, "edit", step 6).
#[hegel::test(test_cases = 50)]
fn fuzzy_mode_keeps_untouched_lines_byte_for_byte(tc: TestCase) {
    let (original, full_lines, indices, edits) = fuzzy_case(&tc);
    let mut expected = full_lines.clone();
    for &i in &indices {
        expected[i] = format!("REPLACED-{i}\n");
    }

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f.txt");
    std::fs::write(&file, &original).unwrap();
    let edit = new_edit(dir.path());
    call(&edit, json!({"path": "f.txt", "edits": edits})).unwrap();

    let written = std::fs::read_to_string(&file).unwrap();
    assert_eq!(written, expected.concat());
}

/// Applying the returned diff to the original content gives exactly
/// the bytes written, in exact mode (round trip).
#[hegel::test(test_cases = 50)]
fn exact_mode_diff_round_trips_to_the_written_bytes(tc: TestCase) {
    let (original, edits) = exact_case(&tc);

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f.txt");
    std::fs::write(&file, &original).unwrap();
    let edit = new_edit(dir.path());
    let output = call(&edit, json!({"path": "f.txt", "edits": edits})).unwrap();

    let written = std::fs::read_to_string(&file).unwrap();
    let diff = output.details.unwrap()["diff"].as_str().unwrap().to_owned();
    assert_eq!(apply_unified_diff(&original, &diff), written);
}

/// Applying the returned diff to the original content gives exactly
/// the bytes written, in fuzzy mode (round trip).
#[hegel::test(test_cases = 50)]
fn fuzzy_mode_diff_round_trips_to_the_written_bytes(tc: TestCase) {
    let (original, _full_lines, _indices, edits) = fuzzy_case(&tc);

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f.txt");
    std::fs::write(&file, &original).unwrap();
    let edit = new_edit(dir.path());
    let output = call(&edit, json!({"path": "f.txt", "edits": edits})).unwrap();

    let written = std::fs::read_to_string(&file).unwrap();
    let diff = output.details.unwrap()["diff"].as_str().unwrap().to_owned();
    assert_eq!(apply_unified_diff(&original, &diff), written);
}

/// A failed edit leaves the file byte-for-byte unchanged: edits are
/// all applied, or none are (`tools.md`, "edit", step 8).
#[hegel::test(test_cases = 50)]
fn a_failed_edit_leaves_the_file_unchanged(tc: TestCase) {
    let (original, mut edits) = exact_case(&tc);
    let bad = tc.draw(
        gs::integers::<usize>()
            .min_value(0)
            .max_value(edits.len() - 1),
    );
    edits[bad]["oldText"] =
        json!("this text does not occur anywhere in the file\n");

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f.txt");
    std::fs::write(&file, &original).unwrap();
    let edit = new_edit(dir.path());
    let result = call(&edit, json!({"path": "f.txt", "edits": edits}));

    assert!(result.is_err());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), original);
}

/// An `oldText` repeated `copies` times, in exact space, is rejected
/// with exactly that count (`tools.md`, "edit", "not unique").
#[hegel::test(test_cases = 50)]
fn duplicate_exact_matches_are_rejected_with_the_right_count(tc: TestCase) {
    let copies = tc.draw(gs::integers::<usize>().min_value(2).max_value(5));
    let token: String =
        tc.draw(gs::text().alphabet("abcdefg").min_size(1).max_size(5));
    let old_text = format!("X{token}X\n");
    let mut content = String::new();
    for i in 0..copies {
        content.push_str(&format!("prefix-{i}\n"));
        content.push_str(&old_text);
    }

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), &content).unwrap();
    let edit = new_edit(dir.path());
    let error = call(
        &edit,
        json!({"path": "f.txt", "edits": [{"oldText": old_text, "newText": "Y\n"}]}),
    )
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        format!(
            "Found {copies} occurrences of the text in f.txt. The text must be unique. Please provide more context to make it unique."
        )
    );
}

/// Per-path lock (`tools.md`, "edit", "Serialization"): overlapping
/// `edit` calls on one file, or on a file and a symlink to it, never
/// interleave their read-modify-write. Every concurrent edit lands,
/// however the tasks are scheduled.
#[hegel::test(test_cases = 50)]
fn concurrent_edits_all_land(tc: TestCase) {
    let n = tc.draw(gs::integers::<usize>().min_value(2).max_value(8));
    let via_symlink = tc.draw(gs::booleans());

    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.txt");
    let mut content = String::new();
    for i in 0..n {
        content.push_str(&format!("PLACEHOLDER_{i}\n"));
    }
    std::fs::write(&target, &content).unwrap();
    if via_symlink {
        symlink(&target, dir.path().join("alias.txt")).unwrap();
    }

    let edit = std::sync::Arc::new(new_edit(dir.path()));
    tau_testing::block_on(async {
        let mut tasks = Vec::new();
        for i in 0..n {
            let edit = edit.clone();
            let path = if via_symlink && i % 2 == 0 {
                "alias.txt"
            } else {
                "target.txt"
            };
            let args = edit.prepare_arguments(json!({
                "path": path,
                "edits": [{"oldText": format!("PLACEHOLDER_{i}\n"), "newText": format!("REPLACED_{i}\n")}],
            }));
            tasks.push(tokio::spawn(
                async move { edit.call(args, ctx()).await },
            ));
        }
        for task in tasks {
            task.await.unwrap().unwrap();
        }
    });

    let result = std::fs::read_to_string(&target).unwrap();
    for i in 0..n {
        assert!(
            result.contains(&format!("REPLACED_{i}\n")),
            "edit {i} was lost:\n{result}"
        );
    }
}

/// `edit` on a missing file (`tools.md`, "Error strings").
#[test]
fn enoent_when_the_file_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let edit = new_edit(dir.path());
    let error = call(
        &edit,
        json!({"path": "missing.txt", "edits": [{"oldText": "a", "newText": "b"}]}),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Could not edit file: missing.txt. Error code: ENOENT."
    );
}

/// `edit` on a read-only file (`tools.md`, "Error strings"). Skips
/// itself under root, which ignores file modes.
#[test]
fn eacces_when_the_file_is_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("ro.txt");
    std::fs::write(&file, "hello\n").unwrap();
    let mut perms = std::fs::metadata(&file).unwrap().permissions();
    perms.set_mode(0o444);
    std::fs::set_permissions(&file, perms).unwrap();

    let edit = new_edit(dir.path());
    let result = call(
        &edit,
        json!({"path": "ro.txt", "edits": [{"oldText": "hello", "newText": "world"}]}),
    );

    if result.is_ok() {
        // Running as root: permissions are not enforced.
        return;
    }
    assert_eq!(
        result.unwrap_err().to_string(),
        "Could not edit file: ro.txt. Error code: EACCES."
    );
}

/// `edit` with no edits (`tools.md`, "Error strings").
#[test]
fn no_edits_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), "hello\n").unwrap();
    let edit = new_edit(dir.path());
    let error = call(&edit, json!({"path": "f.txt", "edits": []})).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Edit tool input is invalid. edits must contain at least one replacement."
    );
}

/// Empty `oldText`, one edit (`tools.md`, "Error strings").
#[test]
fn empty_old_text_single_edit() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), "hello\n").unwrap();
    let edit = new_edit(dir.path());
    let error = call(
        &edit,
        json!({"path": "f.txt", "edits": [{"oldText": "", "newText": "x"}]}),
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "oldText must not be empty in f.txt.");
}

/// Empty `oldText`, several edits (`tools.md`, "Error strings").
#[test]
fn empty_old_text_several_edits() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), "hello\nworld\n").unwrap();
    let edit = new_edit(dir.path());
    let error = call(
        &edit,
        json!({"path": "f.txt", "edits": [
            {"oldText": "hello\n", "newText": "HELLO\n"},
            {"oldText": "", "newText": "x"},
        ]}),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "edits[1].oldText must not be empty in f.txt."
    );
}

/// Text not found, one edit (`tools.md`, "Error strings").
#[test]
fn not_found_single_edit() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), "hello\n").unwrap();
    let edit = new_edit(dir.path());
    let error = call(
        &edit,
        json!({"path": "f.txt", "edits": [{"oldText": "nonexistent", "newText": "x"}]}),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Could not find the exact text in f.txt. The old text must match exactly including all whitespace and newlines."
    );
}

/// Text not found, several edits (`tools.md`, "Error strings").
#[test]
fn not_found_several_edits() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), "hello\nworld\n").unwrap();
    let edit = new_edit(dir.path());
    let error = call(
        &edit,
        json!({"path": "f.txt", "edits": [
            {"oldText": "hello\n", "newText": "HELLO\n"},
            {"oldText": "nonexistent", "newText": "x"},
        ]}),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Could not find edits[1] in f.txt. The oldText must match exactly including all whitespace and newlines."
    );
}

/// Not unique, one edit (`tools.md`, "Error strings").
#[test]
fn not_unique_single_edit() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), "foo foo foo").unwrap();
    let edit = new_edit(dir.path());
    let error =
        call(&edit, json!({"path": "f.txt", "edits": [{"oldText": "foo", "newText": "bar"}]}))
            .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Found 3 occurrences of the text in f.txt. The text must be unique. Please provide more context to make it unique."
    );
}

/// Not unique, several edits (`tools.md`, "Error strings").
#[test]
fn not_unique_several_edits() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), "one\nfoo foo foo\n").unwrap();
    let edit = new_edit(dir.path());
    let error = call(
        &edit,
        json!({"path": "f.txt", "edits": [
            {"oldText": "one\n", "newText": "ONE\n"},
            {"oldText": "foo", "newText": "bar"},
        ]}),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Found 3 occurrences of edits[1] in f.txt. Each oldText must be unique. Please provide more context to make it unique."
    );
}

/// Overlapping edits (`tools.md`, "Error strings"), ported from pi's
/// `tools.test.ts` ("should fail when multi-edit regions overlap").
#[test]
fn overlapping_edits_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), "one\ntwo\nthree\n").unwrap();
    let edit = new_edit(dir.path());
    let error = call(
        &edit,
        json!({"path": "f.txt", "edits": [
            {"oldText": "one\ntwo\n", "newText": "ONE\nTWO\n"},
            {"oldText": "two\nthree\n", "newText": "TWO\nTHREE\n"},
        ]}),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "edits[0] and edits[1] overlap in f.txt. Merge them into one edit or target disjoint regions."
    );
}

/// No change, one edit (`tools.md`, "Error strings").
#[test]
fn no_change_single_edit() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), "hello\n").unwrap();
    let edit = new_edit(dir.path());
    let error =
        call(&edit, json!({"path": "f.txt", "edits": [{"oldText": "hello", "newText": "hello"}]}))
            .unwrap_err();
    assert_eq!(
        error.to_string(),
        "No changes made to f.txt. The replacement produced identical content. This might indicate an issue with special characters or the text not existing as expected."
    );
}

/// No change, several edits (`tools.md`, "Error strings").
#[test]
fn no_change_several_edits() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), "hello\nworld\n").unwrap();
    let edit = new_edit(dir.path());
    let error = call(
        &edit,
        json!({"path": "f.txt", "edits": [
            {"oldText": "hello\n", "newText": "hello\n"},
            {"oldText": "world\n", "newText": "world\n"},
        ]}),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "No changes made to f.txt. The replacements produced identical content."
    );
}

/// Cancelled (`tools.md`, "Error strings", "all"): the file is left
/// untouched.
#[test]
fn aborted_when_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f.txt");
    std::fs::write(&file, "hello\n").unwrap();
    let edit = new_edit(dir.path());
    let args = edit.prepare_arguments(json!({"path": "f.txt", "edits": [{"oldText": "hello", "newText": "world"}]}));
    let result = tau_testing::block_on(edit.call(args, cancelled_ctx()));
    assert_eq!(result.unwrap_err().to_string(), ABORTED);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello\n");
}

/// **Deliberate difference from pi:** a match that is unique in the
/// space it was found in is accepted even though it has a fuzzy
/// near-duplicate elsewhere. pi rejects this (`tools.md`, "edit",
/// "Deliberate difference from pi"; `edit-diff.ts:328`).
#[test]
fn a_unique_exact_match_is_not_rejected_by_a_fuzzy_near_duplicate() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f.txt");
    // Straight quotes match `oldText` exactly and only once; the
    // second line uses curly quotes, which only look the same after
    // fuzzy normalization.
    std::fs::write(&file, "print(\"hi\")\nprint(\u{201C}hi\u{201D})\n")
        .unwrap();
    let edit = new_edit(dir.path());
    call(
        &edit,
        json!({"path": "f.txt", "edits": [{"oldText": "print(\"hi\")", "newText": "print('bye')"}]}),
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "print('bye')\nprint(\u{201C}hi\u{201D})\n"
    );
}

/// pi's `tools.test.ts`: "should replace text in file".
#[test]
fn known_case_replace_text_in_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f.txt");
    std::fs::write(&file, "Hello, world!").unwrap();
    let edit = new_edit(dir.path());
    let output =
        call(&edit, json!({"path": "f.txt", "edits": [{"oldText": "world", "newText": "testing"}]}))
            .unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "Hello, testing!");
    let details = output.details.unwrap();
    assert!(details["diff"].as_str().unwrap().contains("testing"));
}

/// pi's `tools.test.ts`: "should replace multiple disjoint regions in
/// one call".
#[test]
fn known_case_multiple_disjoint_regions() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f.txt");
    std::fs::write(&file, "alpha\nbeta\ngamma\ndelta\n").unwrap();
    let edit = new_edit(dir.path());
    call(
        &edit,
        json!({"path": "f.txt", "edits": [
            {"oldText": "alpha\n", "newText": "ALPHA\n"},
            {"oldText": "gamma\n", "newText": "GAMMA\n"},
        ]}),
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "ALPHA\nbeta\nGAMMA\ndelta\n"
    );
}

/// pi's `tools.test.ts`: "should match edits against the original
/// file, not incrementally".
#[test]
fn known_case_edits_match_the_original_not_incrementally() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f.txt");
    std::fs::write(&file, "foo\nbar\nbaz\n").unwrap();
    let edit = new_edit(dir.path());
    call(
        &edit,
        json!({"path": "f.txt", "edits": [
            {"oldText": "foo\n", "newText": "foo bar\n"},
            {"oldText": "bar\n", "newText": "BAR\n"},
        ]}),
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "foo bar\nBAR\nbaz\n"
    );
}

/// pi's `tools.test.ts` ("edit tool fuzzy matching"): trailing
/// whitespace is stripped before matching.
#[test]
fn known_case_trailing_whitespace_is_stripped() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f.txt");
    std::fs::write(&file, "line one   \nline two  \nline three\n").unwrap();
    let edit = new_edit(dir.path());
    call(
        &edit,
        json!({"path": "f.txt", "edits": [{"oldText": "line one\nline two\n", "newText": "replaced\n"}]}),
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "replaced\nline three\n"
    );
}

/// pi's `tools.test.ts`: fullwidth punctuation matches via NFKC.
#[test]
fn known_case_fullwidth_punctuation() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f.txt");
    std::fs::write(&file, "你好，世界\n你好（世界）\n").unwrap();
    let edit = new_edit(dir.path());
    call(
        &edit,
        json!({"path": "f.txt", "edits": [{"oldText": "你好,世界\n你好(世界)\n", "newText": "你好，pi\n你好(pi)\n"}]}),
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "你好，pi\n你好(pi)\n"
    );
}

/// pi's `tools.test.ts`: compatibility-equivalent Unicode forms match
/// (fullwidth ASCII, a combining mark) via NFKC.
#[test]
fn known_case_unicode_compatibility_forms() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f.txt");
    std::fs::write(
        &file,
        "\u{FF21}\u{FF22}\u{FF23}\u{FF11}\u{FF12}\u{FF13}\ncafe\u{0301}\n",
    )
    .unwrap();
    let edit = new_edit(dir.path());
    call(
        &edit,
        json!({"path": "f.txt", "edits": [{"oldText": "ABC123\ncafé\n", "newText": "XYZ789\ncoffee\n"}]}),
    )
    .unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "XYZ789\ncoffee\n");
}

/// pi's `tools.test.ts`: smart single and double quotes match ASCII
/// quotes.
#[test]
fn known_case_smart_quotes() {
    let dir = tempfile::tempdir().unwrap();
    let single = dir.path().join("single.txt");
    std::fs::write(&single, "console.log(\u{2018}hello\u{2019});\n").unwrap();
    let edit = new_edit(dir.path());
    call(
        &edit,
        json!({"path": "single.txt", "edits": [{"oldText": "console.log('hello');", "newText": "console.log('world');"}]}),
    )
    .unwrap();
    assert!(std::fs::read_to_string(&single).unwrap().contains("world"));

    let double = dir.path().join("double.txt");
    std::fs::write(&double, "const msg = \u{201C}Hello World\u{201D};\n")
        .unwrap();
    call(
        &edit,
        json!({"path": "double.txt", "edits": [{"oldText": "const msg = \"Hello World\";", "newText": "const msg = \"Goodbye\";"}]}),
    )
    .unwrap();
    assert!(
        std::fs::read_to_string(&double)
            .unwrap()
            .contains("Goodbye")
    );
}

/// pi's `tools.test.ts`: Unicode dashes and NBSP match their ASCII
/// equivalents.
#[test]
fn known_case_unicode_dashes_and_nbsp() {
    let dir = tempfile::tempdir().unwrap();
    let dashes = dir.path().join("dashes.txt");
    std::fs::write(&dashes, "range: 1\u{2013}5\nbreak\u{2014}here\n").unwrap();
    let edit = new_edit(dir.path());
    call(
        &edit,
        json!({"path": "dashes.txt", "edits": [{"oldText": "range: 1-5\nbreak-here", "newText": "range: 10-50\nbreak--here"}]}),
    )
    .unwrap();
    assert!(std::fs::read_to_string(&dashes).unwrap().contains("10-50"));

    let nbsp = dir.path().join("nbsp.txt");
    std::fs::write(&nbsp, "hello\u{00A0}world\n").unwrap();
    call(
        &edit,
        json!({"path": "nbsp.txt", "edits": [{"oldText": "hello world", "newText": "hello universe"}]}),
    )
    .unwrap();
    assert!(std::fs::read_to_string(&nbsp).unwrap().contains("universe"));
}

/// pi's `tools.test.ts`: an exact match wins over a fuzzy one.
#[test]
fn known_case_exact_match_preferred_over_fuzzy() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f.txt");
    std::fs::write(&file, "const x = 'exact';\nconst y = 'other';\n").unwrap();
    let edit = new_edit(dir.path());
    call(
        &edit,
        json!({"path": "f.txt", "edits": [{"oldText": "const x = 'exact';", "newText": "const x = 'changed';"}]}),
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "const x = 'changed';\nconst y = 'other';\n"
    );
}

/// pi's `tools.test.ts`: duplicates are detected after fuzzy
/// normalization when no exact match exists.
#[test]
fn known_case_duplicates_after_fuzzy_normalization() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f.txt");
    std::fs::write(&file, "hello world   \nhello world\n").unwrap();
    let edit = new_edit(dir.path());
    let error =
        call(&edit, json!({"path": "f.txt", "edits": [{"oldText": "hello world", "newText": "replaced"}]}))
            .unwrap_err();
    assert!(error.to_string().contains("Found 2 occurrences"), "{error}");
}

/// pi's `tools.test.ts`: fuzzy matching works in multi-edit mode, and
/// preserves the correct occurrence and an applicable patch when a
/// fuzzy replacement equals a nearby (untouched) line.
#[test]
fn known_case_fuzzy_multi_edit_preserves_the_right_occurrence() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f.txt");
    let original = [
        "replace me\u{20}\u{20}\u{20}",
        "after\u{20}\u{20}\u{20}",
        "",
    ]
    .join("\n");
    std::fs::write(&file, &original).unwrap();
    let edit = new_edit(dir.path());
    let output = call(
        &edit,
        json!({"path": "f.txt", "edits": [{"oldText": "replace me\n", "newText": "after\n"}]}),
    )
    .unwrap();
    let expected = ["after", "after\u{20}\u{20}\u{20}", ""].join("\n");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), expected);
    let diff = output.details.unwrap()["diff"].as_str().unwrap().to_owned();
    assert_eq!(apply_unified_diff(&original, &diff), expected);
}

/// In fuzzy mode, a final line with no trailing newline is still kept:
/// it is neither dropped nor duplicated (pins the line splitter's
/// handling of a trailing partial line).
#[test]
fn fuzzy_mode_preserves_a_final_untouched_line_without_a_trailing_newline() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f.txt");
    std::fs::write(&file, "first   \nsecond").unwrap();
    let edit = new_edit(dir.path());
    call(&edit, json!({"path": "f.txt", "edits": [{"oldText": "first\n", "newText": "FIRST\n"}]})).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "FIRST\nsecond");
}

/// In fuzzy mode, an edit whose match ends exactly at a line boundary
/// does not sweep the following, untouched line into its group: that
/// line keeps its own original bytes (`tools.md`, "edit", step 6).
#[test]
fn fuzzy_mode_does_not_sweep_the_following_untouched_line_into_the_group() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f.txt");
    std::fs::write(&file, "first   \nsecond   \nthird\n").unwrap();
    let edit = new_edit(dir.path());
    call(&edit, json!({"path": "f.txt", "edits": [{"oldText": "first\n", "newText": "FIRST\n"}]})).unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "FIRST\nsecond   \nthird\n"
    );
}

/// Two fuzzy edits whose line ranges touch (one spans into the next
/// line) are merged into one group instead of two, which would
/// otherwise slice past the merged range (`edit-diff.ts`'s
/// `applyReplacementsPreservingUnchangedLines`).
#[test]
fn fuzzy_mode_merges_adjacent_touching_line_ranges() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f.txt");
    std::fs::write(&file, "PREFIX-MIDDLE-SUFFIX\nNEXT-LINE\nfourth   \n")
        .unwrap();
    let edit = new_edit(dir.path());
    call(
        &edit,
        json!({"path": "f.txt", "edits": [
            {"oldText": "MIDDLE", "newText": "MIDDLE2"},
            {"oldText": "SUFFIX\nNEXT-LINE\n", "newText": "SUFFIX2\nNEXT-LINE2\n"},
            {"oldText": "fourth\n", "newText": "FOURTH\n"},
        ]}),
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "PREFIX-MIDDLE2-SUFFIX2\nNEXT-LINE2\nFOURTH\n"
    );
}

/// The result's `firstChangedLine` is the line number of the first
/// change in the new file, not just the first line of the file
/// (`tools.md`, "edit", "Result details").
#[test]
fn first_changed_line_points_at_the_actual_change() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f.txt");
    std::fs::write(&file, "a\nb\nc\nTARGET\ne\n").unwrap();
    let edit = new_edit(dir.path());
    let output =
        call(&edit, json!({"path": "f.txt", "edits": [{"oldText": "TARGET\n", "newText": "CHANGED\n"}]}))
            .unwrap();
    assert_eq!(output.details.unwrap()["firstChangedLine"], 4);
}

/// pi's `edit-tool-crlf` suite: LF `oldText` matches CRLF content, and
/// duplicates are detected across CRLF/LF variants of the same text.
#[test]
fn known_case_crlf_matching_and_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f.txt");
    std::fs::write(&file, "first\r\nsecond\r\nthird\r\n").unwrap();
    let edit = new_edit(dir.path());
    call(&edit, json!({"path": "f.txt", "edits": [{"oldText": "second\n", "newText": "REPLACED\n"}]})).unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "first\r\nREPLACED\r\nthird\r\n"
    );

    let mixed = dir.path().join("mixed.txt");
    std::fs::write(&mixed, "hello\r\nworld\r\n---\r\nhello\nworld\n").unwrap();
    let error = call(&edit, json!({"path": "mixed.txt", "edits": [{"oldText": "hello\nworld\n", "newText": "replaced\n"}]}))
        .unwrap_err();
    assert!(error.to_string().contains("Found 2 occurrences"), "{error}");
}

/// pi's `edit-tool-legacy-input.test.ts`: `oldText`/`newText` at the
/// top level fold into `edits`.
#[test]
fn prepare_arguments_folds_legacy_old_new_text() {
    let dir = tempfile::tempdir().unwrap();
    let edit = new_edit(dir.path());
    let prepared = edit.prepare_arguments(json!({
        "path": "file.txt",
        "oldText": "before",
        "newText": "after",
    }));
    assert_eq!(
        prepared,
        json!({"path": "file.txt", "edits": [{"oldText": "before", "newText": "after"}]})
    );
}

/// pi's `edit-tool-legacy-input.test.ts`: a legacy replacement is
/// appended after any edits already present.
#[test]
fn prepare_arguments_appends_legacy_to_existing_edits() {
    let dir = tempfile::tempdir().unwrap();
    let edit = new_edit(dir.path());
    let prepared = edit.prepare_arguments(json!({
        "path": "file.txt",
        "edits": [{"oldText": "a", "newText": "b"}],
        "oldText": "c",
        "newText": "d",
    }));
    assert_eq!(
        prepared,
        json!({"path": "file.txt", "edits": [
            {"oldText": "a", "newText": "b"},
            {"oldText": "c", "newText": "d"},
        ]})
    );
}

/// pi's `edit-tool-legacy-input.test.ts`: `edits` sent as a JSON
/// string is parsed.
#[test]
fn prepare_arguments_parses_a_json_string_edits() {
    let dir = tempfile::tempdir().unwrap();
    let edit = new_edit(dir.path());
    let prepared = edit.prepare_arguments(json!({
        "path": "file.txt",
        "edits": "[{\"oldText\": \"a\", \"newText\": \"b\"}]",
    }));
    assert_eq!(
        prepared,
        json!({"path": "file.txt", "edits": [{"oldText": "a", "newText": "b"}]})
    );
}

/// pi's `edit-tool-legacy-input.test.ts`: invalid JSON in `edits` is
/// left alone.
#[test]
fn prepare_arguments_leaves_invalid_json_edits_alone() {
    let dir = tempfile::tempdir().unwrap();
    let edit = new_edit(dir.path());
    let prepared = edit
        .prepare_arguments(json!({"path": "file.txt", "edits": "not json"}));
    assert_eq!(prepared, json!({"path": "file.txt", "edits": "not json"}));
}

/// pi's `edit.ts`: a single edit object is wrapped in a one-element
/// array.
#[test]
fn prepare_arguments_wraps_a_single_edit_object() {
    let dir = tempfile::tempdir().unwrap();
    let edit = new_edit(dir.path());
    let prepared = edit.prepare_arguments(json!({
        "path": "file.txt",
        "edits": {"oldText": "a", "newText": "b"},
    }));
    assert_eq!(
        prepared,
        json!({"path": "file.txt", "edits": [{"oldText": "a", "newText": "b"}]})
    );
}

/// The tool's name, description and parameter descriptions are ported
/// from pi verbatim (`tools.md`: "Port pi's tool `description` strings
/// and parameter descriptions verbatim").
#[test]
fn tool_metadata_matches_the_ported_pi_strings() {
    let dir = tempfile::tempdir().unwrap();
    let edit = new_edit(dir.path());
    assert_eq!(edit.name(), "edit");
    assert_eq!(
        edit.description(),
        "Edit a single file using exact text replacement. Every edits[].oldText must match a unique, non-overlapping region of the original file. If two changes affect the same block or nearby lines, merge them into one edit instead of emitting overlapping edits. Do not include large unchanged regions just to connect distant changes."
    );
    let schema = edit.parameters();
    assert_eq!(
        schema["properties"]["path"]["description"],
        "Path to the file to edit (relative or absolute)"
    );
    assert_eq!(
        schema["properties"]["edits"]["description"],
        "One or more targeted replacements. Each edit is matched against the original file, not incrementally. Do not include overlapping or nested edits. If two changes touch the same block or nearby lines, merge them into one edit instead."
    );
}
