//! The snapshot's one exception (`docs/reference/vcs.md`, "Scoping
//! rules"): a new file over 1 MiB stays out of `@`, and `vcs_status`
//! names it with its size. Every other file is tracked.

use std::sync::Arc;

use serde_json::{Value, json};
use tau_agent::{
    plugin::Plugin,
    tool::{AgentTool, ToolCtx},
};
use tau_testing::block_on;
use tau_vcs_host::{Identity, MAX_NEW_FILE_SIZE, VcsPlugin};

/// `vcs_status`'s text and details.
fn status(tools: &[Arc<dyn AgentTool>]) -> (String, Value) {
    let ctx = ToolCtx::detached();
    let tool = tools.iter().find(|t| t.name() == "vcs_status").unwrap();
    let output = block_on(tool.call(json!({}), ctx)).unwrap();
    let text = match &output.content[0] {
        tau_ai::message::InputBlock::Text(text) => text.text.clone(),
        other => panic!("expected text, got {other:?}"),
    };
    (text, output.details.unwrap())
}

/// A new file is left out exactly when it is over the limit, and then
/// named with its size, in the details and in the text; a file already
/// tracked stays tracked however large it grows. Every size from two
/// bytes under the limit to two over, new or tracked first: the rule
/// changes between the limit and one byte past it.
#[test]
fn only_new_files_over_the_limit_are_left_out() {
    for offset in -2i64..=2 {
        for tracked_first in [false, true] {
            check_left_out(offset, tracked_first);
        }
    }
}

fn check_left_out(offset: i64, tracked_first: bool) {
    let size = (MAX_NEW_FILE_SIZE as i64 + offset) as usize;
    let dir = tempfile::tempdir().unwrap();
    let tools = VcsPlugin::new(
        tau_testing::block_on_io(tau_vcs_host::Vcs::init(
            dir.path(),
            Identity::default(),
        ))
        .unwrap(),
    )
    .tools();
    let file = dir.path().join("big.bin");
    if tracked_first {
        std::fs::write(&file, b"small").unwrap();
        status(&tools);
    }
    std::fs::write(&file, vec![b'x'; size]).unwrap();
    let (text, details) = status(&tools);
    let left_out = !tracked_first && size as u64 > MAX_NEW_FILE_SIZE;
    let too_large = details["too_large"].as_array().unwrap();
    let changes = details["changes"].as_array().unwrap();
    let case = format!("offset {offset}, tracked first: {tracked_first}");
    if left_out {
        assert_eq!(
            too_large,
            &vec![json!({ "path": "big.bin", "size": size })],
            "{case}"
        );
        assert!(changes.is_empty(), "{case}: {changes:?}");
        assert!(
            text.contains(
                "Left out of @ (new files over 1 MiB are not snapshotted):\n\
                 big.bin (1.0 MiB)"
            ),
            "{case}: {text}"
        );
    } else {
        assert!(too_large.is_empty(), "{case}: {too_large:?}");
        assert!(!text.contains("Left out of @"), "{case}: {text}");
        assert_eq!(changes.len(), 1, "{case}: {changes:?}");
        assert_eq!(changes[0]["path"], json!("big.bin"), "{case}");
    }
}
