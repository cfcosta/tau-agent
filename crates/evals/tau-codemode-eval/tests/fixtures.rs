//! Property inventory:
//! - log padding preserves the golden answer (differential TSV oracle);
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

#[hegel::test]
fn padding_preserves_every_failure(tc: TestCase) {
    let noise_lines = tc.draw(gs::integers::<usize>().max_value(4_000));
    let fixture = fixtures::test_log(noise_lines);
    assert_eq!(failures(&fixture.files[0].text), fixture.expected);
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
    let mut result = Vec::new();
    for file in &fixture.files {
        if !file.path.ends_with(".rs") {
            continue;
        }
        for (index, line) in file.text.lines().enumerate() {
            if line.contains("TODO:") {
                result.push(
                    json!({"path": file.path, "line": index + 1, "text": line}),
                );
            }
        }
    }
    result.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    assert_eq!(Value::Array(result), fixture.expected);
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
