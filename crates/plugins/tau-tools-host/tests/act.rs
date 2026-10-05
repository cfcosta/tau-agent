//! tau-tools' host half answering its cards: a `read` card's preview of
//! an artifact reads only the grants of its own run.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use serde_json::json;

#[test]
fn restarted_inspector_preview_reads_only_its_runs_grant() {
    use std::{io::Cursor, sync::Arc};

    use tau_agent::tool::RunId;
    use tau_artifacts::{Bytes, Quotas};
    use tau_store::{Entry, NewRun, RunKind, TurnUsage};
    use tau_tools_host::ToolsHost;
    use tau_ui_plugin::{
        HOST_RECORD,
        HostCx,
        HostHalf as _,
        HostRecord,
        RepoCtx,
        Services,
    };
    use tokio_util::sync::CancellationToken;

    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let store = runtime
        .block_on(tau_store_sqlite::open(dir.path().join("runs.db")))
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
            workspaces: project.clone(),
            dir: project,
        }],
        Arc::new(|_| {}),
    );
    let action = |run: &str| json!({"action":"read","run":RunId(run.into()),"id":artifact.id(),"offset":0,"encoding":"utf8"});
    let allowed = cx
        .runtime
        .block_on(ToolsHost.act(&(), action("allowed"), &cx))
        .unwrap()
        .unwrap();
    assert_eq!(allowed["range"]["data"], "preview after restart");
    let denied = cx
        .runtime
        .block_on(ToolsHost.act(&(), action("other"), &cx))
        .unwrap()
        .unwrap();
    assert!(denied["error"].as_str().unwrap().contains("not granted"));
}
