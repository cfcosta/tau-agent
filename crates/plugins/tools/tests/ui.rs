//! tau-tools' UI: what its cards read from a call, and that it keeps to
//! the design language.

use hegel::generators as gs;
use serde_json::json;
use tau_tools::ui::{diff_of, output_lines};
use tau_ui_plugin::{CallData, CallResult};

/// A result's diff reads back line for line, counted as it adds and
/// removes; a failed call shows none.
#[hegel::test(test_cases = 200)]
fn a_diff_reads_back(tc: hegel::TestCase) {
    let lines: Vec<(u8, String)> = tc.draw(gs::vecs(hegel::tuples!(
        gs::integers::<u8>().max_value(2),
        gs::text().alphabet("abc xyz").max_size(6),
    )));
    let text: String = lines
        .iter()
        .map(|(kind, line)| {
            let sign = match kind {
                0 => ' ',
                1 => '+',
                _ => '-',
            };
            format!("{sign}{line}\n")
        })
        .collect();
    // The hunk's header counts the old (context and removed) and new
    // (context and added) lines it holds.
    let old = lines.iter().filter(|(kind, _)| *kind != 1).count();
    let new = lines.iter().filter(|(kind, _)| *kind != 2).count();
    let header = format!("@@ -1,{old} +1,{new} @@");
    let error = tc.draw(gs::booleans());
    let data = CallData {
        result: Some(CallResult {
            text: String::new(),
            details: Some(
                json!({ "diff": format!("--- a\n+++ b\n{header}\n{text}") }),
            ),
            error,
        }),
        ..CallData::default()
    };
    let read = diff_of(&data);
    if error {
        assert!(read.is_none());
        return;
    }
    let read = read.unwrap();
    assert_eq!(read.len(), lines.len());
    let added = lines.iter().filter(|(kind, _)| *kind == 1).count();
    let removed = lines.iter().filter(|(kind, _)| *kind == 2).count();
    assert_eq!(
        tau_ui_kit::diff::stat(&read),
        format!("+{added} −{removed}")
    );
    for ((_, line), read) in lines.iter().zip(&read) {
        assert_eq!(&read.text, line);
    }
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
