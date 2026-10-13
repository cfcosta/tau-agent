//! tau-canvas's UI half: the run's canvas folded from its records, the
//! pane that shows it, and the cards of the canvas tools.
//!
//! The pane shows the item revised last, at its latest revision, until
//! the person picks another; whatever comes next takes its place again.
//! Images are not in the records: the pane fetches each from the host
//! half once per window, by its blob. The answer reaches phones only if
//! they are there as it comes, so one still waiting after [`RETRY`] is
//! asked for again.

mod card;
mod pane;

use std::{collections::HashMap, sync::Arc};

use base64::{Engine, engine::general_purpose::STANDARD};
use gpui::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_agent::tool::RunId;
use tau_ui_plugin::{
    Fold,
    Manifest,
    RunCx,
    UiPlugin,
    ViewCx,
    points::{self, AtRun},
};

use crate::{Blob, Canvas, Item, NAME, Record};

/// tau-canvas's UI.
#[derive(Debug, Clone, Copy, Default)]
pub struct CanvasUi;

impl Fold for Canvas {
    type Record = Record;

    fn apply(&mut self, record: Record, _run: &mut dyn RunCx) {
        self.revise(record);
    }
}

/// What the UI asks the host half.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    /// An image's bytes, from the storage of the repository `repo`.
    Fetch { repo: String, blob: Blob },
}

/// What the host half answers an [`Action::Fetch`]: the bytes in
/// base64, or why it could not read them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fetched {
    pub id: String,
    pub data: Option<String>,
    pub error: Option<String>,
}

/// What a window keeps of the canvas.
#[derive(Default)]
pub struct Ui {
    /// What the person picked on each run's canvas, while nothing newer
    /// came.
    picked: HashMap<RunId, Pick>,
    /// The images fetched, or being fetched, by blob.
    images: HashMap<String, Loaded>,
}

/// A revision the person picked to look at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pick {
    pub item: u32,
    /// Its place among the item's revisions.
    pub revision: usize,
    /// How fresh the canvas was then ([`Canvas::fresh`]).
    pub fresh: u64,
}

/// How long an image asked for waits before it is asked for again.
pub const RETRY: std::time::Duration = std::time::Duration::from_secs(10);

/// An image, as far as the window has it.
#[derive(Clone)]
pub enum Loaded {
    /// Asked for at an instant; its type, to decode it with.
    Waiting(String, std::time::Instant),
    Ready(Arc<gpui::Image>),
    Failed(String),
}

impl Ui {
    /// Picks `revision` of `item` on the canvas of `run`.
    pub fn pick(&mut self, run: &RunId, pick: Pick) {
        self.picked.insert(run.clone(), pick);
    }
}

/// The revision the pane shows: the one picked, while nothing newer
/// came, or the latest of the item revised last.
pub fn shown(canvas: &Canvas, pick: Option<Pick>) -> Option<(&Item, usize)> {
    if let Some(pick) = pick.filter(|pick| pick.fresh == canvas.fresh)
        && let Some(item) = canvas.item(pick.item)
    {
        return Some((item, pick.revision.min(item.revisions.len() - 1)));
    }
    let item = canvas.last.and_then(|id| canvas.item(id))?;
    Some((item, item.revisions.len() - 1))
}

/// The image `blob` of type `mime` from the repository `repo`, as far
/// as the window has it; asks the host for it the first time.
fn image(
    view: &mut ViewCx<'_, CanvasUi>,
    repo: &str,
    blob: &Blob,
    mime: &str,
) -> Loaded {
    match view.read_ui().images.get(&blob.id) {
        Some(Loaded::Waiting(_, at)) if at.elapsed() >= RETRY => {}
        Some(loaded) => return loaded.clone(),
        None => {}
    }
    let waiting = Loaded::Waiting(mime.to_owned(), std::time::Instant::now());
    view.ui.update(view.cx, |ui, _| {
        ui.images.insert(blob.id.clone(), waiting.clone());
    });
    view.handle.act(
        Action::Fetch {
            repo: repo.to_owned(),
            blob: blob.clone(),
        },
        view.cx,
    );
    waiting
}

/// The format gpui decodes an image of type `mime` as.
fn format_of(mime: &str) -> Option<gpui::ImageFormat> {
    use gpui::ImageFormat;
    Some(match mime {
        "image/png" => ImageFormat::Png,
        "image/jpeg" => ImageFormat::Jpeg,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::Webp,
        "image/bmp" => ImageFormat::Bmp,
        "image/svg+xml" => ImageFormat::Svg,
        _ => return None,
    })
}

/// What a fetched image becomes, given the type it was asked for as.
fn loaded(fetched: Fetched, mime: &str) -> Loaded {
    let bytes = match (fetched.data, fetched.error) {
        (Some(data), _) => STANDARD.decode(data),
        (None, error) => {
            return Loaded::Failed(
                error.unwrap_or_else(|| "nothing came".to_owned()),
            );
        }
    };
    match (bytes, format_of(mime)) {
        (Ok(bytes), Some(format)) => {
            Loaded::Ready(Arc::new(gpui::Image::from_bytes(format, bytes)))
        }
        (Err(error), _) => Loaded::Failed(error.to_string()),
        (_, None) => Loaded::Failed(format!("cannot draw {mime}")),
    }
}

impl UiPlugin for CanvasUi {
    type State = Canvas;
    type Data = ();
    type RepoData = ();
    type Settings = ();
    type Ui = Ui;

    fn name(&self) -> &'static str {
        NAME
    }

    fn reply(&self, ui: &mut Ui, reply: Value, cx: &mut Context<Ui>) {
        let Ok(fetched) = serde_json::from_value::<Fetched>(reply) else {
            return;
        };
        let Some(Loaded::Waiting(mime, _)) =
            ui.images.get(&fetched.id).cloned()
        else {
            return;
        };
        ui.images.insert(fetched.id.clone(), loaded(fetched, &mime));
        cx.notify();
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .contribute(points::PANE, pane::pane)
            .contribute(points::CARD, card::card)
            .contribute(points::STATUS, |_: &AtRun, view| {
                let canvas = view.state.filter(|c| !c.items.is_empty())?;
                Some(tau_ui_plugin::PluginStatus {
                    name: NAME.into(),
                    state: match canvas.items.len() {
                        1 => "1 item on the canvas".to_owned(),
                        n => format!("{n} items on the canvas"),
                    },
                    tone: tau_ui_kit::theme::Tone::Quiet,
                })
            })
    }
}
