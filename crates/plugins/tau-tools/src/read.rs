//! `read` (`docs/reference/tools.md`, "read"), ported from pi's
//! `read.ts`.
//!
//! Text comes back truncated to 2000 lines or 50 KiB, with a notice
//! saying how to continue. Images, recognized by their bytes, come back
//! as an image block after a short note.
//!
//! **Deliberate difference from pi:** only the requested range is kept.
//! The display reader streams once: the lines before `offset` and after
//! what truncation can show are counted, not stored. Configured artifact
//! publication makes a second bounded pass on the same opened inode.
//! Display memory stays bounded and its result is the same as pi's.

use std::{
    fs::{File, Metadata},
    io::{BufRead, BufReader, Read as _, Seek as _, SeekFrom},
    path::{Path, PathBuf},
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
use tau_artifacts::Bytes;
use tokio_util::sync::CancellationToken;

use crate::{
    ABORTED,
    artifact_grant::{ArtifactGrant, publish_artifact},
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

/// The display and publication use one opened inode. Rechecking both the
/// descriptor and its resolved path prevents a replacement or in-place edit
/// from being described as the displayed snapshot.
fn matches_version(
    file: &File,
    path: &Path,
    original: &Metadata,
) -> std::io::Result<bool> {
    let opened = file.metadata()?;
    let named = match path.metadata() {
        Ok(named) => named,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(false);
        }
        Err(error) => return Err(error),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let same = |current: &Metadata| {
            current.dev() == original.dev()
                && current.ino() == original.ino()
                && current.len() == original.len()
                && current.mtime() == original.mtime()
                && current.mtime_nsec() == original.mtime_nsec()
                && current.ctime() == original.ctime()
                && current.ctime_nsec() == original.ctime_nsec()
        };
        Ok(same(&opened) && same(&named))
    }
    #[cfg(not(unix))]
    {
        let same = |current: &Metadata| {
            current.len() == original.len()
                && current.modified().ok() == original.modified().ok()
        };
        Ok(same(&opened) && same(&named))
    }
}

struct SnapshotReader {
    file: File,
    path: PathBuf,
    version: Metadata,
}

impl std::io::Read for SnapshotReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let count = self.file.read(buffer)?;
        if count == 0
            && !matches_version(&self.file, &self.path, &self.version)?
        {
            return Err(std::io::Error::other(
                "file changed during read; artifact snapshot differs",
            ));
        }
        Ok(count)
    }
}

async fn publish_read_artifact(
    bytes: &Bytes,
    mut file: File,
    path: &Path,
    version: Metadata,
    ctx: &ToolCtx,
) -> Result<ArtifactGrant, ToolError> {
    if !matches_version(&file, path, &version).map_err(ToolError::other)? {
        return Err(
            "file changed during read; artifact snapshot differs".into()
        );
    }
    file.seek(SeekFrom::Start(0)).map_err(ToolError::other)?;
    let source = path.to_string_lossy().into_owned();
    publish_artifact(
        bytes,
        SnapshotReader {
            file,
            path: path.to_owned(),
            version,
        },
        &source,
        ctx,
    )
    .await
}

/// The `read` tool.
#[derive(Debug, Clone)]
pub struct Read {
    root: Root,
    artifacts: Option<Bytes>,
    parameters: Value,
    output_schema: Value,
    description: String,
}

impl Read {
    pub fn new(root: Root) -> Self {
        Self {
            root,
            artifacts: None,
            parameters: serde_json::to_value(schemars::schema_for!(Args))
                .expect("a generated schema is valid JSON"),
            output_schema: output_schema(),
            description: format!(
                "Read the contents of a file. Supports text files and images (jpg, png, gif, webp, bmp). Images are sent as attachments. For text files, output is truncated to {MAX_LINES} lines or {}KB (whichever is hit first). Use offset/limit for large files. When you need the full file, continue with offset until complete.",
                MAX_BYTES / 1024
            ),
        }
    }

    /// Enable immutable full-file artifacts for reads made in a run.
    pub fn with_artifacts(mut self, bytes: Bytes) -> Self {
        self.artifacts = Some(bytes);
        self
    }
}

struct DisplayedFile {
    output: ToolOutput,
    file: File,
    path: PathBuf,
    version: Metadata,
}

struct CancelOnDrop(CancellationToken);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
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
        let cancel = ctx.cancel.child_token();
        let _cancel_on_drop = CancelOnDrop(cancel.clone());
        let work =
            tokio::task::spawn_blocking(move || read(&root, &args, &cancel));
        let displayed = tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => Err(ToolError::from(ABORTED)),
            result = work => result?,
        }?;
        let mut output = displayed.output;
        let artifact = match &self.artifacts {
            None => Err("artifact storage is unavailable".to_owned()),
            Some(bytes) => publish_read_artifact(
                bytes,
                displayed.file,
                &displayed.path,
                displayed.version,
                &ctx,
            )
            .await
            .map_err(|error| error.to_string()),
        };
        if ctx.cancel.is_cancelled() {
            return Err(ABORTED.into());
        }
        let structured = output
            .structured
            .as_mut()
            .expect("read has structured output");
        match artifact {
            Ok(grant) => {
                structured["artifact"] = json!({
                    "id": grant.artifact.id,
                    "digest": grant.artifact.digest,
                    "size_bytes": grant.artifact.size_bytes,
                    "source": grant.source,
                });
            }
            Err(error) => structured["artifact_error"] = json!(error),
        }
        Ok(output)
    }
}

fn read(
    root: &Root,
    args: &Args,
    cancel: &CancellationToken,
) -> Result<DisplayedFile, ToolError> {
    let path = root.resolve_read(&args.path);
    let mut file =
        File::open(&path).map_err(|e| errno::message(&e, "open", &path))?;
    let version = file
        .metadata()
        .map_err(|e| errno::message(&e, "read", &path))?;
    let retained = file
        .try_clone()
        .map_err(|e| errno::message(&e, "read", &path))?;
    let mut head = Vec::with_capacity(SNIFF_BYTES);
    (&mut file)
        .take(SNIFF_BYTES as u64)
        .read_to_end(&mut head)
        .map_err(|e| errno::message(&e, "read", &path))?;
    let output = match image::detect(&head) {
        Some(mime_type) => {
            read_image(&path, &args.path, head, file, mime_type, cancel)
        }
        None => {
            let reader = BufReader::new(std::io::Cursor::new(head).chain(file));
            read_text(
                reader,
                &args.path,
                args.offset,
                args.limit,
                &path,
                cancel,
            )
        }
    }?;
    Ok(DisplayedFile {
        output,
        file: retained,
        path,
        version,
    })
}

fn read_image(
    path: &Path,
    shown_path: &str,
    mut bytes: Vec<u8>,
    mut file: File,
    mime_type: &str,
    cancel: &CancellationToken,
) -> Result<ToolOutput, ToolError> {
    let mut chunk = [0_u8; 64 * 1024];
    loop {
        if cancel.is_cancelled() {
            return Err(ABORTED.into());
        }
        let count = file
            .read(&mut chunk)
            .map_err(|e| errno::message(&e, "read", path))?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
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
    cancel: &CancellationToken,
) -> std::io::Result<Option<Line>> {
    if *done {
        return Ok(None);
    }
    let mut line = Line {
        bytes: Vec::new(),
        len: 0,
    };
    loop {
        if cancel.is_cancelled() {
            return Err(std::io::Error::other(ABORTED));
        }
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
    cancel: &CancellationToken,
) -> Result<ToolOutput, ToolError> {
    let io = |e: std::io::Error| {
        if cancel.is_cancelled() {
            ABORTED.into()
        } else {
            ToolError::from(errno::message(&e, "read", path))
        }
    };
    let start = offset.map_or(0, |o| o.saturating_sub(1)) as usize;
    let end = limit.map(|l| start.saturating_add(l as usize));
    // Enough to decide what truncation shows: one line or byte past.
    let (keep_lines, keep_bytes) = (MAX_LINES + 1, MAX_BYTES + 1);
    let mut kept: Vec<Line> = Vec::new();
    let mut kept_bytes = 0usize;
    let mut total = 0usize;
    let mut done = false;
    while let Some(line) =
        next_line(&mut reader, keep_bytes, &mut done, cancel).map_err(io)?
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
                    "artifact": artifact_schema(),
                    "artifact_error": {"type": "string"},
                },
                "required": [
                    "kind", "path", "text", "offset", "requested_limit",
                    "total_lines", "returned_lines", "next_offset",
                    "truncated", "complete", "truncated_by",
                    "first_line_exceeds_limit"
                ],
                "oneOf": [
                    {"required": ["artifact"]},
                    {"required": ["artifact_error"]},
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
                    "artifact": artifact_schema(),
                    "artifact_error": {"type": "string"},
                },
                "required": ["kind", "path", "content", "omitted"],
                "oneOf": [
                    {"required": ["artifact"]},
                    {"required": ["artifact_error"]},
                ],
                "additionalProperties": false,
            },
        ],
        "discriminator": {"propertyName": "kind"},
    })
}

fn artifact_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": {"type": "string"},
            "digest": {"type": "string"},
            "size_bytes": {"type": "integer", "minimum": 0},
            "source": {"type": "string"},
        },
        "required": ["id", "digest", "size_bytes", "source"],
        "additionalProperties": false,
    })
}

#[cfg(test)]
mod tests {
    use std::io::{Seek as _, SeekFrom};

    use tau_artifacts::{Bytes, Quotas};
    use tokio_util::sync::CancellationToken;

    use super::{Args, SnapshotReader, matches_version, read};
    use crate::path::Root;

    #[test]
    fn changed_or_replaced_source_cannot_publish_a_displayed_snapshot() {
        for replace in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("file.txt");
            std::fs::write(&path, "before").unwrap();
            let displayed = read(
                &Root::new(directory.path()),
                &Args {
                    path: "file.txt".into(),
                    offset: None,
                    limit: None,
                },
                &CancellationToken::new(),
            )
            .unwrap();
            if replace {
                std::fs::rename(&path, directory.path().join("old.txt"))
                    .unwrap();
            }
            std::fs::write(&path, "after extended").unwrap();
            assert!(
                !matches_version(&displayed.file, &path, &displayed.version)
                    .unwrap()
            );

            // If the source changes during streaming, the reader itself
            // refuses EOF, so the byte store cannot commit an object.
            let mut retained = displayed.file;
            retained.seek(SeekFrom::Start(0)).unwrap();
            let storage = Bytes::new(
                directory.path().join("artifacts"),
                Quotas::default(),
            )
            .unwrap();
            let outcome = storage.publish_reader(
                SnapshotReader {
                    file: retained,
                    path: path.clone(),
                    version: displayed.version,
                },
                &CancellationToken::new(),
            );
            assert!(outcome.is_err());
            assert!(
                std::fs::read_dir(directory.path().join("artifacts/objects"))
                    .unwrap()
                    .all(|entry| entry
                        .unwrap()
                        .path()
                        .extension()
                        .is_none_or(|ext| ext != "blob"))
            );
        }
    }
}
