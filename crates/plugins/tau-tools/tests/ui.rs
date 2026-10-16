//! tau-tools' UI: what its cards read from a call, and that it keeps to
//! the design language.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use hegel::generators as gs;
use serde_json::json;
use tau_tools::{
    artifact_grant::ArtifactRecord,
    ui::{
        ArtifactStatus,
        ReadView,
        State,
        artifact_status,
        diff_of,
        output_lines,
    },
};
use tau_ui_kit::diff::{DiffKind, DiffLine};
use tau_ui_plugin::{CallData, CallResult, Fold, testing::FakeRun};

/// A result's diff is read as tau-ui-kit reads any diff (its property
/// is `a_whole_file_diff_gives_back_both_sides`): a successful call's
/// `diff`, line for line. A failed call, a result without details, or
/// details without a diff show none.
#[test]
fn only_a_successful_result_with_a_diff_reads_one() {
    let diff = "--- a\n+++ b\n@@ -1,2 +1,2 @@\n keep\n-old\n+new\n";
    let data = |details: Option<serde_json::Value>, error: bool| CallData {
        result: Some(CallResult {
            text: String::new(),
            details,
            error,
        }),
        ..CallData::default()
    };
    let line = |kind, text: &str| DiffLine {
        kind,
        text: text.to_owned(),
    };
    assert_eq!(
        diff_of(&data(Some(json!({ "diff": diff })), false)),
        Some(vec![
            line(DiffKind::Context, "keep"),
            line(DiffKind::Removed, "old"),
            line(DiffKind::Added, "new"),
        ])
    );
    assert_eq!(diff_of(&data(Some(json!({ "diff": diff })), true)), None);
    assert_eq!(diff_of(&data(None, false)), None);
    assert_eq!(
        diff_of(&data(Some(json!({ "created": true })), false)),
        None
    );
    assert_eq!(diff_of(&CallData::default()), None);
}

/// A command's lines are its result's once it ends, else what it has
/// printed so far.
#[test]
fn output_is_the_result_or_the_output_so_far() {
    let mut data = CallData {
        partial: Some("a\nb".into()),
        ..CallData::default()
    };
    assert_eq!(output_lines(&data), ["a", "b"]);
    data.result = Some(CallResult {
        text: "c".into(),
        details: None,
        error: false,
    });
    assert_eq!(output_lines(&data), ["c"]);
}

#[test]
fn artifact_grant_fold_reloads_and_cards_report_bounded_metadata() {
    let grant = json!({"kind":"artifact_grant","artifact":{
        "id":"0199b283-f06a-722b-8c75-476700ee3488",
        "digest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "size_bytes":1234},"owner_run_id":"run-1","source":"bash output",
        "source_complete":true});
    let mut state = State::default();
    state.apply(
        serde_json::from_value::<ArtifactRecord>(grant).unwrap(),
        &mut FakeRun::default(),
    );
    let restored: State =
        serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
    assert_eq!(restored.grants.len(), 1);
    assert_eq!(
        restored.grants.values().next().unwrap().source_complete,
        Some(true)
    );
    let data = CallData {
        result: Some(CallResult {
            text: "bounded tail".into(),
            details: Some(json!({"artifact":{
            "id":"0199b283-f06a-722b-8c75-476700ee3488",
            "digest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "size_bytes":1234,"source":"bash output"},"source_complete":true})),
            error: false,
        }),
        ..CallData::default()
    };
    assert!(matches!(
        artifact_status(&data),
        Some(ArtifactStatus::Available {
            source_complete: Some(true),
            ..
        })
    ));
    let missing = CallData {
        result: Some(CallResult {
            text: String::new(),
            details: Some(
                json!({"artifact":null,"artifact_error":"quota exceeded"}),
            ),
            error: true,
        }),
        ..CallData::default()
    };
    assert_eq!(
        artifact_status(&missing),
        Some(ArtifactStatus::Unavailable("quota exceeded".into()))
    );
}

/// A `read` card numbers the lines the model got from where they start
/// in the file, leaves out the notice after them, and says which part of
/// the file it holds.
#[hegel::test(test_cases = 200)]
fn a_read_numbers_its_lines(tc: hegel::TestCase) {
    let total: usize =
        tc.draw(gs::integers::<usize>().min_value(1).max_value(50));
    let first: usize =
        tc.draw(gs::integers::<usize>().min_value(1).max_value(total));
    let returned: usize =
        tc.draw(gs::integers::<usize>().max_value(total - first + 1));
    let lines: Vec<String> = (first..first + returned)
        .map(|n| format!("line {n}"))
        .collect();
    let text = format!(
        "{}\n\n[Showing lines {first}-{} of {total}.]",
        lines.join("\n"),
        first + returned
    );
    let data = CallData {
        result: Some(CallResult {
            text,
            details: Some(json!({
                "kind": "text",
                "offset": first,
                "total_lines": total,
                "returned_lines": returned,
            })),
            error: false,
        }),
        ..CallData::default()
    };
    let read = ReadView::of(&data).unwrap();
    assert_eq!(read.first, first);
    assert_eq!(read.lines, lines);
    // The whole file is its line count; a part, its first and last
    // line of the total (a part with no lines names its first).
    let expected = if first == 1 && returned == total {
        if total == 1 {
            "1 line".to_owned()
        } else {
            format!("{total} lines")
        }
    } else {
        let last = if returned == 0 {
            first
        } else {
            first + returned - 1
        };
        format!("{first}–{last} of {total}")
    };
    assert_eq!(read.label(), expected);
}

/// An image, or a failed read, has no lines to show.
#[test]
fn a_read_without_text_has_no_lines() {
    let of = |details, error| {
        ReadView::of(&CallData {
            result: Some(CallResult {
                text: "Read image file [image/png]".into(),
                details: Some(details),
                error,
            }),
            ..CallData::default()
        })
    };
    assert_eq!(of(json!({"kind": "image"}), false), None);
    assert_eq!(of(json!({}), true), None);
    // An older result, without the counts, reads as the whole file.
    assert_eq!(of(json!({}), false).unwrap().label(), "1 line");
}
