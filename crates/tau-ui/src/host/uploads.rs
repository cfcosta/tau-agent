//! The images the person sends runs, kept for interfaces to draw. The
//! model reads them in the run's messages; an interface draws them from
//! here, by key ([`Picture`]), so the runs' views and what phones are
//! sent hold none of their bytes.
//!
//! Each is a file under `uploads/` beside the run store, named by its
//! key and holding its base64. One a run's messages hold but the
//! directory lost is found in them again, and kept.

use std::path::PathBuf;

use tau_ai::message::{ImageContent, InputBlock, Message, UserContent};
use tau_store::Entry;

use super::Host;
use crate::view::Picture;

impl Host {
    /// Where the person's images are kept: beside the run store.
    pub fn uploads_dir(&self) -> PathBuf {
        self.config
            .store
            .parent()
            .map_or_else(|| PathBuf::from("uploads"), |dir| dir.join("uploads"))
    }

    /// Keeps `images`, sent to a run, for interfaces to draw. A failure
    /// is said and the message goes on: the run's messages hold them.
    pub(crate) async fn keep_uploads(&self, images: &[ImageContent]) {
        if images.is_empty() {
            return;
        }
        let dir = self.uploads_dir();
        let images: Vec<(String, String)> = images
            .iter()
            .map(|image| (Picture::of(image).key, image.data.clone()))
            .collect();
        match tokio::task::spawn_blocking(move || keep(&dir, &images)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                eprintln!("tau-ui: cannot keep an image: {error}")
            }
            Err(error) => eprintln!("tau-ui: cannot keep an image: {error}"),
        }
    }

    /// The base64 of `picture`, which the person sent `run`: kept, or
    /// found again in the run's messages.
    pub async fn picture(
        &self,
        run: &tau_agent::tool::RunId,
        picture: &Picture,
    ) -> anyhow::Result<String> {
        anyhow::ensure!(Picture::is_key(&picture.key), "not an image's key");
        let path = self.uploads_dir().join(&picture.key);
        if let Ok(data) = tokio::fs::read_to_string(&path).await {
            return Ok(data);
        }
        let image = self
            .store
            .timeline(&run.0)
            .await?
            .into_iter()
            .filter_map(|entry| match entry {
                Entry::Message { body, .. } => {
                    serde_json::from_str::<Message>(&body).ok()
                }
                _ => None,
            })
            .flat_map(|message| match message {
                Message::User(user) => match user.content {
                    UserContent::Blocks(blocks) => blocks,
                    UserContent::Text(_) => Vec::new(),
                },
                _ => Vec::new(),
            })
            .find_map(|block| match block {
                InputBlock::Image(image)
                    if Picture::of(&image).key == picture.key =>
                {
                    Some(image)
                }
                _ => None,
            })
            .ok_or_else(|| anyhow::anyhow!("the run has no such image"))?;
        self.keep_uploads(std::slice::from_ref(&image)).await;
        Ok(image.data)
    }
}

/// Writes each `(key, base64)` not kept yet, whole or not at all.
fn keep(
    dir: &std::path::Path,
    images: &[(String, String)],
) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for (key, data) in images {
        let path = dir.join(key);
        if path.exists() {
            continue;
        }
        let staged = dir.join(format!(".{key}.part"));
        std::fs::write(&staged, data)?;
        std::fs::rename(&staged, &path)?;
    }
    Ok(())
}
