//! The images the person sent, as a window draws them. A transcript
//! holds each by reference ([`Picture`]); the window asks the host for
//! its bytes the first time it draws it (`WorkspaceEvent::Picture`), and
//! keeps them. The answer comes unnumbered to the phones there now
//! (`HostUpdate::Picture`), so a phone that was away asks again: a
//! picture still waiting after [`RETRY`] is asked for once more.
//!
//! The window that sent an image keeps it as it sends, and never asks.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use gpui::Context;
use tau_agent::tool::RunId;
use tau_ai::message::ImageContent;

use crate::{
    attach::format_of,
    view::Picture,
    workspace::{Workspace, WorkspaceEvent},
};

/// How long a picture asked for waits before it is asked for again.
pub const RETRY: Duration = Duration::from_secs(10);

/// A picture, as far as the window has it.
#[derive(Clone)]
pub enum Shown {
    /// Asked for at this instant.
    Waiting(Instant),
    Ready(Arc<gpui::Image>),
    Failed(String),
}

impl Workspace {
    /// Keeps `image`, drawn as `shown`, for the message it goes with.
    pub(crate) fn keep_picture(
        &self,
        image: &ImageContent,
        shown: Arc<gpui::Image>,
    ) {
        self.pictures
            .borrow_mut()
            .insert(Picture::of(image).key, Shown::Ready(shown));
    }

    /// `picture` of `run`, as far as the window has it; asks the host
    /// for it the first time, and again once [`RETRY`] went by.
    pub(crate) fn picture(
        &self,
        run: &RunId,
        picture: &Picture,
        cx: &mut Context<Self>,
    ) -> Shown {
        let mut pictures = self.pictures.borrow_mut();
        match pictures.get(&picture.key) {
            Some(Shown::Waiting(at)) if at.elapsed() < RETRY => {
                return Shown::Waiting(*at);
            }
            Some(Shown::Waiting(_)) | None => {}
            Some(shown) => return shown.clone(),
        }
        let now = Instant::now();
        pictures.insert(picture.key.clone(), Shown::Waiting(now));
        cx.emit(WorkspaceEvent::Picture {
            run: run.clone(),
            picture: picture.clone(),
        });
        Shown::Waiting(now)
    }

    /// Whether the window has the bytes of the picture `key`.
    pub fn has_picture(&self, key: &str) -> bool {
        matches!(self.pictures.borrow().get(key), Some(Shown::Ready(_)))
    }

    /// The host answered for `picture`: its bytes in base64, or why it
    /// has none.
    pub(crate) fn picture_came(
        &mut self,
        picture: &Picture,
        data: Option<&str>,
        error: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let shown = match (data, format_of(&picture.mime)) {
            (Some(data), Some(format)) => match STANDARD.decode(data) {
                Ok(bytes) => Shown::Ready(Arc::new(gpui::Image::from_bytes(
                    format, bytes,
                ))),
                Err(error) => Shown::Failed(error.to_string()),
            },
            (Some(_), None) => {
                Shown::Failed(format!("cannot draw {}", picture.mime))
            }
            (None, _) => Shown::Failed(
                error.unwrap_or("the host has no such image").to_owned(),
            ),
        };
        self.pictures
            .borrow_mut()
            .insert(picture.key.clone(), shown);
        cx.notify();
    }
}
