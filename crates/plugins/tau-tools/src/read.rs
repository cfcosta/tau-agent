//! `read` (`docs/reference/tools.md`, "read"), ported from pi's
//! `read.ts`.
//!
//! Text comes back truncated to 2000 lines or 50 KiB, with a notice
//! saying how to continue. Images, recognized by their bytes, come back
//! as an image block after a short note.
//!
//! **Deliberate difference from pi:** only the requested range is kept.
//! The file is streamed once: the lines before `offset` and after what
//! truncation can show are counted, not stored, so a huge file costs no
//! more memory than a small one. The result is the same as pi's.

use std::{
    fs::File,
    io::{BufRead, BufReader, Read as _},
    path::Path,
};

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use tau_agent::{
    error::ToolError,
    tool::{AgentTool, ToolCtx, ToolOutput},
};
use tau_ai::message::{ImageContent, InputBlock, TextContent};

use crate::{
    ABORTED,
    errno,
    image,
    path::Root,
    truncate::{Limit, MAX_BYTES, MAX_LINES, format_size, truncate_head},
};

/// Bytes read to recognize an image, as pi sniffs them.
const SNIFF_BYTES: usize = 4100;

#[derive(Debug, Deserialize, JsonSchema)]
struct Args {
    /// Path to the file to read (relative or absolute)
    path: String,
    /// Line number to start reading from (1-indexed)
    offset: Option<u64>,
    /// Maximum number of lines to read
    limit: Option<u64>,
}

/// The `read` tool.
#[derive(Debug, Clone)]
pub struct Read {
    root: Root,
    parameters: Value,
    output_schema: Value,
    description: String,
}

impl Read {
    pub fn new(root: Root) -> Self {
        Self {
            root,
            parameters: serde_json::to_value(schemars::schema_for!(Args))
                .expect("a generated schema is valid JSON"),
            output_schema: output_schema(),
            description: format!(
                "Read the contents of a file. Supports text files and images (jpg, png, gif, webp, bmp). Images are sent as attachments. For text files, output is truncated to {MAX_LINES} lines or {}KB (whichever is hit first). Use offset/limit for large files. When you need the full file, continue with offset until complete.",
                MAX_BYTES / 1024
            ),
        }
    }
}

#[async_trait]
impl AgentTool for Read {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn output_schema(&self) -> Option<&Value> {
        Some(&self.output_schema)
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let args: Args = serde_json::from_value(args)?;
        if ctx.cancel.is_cancelled() {
            return Err(ToolError::from(ABORTED));
        }
        let root = self.root.clone();
        let work = tokio::task::spawn_blocking(move || read(&root, &args));
        tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => Err(ToolError::from(ABORTED)),
            result = work => result?,
        }
    }
}

fn read(root: &Root, args: &Args) -> Result<ToolOutput, ToolError> {
    let path = root.resolve_read(&args.path);
    let mut file =
        File::open(&path).map_err(|e| errno::message(&e, "open", &path))?;
    let mut head = Vec::with_capacity(SNIFF_BYTES);
    (&mut file)
        .take(SNIFF_BYTES as u64)
        .read_to_end(&mut head)
        .map_err(|e| errno::message(&e, "read", &path))?;
    match image::detect(&head) {
        Some(mime_type) => read_image(&path, &args.path, head, file, mime_type),
        None => {
            let reader = BufReader::new(std::io::Cursor::new(head).chain(file));
            read_text(reader, &args.path, args.offset, args.limit, &path)
        }
    }
}

fn read_image(
    path: &Path,
    shown_path: &str,
    mut bytes: Vec<u8>,
    mut file: File,
    mime_type: &str,
) -> Result<ToolOutput, ToolError> {
    file.read_to_end(&mut bytes)
        .map_err(|e| errno::message(&e, "read", path))?;
    let (content, omitted) = match image::process(&bytes, mime_type) {
        Ok(processed) => {
            let mut note = format!("Read image file [{}]", processed.mime_type);
            for hint in &processed.hints {
                note.push('\n');
                note.push_str(hint);
            }
            (
                vec![
                    text(note),
                    InputBlock::Image(ImageContent {
                        data: processed.data,
                        mime_type: processed.mime_type,
                    }),
                ],
                false,
            )
        }
        Err(image::Omitted(message)) => (
            vec![text(format!("Read image file [{mime_type}]\n{message}"))],
            true,
        ),
    };
    let structured_content = serde_json::to_value(&content)
        .expect("tool content blocks serialize as JSON");
    Ok(ToolOutput {
        content,
        details: None,
        structured: Some(json!({
            "kind": "image",
            "path": shown_path,
            "content": structured_content,
            "omitted": omitted,
        })),
    })
}

fn text(text: String) -> InputBlock {
    InputBlock::Text(TextContent {
        text,
        text_signature: None,
    })
}

/// One line as far as it matters: its length, and its bytes up to just
/// over [`MAX_BYTES`] (any more cannot be shown).
struct Line {
    bytes: Vec<u8>,
    len: usize,
}

/// Reads the next `\n`-separated piece, storing at most `keep` bytes of
/// it. `None` at the end of the input; a final piece after the last
/// `\n` is returned even when empty, as `split("\n")` returns it.
fn next_line(
    reader: &mut impl BufRead,
    keep: usize,
    done: &mut bool,
) -> std::io::Result<Option<Line>> {
    if *done {
        return Ok(None);
    }
    let mut line = Line {
        bytes: Vec::new(),
        len: 0,
    };
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            *done = true;
            return Ok(Some(line));
        }
        let (chunk, found) = match buffer.iter().position(|&b| b == b'\n') {
            Some(at) => (&buffer[..at], true),
            None => (buffer, false),
        };
        let room = keep.saturating_sub(line.bytes.len());
        line.bytes
            .extend_from_slice(&chunk[..chunk.len().min(room)]);
        line.len += chunk.len();
        let consumed = chunk.len() + usize::from(found);
        reader.consume(consumed);
        if found {
            return Ok(Some(line));
        }
    }
}

fn read_text(
    mut reader: impl BufRead,
    shown_path: &str,
    offset: Option<u64>,
    limit: Option<u64>,
    path: &Path,
) -> Result<ToolOutput, ToolError> {
    let io =
        |e: std::io::Error| ToolError::from(errno::message(&e, "read", path));
    let start = offset.map_or(0, |o| o.saturating_sub(1)) as usize;
    let end = limit.map(|l| start.saturating_add(l as usize));
    // Enough to decide what truncation shows: one line or byte past.
    let (keep_lines, keep_bytes) = (MAX_LINES + 1, MAX_BYTES + 1);
    let mut kept: Vec<Line> = Vec::new();
    let mut kept_bytes = 0usize;
    let mut total = 0usize;
    let mut done = false;
    while let Some(line) =
        next_line(&mut reader, keep_bytes, &mut done).map_err(io)?
    {
        let in_range = total >= start && end.is_none_or(|end| total < end);
        if in_range && kept.len() < keep_lines && kept_bytes <= keep_bytes {
            kept_bytes += line.bytes.len() + 1;
            kept.push(line);
        }
        total += 1;
    }
    if start >= total {
        return Err(ToolError::from(format!(
            "Offset {} is beyond end of file ({total} lines total)",
            offset.unwrap_or(0)
        )));
    }
    let selected = kept
        .iter()
        .map(|line| String::from_utf8_lossy(&line.bytes))
        .collect::<Vec<_>>()
        .join("\n");
    let truncation = truncate_head(&selected, MAX_LINES, MAX_BYTES);
    let first = start + 1;
    let next_offset = if truncation.first_line_exceeds_limit {
        None
    } else if truncation.truncated() {
        let next_line = start.saturating_add(truncation.output_lines);
        (next_line < total).then(|| (next_line as u64).saturating_add(1))
    } else {
        end.filter(|end| *end < total)
            .map(|end| (end as u64).saturating_add(1))
    };
    let complete = start == 0
        && end.is_none_or(|end| end >= total)
        && !truncation.truncated();
    let structured = json!({
        "kind": "text",
        "path": shown_path,
        "text": truncation.content,
        "offset": (first as u64),
        "requested_limit": limit,
        "total_lines": total,
        "returned_lines": truncation.output_lines,
        "next_offset": next_offset,
        "truncated": truncation.truncated(),
        "complete": complete,
        "truncated_by": match truncation.by {
            Some(Limit::Lines) => Some("lines"),
            Some(Limit::Bytes) => Some("bytes"),
            None => None,
        },
        "first_line_exceeds_limit": truncation.first_line_exceeds_limit,
    });
    let text = if truncation.first_line_exceeds_limit {
        format!(
            "[Line {first} is {}, exceeds {} limit. Use bash: sed -n '{first}p' {shown_path} | head -c {MAX_BYTES}]",
            format_size(kept[0].len),
            format_size(MAX_BYTES)
        )
    } else if truncation.truncated() {
        let last = first + truncation.output_lines - 1;
        let limit_note = match truncation.by {
            Some(Limit::Bytes) => {
                format!(" ({} limit)", format_size(MAX_BYTES))
            }
            _ => String::new(),
        };
        format!(
            "{}\n\n[Showing lines {first}-{last} of {total}{limit_note}. Use offset={} to continue.]",
            truncation.content,
            last + 1
        )
    } else {
        match end {
            Some(end) if end < total => format!(
                "{}\n\n[{} more lines in file. Use offset={} to continue.]",
                truncation.content,
                total - end,
                end + 1
            ),
            _ => truncation.content.clone(),
        }
    };
    let details = truncation.truncated().then(|| {
        json!({"truncation": {
            "truncatedBy": match truncation.by {
                Some(Limit::Lines) => "lines",
                _ => "bytes",
            },
            "outputLines": truncation.output_lines,
            "firstLineExceedsLimit": truncation.first_line_exceeds_limit,
        }})
    });
    Ok(ToolOutput {
        content: vec![InputBlock::Text(TextContent {
            text,
            text_signature: None,
        })],
        details,
        structured: Some(structured),
    })
}

fn output_schema() -> Value {
    json!({
        "oneOf": [
            {
                "type": "object",
                "properties": {
                    "kind": {"const": "text"},
                    "path": {"type": "string"},
                    "text": {"type": "string"},
                    "offset": {"type": "integer", "minimum": 1},
                    "requested_limit": {
                        "anyOf": [
                            {"type": "integer", "minimum": 0},
                            {"type": "null"},
                        ]
                    },
                    "total_lines": {"type": "integer", "minimum": 0},
                    "returned_lines": {"type": "integer", "minimum": 0},
                    "next_offset": {
                        "anyOf": [
                            {"type": "integer", "minimum": 1},
                            {"type": "null"},
                        ]
                    },
                    "truncated": {"type": "boolean"},
                    "complete": {"type": "boolean"},
                    "truncated_by": {
                        "enum": ["lines", "bytes", null]
                    },
                    "first_line_exceeds_limit": {"type": "boolean"},
                },
                "required": [
                    "kind", "path", "text", "offset", "requested_limit",
                    "total_lines", "returned_lines", "next_offset",
                    "truncated", "complete", "truncated_by",
                    "first_line_exceeds_limit"
                ],
                "additionalProperties": false,
            },
            {
                "type": "object",
                "properties": {
                    "kind": {"const": "image"},
                    "path": {"type": "string"},
                    "content": {
                        "type": "array",
                        "items": {
                            "oneOf": [
                                {
                                    "type": "object",
                                    "properties": {
                                        "type": {"const": "text"},
                                        "text": {"type": "string"},
                                    },
                                    "required": ["type", "text"],
                                    "additionalProperties": false,
                                },
                                {
                                    "type": "object",
                                    "properties": {
                                        "type": {"const": "image"},
                                        "data": {"type": "string"},
                                        "mimeType": {"type": "string"},
                                    },
                                    "required": ["type", "data", "mimeType"],
                                    "additionalProperties": false,
                                },
                            ]
                        },
                    },
                    "omitted": {"type": "boolean"},
                },
                "required": ["kind", "path", "content", "omitted"],
                "additionalProperties": false,
            },
        ],
        "discriminator": {"propertyName": "kind"},
    })
}
