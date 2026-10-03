//! tau-tools' UI: what its cards read from a call, and that it keeps to
//! the design language.

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
use tau_ui_plugin::{CallData, CallResult, Fold, testing::FakeRun};

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
    let label = read.label();
    if first == 1 && returned == total {
        assert!(label.ends_with(if total == 1 { " line" } else { " lines" }));
    } else {
        assert!(label.starts_with(&format!("{first}–")), "{label}");
        assert!(label.ends_with(&format!(" of {total}")), "{label}");
    }
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

#[cfg(feature = "host")]
#[test]
fn restarted_inspector_preview_reads_only_its_runs_grant() {
    use std::{io::Cursor, sync::Arc};

    use tau_agent::tool::RunId;
    use tau_artifacts::{Bytes, Quotas};
    use tau_store::{Entry, NewRun, RunKind, Store, TurnUsage};
    use tau_tools::ui::ToolsUi;
    use tau_ui_plugin::{
        HOST_RECORD,
        HostCx,
        HostRecord,
        RepoCtx,
        Services,
        UiPlugin,
    };
    use tokio_util::sync::CancellationToken;

    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let store = runtime
        .block_on(Store::open(dir.path().join("runs.db")))
        .unwrap();
    let bytes =
        Bytes::new(project.join("artifacts"), Quotas::default()).unwrap();
    let artifact = bytes
        .publish_reader(
            Cursor::new(b"preview after restart"),
            &CancellationToken::new(),
        )
        .unwrap();
    drop(bytes);
    for id in ["allowed", "other"] {
        runtime.block_on(async {
            store
                .create_run(&NewRun {
                    id,
                    workflow_id: None,
                    agent: "test",
                    kind: RunKind::Root,
                    model: "fake",
                    turns: 0,
                })
                .await
                .unwrap();
            store
                .append_turn(
                    id,
                    &[Entry::Plugin {
                        plugin: HOST_RECORD.into(),
                        body: json!(HostRecord {
                            repo: "repo".into(),
                            ..HostRecord::default()
                        })
                        .to_string(),
                    }],
                    TurnUsage::default(),
                )
                .await
                .unwrap();
        });
    }
    runtime.block_on(store.append_turn("allowed", &[Entry::Plugin { plugin: tau_tools::ui::NAME.into(),
        body: json!({"kind":"artifact_grant","artifact":artifact.clone(),"owner_run_id":"allowed","source":"read file","source_complete":true}).to_string()
    }], TurnUsage::default())).unwrap();
    let cx = HostCx::new(
        store,
        runtime.handle().clone(),
        Services::default(),
        dir.path().into(),
        vec![RepoCtx {
            name: "repo".into(),
            checkout: project.clone(),
            dir: project,
        }],
        Arc::new(|_| {}),
    );
    let action = |run: &str| json!({"action":"read","run":RunId(run.into()),"id":artifact.id(),"offset":0,"encoding":"utf8"});
    let allowed = ToolsUi.act(&(), action("allowed"), &cx).unwrap().unwrap();
    assert_eq!(allowed["range"]["data"], "preview after restart");
    let denied = ToolsUi.act(&(), action("other"), &cx).unwrap().unwrap();
    assert!(denied["error"].as_str().unwrap().contains("not granted"));
}
