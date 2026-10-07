//! The history as OptChat keeps it, without I/O: every message folded
//! so far as entries, word for word; a binary tree of one-line
//! summaries over them; and the view, the list of tree nodes that tiles
//! the whole history within a byte budget
//! (`docs/reference/tree-compaction.md`).
//!
//! Node `(level, index)` covers entries `[index·2^level,
//! (index+1)·2^level)`. A level-0 node summarizes one entry; a parent
//! merges its two children. Nodes are named `id+n` by the first entry
//! they cover and how many: the label the model reads in the view and
//! passes to `zoom`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use tau_ai::message::{AssistantBlock, InputBlock, Message, UserContent};

/// The target size of one summary line, in UTF-8 bytes. A target, not a
/// bound: a line the model could not bring under it after every try
/// keeps its shortest form, and the view measures real sizes.
pub const NODE_BYTES: usize = 512;

/// The most characters an entry keeps of one message; a longer one
/// keeps its head and its tail.
pub const ENTRY_CHARS: usize = 30_000;

/// Who a message's part came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// The user's words.
    User,
    /// The agent's replies.
    Talk,
    /// The agent's tool calls: name and arguments.
    Tool,
    /// What a tool call returned.
    Echo,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Talk => "talk",
            Self::Tool => "tool",
            Self::Echo => "echo",
        }
    }
}

/// One message's part, as the history keeps it for good.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub kind: Kind,
    pub text: String,
}

impl Entry {
    pub fn new(kind: Kind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
        }
    }

    /// The entry as its own line: `kind: text`. A level-0 node whose
    /// entry fits is this, word for word.
    pub fn line(&self) -> String {
        format!("{}: {}", self.kind.name(), self.text)
    }
}

/// `text` cut to [`ENTRY_CHARS`] characters, its head and its tail
/// kept around a note of what was cut.
pub fn cap(text: &str) -> String {
    let total = text.chars().count();
    if total <= ENTRY_CHARS {
        return text.to_owned();
    }
    let keep = ENTRY_CHARS / 2;
    let head: String = text.chars().take(keep).collect();
    let tail: String = text.chars().skip(total - keep).collect();
    format!("{head}\n[… {} characters cut …]\n{tail}", total - 2 * keep)
}

/// A message as entries: a user message is one; an assistant message is
/// its text, if any, then one per tool call (its thinking is left out);
/// a tool result is one, named by its tool. Empty parts give none.
pub fn entries(message: &Message) -> Vec<Entry> {
    let mut out = Vec::new();
    match message {
        Message::User(user) => {
            let text = match &user.content {
                UserContent::Text(text) => text.clone(),
                UserContent::Blocks(blocks) => blocks_text(blocks),
            };
            if !text.trim().is_empty() {
                out.push(Entry::new(Kind::User, cap(&text)));
            }
        }
        Message::Assistant(reply) => {
            let talk: Vec<&str> = reply
                .content
                .iter()
                .filter_map(|block| match block {
                    AssistantBlock::Text(text) => Some(text.text.as_str()),
                    _ => None,
                })
                .collect();
            let talk = talk.join("\n");
            if !talk.trim().is_empty() {
                out.push(Entry::new(Kind::Talk, cap(&talk)));
            }
            for call in reply.tool_calls() {
                let args = serde_json::Value::Object(call.arguments.clone());
                out.push(Entry::new(
                    Kind::Tool,
                    cap(&format!("{} {args}", call.name)),
                ));
            }
        }
        Message::ToolResult(result) => {
            let text = blocks_text(&result.content);
            let failed = if result.is_error { " failed" } else { "" };
            out.push(Entry::new(
                Kind::Echo,
                cap(&format!("[{}{failed}] {text}", result.tool_name)),
            ));
        }
    }
    out
}

fn blocks_text(blocks: &[InputBlock]) -> String {
    blocks
        .iter()
        .map(|block| match block {
            InputBlock::Text(text) => text.text.as_str(),
            InputBlock::Image(_) => "[image]",
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A node of the tree: level `level` covers `2^level` entries, from
/// `index·2^level` on.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
)]
pub struct Node {
    pub level: u32,
    pub index: usize,
}

impl Node {
    pub fn new(level: u32, index: usize) -> Self {
        Self { level, index }
    }

    /// The level-0 node of entry `id`.
    pub fn leaf(id: usize) -> Self {
        Self::new(0, id)
    }

    /// How many entries it covers.
    pub fn count(self) -> usize {
        1 << self.level
    }

    /// The first entry it covers.
    pub fn first(self) -> usize {
        self.index << self.level
    }

    /// One past the last entry it covers.
    pub fn end(self) -> usize {
        self.first() + self.count()
    }

    pub fn parent(self) -> Self {
        Self::new(self.level + 1, self.index / 2)
    }

    /// Its two children; `None` for a level-0 node.
    pub fn children(self) -> Option<(Self, Self)> {
        let level = self.level.checked_sub(1)?;
        Some((
            Self::new(level, self.index * 2),
            Self::new(level, self.index * 2 + 1),
        ))
    }

    /// The node `zoom(id, n)` names, when `n` is a power of two and `id`
    /// a multiple of it.
    pub fn named(id: usize, n: usize) -> Option<Self> {
        (n.is_power_of_two() && id.is_multiple_of(n))
            .then(|| Self::new(n.trailing_zeros(), id / n))
    }

    /// Its name in the view: `id+n`.
    pub fn label(self) -> String {
        format!("{}+{}", self.first(), self.count())
    }
}

/// The history: entries, the summaries built over them, and the view.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct History {
    entries: Vec<Entry>,
    nodes: BTreeMap<Node, String>,
    view: Vec<Node>,
}

/// How [`History::fit`] left the view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    /// Within its budget.
    Fits,
    /// Over its budget, with no merge left whose line is built.
    Waiting,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    /// A history from what was kept of one: its entries, its built
    /// nodes and its view. Nodes past the entries are dropped, and a
    /// view that does not tile the entries in order is rebuilt from the
    /// entries' level-0 nodes.
    pub fn restore(
        entries: Vec<Entry>,
        nodes: impl IntoIterator<Item = (Node, String)>,
        view: Vec<Node>,
    ) -> Self {
        let total = entries.len();
        let nodes = nodes
            .into_iter()
            .filter(|(node, _)| node.end() <= total)
            .collect();
        let mut history = Self {
            entries,
            nodes,
            view: Vec::new(),
        };
        let tiles = view.iter().try_fold(0, |at, node| {
            (node.first() == at && history.nodes.contains_key(node))
                .then(|| node.end())
        }) == Some(total);
        history.view = if tiles {
            view
        } else {
            (0..total).map(Node::leaf).collect()
        };
        history
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn view(&self) -> &[Node] {
        &self.view
    }

    /// Every built node and its line.
    pub fn nodes(&self) -> impl Iterator<Item = (Node, &str)> {
        self.nodes.iter().map(|(node, text)| (*node, text.as_str()))
    }

    pub fn text(&self, node: Node) -> Option<&str> {
        self.nodes.get(&node).map(String::as_str)
    }

    pub fn is_built(&self, node: Node) -> bool {
        self.nodes.contains_key(&node)
    }

    /// Appends `entry` and its level-0 node to the view, unbuilt. The
    /// view is not fitted: see [`Self::fit`].
    pub fn push(&mut self, entry: Entry) -> usize {
        let id = self.entries.len();
        self.entries.push(entry);
        self.view.push(Node::leaf(id));
        id
    }

    /// Sets a node's line.
    ///
    /// # Panics
    ///
    /// When the node covers entries the history does not have.
    pub fn set(&mut self, node: Node, text: String) {
        assert!(
            node.end() <= self.len(),
            "{} is past the history",
            node.label()
        );
        self.nodes.insert(node, text);
    }

    /// Whether a node's sources are there: its entry, or its two
    /// children built.
    pub fn is_ready(&self, node: Node) -> bool {
        match node.children() {
            None => node.index < self.len(),
            Some((a, b)) => self.is_built(a) && self.is_built(b),
        }
    }

    /// A node's line when it needs no model: a level-0 node whose entry
    /// fits in [`NODE_BYTES`] is the entry, word for word; a parent whose
    /// children fit together is the two of them, one per line.
    pub fn free(&self, node: Node) -> Option<String> {
        let text = match node.children() {
            None => self.entries.get(node.index)?.line(),
            Some((a, b)) => format!("{}\n{}", self.text(a)?, self.text(b)?),
        };
        (text.len() <= NODE_BYTES).then_some(text)
    }

    /// The view's size: the bytes of its lines' texts, an unbuilt line
    /// counted at `unbuilt` bytes.
    pub fn view_bytes(&self, unbuilt: usize) -> usize {
        self.view
            .iter()
            .map(|node| self.text(*node).map_or(unbuilt, str::len))
            .sum()
    }

    /// Merges the view's lines, most due first, until it fits in
    /// `budget` bytes, passing over merges whose line is not built yet
    /// (OptChat's `fit`). Never splits a line.
    pub fn fit(&mut self, budget: usize) -> Fit {
        let mut size = self.view_bytes(NODE_BYTES);
        while size > budget {
            let Some(at) = most_due(&self.view, self.len(), |parent| {
                self.is_built(parent)
            }) else {
                return Fit::Waiting;
            };
            let (a, b) = (self.view[at], self.view[at + 1]);
            let parent = a.parent();
            size = size + self.text(parent).map_or(0, str::len)
                - self.text(a).map_or(NODE_BYTES, str::len)
                - self.text(b).map_or(NODE_BYTES, str::len);
            self.view.splice(at..at + 2, [parent]);
        }
        Fit::Fits
    }

    /// The parents [`Self::fit`] would merge into, in order, if every
    /// line were built, an unbuilt one measured as the most it can be:
    /// what to build so the view fits in `budget`.
    pub fn wanted(&self, budget: usize) -> Vec<Node> {
        let mut view = self.view.clone();
        let mut sizes: BTreeMap<Node, usize> = self
            .view
            .iter()
            .map(|node| (*node, self.text(*node).map_or(NODE_BYTES, str::len)))
            .collect();
        let mut size: usize = sizes.values().sum();
        let mut wanted = Vec::new();
        while size > budget {
            let Some(at) = most_due(&view, self.len(), |_| true) else {
                break;
            };
            let (a, b) = (view[at], view[at + 1]);
            let parent = a.parent();
            let parent_size = self.text(parent).map_or_else(
                || (sizes[&a] + 1 + sizes[&b]).min(NODE_BYTES),
                str::len,
            );
            size = size + parent_size - sizes[&a] - sizes[&b];
            sizes.insert(parent, parent_size);
            if !self.is_built(parent) {
                wanted.push(parent);
            }
            view.splice(at..at + 2, [parent]);
        }
        wanted
    }

    /// The view as the model reads it: `id+n|text` a line, oldest first,
    /// newlines in a text shown as spaces. An unbuilt line says so.
    pub fn render(&self) -> String {
        self.view
            .iter()
            .map(|node| format!("{}|{}", node.label(), self.flat(*node)))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The view's lines before entry `end`, bare (no labels), up to the
    /// first that is not built: what a compactor call reads as context.
    /// Ids in a compactor's input get copied into its output (OptChat,
    /// section 4.2).
    pub fn context(&self, end: usize) -> String {
        self.view
            .iter()
            .take_while(|node| node.end() <= end && self.is_built(**node))
            .map(|node| self.flat(*node))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A node's line on one line.
    pub fn flat(&self, node: Node) -> String {
        match self.text(node) {
            Some(text) => text.split_whitespace().collect::<Vec<_>>().join(" "),
            None => "(not summarized yet: zoom it)".to_owned(),
        }
    }

    /// What `zoom(id, n)` answers: line `id+n` opened into the two lines
    /// under it, or with `n = 1`, entry `id` whole.
    pub fn zoom(&self, id: usize, n: usize) -> Result<String, String> {
        let missing = || format!("No line {id}+{n}.");
        let node = Node::named(id, n).ok_or_else(missing)?;
        if node.end() > self.len() {
            return Err(missing());
        }
        match node.children() {
            None => {
                let entry = &self.entries[id];
                Ok(format!("{id}+1|{}", entry.line()))
            }
            Some((a, b)) => {
                if !self.is_built(a) || !self.is_built(b) {
                    return Err(missing());
                }
                Ok(format!(
                    "{}|{}\n{}|{}",
                    a.label(),
                    self.flat(a),
                    b.label(),
                    self.flat(b)
                ))
            }
        }
    }
}

/// The index in `view` of the pair to merge next: two siblings side by
/// side whose parent `ready` accepts, the most due first. A pair at
/// level `l` from entry `start` is due `(total - start) / 2^(l+2)`:
/// detail fades with age while each level keeps about as many lines.
/// Ties go to the older pair.
fn most_due(
    view: &[Node],
    total: usize,
    ready: impl Fn(Node) -> bool,
) -> Option<usize> {
    let mut best: Option<(usize, f64)> = None;
    for at in 0..view.len().saturating_sub(1) {
        let (a, b) = (view[at], view[at + 1]);
        if a.level != b.level
            || !a.index.is_multiple_of(2)
            || b.index != a.index + 1
            || !ready(a.parent())
        {
            continue;
        }
        let due = (total - a.first()) as f64 / (1u64 << (a.level + 2)) as f64;
        if best.is_none_or(|(_, top)| due > top) {
            best = Some((at, due));
        }
    }
    best.map(|(at, _)| at)
}
