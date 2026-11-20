//! heiko-tools' UI: what its cards read from a call, and that it keeps to
//! the design language.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use hegel::generators as gs;
use heiko_agent::tool::RunId;
use heiko_tools::{
    artifact_grant::ArtifactRecord,
    ui::{
        ArtifactStatus,
        ReadView,
        State,
        artifact_status,
        diff_of,
        output_lines,
        term::TermOutput,
        tests_stat,
    },
};
use heiko_ui_kit::{
    diff::{DiffKind, DiffLine},
    theme::Tone,
};
use heiko_ui_plugin::{
    CallData,
    CallResult,
    Fold,
    RunInfo,
    points::AtCard,
    testing::FakeRun,
};
use serde_json::json;

/// A result's diff is read as heiko-ui-kit reads any diff (its property
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

/// A test run's summary in an ended command's output gives the compare
/// screen its tests: the counts, failing when fewer passed than ran.
/// Another tool's card, or a command still going, gives none.
#[hegel::test(test_cases = 200)]
fn a_test_summary_gives_the_branch_its_counts(tc: hegel::TestCase) {
    let total = tc.draw(gs::integers::<u32>().max_value(5_000));
    let passed = tc.draw(gs::integers::<u32>().max_value(total));
    let text = format!(
        "Starting 3 tests\n     Summary [   4.200s] {total} tests run: \
         {passed} \u{1b}[32mpassed\u{1b}[0m, 0 skipped\n"
    );
    let at = |tool: &str, ended: bool| AtCard {
        run: RunInfo {
            id: RunId("r".into()),
            repo: "heiko".into(),
            live: false,
            title: "t".into(),
            answer: None,
            context: 0,
            window: None,
        },
        call_id: "c1".into(),
        tool: tool.into(),
        keys: Vec::new(),
        data: std::sync::Arc::new(CallData {
            result: ended.then(|| CallResult {
                text: text.clone(),
                details: None,
                error: false,
            }),
            partial: Some(text.clone()),
            ..CallData::default()
        }),
        summary: String::new(),
        cut: None,
    };
    let stat = tests_stat(&at("exec_command", true)).expect("a stat");
    assert_eq!(stat.name, "tests");
    assert_eq!(stat.state, format!("{passed} / {total}"));
    assert_eq!(stat.tone == Tone::Danger, passed != total);
    assert_eq!(tests_stat(&at("exec_command", false)), None);
    assert_eq!(tests_stat(&at("read", true)), None);
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
/// in the file, leaves out the notice after them, and says how many of
/// the file's lines it holds.
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
    // The lines it shows, of the file's (ADR 0060): the count alone
    // when it shows the whole file.
    let grouped = heiko_ui_kit::format::grouped;
    let lines_of = |n: usize| match n {
        1 => "1 line".to_owned(),
        n => format!("{} lines", grouped(n)),
    };
    let expected = if first == 1 && returned >= total {
        lines_of(total)
    } else {
        format!("{} of {}", grouped(returned), lines_of(total))
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

/// A command's terminal is its chunks joined in order while it runs, and
/// none before it wrote anything.
#[test]
fn a_running_terminal_is_its_chunks_in_order() {
    let chunk = |seq: u64, text: &str| {
        use base64::Engine as _;
        json!({"term": {
            "seq": seq,
            "bytes": base64::engine::general_purpose::STANDARD.encode(text),
        }})
    };
    let mut data = CallData::default();
    assert_eq!(TermOutput::of(&data, false), None);
    data.updates = vec![chunk(0, "a"), chunk(2, "c"), chunk(1, "b")];
    let term = TermOutput::of(&data, false).expect("a terminal");
    assert_eq!(term.bytes.as_slice(), b"ab");
    assert_eq!(term.end, None);
}
