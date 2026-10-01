//! Results (`docs/reference/mcp.md`, "Results"): each kind of content
//! block as examples. The cut itself is `tau_agent::output`'s.

use std::os::unix::fs::PermissionsExt;

use base64::Engine;
use serde_json::{Value, json};
use tau_agent::output::Spill;
use tau_ai::message::InputBlock;
use tau_mcp::results::{TEXT_TOKENS, error_text, map_result};

fn texts(blocks: &[InputBlock]) -> Vec<&str> {
    blocks
        .iter()
        .filter_map(|block| match block {
            InputBlock::Text(text) => Some(text.text.as_str()),
            InputBlock::Image(_) => None,
        })
        .collect()
}

fn spill() -> (tempfile::TempDir, Spill) {
    let dir = tempfile::tempdir().unwrap();
    let spill = Spill::new(dir.path(), "tau-mcp");
    (dir, spill)
}

#[test]
fn content_blocks_for_the_model() {
    let (dir, spill) = spill();
    let blob =
        base64::engine::general_purpose::STANDARD.encode(b"\x00\x01binary");
    let result = json!({
        "content": [
            {"type": "text", "text": "hello"},
            {"type": "image", "data": "aGk=", "mimeType": "image/png"},
            {"type": "resource_link", "uri": "file:///a", "name": "a", "title": "A file",
             "mimeType": "text/plain", "size": 12, "description": "The a file."},
            {"type": "resource_link", "uri": "file:///b", "name": "b"},
            {"type": "resource", "resource": {"uri": "file:///t", "text": "embedded"}},
            {"type": "resource", "resource": {"uri": "file:///z", "mimeType": "application/zip", "blob": blob}}
        ],
        "_meta": {"secret": true}
    });
    let mapped = map_result("s", "t", &result, &spill);
    assert!(!mapped.is_error);
    let content = &mapped.output.content;
    assert!(
        matches!(&content[1], InputBlock::Image(image) if image.mime_type == "image/png")
    );
    let texts = texts(content);
    assert_eq!(texts[0], "hello");
    assert_eq!(
        texts[1],
        "[Resource file:///a \"A file\" (text/plain, 12): The a file. Read it with \
         read_mcp_resource (server \"s\")]"
    );
    assert_eq!(
        texts[2],
        "[Resource file:///b \"b\". Read it with read_mcp_resource (server \"s\")]"
    );
    assert_eq!(texts[3], "embedded");
    let saved = texts[4]
        .strip_prefix("[Resource file:///z (application/zip) saved to ")
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap();
    assert!(saved.starts_with(dir.path().to_str().unwrap()));
    assert_eq!(std::fs::read(saved).unwrap(), b"\x00\x01binary");
    let mode = std::fs::metadata(saved).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);

    // Scripts get the whole result but `_meta`, never cut.
    let structured = mapped.output.structured.unwrap();
    assert_eq!(structured["content"], result["content"]);
    assert_eq!(structured["isError"], json!(false));
    assert!(structured.get("_meta").is_none());
    assert!(structured.get("structuredContent").is_none());
}

#[test]
fn structured_content_is_shown_when_there_is_no_content() {
    let (_dir, spill) = spill();
    let result = json!({"content": [], "structuredContent": {"n": 1}});
    let mapped = map_result("s", "t", &result, &spill);
    assert_eq!(texts(&mapped.output.content), ["{\n  \"n\": 1\n}"]);
    assert_eq!(
        mapped.output.structured.unwrap(),
        json!({"content": [], "structuredContent": {"n": 1}, "isError": false})
    );
}

#[test]
fn an_error_without_text_says_so() {
    let (_dir, spill) = spill();
    let mapped = map_result(
        "git",
        "push",
        &json!({"content": [], "isError": true}),
        &spill,
    );
    assert!(mapped.is_error);
    assert_eq!(texts(&mapped.output.content), [error_text("git", "push")]);
    assert_eq!(
        texts(&mapped.output.content),
        ["MCP tool git/push returned an error"]
    );
    assert_eq!(mapped.output.structured.unwrap()["isError"], json!(true));

    let mapped = map_result(
        "git",
        "push",
        &json!({"content": [{"type": "text", "text": "rejected"}], "isError": true}),
        &spill,
    );
    assert_eq!(texts(&mapped.output.content), ["rejected"]);
}

/// Text past the model's share is merged and cut for the model; scripts
/// still get all of it.
#[test]
fn long_text_is_cut_for_the_model_only() {
    let (dir, spill) = spill();
    let long = "é".repeat(TEXT_TOKENS as usize * 5);
    let result = json!({"content": [
        {"type": "text", "text": long},
        {"type": "image", "data": "aGk=", "mimeType": "image/png"},
        {"type": "text", "text": "end"}
    ]});
    let mapped = map_result("s", "t", &result, &spill);
    let content = &mapped.output.content;
    assert_eq!(content.len(), 2);
    assert!(matches!(content[1], InputBlock::Image(_)));
    let text = texts(content)[0];
    assert!(text.starts_with("Warning: truncated output"));
    assert!(text.contains("end\n\n[Full output: "));
    let spilled = std::fs::read_dir(dir.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(spilled.path()).unwrap(),
        format!("{long}\nend")
    );
    let structured: Value = mapped.output.structured.unwrap();
    assert_eq!(structured["content"][0]["text"], json!(long));
}
