//! tau-canvas's UI half: the run's canvas folded from its records, the
//! pane that shows it, and the cards of the canvas tools.
//!
//! The pane shows the item revised last, at its latest revision, until
//! the person picks another; whatever comes next takes its place again.
//! What was picked is UI state, under [`pick_key`]; the pane and the
//! cards are view trees (ADR 0045), but for the images in them.
//! Images are not in the records: the pane fetches each from the host
//! half once per window, by its blob. The answer reaches phones only if
//! they are there as it comes, so one still waiting after [`RETRY`] is
//! asked for again.

mod card;
mod pane;

use gpui::App;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_agent::tool::RunId;
use tau_ui_plugin::{
    Fold,
    Handle,
    Manifest,
    RunCx,
    UiPlugin,
    ViewCx,
    points::{self, AtRun},
    tree::Node,
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

/// A revision the person picked to look at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pick {
    pub item: u32,
    /// Its place among the item's revisions.
    pub revision: usize,
    /// How fresh the canvas was then ([`Canvas::fresh`]).
    pub fresh: u64,
}

/// The UI state key under which the canvas of `run` keeps its
/// [`Pick`].
pub fn pick_key(run: &RunId) -> String {
    format!("pick.{run}")
}

/// What the person picked on the canvas of `run`, if anything.
fn picked(view: &ViewCx<'_, CanvasUi>, run: &RunId) -> Option<Pick> {
    let value = view.ui_value(&pick_key(run))?;
    serde_json::from_value(value.clone()).ok()
}

/// The trees' actions: show the [`Pick`] in the arguments, and with
/// [`OPEN`], open the pane too.
const PICK: &str = "pick";
const OPEN: &str = "open";

/// Carries out a tree's action on the canvas of `run`.
fn on_action(
    handle: Handle,
    run: RunId,
) -> impl Fn(&str, &Value, &mut App) + 'static {
    move |action, args, cx| {
        if action != PICK && action != OPEN {
            return;
        }
        if serde_json::from_value::<Pick>(args.clone()).is_err() {
            return;
        }
        handle.set_ui(pick_key(&run), Some(args.clone()), cx);
        if action == OPEN {
            handle.open_pane(cx);
        }
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

/// The image `shown` from the repository `repo` as a tree piece, `height`
/// rems tall: the interface fetches it with this plugin's action and
/// keeps it, by its blob.
fn image(repo: &str, shown: &crate::ImageRef, height: f32) -> Node {
    Node::Image {
        key: shown.blob.id.clone(),
        mime: shown.mime.clone(),
        fetch: serde_json::to_value(Action::Fetch {
            repo: repo.to_owned(),
            blob: shown.blob.clone(),
        })
        .unwrap_or_default(),
        height: Some(height),
    }
}

impl UiPlugin for CanvasUi {
    type State = Canvas;
    type Data = ();
    type RepoData = ();
    type Settings = ();

    fn name(&self) -> &'static str {
        NAME
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
