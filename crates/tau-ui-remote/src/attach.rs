//! What goes with the next message besides its text: text files the
//! person picks with the composer's paperclip, whose content goes in the
//! message as the model sees it, and images, picked, pasted or dropped
//! on the composer, which go to the model as images.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use gpui::{Context, PathPromptOptions};
use tau_ai::message::ImageContent;

use crate::workspace::Workspace;

/// The largest text file that can be attached.
pub const MAX_ATTACHMENT: usize = 200_000;

/// The largest image that can be attached: 8 MiB, so a phone's message
/// carrying it stays within one WebSocket message.
pub const MAX_IMAGE: usize = 8 * 1024 * 1024;

/// Something attached to the next message.
#[derive(Debug, Clone)]
pub struct Attachment {
    /// What the chip and the message call it: the file's name.
    pub name: String,
    /// The file it came from; none for a pasted image.
    pub path: Option<PathBuf>,
    pub content: Attached,
}

/// What an attachment holds.
#[derive(Debug, Clone)]
pub enum Attached {
    /// A text file's content.
    Text(String),
    /// An image, as the model gets it, and as the chip draws it.
    Image {
        image: ImageContent,
        shown: Arc<gpui::Image>,
    },
}

/// The type of image `bytes` hold, by their first bytes: one the model
/// takes (PNG, JPEG, GIF, WebP).
pub fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    const PNG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if bytes.starts_with(&PNG) {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else {
        None
    }
}

/// The format gpui draws an image of type `mime` in.
pub fn format_of(mime: &str) -> Option<gpui::ImageFormat> {
    use gpui::ImageFormat;
    Some(match mime {
        "image/png" => ImageFormat::Png,
        "image/jpeg" => ImageFormat::Jpeg,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::Webp,
        _ => return None,
    })
}

impl Workspace {
    /// Asks for files to attach, with the system's file picker.
    pub fn pick_attachments(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        cx.spawn(async move |ws, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let _ = ws.update(cx, |ws, cx| {
                for path in paths {
                    ws.attach_path(&path, cx);
                }
            });
        })
        .detach();
    }

    /// Attaches the file at `path`: an image as an image, anything else
    /// as text. Says why it cannot when it cannot.
    pub fn attach_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let read = std::fs::read(path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| match image_mime(&bytes) {
                Some(mime) => image(mime, bytes),
                None => text(bytes),
            });
        match read {
            Ok(content) => {
                self.attachments.push(Attachment {
                    name,
                    path: Some(path.to_owned()),
                    content,
                });
                cx.notify();
            }
            Err(why) => {
                self.show_alert(format!("Could not attach {name}"), why, cx)
            }
        }
    }

    /// Attaches an image pasted into the composer.
    pub fn attach_pasted(
        &mut self,
        pasted: &gpui::Image,
        cx: &mut Context<Self>,
    ) {
        let mime = image_mime(&pasted.bytes);
        let attached = mime
            .ok_or_else(|| {
                "only PNG, JPEG, GIF and WebP images can be attached".to_owned()
            })
            .and_then(|mime| image(mime, pasted.bytes.clone()));
        match attached {
            Ok(content) => {
                let n = self
                    .attachments
                    .iter()
                    .filter(|attached| attached.path.is_none())
                    .count();
                let extension = mime
                    .and_then(|mime| mime.strip_prefix("image/"))
                    .unwrap_or("png");
                self.attachments.push(Attachment {
                    name: format!("pasted-{}.{extension}", n + 1),
                    path: None,
                    content,
                });
                cx.notify();
            }
            Err(why) => {
                self.show_alert("Could not attach the pasted image", why, cx)
            }
        }
    }

    pub fn remove_attachment(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.attachments.len() {
            self.attachments.remove(index);
            cx.notify();
        }
    }

    pub fn attachments(&self) -> &[Attachment] {
        &self.attachments
    }

    /// `text` with the attached text files after it, and the attached
    /// images, which are then gone. The window keeps each image, so its
    /// message shows it without asking the host.
    pub(crate) fn take_attachments(
        &mut self,
        text: String,
    ) -> (String, Vec<ImageContent>) {
        let mut images = Vec::new();
        let mut text = text;
        for attached in std::mem::take(&mut self.attachments) {
            match attached.content {
                Attached::Text(content) => {
                    text = format!(
                        "{text}\n\n<attached file=\"{}\">\n{}\n</attached>",
                        attached.name,
                        content.trim_end()
                    );
                }
                Attached::Image { image, shown } => {
                    self.keep_picture(&image, shown);
                    images.push(image);
                }
            }
        }
        (text, images)
    }
}

/// An image's bytes as an attachment, within [`MAX_IMAGE`].
fn image(mime: &str, bytes: Vec<u8>) -> Result<Attached, String> {
    if bytes.len() > MAX_IMAGE {
        return Err(format!(
            "it is {} MB; images up to {} MB can be attached",
            bytes.len() / (1024 * 1024),
            MAX_IMAGE / (1024 * 1024)
        ));
    }
    let format = format_of(mime).ok_or("the image cannot be drawn")?;
    Ok(Attached::Image {
        image: ImageContent {
            data: STANDARD.encode(&bytes),
            mime_type: mime.to_owned(),
        },
        shown: Arc::new(gpui::Image::from_bytes(format, bytes)),
    })
}

/// A text file's bytes as an attachment, within [`MAX_ATTACHMENT`].
fn text(bytes: Vec<u8>) -> Result<Attached, String> {
    if bytes.len() > MAX_ATTACHMENT {
        return Err(format!(
            "it is {} KB; files up to {} KB can be attached",
            bytes.len() / 1000,
            MAX_ATTACHMENT / 1000
        ));
    }
    String::from_utf8(bytes).map(Attached::Text).map_err(|_| {
        "it is neither text nor a PNG, JPEG, GIF or WebP image".into()
    })
}
