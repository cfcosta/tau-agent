//! Results (`docs/reference/mcp.md`, "Results"): each kind of content
//! block as examples. The cut itself is `heiko_agent::output`'s.

use std::os::unix::fs::PermissionsExt;

use base64::Engine;
use hegel::{TestCase, generators as gs};
use heiko_agent::output::Spill;
use heiko_ai::message::{ImageContent, InputBlock, TextContent};
use heiko_mcp_host::results::{TEXT_TOKENS, error_text, map_result};
use serde_json::{Value, json};

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
    let spill = Spill::new(dir.path(), "heiko-mcp");
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

/// A content block the property draws: text or an image.
#[derive(Debug, Clone, hegel::PrettyPrintable)]
enum Block {
    Text(String),
    Image(String),
}

/// Text in any of four shapes: short; a drawn piece repeated, up to
/// well past the budget; exactly at the budget's edge in characters; or
/// many single characters, whose tokens add up past the budget while
/// their join does not.
#[hegel::composite]
fn block(tc: &TestCase) -> Block {
    let alphabet = "ab é\n雪…";
    match tc.draw(gs::integers::<u8>().max_value(4)) {
        0 => Block::Image(tc.draw(gs::sampled_from(vec![
            "aGk=".to_owned(),
            "AAEC".to_owned(),
            "iVBORw0K".to_owned(),
        ]))),
        1 => Block::Text(tc.draw(gs::text().alphabet(alphabet).max_size(30))),
        2 => {
            let piece =
                tc.draw(gs::text().alphabet(alphabet).min_size(1).max_size(8));
            let times = tc.draw(gs::integers::<usize>().max_value(6_000));
            Block::Text(piece.repeat(times))
        }
        3 => {
            let budget = TEXT_TOKENS as usize * 4;
            let len =
                tc.draw(gs::sampled_from(vec![budget - 1, budget, budget + 1]));
            let piece = tc.draw(gs::sampled_from(vec!["a", "é", "雪"]));
            Block::Text(piece.repeat(len))
        }
        _ => Block::Text("x".to_owned()),
    }
}

fn wire(block: &Block) -> Value {
    match block {
        Block::Text(text) => json!({"type": "text", "text": text}),
        Block::Image(data) => {
            json!({"type": "image", "data": data, "mimeType": "image/png"})
        }
    }
}

fn input(block: &Block) -> InputBlock {
    match block {
        Block::Text(text) => InputBlock::Text(TextContent {
            text: text.clone(),
            text_signature: None,
        }),
        Block::Image(data) => InputBlock::Image(ImageContent {
            data: data.clone(),
            mime_type: "image/png".to_owned(),
        }),
    }
}

/// Tokens as the docs count them: four characters a token, rounded up.
fn tokens(text: &str) -> u64 {
    (text.chars().count() as u64).div_ceil(4)
}

/// The files in a spill directory.
fn spilled(dir: &tempfile::TempDir) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect()
}

/// Text and images within the model's share reach it as they came.
/// Past it, the texts are joined by line breaks into one block before
/// the images, which keep their order; joined text still within the
/// share is whole, and longer text keeps its first and last half of the
/// share's characters under a header, with the whole text in a spill
/// file the block names. Scripts get every block, never cut.
#[hegel::test(test_cases = 150)]
fn the_model_gets_its_share_and_the_spill_the_rest(tc: TestCase) {
    let blocks: Vec<Block> = tc.draw(gs::vecs(block()).max_size(8));
    let blocks = if tc.draw(gs::booleans()) {
        blocks
    } else {
        // Single characters past the budget by count, within it joined.
        let mut many = blocks;
        many.extend((0..=TEXT_TOKENS).map(|_| Block::Text("x".to_owned())));
        many
    };
    let (dir, spill) = spill();
    let wired: Vec<Value> = blocks.iter().map(wire).collect();
    let mapped = map_result("s", "t", &json!({"content": wired}), &spill);
    let content = &mapped.output.content;
    assert_eq!(
        mapped.output.structured.unwrap()["content"],
        Value::Array(wired.clone())
    );

    let texts: Vec<&str> = blocks
        .iter()
        .filter_map(|block| match block {
            Block::Text(text) => Some(text.as_str()),
            Block::Image(_) => None,
        })
        .collect();
    let images: Vec<InputBlock> = blocks
        .iter()
        .filter(|block| matches!(block, Block::Image(_)))
        .map(input)
        .collect();
    let total: u64 = texts.iter().map(|text| tokens(text)).sum();
    if total <= TEXT_TOKENS {
        let expected: Vec<InputBlock> = blocks.iter().map(input).collect();
        assert_eq!(content, &expected);
        assert!(spilled(&dir).is_empty());
        return;
    }

    assert_eq!(
        content[1..],
        images[..],
        "the images, in order, after the text"
    );
    let InputBlock::Text(shown) = &content[0] else {
        panic!("the text comes first: {content:?}");
    };
    let shown = shown.text.as_str();
    let joined = texts.join("\n");
    if tokens(&joined) <= TEXT_TOKENS {
        assert_eq!(shown, joined);
        assert!(spilled(&dir).is_empty());
        return;
    }

    let files = spilled(&dir);
    assert_eq!(files.len(), 1, "{files:?}");
    assert_eq!(std::fs::read_to_string(&files[0]).unwrap(), joined);
    let chars: Vec<char> = joined.chars().collect();
    let half = TEXT_TOKENS as usize * 2;
    let head: String = chars[..half].iter().collect();
    let tail: String = chars[chars.len() - half..].iter().collect();
    let removed: String = chars[half..chars.len() - half].iter().collect();
    let header = format!(
        "Warning: truncated output (original token count: {})\n\
         Total output lines: {}\n\n",
        tokens(&joined),
        joined.lines().count().max(1)
    );
    let footer = format!(
        "\n\n[Full output: {} (read it with offset/limit)]",
        files[0].display()
    );
    let marker = format!("…{} tokens truncated…", tokens(&removed));
    assert_eq!(shown, format!("{header}{head}{marker}{tail}{footer}"));
    assert!(
        shown.chars().count()
            <= TEXT_TOKENS as usize * 4
                + header.chars().count()
                + marker.chars().count()
                + footer.chars().count()
    );
}

/// A text block of exactly the share's characters is whole; one more
/// character is cut.
#[test]
fn the_share_is_inclusive() {
    let budget = TEXT_TOKENS as usize * 4;
    let (dir, spill) = spill();
    let at = "é".repeat(budget);
    let mapped = map_result(
        "s",
        "t",
        &json!({"content": [{"type": "text", "text": at}]}),
        &spill,
    );
    assert_eq!(texts(&mapped.output.content), [at.as_str()]);
    assert!(spilled(&dir).is_empty());

    // Two texts at the share between them, an image first: as they came.
    let half = "é".repeat(budget / 2);
    let mapped = map_result(
        "s",
        "t",
        &json!({"content": [
            {"type": "image", "data": "aGk=", "mimeType": "image/png"},
            {"type": "text", "text": half},
            {"type": "text", "text": half}
        ]}),
        &spill,
    );
    assert!(matches!(mapped.output.content[0], InputBlock::Image(_)));
    assert_eq!(
        texts(&mapped.output.content),
        [half.as_str(), half.as_str()]
    );
    assert!(spilled(&dir).is_empty());

    let over = "é".repeat(budget + 1);
    let mapped = map_result(
        "s",
        "t",
        &json!({"content": [{"type": "text", "text": over}]}),
        &spill,
    );
    assert!(
        texts(&mapped.output.content)[0]
            .starts_with("Warning: truncated output")
    );
    assert_eq!(spilled(&dir).len(), 1);
}

/// Many short texts are past the share by their own counts, rounded up
/// each, and within it once joined: the model gets them joined and
/// whole, with nothing spilled.
#[test]
fn short_texts_past_the_share_are_joined_whole() {
    let (dir, spill) = spill();
    let blocks: Vec<Value> = (0..=TEXT_TOKENS)
        .map(|_| json!({"type": "text", "text": "x"}))
        .collect();
    let mapped = map_result("s", "t", &json!({"content": blocks}), &spill);
    let joined = vec!["x"; TEXT_TOKENS as usize + 1].join("\n");
    assert_eq!(texts(&mapped.output.content), [joined.as_str()]);
    assert!(spilled(&dir).is_empty());
}
