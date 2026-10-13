//! The canvas tools, and the storage that keeps the images they show.
//!
//! The tools read the canvas from the run's records each call, along
//! its fork chain, so a fork or a resumed run goes on with the items it
//! inherits. Calls in one run take turns: two items made at once would
//! get the same number.

use std::{
    io::Cursor,
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use tau_agent::{
    error::ToolError,
    plugin::{Plugin, PluginCtx},
    tool::{AgentTool, ToolCtx, ToolOutput, TypedTool, typed},
};
use tau_artifacts::{Artifact, Bytes, Encoding, Quotas};
use tokio_util::sync::CancellationToken;

use crate::{
    Blob,
    Canvas,
    Content,
    EDIT,
    Edit,
    ImageRef,
    Kind,
    MAX_DOCUMENT_BYTES,
    MAX_IMAGE_BYTES,
    NAME,
    READ,
    Record,
    SHOW,
    WRITE,
    apply_edits,
    format_name,
};

/// The most the storage keeps in all: 1 GiB.
const TOTAL_BYTES: u64 = 1024 * 1024 * 1024;

/// One read of the storage: the most a range gives that is a multiple
/// of 3, so its base64 joins the next range's whole.
const RANGE: u32 = 65_535;

/// Where the canvas keeps the images it shows: an artifact store of its
/// own in the repository's directory, `canvas/`. The canvas keeps a
/// copy, so an image still shows once its file changes or goes.
#[derive(Clone)]
pub struct Storage(Bytes);

impl Storage {
    /// The storage in `dir`, made if it is not there. Blocks: call it
    /// from `spawn_blocking`.
    pub fn open(dir: &Path) -> anyhow::Result<Self> {
        Ok(Self(Bytes::new(
            dir,
            Quotas {
                max_artifact_bytes: MAX_IMAGE_BYTES,
                total_bytes: TOTAL_BYTES,
            },
        )?))
    }

    /// The storage of the repository whose directory is `dir`.
    pub fn dir_of(repo_dir: &Path) -> PathBuf {
        repo_dir.join("canvas")
    }

    /// Keeps `bytes`, and says where.
    pub async fn put(
        &self,
        bytes: Vec<u8>,
        cancel: &CancellationToken,
    ) -> anyhow::Result<Blob> {
        let storage = self.0.clone();
        let cancel = cancel.clone();
        let artifact = tokio::task::spawn_blocking(move || {
            storage.publish_reader(Cursor::new(bytes), &cancel)
        })
        .await??;
        Ok(Blob {
            id: artifact.id().to_owned(),
            digest: artifact.digest().to_owned(),
            size_bytes: artifact.size_bytes(),
        })
    }

    /// The bytes `blob` refers to, in base64.
    pub async fn get(&self, blob: &Blob) -> anyhow::Result<String> {
        let artifact: Artifact = serde_json::from_value(json!(blob))?;
        let storage = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let cancel = CancellationToken::new();
            let mut data = String::new();
            let mut offset = 0;
            loop {
                let range = storage.read_range(
                    &artifact,
                    offset,
                    RANGE,
                    Encoding::Base64,
                    &cancel,
                )?;
                data.push_str(&range.data);
                match range.next_offset {
                    Some(next) if !range.eof => offset = next,
                    _ => return Ok(data),
                }
            }
        })
        .await?
    }
}

/// What the tools share in a run: the storage, the directory relative
/// paths resolve against, and the turn calls take.
struct Shared {
    storage: Storage,
    root: PathBuf,
    home: Option<PathBuf>,
    turn: tokio::sync::Mutex<()>,
}

impl Shared {
    /// The file `input` names: relative to the run's workspace, or from
    /// home with `~/`.
    fn resolve(&self, input: &str) -> PathBuf {
        let input = input.trim();
        let input = input.strip_prefix('@').unwrap_or(input);
        let input = input.strip_prefix("file://").unwrap_or(input);
        match (input.strip_prefix("~/"), &self.home) {
            (Some(rest), Some(home)) => home.join(rest),
            _ => self.root.join(input),
        }
    }
}

/// The canvas tools for one run.
pub struct CanvasPlugin {
    tools: Vec<Arc<dyn AgentTool>>,
}

impl CanvasPlugin {
    /// The tools, keeping images in `storage` and reading files relative
    /// to `root`, the run's workspace.
    pub fn new(storage: Storage, root: PathBuf) -> Self {
        let shared = Arc::new(Shared {
            storage,
            root,
            home: std::env::var_os("HOME").map(PathBuf::from),
            turn: tokio::sync::Mutex::new(()),
        });
        Self {
            tools: vec![
                Arc::new(typed(Show(shared.clone()))),
                Arc::new(typed(Write(shared.clone()))),
                Arc::new(typed(EditTool(shared))),
                Arc::new(typed(Read)),
            ],
        }
    }
}

#[async_trait]
impl Plugin for CanvasPlugin {
    fn name(&self) -> &str {
        NAME
    }

    fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        self.tools.clone()
    }
}

/// The run's plugin context, which the canvas lives in.
fn plugin(ctx: &ToolCtx) -> Result<&PluginCtx, ToolError> {
    ctx.plugin()
        .ok_or_else(|| ToolError::from("The canvas works only in a run."))
}

/// The run's canvas as stored now.
async fn canvas(plugin: &PluginCtx) -> Result<Canvas, ToolError> {
    let records = plugin.records().await.map_err(ToolError::other)?;
    Ok(Canvas::from_records(&records))
}

/// The item `id` must be there, and be of `kind`.
fn existing(canvas: &Canvas, id: u32, kind: Kind) -> Result<(), ToolError> {
    match canvas.item(id) {
        None => Err(format!("There is no item {id} on the canvas.").into()),
        Some(item) if item.kind() != kind => Err(format!(
            "Item {id} is {} {}, not {} {}.",
            article(item.kind()),
            item.kind().label(),
            article(kind),
            kind.label()
        )
        .into()),
        Some(_) => Ok(()),
    }
}

fn article(kind: Kind) -> &'static str {
    match kind {
        Kind::Image => "an",
        Kind::Document => "a",
    }
}

/// Publishes the revision `content` of `item`, and says what the call
/// did, with `details` for its card.
async fn revise(
    plugin: &PluginCtx,
    ctx: &ToolCtx,
    item: u32,
    revision: usize,
    title: String,
    content: Content,
    text: String,
) -> Result<ToolOutput, ToolError> {
    let kind = content.kind();
    plugin
        .try_publish(&Record::Revised {
            call: ctx.call_id().to_owned(),
            item,
            title,
            content,
        })
        .await
        .map_err(ToolError::other)?;
    let mut output = ToolOutput::text(text);
    output.details = Some(json!({
        "item": item,
        "revision": revision,
        "kind": kind,
    }));
    Ok(output)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowArgs {
    /// The image file: relative to the workspace, absolute, or from
    /// home with `~/`.
    pub path: String,
    /// A short title for the image.
    pub title: String,
    /// A line under the image saying what it shows.
    #[serde(default)]
    pub caption: Option<String>,
    /// The image on the canvas to replace with a new version, by its
    /// number. Leave it out to add a new image.
    #[serde(default)]
    pub item: Option<u32>,
}

struct Show(Arc<Shared>);

#[async_trait]
impl TypedTool for Show {
    type Args = ShowArgs;
    const NAME: &'static str = SHOW;
    const DESCRIPTION: &'static str = "Shows the person an image file on \
        the canvas, a pane beside the conversation. Use it for a picture \
        they should look at: a chart you plotted, a screenshot, a diagram \
        you rendered. The file must be PNG, JPEG, GIF, WebP, BMP or SVG, \
        at most 8 MiB. The canvas keeps a copy, so the file may change \
        afterwards. Give `item` to replace an image on the canvas with a \
        new version; leave it out to add one.";

    async fn call(
        &self,
        args: ShowArgs,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let plugin = plugin(&ctx)?;
        let path = self.0.resolve(&args.path);
        let size = tokio::fs::metadata(&path)
            .await
            .map_err(|error| format!("Cannot read {}: {error}", args.path))?
            .len();
        if size > MAX_IMAGE_BYTES {
            return Err(format!(
                "{} is {size} bytes; the canvas takes images of at most \
                 {MAX_IMAGE_BYTES} bytes.",
                args.path
            )
            .into());
        }
        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|error| format!("Cannot read {}: {error}", args.path))?;
        let Some(mime) = detect(&bytes) else {
            return Err(format!(
                "{} is not a PNG, JPEG, GIF, WebP, BMP or SVG image.",
                args.path
            )
            .into());
        };
        let (width, height) = dimensions(&bytes, mime).unzip();
        let _turn = self.0.turn.lock().await;
        let canvas = canvas(plugin).await?;
        if let Some(item) = args.item {
            existing(&canvas, item, Kind::Image)?;
        }
        let blob =
            self.0
                .storage
                .put(bytes, &ctx.cancel)
                .await
                .map_err(|error| {
                    format!("The canvas could not keep it: {error}")
                })?;
        let image = ImageRef {
            blob,
            mime: mime.to_owned(),
            width,
            height,
            path: args.path,
        };
        let item = args.item.unwrap_or_else(|| canvas.next_id());
        let revision =
            canvas.item(item).map_or(1, |item| item.revisions.len() + 1);
        let text = match revision {
            1 => format!(
                "Image {item}, \"{}\", is on the canvas ({}).",
                args.title,
                image.describe()
            ),
            n => format!(
                "Image {item}, \"{}\", shows version {n} ({}).",
                args.title,
                image.describe()
            ),
        };
        revise(
            plugin,
            &ctx,
            item,
            revision,
            args.title,
            Content::Image {
                image,
                caption: args.caption.unwrap_or_default(),
            },
            text,
        )
        .await
    }
}

/// The image type `bytes` hold, by their first bytes and never the
/// file's extension: one the canvas draws.
fn detect(bytes: &[u8]) -> Option<&'static str> {
    use image::ImageFormat;
    match image::guess_format(bytes) {
        Ok(ImageFormat::Png) => return Some("image/png"),
        Ok(ImageFormat::Jpeg) => return Some("image/jpeg"),
        Ok(ImageFormat::Gif) => return Some("image/gif"),
        Ok(ImageFormat::WebP) => return Some("image/webp"),
        Ok(ImageFormat::Bmp) => return Some("image/bmp"),
        _ => {}
    }
    // An SVG is text: an `<svg` element near the start, maybe after an
    // XML declaration, a doctype or comments.
    let head = &bytes[..bytes.len().min(4096)];
    let head = String::from_utf8_lossy(head);
    let head = head.trim_start_matches('\u{feff}').trim_start();
    (head.starts_with('<') && head.contains("<svg")).then_some("image/svg+xml")
}

/// The image's width and height in pixels, read from its header; none
/// for an SVG.
fn dimensions(bytes: &[u8], mime: &str) -> Option<(u32, u32)> {
    if mime == "image/svg+xml" {
        return None;
    }
    image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteArgs {
    /// The document's title.
    pub title: String,
    /// The whole document, in Markdown.
    pub text: String,
    /// The document on the canvas whose text this replaces, as a new
    /// revision, by its number. Leave it out to start a new document.
    #[serde(default)]
    pub item: Option<u32>,
}

struct Write(Arc<Shared>);

#[async_trait]
impl TypedTool for Write {
    type Args = WriteArgs;
    const NAME: &'static str = WRITE;
    const DESCRIPTION: &'static str = "Writes a Markdown document on the \
        canvas, a pane beside the conversation, for the person to read: a \
        plan, a report, a draft, anything longer than a reply that they \
        will want to keep open while you talk. Give `item` to replace a \
        document's whole text with a new revision; leave it out to start \
        a new document. To change part of a document, use canvas_edit.";

    async fn call(
        &self,
        args: WriteArgs,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let plugin = plugin(&ctx)?;
        check_size(&args.text)?;
        let _turn = self.0.turn.lock().await;
        let canvas = canvas(plugin).await?;
        if let Some(item) = args.item {
            existing(&canvas, item, Kind::Document)?;
        }
        let item = args.item.unwrap_or_else(|| canvas.next_id());
        let revision =
            canvas.item(item).map_or(1, |item| item.revisions.len() + 1);
        let text = match revision {
            1 => format!(
                "Document {item}, \"{}\", is on the canvas.",
                args.title
            ),
            n => format!(
                "Document {item}, \"{}\", is at revision {n}.",
                args.title
            ),
        };
        revise(
            plugin,
            &ctx,
            item,
            revision,
            args.title,
            Content::Document { text: args.text },
            text,
        )
        .await
    }
}

/// A document's text must fit [`MAX_DOCUMENT_BYTES`].
fn check_size(text: &str) -> Result<(), ToolError> {
    if text.len() > MAX_DOCUMENT_BYTES {
        return Err(format!(
            "The document is {} bytes; the canvas takes documents of at \
             most {MAX_DOCUMENT_BYTES} bytes.",
            text.len()
        )
        .into());
    }
    Ok(())
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditArgs {
    /// The document to revise, by its number.
    pub item: u32,
    /// Replacements made in turn.
    pub edits: Vec<Edit>,
    /// A new title, if it changes.
    #[serde(default)]
    pub title: Option<String>,
}

struct EditTool(Arc<Shared>);

#[async_trait]
impl TypedTool for EditTool {
    type Args = EditArgs;
    const NAME: &'static str = EDIT;
    const DESCRIPTION: &'static str = "Revises a document on the canvas by \
        exact replacements: each `old` must occur exactly once in the text \
        as the edits before it left it, and becomes `new`. The result is a \
        new revision; the person can step back through earlier ones.";

    async fn call(
        &self,
        args: EditArgs,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let plugin = plugin(&ctx)?;
        let _turn = self.0.turn.lock().await;
        let canvas = canvas(plugin).await?;
        existing(&canvas, args.item, Kind::Document)?;
        let item = canvas.item(args.item).expect("checked");
        let Content::Document { text } = &item.latest().content else {
            unreachable!("checked to be a document");
        };
        let text = apply_edits(text, &args.edits)?;
        check_size(&text)?;
        let title = args.title.unwrap_or_else(|| item.title.clone());
        let revision = item.revisions.len() + 1;
        let said = format!(
            "Document {}, \"{title}\", is at revision {revision}.",
            args.item
        );
        revise(
            plugin,
            &ctx,
            args.item,
            revision,
            title,
            Content::Document { text },
            said,
        )
        .await
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadArgs {
    /// The document to read, by its number.
    pub item: u32,
    /// The revision to read, from 1. Leave it out for the latest.
    #[serde(default)]
    pub revision: Option<usize>,
}

struct Read;

#[async_trait]
impl TypedTool for Read {
    type Args = ReadArgs;
    const NAME: &'static str = READ;
    const DESCRIPTION: &'static str = "Reads a document's text from the \
        canvas: its latest revision, or `revision`, numbered from 1. An \
        image reads as its title, caption and file.";

    async fn call(
        &self,
        args: ReadArgs,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let plugin = plugin(&ctx)?;
        let canvas = canvas(plugin).await?;
        let Some(item) = canvas.item(args.item) else {
            return Err(format!(
                "There is no item {} on the canvas.",
                args.item
            )
            .into());
        };
        let count = item.revisions.len();
        let revision = args.revision.unwrap_or(count);
        let Some(found) = revision
            .checked_sub(1)
            .and_then(|at| item.revisions.get(at))
        else {
            return Err(format!(
                "Item {} has revisions 1 to {count}.",
                args.item
            )
            .into());
        };
        let text = match &found.content {
            Content::Document { text } => text.clone(),
            Content::Image { image, caption } => format!(
                "Image {}, \"{}\": {} from {}{}",
                args.item,
                item.title,
                format_name(&image.mime),
                image.path,
                if caption.is_empty() {
                    String::new()
                } else {
                    format!(". {caption}")
                }
            ),
        };
        let mut output = ToolOutput::text(text);
        output.details = Some(json!({
            "item": args.item,
            "revision": revision,
            "kind": item.kind(),
        }));
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_images_by_their_bytes() {
        let png = {
            let mut bytes = Vec::new();
            image::RgbImage::new(3, 2)
                .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
                .unwrap();
            bytes
        };
        assert_eq!(detect(&png), Some("image/png"));
        assert_eq!(dimensions(&png, "image/png"), Some((3, 2)));
        let svg = b"<?xml version=\"1.0\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\"/>";
        assert_eq!(detect(svg), Some("image/svg+xml"));
        assert_eq!(dimensions(svg, "image/svg+xml"), None);
        assert_eq!(detect(b"<html><body>no</body></html>"), None);
        assert_eq!(detect(b"plain text"), None);
    }
}
