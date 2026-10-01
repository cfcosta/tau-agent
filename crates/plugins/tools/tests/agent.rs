//! The tools inside a run: an agent with the `CodingTools` plugin
//! writes, edits, reads, searches and runs a command in a real
//! directory, with `ScriptedModel` choosing the calls.

#![cfg(unix)]

use serde_json::json;
use tau_agent::agent::Agent;
use tau_ai::message::{InputBlock, Message};
use tau_store::{Entry, Store};
use tau_testing::scripted::ScriptedModel;
use tau_tools::{
    coding_tools,
    path::Root,
    plugin::{CodingTools, Tool},
};

/// Every tool is offered to the model in strict or plain form, each
/// call runs against the root, and each result lands in the transcript
/// in order: the file is written, edited, read back, found, listed,
/// grepped and printed by the shell.
#[test]
fn a_run_uses_every_tool() {
    let dir = tempfile::tempdir().unwrap();
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("write", json!({"path": "notes/a.txt", "content": "hello world\n"})))
        .turn(|t| {
            t.tool_call(
                "edit",
                json!({"path": "notes/a.txt", "edits": [{"oldText": "world", "newText": "tau"}]}),
            )
        })
        .turn(|t| {
            t.tool_call("read", json!({"path": "notes/a.txt"}))
                .tool_call("find", json!({"pattern": "*.txt"}))
                .tool_call("ls", json!({"path": "notes"}))
                .tool_call("grep", json!({"pattern": "tau"}))
                .tool_call("bash", json!({"command": "cat notes/a.txt"}))
        })
        .turn(|t| t.text("done"));
    let outcome = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let store = Store::memory().await.unwrap();
            let outcome = Agent::new(llm.clone())
                .plugin(CodingTools::new(Root::new(dir.path())))
                .run("edit the notes", &store)
                .await
                .unwrap();
            let entries = store.transcript(&outcome.run.0).await.unwrap();
            (outcome, entries)
        });
    let (outcome, entries) = outcome;
    assert_eq!(outcome.text, "done");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("notes/a.txt")).unwrap(),
        "hello tau\n"
    );

    let names: Vec<String> = llm.requests()[0]
        .settings
        .tools
        .iter()
        .map(|t| t.name.clone())
        .collect();
    assert_eq!(
        names,
        ["read", "bash", "edit", "write", "grep", "find", "ls"]
    );

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
        ("write", "Successfully wrote"),
        ("edit", "notes/a.txt"),
        ("read", "hello tau"),
        ("find", "notes/a.txt"),
        ("ls", "a.txt"),
        ("grep", "notes/a.txt:1: hello tau"),
        ("bash", "hello tau"),
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

fn tool_names(
    tools: &[std::sync::Arc<dyn tau_agent::tool::AgentTool>],
) -> Vec<&str> {
    tools.iter().map(|tool| tool.name()).collect()
}

/// `coding_tools` and the plugin offer the same seven tools, in pi's
/// order; `only` and `without` pick a subset, and the order stays pi's
/// whatever order the subset was named in.
#[test]
fn the_plugin_picks_tools_in_pis_order() {
    use tau_agent::plugin::Plugin;
    let root = Root::new("/tmp");
    let all = CodingTools::new(root.clone());
    assert_eq!(all.name(), tau_tools::ui::NAME);
    assert_eq!(tool_names(&all.tools()), tool_names(&coding_tools(&root)));
    assert_eq!(tool_names(&all.tools()), Tool::ALL.map(Tool::name).to_vec());
    let picked = CodingTools::new(root.clone()).only(&[
        Tool::Ls,
        Tool::Read,
        Tool::Grep,
    ]);
    assert_eq!(picked.selected(), [Tool::Read, Tool::Grep, Tool::Ls]);
    assert_eq!(tool_names(&picked.tools()), ["read", "grep", "ls"]);
    let no_shell = CodingTools::new(root).without(Tool::Bash);
    assert_eq!(
        tool_names(&no_shell.tools()),
        ["read", "edit", "write", "grep", "find", "ls"]
    );
}
