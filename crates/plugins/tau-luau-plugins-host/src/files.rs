//! What a Luau plugin's tools read and keep (ADR 0050, gap G): the files
//! of the folders its `uses.files` names, read-only, a path relative to
//! the run's workspace, `~/` from home, or absolute; and blobs, the
//! bytes of files it keeps in the run's repository for `ui.image`, which
//! never pass through the VM.
//!
//! A path is read where it really is: through a link or `..`, it must
//! still land in one of the folders.
//!
//! The folders it names `{ name, write = true }` it writes too (ADR 0050,
//! gap L): a file whole or not at all, a folder made, a file removed or
//! moved, each where it really is, in one of those.

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
    /// The declared folders it writes too.
    writes: Vec<String>,
    /// tau's directory for the run's repository (`uses.files`'
    /// `repository`).
    repository: Option<PathBuf>,
    /// tau's directory for each repository, by name, for hooks outside
    /// their runs (`ctx.files.repository`).
    repositories: Vec<(String, PathBuf)>,
    /// The repositories tau lists, `{ id, name, folder, remote,
    /// normalized }` each, for a plugin whose `uses.files` names
    /// `repository` (`ctx.files.repositories`, ADR 0050, gap L).
    listed: Vec<Value>,
    workspace: Option<PathBuf>,
    home: Option<PathBuf>,
    blobs: Option<PathBuf>,
    /// The plugin's own package, for a plugin tau comes with.
    package: Option<PathBuf>,
    /// tau's data folder.
    data: Option<PathBuf>,
    /// The person's skills folder, as the host has it.
    skills: Option<PathBuf>,
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
            writes: Vec::new(),
            repository: None,
            repositories: Vec::new(),
            listed: Vec::new(),
            workspace,
            home: std::env::var_os("HOME").map(PathBuf::from),
            blobs,
            package: None,
            data: Some(crate::packages::data_dir()),
            skills: None,
        }
    }

    /// With the person's skills folder at `dir` (`uses.files`' `skills`).
    pub fn with_skills(mut self, dir: Option<PathBuf>) -> Self {
        self.skills = dir;
        self
    }

    /// With the plugin's own package at `dir` (`uses.files`' `package`).
    pub fn with_package(mut self, dir: Option<PathBuf>) -> Self {
        self.package = dir;
        self
    }

    /// Writing the declared folders `writes` too (`uses.writes`).
    pub fn writing(mut self, writes: &[String]) -> Self {
        self.writes = writes.to_vec();
        self
    }

    /// With tau's directory for the run's repository at `dir`
    /// (`uses.files`' `repository`).
    pub fn with_repository(mut self, dir: Option<PathBuf>) -> Self {
        self.repository = dir;
        self
    }

    /// With tau's directory for each repository, by name
    /// (`ctx.files.repository`).
    pub fn with_repositories(mut self, dirs: Vec<(String, PathBuf)>) -> Self {
        self.repositories = dirs;
        self
    }

    /// With the repositories tau lists (`ctx.files.repositories`): their
    /// folders and remotes, as the host has them.
    pub fn with_listed(mut self, repos: &[tau_ui_plugin::RepoCtx]) -> Self {
        self.listed = repos
            .iter()
            .map(|repo| {
                json!({
                    "id": repo.id.0.to_string(),
                    "name": repo.name,
                    "folder": repo.folder.as_ref().map(|f| f.display().to_string()),
                    "remote": repo.remote,
                    "normalized": repo.remote.as_deref().and_then(crate::remote::normalize),
                })
            })
            .collect();
        self
    }

    /// With tau's data folder at `dir` (`uses.files`' `data`).
    pub fn with_data(mut self, dir: PathBuf) -> Self {
        self.data = Some(dir);
        self
    }

    /// The calls a script makes for `ctx.files` and `ctx.blobs`.
    pub fn entries() -> Vec<ToolEntry> {
        [
            ("files_read", "A file's text, or bytes in base64."),
            ("files_list", "What a folder holds."),
            ("files_image", "A file's size, and the image it is."),
            (
                "files_frontmatter",
                "A Markdown file's YAML frontmatter, and the text after it.",
            ),
            ("files_roots", "Where the folders `uses.files` names are."),
            ("files_write", "Writes a file whole, in a folder it writes."),
            ("files_remove", "Removes a file or an empty folder."),
            ("files_rename", "Moves a file in place of another."),
            ("files_mkdir", "Makes a folder and those it is in."),
            ("files_remote", "The Git remote of a folder's repository."),
            (
                "files_repository",
                "tau's folder for a repository, by its name.",
            ),
            ("files_repositories", "The repositories tau lists."),
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
            "files_read"
                | "files_list"
                | "files_image"
                | "files_frontmatter"
                | "files_roots"
                | "files_write"
                | "files_remove"
                | "files_rename"
                | "files_mkdir"
                | "files_remote"
                | "files_repository"
                | "files_repositories"
                | "blobs_put"
        )
    }

    /// Carries out the call `name` with `args`.
    pub async fn call(
        &self,
        name: &str,
        args: &Value,
    ) -> Result<ToolReply, String> {
        if name == "files_roots" {
            return Ok(ToolReply::success(self.named_roots()));
        }
        if name == "files_repositories" {
            // Where each is and what it pushes to: a plugin that keeps
            // what it knows per repository reads them.
            let keeps = self.declared.iter().any(|folder| {
                folder == tau_luau_plugins::REPOSITORY
                    || folder.starts_with(&format!(
                        "{}/",
                        tau_luau_plugins::REPOSITORY
                    ))
            });
            if !keeps {
                return Err("`ctx.files.repositories` needs `repository` among uses.files".into());
            }
            return Ok(ToolReply::success(json!(self.listed)));
        }
        if name == "files_repository" {
            let wanted = args["name"].as_str().unwrap_or_default();
            return Ok(ToolReply::success(
                self.repositories
                    .iter()
                    .find(|(name, _)| name == wanted)
                    .map_or(Value::Null, |(_, dir)| {
                        json!(dir.display().to_string())
                    }),
            ));
        }
        let input = args["path"]
            .as_str()
            .ok_or_else(|| format!("`{name}` needs its `path`, a string"))?;
        match name {
            "files_write" => {
                let text = args["text"]
                    .as_str()
                    .ok_or("`files_write` needs its `text`, a string")?;
                let path = self.writable(input).await?;
                write(input, &path, text).await?;
                return Ok(ToolReply::success(Value::Null));
            }
            "files_remove" => {
                let path = self.writable(input).await?;
                remove(input, &path).await?;
                return Ok(ToolReply::success(Value::Null));
            }
            "files_rename" => {
                let to = args["to"]
                    .as_str()
                    .ok_or("`files_rename` needs `to`, a string")?;
                let from = self.writable(input).await?;
                let target = self.writable(to).await?;
                rename(input, &from, &target).await?;
                return Ok(ToolReply::success(Value::Null));
            }
            "files_mkdir" => {
                let path = self.writable(input).await?;
                tokio::fs::create_dir_all(&path)
                    .await
                    .map_err(|error| format!("Cannot make {input}: {error}"))?;
                return Ok(ToolReply::success(Value::Null));
            }
            _ => {}
        }
        let path = self.resolve(input).await?;
        if name == "files_remote" {
            let remote = crate::remote::of_folder(&path).map(|url| {
                json!({
                    "normalized": crate::remote::normalize(&url),
                    "url": url,
                })
            });
            return Ok(ToolReply::success(remote.unwrap_or(Value::Null)));
        }
        let value = match name {
            "files_read" => read(input, &path, args).await?,
            "files_list" => list(input, &path).await?,
            "files_image" => image(input, &path).await?,
            "files_frontmatter" => frontmatter(input, &path).await?,
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
        let path = self.path_of(input)?;
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

    /// Where the path `input` names really is, made or not, when one of
    /// the folders it writes holds it and it is not that folder itself.
    async fn writable(&self, input: &str) -> Result<PathBuf, String> {
        let path = self.path_of(input)?;
        let real = real_path(&path)
            .await
            .map_err(|error| format!("Cannot write {input}: {error}"))?;
        for root in self.write_roots() {
            if let Ok(root) = real_path(&root).await
                && real.starts_with(&root)
            {
                if real == root {
                    return Err(format!(
                        "Cannot write {input}: it is a folder `uses.files` \
                         names, not one in it"
                    ));
                }
                return Ok(real);
            }
        }
        Err(format!(
            "This plugin does not write {input}: its folder is not one \
             `uses.files` names {{ name, write = true }}"
        ))
    }

    /// The folders it writes, as paths.
    fn write_roots(&self) -> Vec<PathBuf> {
        self.writes
            .iter()
            .filter(|folder| self.declared.contains(folder))
            .flat_map(|folder| self.roots_of(folder))
            .collect()
    }

    /// The path `input` names: relative to the workspace, `~/` from home,
    /// or absolute.
    fn path_of(&self, input: &str) -> Result<PathBuf, String> {
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
        Ok(path)
    }

    /// The folders `uses.files` names, as paths.
    fn roots(&self) -> Vec<PathBuf> {
        self.declared
            .iter()
            .flat_map(|folder| self.roots_of(folder))
            .collect()
    }

    /// Where the folder `folder` of `uses.files` is: [`Self::root`], or,
    /// for a `repository` folder in a hook outside any repository, that
    /// folder of each repository tau has (ADR 0050, gap L).
    fn roots_of(&self, folder: &str) -> Vec<PathBuf> {
        if let Some(root) = self.root(folder) {
            return vec![root];
        }
        let rest = if folder == tau_luau_plugins::REPOSITORY {
            Some("")
        } else {
            folder
                .strip_prefix(tau_luau_plugins::REPOSITORY)
                .and_then(|rest| rest.strip_prefix('/'))
        };
        match (rest, &self.repository) {
            (Some(rest), None) => self
                .repositories
                .iter()
                .map(|(_, dir)| {
                    if rest.is_empty() {
                        dir.clone()
                    } else {
                        dir.join(rest)
                    }
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Where the folder `folder` of `uses.files` is, if it is anywhere.
    fn root(&self, folder: &str) -> Option<PathBuf> {
        let under = |base: &Option<PathBuf>, token: &str| {
            let base = base.as_ref()?;
            if folder == token {
                return Some(base.clone());
            }
            let rest = folder.strip_prefix(token)?.strip_prefix('/')?;
            Some(base.join(rest))
        };
        if folder == tau_luau_plugins::WORKSPACE {
            return self.workspace.clone();
        }
        if folder == "~" {
            return self.home.clone();
        }
        if let Some(rest) = folder.strip_prefix("~/") {
            return self.home.as_ref().map(|home| home.join(rest));
        }
        if let Some(path) = under(&self.package, tau_luau_plugins::PACKAGE)
            .or_else(|| under(&self.data, tau_luau_plugins::DATA))
            .or_else(|| under(&self.skills, tau_luau_plugins::SKILLS))
            .or_else(|| under(&self.repository, tau_luau_plugins::REPOSITORY))
        {
            return Some(path);
        }
        Path::new(folder)
            .is_absolute()
            .then(|| PathBuf::from(folder))
    }

    /// Where each folder `uses.files` names is, of those that are
    /// somewhere.
    pub fn root_paths(&self) -> Vec<String> {
        self.roots()
            .into_iter()
            .map(|path| path.display().to_string())
            .collect()
    }

    /// Each folder `uses.files` names that is somewhere, to its path.
    fn named_roots(&self) -> Value {
        let mut named = serde_json::Map::new();
        for folder in &self.declared {
            if let Some(path) = self.root(folder) {
                named.insert(folder.clone(), json!(path.display().to_string()));
            }
        }
        Value::Object(named)
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

/// Where `path` really is, made or not: its nearest ancestor that is
/// there, through its links, then the rest, which may not climb out with
/// `..`.
async fn real_path(path: &Path) -> std::io::Result<PathBuf> {
    use std::path::Component;
    let mut rest = Vec::new();
    let mut there = path;
    loop {
        match tokio::fs::canonicalize(there).await {
            Ok(real) => {
                let mut real = real;
                for part in rest.iter().rev() {
                    real.push(part);
                }
                return Ok(real);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = match there.components().next_back() {
                    Some(Component::Normal(name)) => name.to_owned(),
                    _ => {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "a path not made yet cannot go through `..`",
                        ));
                    }
                };
                rest.push(name);
                there = there.parent().ok_or(error)?;
            }
            Err(error) => return Err(error),
        }
    }
}

/// Writes `text` to `path` whole: to a file beside it, then in its
/// place, the folders it is in made first.
async fn write(input: &str, path: &Path, text: &str) -> Result<(), String> {
    if text.len() as u64 > MAX_BYTES {
        return Err(format!(
            "Cannot write {input}: {} bytes; a file is at most {MAX_BYTES}",
            text.len()
        ));
    }
    let failed =
        |error: std::io::Error| format!("Cannot write {input}: {error}");
    if tokio::fs::metadata(path)
        .await
        .is_ok_and(|meta| meta.is_dir())
    {
        return Err(format!("Cannot write {input}: it is a folder"));
    }
    let parent = path.parent().ok_or("Cannot write the root")?;
    tokio::fs::create_dir_all(parent).await.map_err(failed)?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    static WRITES: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0);
    let temporary = parent.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    tokio::fs::write(&temporary, text).await.map_err(failed)?;
    if let Err(error) = tokio::fs::rename(&temporary, path).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(failed(error));
    }
    Ok(())
}

/// Removes the file, or the empty folder, at `path`.
async fn remove(input: &str, path: &Path) -> Result<(), String> {
    let failed =
        |error: std::io::Error| format!("Cannot remove {input}: {error}");
    let meta = tokio::fs::symlink_metadata(path).await.map_err(failed)?;
    if meta.is_dir() {
        tokio::fs::remove_dir(path).await.map_err(failed)
    } else {
        tokio::fs::remove_file(path).await.map_err(failed)
    }
}

/// Moves the file or folder at `from` to `to`, in place of a file there,
/// the folders `to` is in made first.
async fn rename(input: &str, from: &Path, to: &Path) -> Result<(), String> {
    let failed =
        |error: std::io::Error| format!("Cannot move {input}: {error}");
    tokio::fs::symlink_metadata(from).await.map_err(failed)?;
    if let Some(parent) = to.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(failed)?;
    }
    tokio::fs::rename(from, to).await.map_err(failed)
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

/// The file's text, or with `bytes` its bytes in base64, from `offset`
/// bytes in (none past its end); `limit` bytes at most, else the rest of
/// it up to [`MAX_BYTES`]. With `lossy`, bytes that are not UTF-8 read as
/// U+FFFD rather than failing.
async fn read(input: &str, path: &Path, args: &Value) -> Result<Value, String> {
    use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _};
    let size = size(input, path).await?;
    let offset = args["offset"].as_u64().unwrap_or(0).min(size);
    let limit = args["limit"].as_u64();
    if limit.is_none() && size - offset > MAX_BYTES {
        return Err(format!(
            "{input} is {size} bytes; a read takes at most {MAX_BYTES}: give a `limit`"
        ));
    }
    let cannot =
        |error: std::io::Error| format!("Cannot read {input}: {error}");
    let mut file = tokio::fs::File::open(path).await.map_err(cannot)?;
    file.seek(std::io::SeekFrom::Start(offset))
        .await
        .map_err(cannot)?;
    let mut bytes = Vec::new();
    file.take(limit.unwrap_or(MAX_BYTES).min(MAX_BYTES))
        .read_to_end(&mut bytes)
        .await
        .map_err(cannot)?;
    if args["bytes"].as_bool() == Some(true) {
        return Ok(json!(
            base64::engine::general_purpose::STANDARD.encode(bytes)
        ));
    }
    if args["lossy"].as_bool() == Some(true) {
        return Ok(json!(String::from_utf8_lossy(&bytes)));
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
        // Through a link, as a folder linked in is one.
        let meta = tokio::fs::metadata(entry.path()).await.ok();
        // When it was last written, in milliseconds since 1970.
        let modified = meta
            .as_ref()
            .and_then(|meta| meta.modified().ok())
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |since| since.as_millis() as u64);
        found.push(json!({
            "name": entry.file_name().to_string_lossy(),
            "dir": meta.as_ref().is_some_and(std::fs::Metadata::is_dir),
            "size": meta.map_or(0, |meta| meta.len()),
            "modified": modified,
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

/// The Markdown file at `path`'s YAML frontmatter, between `---` lines at
/// its start, and the text after it: `{ keys, body }`, each key with its
/// value's `kind` (`string`, `integer`, `float`, `boolean`, `null` or
/// `other`) and its `text` as written; or `{ error }` when it has none,
/// or it is not a mapping of YAML.
async fn frontmatter(input: &str, path: &Path) -> Result<Value, String> {
    let text = tokio::fs::read_to_string(path)
        .await
        .map_err(|error| format!("Cannot read {input}: {error}"))?;
    let file = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| input.to_owned());
    Ok(match split_front(&text, &file) {
        Err(error) => json!({ "error": error }),
        Ok((front, body)) => match front_keys(front) {
            Err(error) => json!({ "error": error, "body": body }),
            Ok(keys) => json!({ "keys": keys, "body": body }),
        },
    })
}

/// `text` split into its frontmatter and what follows it.
fn split_front<'a>(
    text: &'a str,
    file: &str,
) -> Result<(&'a str, &'a str), String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let rest = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
        .ok_or_else(|| {
            format!("{file} does not start with frontmatter (---)")
        })?;
    let mut at = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return Ok((&rest[..at], &rest[at + line.len()..]));
        }
        at += line.len();
    }
    if rest[at..].trim_end() == "---" {
        return Ok((&rest[..at], ""));
    }
    Err(format!("{file}'s frontmatter has no closing ---"))
}

/// The keys of the frontmatter `front`, in order.
fn front_keys(front: &str) -> Result<Vec<Value>, String> {
    use saphyr::{LoadableYamlNode, Scalar, Yaml};
    let not_yaml = |error: &dyn std::fmt::Display| {
        format!("its frontmatter is not YAML: {error}")
    };
    let mut documents =
        Yaml::load_from_str(front).map_err(|error| not_yaml(&error))?;
    if documents.len() > 1 {
        return Err(not_yaml(&"it holds more than one document"));
    }
    let map = match documents.pop() {
        None | Some(Yaml::Value(Scalar::Null)) => return Ok(Vec::new()),
        Some(Yaml::Mapping(map)) => map,
        Some(_) => {
            return Err(
                "its frontmatter is not a list of keys and values".into()
            );
        }
    };
    Ok(map
        .into_iter()
        .filter_map(|(key, value)| {
            let Yaml::Value(Scalar::String(key)) = key else {
                return None;
            };
            let (kind, text) = match &value {
                Yaml::Value(Scalar::Null) => ("null", Value::Null),
                Yaml::Value(Scalar::String(text)) => {
                    ("string", json!(text.to_string()))
                }
                Yaml::Value(Scalar::Integer(number)) => {
                    ("integer", json!(number.to_string()))
                }
                // `1.0` stays `1.0`, which `Display` would make `1`.
                Yaml::Value(Scalar::FloatingPoint(number)) => {
                    ("float", json!(format!("{:?}", number.0)))
                }
                Yaml::Value(Scalar::Boolean(flag)) => {
                    ("boolean", json!(flag.to_string()))
                }
                _ => ("other", Value::Null),
            };
            Some(json!({ "key": key.to_string(), "kind": kind, "text": text }))
        })
        .collect())
}
