//! tau-canvas: what a run makes for the person to look at, in a pane
//! beside its transcript.
//!
//! The model shows an image file with `canvas_show`, writes a Markdown
//! document with `canvas_write`, revises one with `canvas_edit`, and
//! reads one back with `canvas_read`. Each image or document is an item
//! on the run's canvas, numbered from 1, and every revision of it is
//! kept: the person can step back through them.
//!
//! - [`Record`]: what the plugin publishes, one revision at a time.
//! - [`Canvas`]: the items, as the records fold into them, on the host
//!   and in every interface alike.
//! - [`ui`]: the run's pane, and the cards of the canvas tools.
//! - `host` (feature `host`): the tools, and the storage that keeps the
//!   images they show.

#[cfg(feature = "demo")]
pub mod demo;
#[cfg(feature = "host")]
mod half;
#[cfg(feature = "host")]
pub mod host;
pub mod ui;

#[cfg(feature = "host")]
pub use half::CanvasHost;
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub use ui::CanvasUi;

/// The plugin's name, for both its halves.
pub const NAME: &str = "tau-canvas";

/// The tools' names, as the model calls them.
pub const SHOW: &str = "canvas_show";
pub const WRITE: &str = "canvas_write";
pub const EDIT: &str = "canvas_edit";
pub const READ: &str = "canvas_read";
pub const TOOLS: [&str; 4] = [SHOW, WRITE, EDIT, READ];

/// The largest image the canvas takes: 8 MiB, so its bytes reach a
/// phone in one message, base64 and all.
pub const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;

/// The longest document the canvas takes: 256 KiB of text. Every
/// revision travels with the run's state to each interface.
pub const MAX_DOCUMENT_BYTES: usize = 256 * 1024;

/// What tau-canvas publishes, as stored and reported.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Record {
    /// The call `call` made a revision of item `item`; its first
    /// revision makes the item.
    Revised {
        call: String,
        item: u32,
        title: String,
        content: Content,
    },
}

impl Record {
    /// The record `body` holds, or none, said the first time.
    pub fn parse(body: &Value) -> Option<Self> {
        tau_agent::plugin::read_record(NAME, body)
    }
}

/// What a revision holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Content {
    /// An image, kept in the plugin's storage.
    Image { image: ImageRef, caption: String },
    /// A Markdown document.
    Document { text: String },
}

impl Content {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Image { .. } => Kind::Image,
            Self::Document { .. } => Kind::Document,
        }
    }
}

/// What an item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Image,
    Document,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Document => "document",
        }
    }
}

/// An image the canvas shows: where its bytes are kept, and what they
/// are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageRef {
    pub blob: Blob,
    /// `image/png`, `image/svg+xml`, ...
    pub mime: String,
    /// Its size in pixels, when it has one (an SVG may not).
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// The file it was read from, as the model named it.
    pub path: String,
}

impl ImageRef {
    /// `1200×800 PNG`, or `SVG`.
    pub fn describe(&self) -> String {
        let format = format_name(&self.mime);
        match (self.width, self.height) {
            (Some(width), Some(height)) => format!("{width}×{height} {format}"),
            _ => format.to_owned(),
        }
    }
}

/// What `mime` is called: `PNG`, `SVG`.
pub fn format_name(mime: &str) -> &str {
    match mime {
        "image/png" => "PNG",
        "image/jpeg" => "JPEG",
        "image/gif" => "GIF",
        "image/webp" => "WebP",
        "image/bmp" => "BMP",
        "image/svg+xml" => "SVG",
        other => other,
    }
}

/// Bytes kept in the plugin's storage (tau-artifacts), by reference:
/// an artifact's metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blob {
    pub id: String,
    pub digest: String,
    pub size_bytes: u64,
}

/// A run's canvas: its items, as its records fold into them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Canvas {
    /// In the order they were made.
    pub items: Vec<Item>,
    /// How many revisions came: it grows with each.
    pub fresh: u64,
    /// The item revised last.
    pub last: Option<u32>,
}

/// An image or a document on the canvas, with every revision of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Item {
    /// Its number, from 1, as the model names it.
    pub id: u32,
    /// Its latest title.
    pub title: String,
    /// Oldest first; never empty.
    pub revisions: Vec<Revision>,
}

/// One revision of an item: the call that made it, and what it holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Revision {
    pub call: String,
    pub content: Content,
}

impl Item {
    pub fn kind(&self) -> Kind {
        self.latest().content.kind()
    }

    pub fn latest(&self) -> &Revision {
        self.revisions.last().expect("an item has a revision")
    }
}

impl Canvas {
    /// The canvas the records `bodies` fold into, skipping any that
    /// are not tau-canvas's.
    pub fn from_records(bodies: &[Value]) -> Self {
        let mut canvas = Self::default();
        for record in bodies.iter().filter_map(Record::parse) {
            canvas.revise(record);
        }
        canvas
    }

    /// Folds `record` in: a new revision of its item, or a new item.
    pub fn revise(&mut self, record: Record) {
        let Record::Revised {
            call,
            item,
            title,
            content,
        } = record;
        let revision = Revision { call, content };
        match self.items.iter_mut().find(|found| found.id == item) {
            Some(found) => {
                found.title = title;
                found.revisions.push(revision);
            }
            None => self.items.push(Item {
                id: item,
                title,
                revisions: vec![revision],
            }),
        }
        self.fresh += 1;
        self.last = Some(item);
    }

    pub fn item(&self, id: u32) -> Option<&Item> {
        self.items.iter().find(|item| item.id == id)
    }

    /// The number the next new item gets.
    pub fn next_id(&self) -> u32 {
        self.items.iter().map(|item| item.id).max().unwrap_or(0) + 1
    }

    /// The revision the call `call` made: its item, and the revision's
    /// place among the item's.
    pub fn by_call(&self, call: &str) -> Option<(&Item, usize)> {
        self.items.iter().find_map(|item| {
            item.revisions
                .iter()
                .position(|revision| revision.call == call)
                .map(|at| (item, at))
        })
    }
}

/// An exact replacement in a document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "host", derive(schemars::JsonSchema))]
pub struct Edit {
    /// Text that occurs exactly once in the document, as the edits
    /// before this one left it.
    pub old: String,
    /// What replaces it.
    pub new: String,
}

/// `text` with `edits` made in turn: each `old` must occur exactly once
/// in the text the edits before it left. Says which edit failed, and
/// why, when one does.
pub fn apply_edits(text: &str, edits: &[Edit]) -> Result<String, String> {
    if edits.is_empty() {
        return Err("No edits were given.".to_owned());
    }
    let mut text = text.to_owned();
    for (n, edit) in edits.iter().enumerate() {
        let n = n + 1;
        if edit.old.is_empty() {
            return Err(format!("Edit {n}: `old` is empty."));
        }
        let mut found = text.match_indices(&edit.old);
        match (found.next(), found.next()) {
            (Some((at, _)), None) => {
                text.replace_range(at..at + edit.old.len(), &edit.new);
            }
            (None, _) => {
                return Err(format!(
                    "Edit {n}: `old` does not occur in the document."
                ));
            }
            (Some(_), Some(_)) => {
                return Err(format!(
                    "Edit {n}: `old` occurs more than once in the \
                     document; include more of the text around it."
                ));
            }
        }
    }
    Ok(text)
}

/// Words in `text`, for a document's card.
pub fn words(text: &str) -> usize {
    text.split_whitespace().count()
}

#[cfg(test)]
mod tests;
