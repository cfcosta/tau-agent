//! Property inventory:
//! - log padding preserves the golden answer (differential TSV oracle);
//! - varied colon-bearing paths preserve search records (independent scan);
//! - padded logs distinguish complete scans from one display window;
//! - fixture serialization preserves every input and answer (round trip).
//!
//! Noise counts are bounded integers, valid by construction. Shrinking
//! removes padding while retaining all three failures. Workspace Hegel
//! profiles supply reproducible CI cases and the local failure database.

use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_codemode_eval::fixtures;

/// A reference reader, not the implementation a Codemode program will use.
fn failures(text: &str) -> Value {
    Value::Array(
        text.lines()
            .filter_map(|line| {
                let fields: Vec<_> = line.split('\t').collect();
                (fields.first() == Some(&"FAIL")).then(|| {
                    assert_eq!(fields.len(), 3);
                    json!({"test": fields[1], "message": fields[2]})
                })
            })
            .collect(),
    )
}

fn search_records(fixture: &fixtures::Fixture) -> Value {
    let mut records = Vec::new();
    for file in &fixture.files {
        if !file.path.ends_with(".rs") {
            continue;
        }
        for (index, line) in file.text.lines().enumerate() {
            if line.contains("TODO:") {
                records.push(
                    json!({"path": file.path, "line": index + 1, "text": line}),
                );
            }
        }
    }
    records.sort_by(|a, b| {
        a["path"]
            .as_str()
            .cmp(&b["path"].as_str())
            .then(a["line"].as_u64().cmp(&b["line"].as_u64()))
    });
    Value::Array(records)
}

#[hegel::test]
fn padding_preserves_every_failure(tc: TestCase) {
    let noise_lines = tc.draw(gs::integers::<usize>().max_value(4_000));
    let fixture = fixtures::test_log(noise_lines);
    assert_eq!(failures(&fixture.files[0].text), fixture.expected);
}

#[hegel::test]
fn colon_paths_preserve_complete_search_answers(tc: TestCase) {
    // Valid path segments are generated directly. Shorter segments shrink
    // toward a small counterexample while the colon remains present.
    let segment: String = tc.draw(gs::from_regex("[a-z]{1,12}"));
    let path = format!("src/{segment}: case.rs");
    let fixture = fixtures::search_with_path(&path);
    assert_eq!(search_records(&fixture), fixture.expected);
}

#[hegel::test]
fn padded_display_is_partial_but_full_scan_is_complete(tc: TestCase) {
    // Padding is valid by construction and shrinks to the first size that
    // still hides the middle marker from a 2,000-line head window.
    let padding =
        tc.draw(gs::integers::<usize>().min_value(2_100).max_value(4_000));
    let fixture = fixtures::test_log(padding);
    let text = &fixture.files[0].text;
    let display = text.lines().take(2_000).collect::<Vec<_>>().join("\n");
    assert_eq!(failures(text), fixture.expected);
    assert_ne!(failures(&display), fixture.expected);
}

#[hegel::test]
fn serialized_workloads_preserve_inputs_and_answers(tc: TestCase) {
    let noise_lines = tc.draw(gs::integers::<usize>().max_value(100));
    let fixture = fixtures::test_log(noise_lines);
    let serialized = serde_json::to_vec(&fixture).unwrap();
    let decoded: fixtures::Fixture =
        serde_json::from_slice(&serialized).unwrap();
    assert_eq!(decoded, fixture);
}

#[hegel::test]
fn unicode_page_boundary_markers_preserve_failure_oracle(tc: TestCase) {
    // Inventory: changing padding and Unicode marker preserves the independent
    // TSV oracle while a multibyte codepoint crosses the first 8 KiB page.
    // The marker is selected from valid UTF-8 codepoints by construction;
    // shrinking reduces the padding and marker index, not the boundary rule.
    // Workspace hegel.toml supplies local and CI case counts.
    let noise_lines =
        tc.draw(gs::integers::<usize>().min_value(2_100).max_value(4_000));
    let marker_index = tc.draw(gs::integers::<usize>().max_value(2));
    let marker = ["雪", "🚀", "é"][marker_index];
    let fixture = fixtures::changed_log_with_page_marker(noise_lines, marker);
    let text = &fixture.files[0].text;
    let marker_text = format!("{marker} marker");
    assert_eq!(text.find(marker_text.as_str()), Some(8_191));
    assert_eq!(failures(text), fixture.expected);
    assert!(text.len() > 50 * 1024);
}

#[test]
fn display_windows_omit_required_evidence() {
    let fixture = fixtures::test_log(2_400);
    let log = &fixture.files[0].text;
    assert!(log.len() > 50 * 1024);
    let head = log.lines().take(2_000).collect::<Vec<_>>().join("\n");
    let lines: Vec<_> = log.lines().collect();
    let tail = lines[lines.len() - 2_000..].join("\n");
    assert_ne!(failures(&head), fixture.expected);
    assert_ne!(failures(&tail), fixture.expected);
    let byte_head = &log[..50 * 1024];
    let byte_tail = &log[log.len() - 50 * 1024..];
    assert_ne!(failures(byte_head), fixture.expected);
    assert_ne!(failures(byte_tail), fixture.expected);
}

#[test]
fn search_answer_matches_a_literal_reference_scan() {
    let fixture = fixtures::search();
    assert_eq!(search_records(&fixture), fixture.expected);
}

#[test]
fn changed_workflow_requires_a_new_input_not_a_cached_answer() {
    let original = fixtures::repeated_workflow(false);
    let changed = fixtures::repeated_workflow(true);
    assert_ne!(original.expected, changed.expected);
    for fixture in [original, changed] {
        let config: Value =
            serde_json::from_str(&fixture.files[0].text).unwrap();
        let command = config
            .get("verify_command")
            .or_else(|| config.get("test_command"))
            .unwrap();
        assert_eq!(json!({"command": command}), fixture.expected);
    }
}

#[test]
fn corpus_has_distinct_names_and_semantic_negative_evidence() {
    let corpus = fixtures::corpus();
    let names: std::collections::BTreeSet<_> =
        corpus.iter().map(|case| &case.name).collect();
    assert_eq!(names.len(), corpus.len());
    let semantic = fixtures::semantic_changes();
    assert!(semantic.files[0].text.contains("still accepted"));
    assert_eq!(semantic.expected.as_array().unwrap().len(), 2);
}
