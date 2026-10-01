//! Files attached to the next message: text files the user picks with
//! the composer's paperclip. Their content goes with the message, as
//! the model sees it, whatever the run's workspace holds.

use std::path::{Path, PathBuf};

use gpui::{Context, PathPromptOptions};

use crate::workspace::Workspace;

/// The largest file that can be attached.
pub const MAX_ATTACHMENT: usize = 200_000;

/// A file attached to the next message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    /// What the chip and the message call it: the file's name.
    pub name: String,
    pub path: PathBuf,
    pub text: String,
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

    /// Attaches the text file at `path`, or says why it cannot.
    pub fn attach_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let read = std::fs::read(path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                if bytes.len() > MAX_ATTACHMENT {
                    return Err(format!(
                        "it is {} KB; files up to {} KB can be attached",
                        bytes.len() / 1000,
                        MAX_ATTACHMENT / 1000
                    ));
                }
                String::from_utf8(bytes).map_err(|_| {
                    "it is not text; only text files can be attached".into()
                })
            });
        match read {
            Ok(text) => {
                self.attachments.push(Attachment {
                    name,
                    path: path.to_owned(),
                    text,
                });
                cx.notify();
            }
            Err(why) => {
                self.show_alert(format!("Could not attach {name}"), why, cx)
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

    /// `text` with the attached files after it, which are then gone.
    pub(crate) fn with_attachments(&mut self, text: String) -> String {
        let attached = std::mem::take(&mut self.attachments);
        attached.into_iter().fold(text, |text, file| {
            format!(
                "{text}\n\n<attached file=\"{}\">\n{}\n</attached>",
                file.name,
                file.text.trim_end()
            )
        })
    }
}
