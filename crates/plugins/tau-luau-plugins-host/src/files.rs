//! What a Luau plugin's tools read and keep (ADR 0050, gap G): the files
//! of the folders its `uses.files` names, read-only, a path relative to
//! the run's workspace, `~/` from home, or absolute; and blobs, the
//! bytes of files it keeps in the run's repository for `ui.image`, which
//! never pass through the VM.
//!
//! A path is read where it really is: through a link or `..`, it must
//! still land in one of the folders.

use std::{
    io::Cursor,
    path::{Path, PathBuf},
};

use base64::Engine as _;
use serde_json::{Value, json};
use tau_codemode_host::{ToolEntry, ToolReply};
use tokio_util::sync::CancellationToken;

/// The most a file read, or a blob, takes: 8 MiB, so a blob's base64
/// reaches a phone in one message.
pub const MAX_BYTES: u64 = 8 * 1024 * 1024;

/// The most a plugin's blobs in a repository take in all: 1 GiB.
const TOTAL_BYTES: u64 = 1024 * 1024 * 1024;

/// One read of a blob: the most a range gives that is a multiple of 3,
/// so its base64 joins the next range's whole.
const RANGE: u32 = 65_535;

/// What a plugin's tools reach of the files: its folders, and its blobs
/// in the run's repository.
pub struct Files {
    declared: Vec<String>,
    workspace: Option<PathBuf>,
    home: Option<PathBuf>,
    blobs: Option<PathBuf>,
}

impl Files {
    /// The folders `declared` names (`uses.files`), the run's workspace
    /// at `workspace`, and the plugin's blobs kept in `blobs`.
    pub fn new(
        declared: &[String],
        workspace: Option<PathBuf>,
        blobs: Option<PathBuf>,
    ) -> Self {
        Self {
            declared: declared.to_vec(),
            workspace,
            home: std::env::var_os("HOME").map(PathBuf::from),
            blobs,
        }
    }

    /// The calls a script makes for `ctx.files` and `ctx.blobs`.
    pub fn entries() -> Vec<ToolEntry> {
        [
            ("files_read", "A file's text, or bytes in base64."),
            ("files_list", "What a folder holds."),
            ("files_image", "A file's size, and the image it is."),
            ("blobs_put", "Keeps a file's bytes as a blob."),
        ]
        .into_iter()
        .map(|(name, description)| ToolEntry {
            name: name.into(),
            description: description.into(),
            input_schema: json!({ "type": "object" }),
            output_schema: None,
            namespace: None,
            sequential: false,
        })
        .collect()
    }

    /// Whether `name` is one of [`Self::entries`].
    pub fn handles(name: &str) -> bool {
        matches!(
            name,
            "files_read" | "files_list" | "files_image" | "blobs_put"
        )
    }

    /// Carries out the call `name` with `args`.
    pub async fn call(
        &self,
        name: &str,
        args: &Value,
    ) -> Result<ToolReply, String> {
        let input = args["path"]
            .as_str()
            .ok_or_else(|| format!("`{name}` needs its `path`, a string"))?;
        let path = self.resolve(input).await?;
        let value = match name {
            "files_read" => read(input, &path, args).await?,
            "files_list" => list(input, &path).await?,
            "files_image" => image(input, &path).await?,
            "blobs_put" => self.put(input, &path).await?,
            other => {
                return Err(format!("`{other}` is not a call of ctx.files"));
            }
        };
        Ok(ToolReply::success(value))
    }

    /// Where the path `input` names really is, when one of the declared
    /// folders holds it.
    async fn resolve(&self, input: &str) -> Result<PathBuf, String> {
        let given = input.trim();
        let given = given.strip_prefix('@').unwrap_or(given);
        let given = given.strip_prefix("file://").unwrap_or(given);
        let path = match (given.strip_prefix("~/"), given == "~", &self.home) {
            (Some(rest), _, Some(home)) => home.join(rest),
            (_, true, Some(home)) => home.clone(),
            _ if Path::new(given).is_absolute() => PathBuf::from(given),
            _ => match &self.workspace {
                Some(workspace) => workspace.join(given),
                None => {
                    return Err(format!(
                        "Cannot read {input}: this hook has no workspace"
                    ));
                }
            },
        };
        let real = tokio::fs::canonicalize(&path)
            .await
            .map_err(|error| format!("Cannot read {input}: {error}"))?;
        for root in self.roots() {
            if let Ok(root) = tokio::fs::canonicalize(&root).await
                && real.starts_with(&root)
            {
                return Ok(real);
            }
        }
        Err(format!(
            "This plugin does not read {input}: its folder is not in `uses.files`"
        ))
    }

    /// The folders `uses.files` names, as paths.
    fn roots(&self) -> Vec<PathBuf> {
        self.declared
            .iter()
            .filter_map(|folder| {
                if folder == tau_luau_plugins::WORKSPACE {
                    return self.workspace.clone();
                }
                if folder == "~" {
                    return self.home.clone();
                }
                if let Some(rest) = folder.strip_prefix("~/") {
                    return self.home.as_ref().map(|home| home.join(rest));
                }
                Path::new(folder)
                    .is_absolute()
                    .then(|| PathBuf::from(folder))
            })
            .collect()
    }

    /// Keeps the file at `path` as a blob.
    async fn put(&self, input: &str, path: &Path) -> Result<Value, String> {
        let dir = self
            .blobs
            .clone()
            .ok_or("this hook keeps no blobs: it is in no repository")?;
        let size = size(input, path).await?;
        if size > MAX_BYTES {
            return Err(format!(
                "{input} is {size} bytes; a blob is at most {MAX_BYTES} bytes."
            ));
        }
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|error| format!("Cannot read {input}: {error}"))?;
        put_bytes(dir, bytes)
            .await
            .map_err(|error| format!("Cannot keep {input}: {error:#}"))
    }
}

/// Keeps `bytes` as a blob in `dir`, as `ctx.blobs.put_file` does:
/// `{ id, digest, size_bytes }`.
pub async fn put_bytes(dir: PathBuf, bytes: Vec<u8>) -> anyhow::Result<Value> {
    let blob = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let store = open(&dir)?;
        Ok(store
            .publish_reader(Cursor::new(bytes), &CancellationToken::new())?)
    })
    .await??;
    Ok(json!({
        "id": blob.id(),
        "digest": blob.digest(),
        "size_bytes": blob.size_bytes(),
    }))
}

/// The blobs of `plugin` in the repository `repo`, under `root`.
pub fn blobs_dir(root: &Path, plugin: &str, repo: &str) -> PathBuf {
    let repo: String = repo
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    root.join(plugin).join(repo)
}

fn open(dir: &Path) -> anyhow::Result<tau_artifacts::Bytes> {
    Ok(tau_artifacts::Bytes::new(
        dir,
        tau_artifacts::Quotas {
            max_artifact_bytes: MAX_BYTES,
            total_bytes: TOTAL_BYTES,
        },
    )?)
}

/// The bytes of the blob `blob` kept in `dir`, in base64.
pub async fn blob(dir: PathBuf, blob: &Value) -> anyhow::Result<String> {
    let artifact: tau_artifacts::Artifact =
        serde_json::from_value(blob.clone())?;
    tokio::task::spawn_blocking(move || {
        let store = open(&dir)?;
        let cancel = CancellationToken::new();
        let mut data = String::new();
        let mut offset = 0;
        loop {
            let range = store.read_range(
                &artifact,
                offset,
                RANGE,
                tau_artifacts::Encoding::Base64,
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

async fn size(input: &str, path: &Path) -> Result<u64, String> {
    Ok(tokio::fs::metadata(path)
        .await
        .map_err(|error| format!("Cannot read {input}: {error}"))?
        .len())
}

/// The file's text, or with `bytes` its bytes in base64; `limit` bytes
/// at most, else the whole of it up to [`MAX_BYTES`].
async fn read(input: &str, path: &Path, args: &Value) -> Result<Value, String> {
    let size = size(input, path).await?;
    let limit = args["limit"].as_u64();
    if limit.is_none() && size > MAX_BYTES {
        return Err(format!(
            "{input} is {size} bytes; a read takes at most {MAX_BYTES}: give a `limit`"
        ));
    }
    let mut bytes = tokio::fs::read(path)
        .await
        .map_err(|error| format!("Cannot read {input}: {error}"))?;
    if let Some(limit) = limit {
        bytes.truncate(limit.min(MAX_BYTES) as usize);
    }
    if args["bytes"].as_bool() == Some(true) {
        return Ok(json!(
            base64::engine::general_purpose::STANDARD.encode(bytes)
        ));
    }
    String::from_utf8(bytes).map(Value::String).map_err(|_| {
        format!("{input} is not text: read it with `{{ bytes = true }}`")
    })
}

/// What the folder at `path` holds, by name.
async fn list(input: &str, path: &Path) -> Result<Value, String> {
    let mut entries = tokio::fs::read_dir(path)
        .await
        .map_err(|error| format!("Cannot list {input}: {error}"))?;
    let mut found = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| format!("Cannot list {input}: {error}"))?
    {
        let meta = entry.metadata().await.ok();
        found.push(json!({
            "name": entry.file_name().to_string_lossy(),
            "dir": meta.as_ref().is_some_and(std::fs::Metadata::is_dir),
            "size": meta.map_or(0, |meta| meta.len()),
        }));
    }
    found.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Ok(Value::Array(found))
}

/// The file's size, and the image its bytes are: never by its
/// extension.
async fn image(input: &str, path: &Path) -> Result<Value, String> {
    let size = size(input, path).await?;
    let mut info = json!({ "size": size });
    if size > MAX_BYTES {
        return Ok(info);
    }
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|error| format!("Cannot read {input}: {error}"))?;
    if let Some(mime) = detect(&bytes) {
        info["mime"] = json!(mime);
        if let Some((width, height)) = dimensions(&bytes, mime) {
            info["width"] = json!(width);
            info["height"] = json!(height);
        }
    }
    Ok(info)
}

/// The image type `bytes` hold, by their first bytes: one tau draws.
pub fn detect(bytes: &[u8]) -> Option<&'static str> {
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
