//! `write` (`docs/reference/tools.md`, "write"), ported from pi's
//! `write.ts` and `tools.test.ts`.

use std::sync::Arc;

use hegel::{TestCase, generators as gs};
use serde_json::json;
use tau_agent::tool::{AgentTool, RunId, ToolCtx, ToolUpdates};
use tau_tools::{ABORTED, path::Root, write::Write};

fn ctx() -> ToolCtx {
    let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
    ToolCtx::new(
        Default::default(),
        ToolUpdates::for_tests("call_1", sender),
        RunId("run_1".into()),
    )
}

fn cancelled_ctx() -> ToolCtx {
    let token = tokio_util::sync::CancellationToken::new();
    token.cancel();
    let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
    ToolCtx::new(
        token,
        ToolUpdates::for_tests("call_1", sender),
        RunId("run_1".into()),
    )
}

fn new_write(dir: &std::path::Path) -> Write {
    Write::new(Root::new(dir))
}

/// `write` returns exactly `Successfully wrote to <path>` and the file
/// holds exactly the given content, for any generated text
/// (`tools.md`, "write"; round trip).
#[hegel::test(test_cases = 50)]
fn write_round_trips_the_content(tc: TestCase) {
    let content: String = tc.draw(gs::text().max_size(200));
    let dir = tempfile::tempdir().unwrap();
    let write = new_write(dir.path());

    let output = tau_testing::block_on(
        write.call(json!({"path": "f.txt", "content": content}), ctx()),
    )
    .unwrap();

    assert_eq!(
        output.content,
        tau_agent::tool::ToolOutput::text("Successfully wrote to f.txt")
            .content
    );
    assert_eq!(output.details, None);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("f.txt")).unwrap(),
        content
    );
}

/// `write` creates missing parent directories (`tools.md`, "write").
#[test]
fn write_creates_parent_directories() {
    let dir = tempfile::tempdir().unwrap();
    let write = new_write(dir.path());
    let output = tau_testing::block_on(
        write.call(json!({"path": "a/b/c.txt", "content": "hi"}), ctx()),
    )
    .unwrap();
    assert!(output.content.iter().any(|_| true));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a/b/c.txt")).unwrap(),
        "hi"
    );
}

/// `write` overwrites an existing file completely (`tools.md`,
/// "write": "overwrites if it does").
#[test]
fn write_overwrites_existing_content() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f.txt");
    std::fs::write(&file, "old content, much longer than the new one").unwrap();
    let write = new_write(dir.path());
    tau_testing::block_on(
        write.call(json!({"path": "f.txt", "content": "new"}), ctx()),
    )
    .unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "new");
}

/// Cancelled (`tools.md`, "Error strings", "all"): nothing is written.
#[test]
fn aborted_when_cancelled_before_writing() {
    let dir = tempfile::tempdir().unwrap();
    let write = new_write(dir.path());
    let result = tau_testing::block_on(
        write.call(json!({"path": "f.txt", "content": "hi"}), cancelled_ctx()),
    );
    assert_eq!(result.unwrap_err().to_string(), ABORTED);
    assert!(!dir.path().join("f.txt").exists());
}

/// Per-path lock, shared with `edit` (`tools.md`, "write": "Holds the
/// same per-path mutex as `edit`"; `lock.rs`): overlapping `write`
/// calls to one file never interleave their write, so the file always
/// ends up holding exactly one writer's whole content, never a mix of
/// two.
#[hegel::test(test_cases = 50)]
fn concurrent_writes_never_interleave(tc: TestCase) {
    let n = tc.draw(gs::integers::<usize>().min_value(2).max_value(6));
    let size = tc.draw(gs::integers::<usize>().min_value(2000).max_value(6000));

    let dir = tempfile::tempdir().unwrap();
    let write = Arc::new(new_write(dir.path()));
    let contents: Vec<String> = (0..n)
        .map(|i| char::from(b'a' + i as u8).to_string().repeat(size))
        .collect();

    tau_testing::block_on(async {
        let mut tasks = Vec::new();
        for content in &contents {
            let write = write.clone();
            let content = content.clone();
            tasks.push(tokio::spawn(async move {
                write
                    .call(json!({"path": "f.txt", "content": content}), ctx())
                    .await
            }));
        }
        for task in tasks {
            task.await.unwrap().unwrap();
        }
    });

    let result = std::fs::read_to_string(dir.path().join("f.txt")).unwrap();
    assert!(
        contents.contains(&result),
        "the file mixed bytes from more than one writer"
    );
}

/// The tool's name, description and parameter descriptions are ported
/// from pi verbatim (`tools.md`: "Port pi's tool `description` strings
/// and parameter descriptions verbatim").
#[test]
fn tool_metadata_matches_the_ported_pi_strings() {
    let dir = tempfile::tempdir().unwrap();
    let write = new_write(dir.path());
    assert_eq!(write.name(), "write");
    assert_eq!(
        write.description(),
        "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Automatically creates parent directories."
    );
    let schema = write.parameters();
    assert_eq!(
        schema["properties"]["path"]["description"],
        "Path to the file to write (relative or absolute)"
    );
    assert_eq!(
        schema["properties"]["content"]["description"],
        "Content to write to the file"
    );
}
