//! The snapshot's one exception (`docs/reference/vcs.md`, "Scoping
//! rules"): a new file over 1 MiB stays out of `@`, and `vcs_status`
//! names it with its size. Every other file is tracked.

use std::sync::Arc;

use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_agent::{
    plugin::Plugin,
    tool::{AgentTool, ToolCtx},
};
use tau_testing::block_on;
use tau_vcs::{Identity, MAX_NEW_FILE_SIZE, VcsPlugin};

fn status(tools: &[Arc<dyn AgentTool>]) -> Value {
    let ctx = ToolCtx::detached();
    let tool = tools.iter().find(|t| t.name() == "vcs_status").unwrap();
    block_on(tool.call(json!({}), ctx))
        .unwrap()
        .details
        .unwrap()
}

/// A new file is left out exactly when it is over the limit, and then
/// named with its size; a file already tracked stays tracked however
/// large it grows.
#[hegel::test(test_cases = 12)]
fn only_new_files_over_the_limit_are_left_out(tc: TestCase) {
    let offset = tc.draw(gs::integers::<i64>().min_value(-2).max_value(2));
    let tracked_first = tc.draw(gs::booleans());
    let size = (MAX_NEW_FILE_SIZE as i64 + offset) as usize;
    let dir = tempfile::tempdir().unwrap();
    let tools = VcsPlugin::new(
        tau_testing::block_on_io(tau_vcs::Vcs::init(
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

    let details = status(&tools);
    let left_out = !tracked_first && size as u64 > MAX_NEW_FILE_SIZE;
    let too_large = details["too_large"].as_array().unwrap();
    let changes = details["changes"].as_array().unwrap();
    if left_out {
        assert_eq!(
            too_large,
            &vec![json!({ "path": "big.bin", "size": size })]
        );
        assert!(changes.is_empty(), "{changes:?}");
    } else {
        assert!(too_large.is_empty(), "{too_large:?}");
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(changes[0]["path"], json!("big.bin"));
    }
}
