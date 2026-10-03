//! Property inventory:
//! - corpus integrity: independent TSV and Rust-line readers verify saved
//!   answers while generated padding, path, and Unicode marker preserve the
//!   display-window and byte-boundary facts. Bounds are 2,100..=4,000 noise
//!   lines and short valid paths; shrinking shortens both, retaining signals.
//! - serialization: every finite corpus kind, including changed variants,
//!   round-trips all fields and still matches an independent saved-answer
//!   oracle. The complete finite corpus is enumerated, not sampled.
//!
//! Workspace Hegel profiles supply case counts and reproducible CI behavior.

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
fn generated_corpus_keeps_saved_answers_and_boundary_evidence(tc: TestCase) {
    let noise_lines =
        tc.draw(gs::integers::<usize>().min_value(2_100).max_value(4_000));
    let segment: String = tc.draw(gs::from_regex("[a-z]{1,12}"));
    let path = format!("src/{segment}: case.rs");
    let search = fixtures::search_with_path(&path);
    assert_eq!(search.files[0].path, path);
    assert_eq!(search_records(&search), search.expected);

    let log = fixtures::test_log(noise_lines);
    let text = &log.files[0].text;
    let display = text.lines().take(2_000).collect::<Vec<_>>().join("\n");
    assert_eq!(failures(text), log.expected);
    assert_ne!(failures(&display), log.expected);

    let marker_index = tc.draw(gs::integers::<usize>().max_value(2));
    let marker = ["雪", "🚀", "é"][marker_index];
    let changed = fixtures::changed_log_with_page_marker(noise_lines, marker);
    let text = &changed.files[0].text;
    let marker_text = format!("{marker} marker");
    assert_eq!(text.find(marker_text.as_str()), Some(8_191));
    assert_eq!(failures(text), changed.expected);
    assert!(text.len() > 50 * 1024);
}

#[test]
fn serialized_corpus_kinds_keep_inputs_and_independent_answers() {
    let corpus = fixtures::matrix_corpus();
    assert_eq!(corpus.len(), 8);
    for fixture in &corpus {
        let serialized = serde_json::to_vec(fixture).unwrap();
        let decoded: fixtures::Fixture =
            serde_json::from_slice(&serialized).unwrap();
        assert_eq!(&decoded, fixture);
        let expected = if fixture.name.starts_with("structured-search") {
            search_records(fixture)
        } else if fixture.name.starts_with("complete-test-log") {
            failures(&fixture.files[0].text)
        } else if fixture.name.starts_with("compatibility-extraction") {
            let document = &fixture.files[0].text;
            let (before, after) = if fixture.name.ends_with("-changed") {
                ("retry_limit", "retry_ceiling")
            } else {
                ("retry_count", "max_attempts")
            };
            assert!(document.contains(&format!("`{before}` was removed")));
            assert!(document.contains(&format!("`{after}` instead")));
            assert!(document.contains("returns null when no item exists"));
            json!([
                {"component":"connection option","before":before,"after":after},
                {"component":"lookup function","before":"raises NotFound","after":"returns null"}
            ])
        } else {
            let config: Value =
                serde_json::from_str(&fixture.files[0].text).unwrap();
            json!({"command":config.get("verify_command").or_else(|| config.get("test_command")).unwrap()})
        };
        assert_eq!(fixture.expected, expected);
        assert_eq!(decoded.expected, expected);
    }
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
