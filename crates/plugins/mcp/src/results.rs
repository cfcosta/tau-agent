//! What a call's `CallToolResult` becomes (`docs/reference/mcp.md`,
//! "Results"): content for the model, cut at 20 KB, and the whole
//! result, never cut, for scripts.
//!
//! The result arrives as JSON (the wire's `CallToolResult`), so nothing
//! here depends on the MCP client's types.

use std::{
    fs::OpenOptions,
    io::{self, Write},
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
};

use base64::Engine;
use serde_json::{Map, Value, json};
use tau_agent::tool::ToolOutput;
use tau_ai::message::{ImageContent, InputBlock, TextContent};

use crate::config::hex;

/// The most text the model gets from one call, in bytes. Past it, the
/// text is cut in the middle.
pub const TEXT_LIMIT: usize = 20 * 1024;

/// Where results too large or too binary for the model are written:
/// `$TMPDIR` by default.
#[derive(Debug, Clone)]
pub struct Spill {
    dir: PathBuf,
}

impl Default for Spill {
    fn default() -> Self {
        Self::new(std::env::temp_dir())
    }
}

impl Spill {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// Writes `bytes` to a new file `tau-mcp-<hex>.<extension>`, readable
    /// by its owner only.
    pub fn write(&self, bytes: &[u8], extension: &str) -> io::Result<PathBuf> {
        let mut id = [0u8; 8];
        getrandom::fill(&mut id).map_err(io::Error::other)?;
        let path = self.dir.join(format!("tau-mcp-{}.{extension}", hex(&id)));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        file.write_all(bytes)?;
        Ok(path)
    }
}

/// Where to cut `text` so at most `limit` bytes are kept: a prefix of at
/// most half the limit and a suffix of the rest, both at char
/// boundaries. `None` when the whole text fits.
pub fn cut_middle(text: &str, limit: usize) -> Option<(&str, &str)> {
    if text.len() <= limit {
        return None;
    }
    let mut head = limit / 2;
    while !text.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = text.len() - (limit - head);
    while !text.is_char_boundary(tail) {
        tail += 1;
    }
    Some((&text[..head], &text[tail..]))
}

/// Tokens, as Codemode counts them: chars / 4, rounded up.
fn tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

/// `text`, cut in the middle past `limit` bytes in Codemode's format,
/// with the full text written to a spill file.
pub fn truncate(text: &str, limit: usize, spill: &Spill) -> String {
    let Some((head, tail)) = cut_middle(text, limit) else {
        return text.to_owned();
    };
    let omitted = &text[head.len()..text.len() - tail.len()];
    let saved = match spill.write(text.as_bytes(), "txt") {
        Ok(path) => format!(
            "[Full output: {} (read it with offset/limit)]",
            path.display()
        ),
        Err(error) => format!("[The full output could not be saved: {error}]"),
    };
    format!(
        "Warning: truncated output (original token count: {})\nTotal output lines: {}\n\n{head}…{} tokens truncated…{tail}\n\n{saved}",
        tokens(text),
        text.lines().count(),
        tokens(omitted),
    )
}

/// The text a call that failed without any says.
pub fn error_text(server: &str, tool: &str) -> String {
    format!("MCP tool {server}/{tool} returned an error")
}

/// A mapped result.
#[derive(Debug, Clone, PartialEq)]
pub struct Mapped {
    /// Content for the model; `structured` for scripts, the whole result
    /// but `_meta`.
    pub output: ToolOutput,
    /// The result's `isError`: the call fails, with `output`.
    pub is_error: bool,
}

/// Maps a `CallToolResult`, as JSON, for the model and for scripts.
pub fn map_result(
    server: &str,
    tool: &str,
    result: &Value,
    spill: &Spill,
) -> Mapped {
    let blocks = result
        .get("content")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut structured = Map::new();
    structured.insert("content".into(), Value::Array(blocks.clone()));
    if let Some(value) = result.get("structuredContent") {
        structured.insert("structuredContent".into(), value.clone());
    }
    structured.insert("isError".into(), json!(is_error));

    let mut items: Vec<InputBlock> = blocks
        .iter()
        .map(|block| content_block(server, block, spill))
        .collect();
    if items.is_empty()
        && let Some(value) = result.get("structuredContent")
    {
        items.push(text_block(
            serde_json::to_string_pretty(value).unwrap_or_default(),
        ));
    }
    let mut items = limit_text(items, spill);
    if is_error && !items.iter().any(|item| matches!(item, InputBlock::Text(_)))
    {
        items.insert(0, text_block(error_text(server, tool)));
    }
    Mapped {
        output: ToolOutput {
            content: items,
            details: None,
            structured: Some(Value::Object(structured)),
        },
        is_error,
    }
}

pub(crate) fn text_block(text: impl Into<String>) -> InputBlock {
    InputBlock::Text(TextContent {
        text: text.into(),
        text_signature: None,
    })
}

fn str_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// One content block for the model.
fn content_block(server: &str, block: &Value, spill: &Spill) -> InputBlock {
    match str_field(block, "type") {
        Some("text") => {
            text_block(str_field(block, "text").unwrap_or_default())
        }
        Some("image") => {
            match (str_field(block, "data"), str_field(block, "mimeType")) {
                (Some(data), Some(mime)) => InputBlock::Image(ImageContent {
                    data: data.to_owned(),
                    mime_type: mime.to_owned(),
                }),
                _ => text_block("[An image without data or type]"),
            }
        }
        Some("audio") => {
            let mime = str_field(block, "mimeType").unwrap_or("audio");
            text_block(match save(str_field(block, "data"), mime, spill) {
                Ok(path) => {
                    format!("[Audio ({mime}) saved to {}]", path.display())
                }
                Err(error) => format!("[Audio ({mime}): {error}]"),
            })
        }
        Some("resource_link") => text_block(resource_link(server, block)),
        Some("resource") => {
            let resource = block.get("resource").unwrap_or(&Value::Null);
            let uri = str_field(resource, "uri").unwrap_or_default();
            if let Some(text) = str_field(resource, "text") {
                return text_block(text);
            }
            let mime = str_field(resource, "mimeType")
                .unwrap_or("application/octet-stream");
            text_block(match save(str_field(resource, "blob"), mime, spill) {
                Ok(path) => {
                    format!(
                        "[Resource {uri} ({mime}) saved to {}]",
                        path.display()
                    )
                }
                Err(error) => format!("[Resource {uri} ({mime}): {error}]"),
            })
        }
        _ => text_block(block.to_string()),
    }
}

/// `[Resource <uri> "<title>" (<mime>, <size>): <description>. Read it
/// with read_mcp_resource (server "<server>")]`, each part left out when
/// the link does not have it. The title is the link's `title`, else its
/// `name`.
pub fn resource_link(server: &str, block: &Value) -> String {
    let mut text =
        format!("[Resource {}", str_field(block, "uri").unwrap_or_default());
    if let Some(title) =
        str_field(block, "title").or_else(|| str_field(block, "name"))
    {
        text.push_str(&format!(" \"{title}\""));
    }
    let mut about = Vec::new();
    if let Some(mime) = str_field(block, "mimeType") {
        about.push(mime.to_owned());
    }
    if let Some(size) = block.get("size").and_then(Value::as_u64) {
        about.push(size.to_string());
    }
    if !about.is_empty() {
        text.push_str(&format!(" ({})", about.join(", ")));
    }
    if let Some(description) = str_field(block, "description") {
        text.push_str(&format!(": {}", description.trim_end_matches('.')));
    }
    text.push_str(&format!(
        ". Read it with read_mcp_resource (server \"{server}\")]"
    ));
    text
}

/// What a `ReadResourceResult`'s `contents` become for the model: text as
/// text, images as images, other binary saved to a spill file and named
/// by its path, the text cut past [`TEXT_LIMIT`] as a call's is. MCP
/// apps' contents are left out ([`crate::resources::is_app`]).
pub fn resource_contents(contents: &[Value], spill: &Spill) -> Vec<InputBlock> {
    let items = contents
        .iter()
        .filter(|content| {
            !crate::resources::is_app(
                str_field(content, "uri").unwrap_or_default(),
                str_field(content, "mimeType"),
            )
        })
        .map(|content| {
            let uri = str_field(content, "uri").unwrap_or_default();
            if let Some(text) = str_field(content, "text") {
                return text_block(text);
            }
            let mime = str_field(content, "mimeType")
                .unwrap_or("application/octet-stream");
            let blob = str_field(content, "blob");
            if mime.starts_with("image/")
                && let Some(data) = blob
            {
                return InputBlock::Image(ImageContent {
                    data: data.trim().to_owned(),
                    mime_type: mime.to_owned(),
                });
            }
            text_block(match save(blob, mime, spill) {
                Ok(path) => format!(
                    "[Resource {uri} ({mime}) saved to {}]",
                    path.display()
                ),
                Err(error) => format!("[Resource {uri} ({mime}): {error}]"),
            })
        })
        .collect();
    limit_text(items, spill)
}

/// Decodes base64 `data` and writes it to a spill file.
fn save(
    data: Option<&str>,
    mime: &str,
    spill: &Spill,
) -> Result<PathBuf, String> {
    let data = data.ok_or("no data")?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.trim())
        .map_err(|_| "its data is not valid base64".to_owned())?;
    spill
        .write(&bytes, extension(mime))
        .map_err(|error| format!("could not be saved: {error}"))
}

fn extension(mime: &str) -> &'static str {
    match mime.split(';').next().unwrap_or_default().trim() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "application/pdf" => "pdf",
        "application/json" => "json",
        "application/zip" => "zip",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/mpeg" => "mp3",
        "audio/ogg" => "ogg",
        mime if mime.starts_with("text/") => "txt",
        _ => "bin",
    }
}

/// Keeps text blocks as they are while their text fits; past
/// [`TEXT_LIMIT`], merges them into one cut block, first, images after.
fn limit_text(items: Vec<InputBlock>, spill: &Spill) -> Vec<InputBlock> {
    let total: usize = items
        .iter()
        .map(|item| match item {
            InputBlock::Text(text) => text.text.len(),
            InputBlock::Image(_) => 0,
        })
        .sum();
    if total <= TEXT_LIMIT {
        return items;
    }
    let mut texts = Vec::new();
    let mut images = Vec::new();
    for item in items {
        match item {
            InputBlock::Text(text) => texts.push(text.text),
            image @ InputBlock::Image(_) => images.push(image),
        }
    }
    let joined = texts.join("\n");
    let mut out = vec![text_block(truncate(&joined, TEXT_LIMIT, spill))];
    out.extend(images);
    out
}
