//! The vcs tools inside a run: an agent with `VcsPlugin` checks the
//! status, commits and reads the log, with `ScriptedModel` choosing the
//! calls.

use serde_json::json;
use tau_agent::agent::Agent;
use tau_ai::message::{InputBlock, Message};
use tau_store::{Entry, Store};
use tau_testing::scripted::ScriptedModel;
use tau_vcs::{Identity, Vcs, VcsPlugin};

#[test]
fn a_run_commits_through_the_tools() {
    let dir = tempfile::tempdir().unwrap();
    let vcs = Vcs::init(dir.path(), Identity::default()).unwrap();
    std::fs::write(dir.path().join("notes.txt"), "hello\n").unwrap();
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("vcs_status", json!({})))
        .turn(|t| {
            t.tool_call("vcs_commit", json!({"message": "Add notes"}))
                .tool_call("vcs_log", json!({}))
        })
        .turn(|t| t.text("done"));
    let (outcome, entries) = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let store = Store::memory().await.unwrap();
            let outcome = Agent::new(llm.clone())
                .plugin(VcsPlugin::new(vcs))
                .run("commit the notes", &store)
                .await
                .unwrap();
            let entries = store.transcript(&outcome.run.0).await.unwrap();
            (outcome, entries)
        });
    assert_eq!(outcome.text, "done");

    let names: Vec<String> = llm.requests()[0]
        .settings
        .tools
        .iter()
        .map(|t| t.name.clone())
        .collect();
    assert_eq!(names.len(), 9);
    assert_eq!(names[0], "vcs_status");

    let results: Vec<(String, bool, String)> = entries
        .into_iter()
        .filter_map(|entry| match entry {
            Entry::Message { body, .. } => {
                serde_json::from_str::<Message>(&body).ok()
            }
            _ => None,
        })
        .filter_map(|message| match message {
            Message::ToolResult(result) => {
                let text = result
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        InputBlock::Text(t) => Some(t.text.clone()),
                        InputBlock::Image(_) => None,
                    })
                    .collect();
                Some((result.tool_name, result.is_error, text))
            }
            _ => None,
        })
        .collect();
    let expect = [
        ("vcs_status", "A notes.txt"),
        ("vcs_commit", "Add notes"),
        ("vcs_log", "Add notes"),
    ];
    assert_eq!(results.len(), expect.len(), "{results:?}");
    for ((name, is_error, text), (want_name, want_text)) in
        results.iter().zip(expect)
    {
        assert_eq!(name, want_name);
        assert!(!is_error, "{name}: {text}");
        assert!(text.contains(want_text), "{name}: {text}");
    }
}
