//! Tree compaction as a plugin: when the context nears the window, the
//! messages before the kept tail are folded into the history, the tree
//! grows over them, and the transcript becomes the view followed by the
//! tail. The `zoom` tool opens the view's lines.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tau_agent::{
    error::{PluginError, ToolError},
    plugin::{
        ContextView,
        Plugin,
        PluginCtx,
        PluginRun,
        Rewrite,
        RunPlan,
        Trigger,
    },
    tool::{ToolCtx, ToolOutput, TypedTool, typed},
};
use tau_ai::{
    message::{Message, Timestamp, UserContent, UserMessage},
    model,
};
use tau_compaction::{find_cut_point, should_compact};

use crate::{
    NAME,
    build::{BuildError, Builder},
    tree::{Entry, History, Node, entries},
};

/// Tree compaction's settings (`docs/reference/tree-compaction.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeCompaction {
    /// Tokens reserved for the model's response and the next request's
    /// growth: compaction runs once the estimate passes `context_window
    /// - reserve_tokens`. Defaults to 16,384, as tau-compaction's.
    pub reserve_tokens: u64,
    /// The tokens of newest messages kept as they are. Defaults to
    /// 20,000, as tau-compaction's.
    pub keep_recent_tokens: u64,
    /// The model's context window, when the registry does not know it.
    pub context_window: Option<u64>,
    /// The view's budget, in bytes of its lines' texts. Defaults to
    /// 64,000: about 16,000 tokens, a quarter of OptChat's, since the
    /// view shares the window with the kept messages.
    pub view_bytes: usize,
    /// Compactor requests at once. Defaults to 8, as OptChat's.
    pub jobs: usize,
}

impl Default for TreeCompaction {
    fn default() -> Self {
        Self {
            reserve_tokens: 16_384,
            keep_recent_tokens: 20_000,
            context_window: None,
            view_bytes: 64_000,
            jobs: 8,
        }
    }
}

impl TreeCompaction {
    pub fn reserve_tokens(mut self, tokens: u64) -> Self {
        self.reserve_tokens = tokens;
        self
    }

    pub fn keep_recent_tokens(mut self, tokens: u64) -> Self {
        self.keep_recent_tokens = tokens;
        self
    }

    pub fn context_window(mut self, tokens: u64) -> Self {
        self.context_window = Some(tokens);
        self
    }

    pub fn view_bytes(mut self, bytes: usize) -> Self {
        self.view_bytes = bytes;
        self
    }

    pub fn jobs(mut self, jobs: usize) -> Self {
        self.jobs = jobs;
        self
    }

    /// The thresholds as tau-compaction reads them.
    fn thresholds(&self) -> tau_compaction::Compaction {
        tau_compaction::Compaction::default()
            .reserve_tokens(self.reserve_tokens)
            .keep_recent_tokens(self.keep_recent_tokens)
    }
}

/// What tree compaction publishes and stores with a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Record {
    /// Whether it is on, as a run starts: what the interface folds,
    /// never stored.
    Starting { on: bool },
    /// What a compaction folded into the history: its entries from
    /// `first` on, and every line built for them, so a fork or a resumed
    /// run can zoom into them.
    Folded {
        first: usize,
        entries: Vec<Entry>,
        nodes: Vec<(Node, String)>,
    },
    /// What a compaction did, for the run's plugin list.
    Compacted {
        /// Messages folded in this time.
        messages: usize,
        /// Entries the history holds.
        entries: usize,
        /// Lines the view holds.
        lines: usize,
        /// The view's size, in bytes.
        bytes: usize,
        /// Lines that took a request this time.
        asked: usize,
        tokens_before: u64,
    },
}

/// The body of a stored rewrite: the view it put first, and when.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Details {
    pub view: Vec<Node>,
    /// Entries the history held.
    pub entries: usize,
    pub tokens_before: u64,
    /// The view message's timestamp.
    pub timestamp: Timestamp,
}

/// The history a run's view and its `zoom` share.
type Shared = Arc<Mutex<History>>;

#[async_trait]
impl Plugin for TreeCompaction {
    fn name(&self) -> &str {
        NAME
    }

    async fn start(
        &self,
        plan: &mut RunPlan,
        _ctx: &PluginCtx,
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        let details: Option<Details> = plan
            .last_rewrite()
            .map(|details| serde_json::from_value(details.clone()))
            .transpose()?;
        let history = restore(plan.records(), details.as_ref());
        let shared: Shared = Arc::new(Mutex::new(history));
        plan.add_tool(Arc::new(typed(Zoom(shared.clone()))));
        let mut builder = Builder::new(plan.model());
        builder.reasoning = plan.reasoning;
        builder.jobs = self.jobs;
        Ok(Box::new(TreeRun {
            settings: *self,
            window: self
                .context_window
                .or(model::find(plan.model()).map(|m| m.context_window)),
            builder,
            shared,
            timestamp: details.map(|details| details.timestamp),
            failures: 0,
            retry_at: 0,
        }))
    }
}

/// The history a run inherits: the entries and lines its forks' records
/// hold, under the view of the last rewrite.
fn restore(
    records: &[serde_json::Value],
    details: Option<&Details>,
) -> History {
    let mut entries = Vec::new();
    let mut nodes = Vec::new();
    for record in records {
        let Ok(Record::Folded {
            first,
            entries: more,
            nodes: built,
        }) = serde_json::from_value(record.clone())
        else {
            continue;
        };
        // A record that does not follow on from the last is from a
        // branch this run does not stand on.
        if first != entries.len() {
            continue;
        }
        entries.extend(more);
        nodes.extend(built);
    }
    let Some(details) = details else {
        return History::new();
    };
    entries.truncate(details.entries);
    History::restore(entries, nodes, details.view.clone())
}

struct TreeRun {
    settings: TreeCompaction,
    window: Option<u64>,
    builder: Builder,
    shared: Shared,
    /// When the view message the transcript opens with was made.
    timestamp: Option<Timestamp>,
    failures: u32,
    retry_at: u32,
}

/// The most turns a failed compaction waits before trying again.
const MAX_BACKOFF_TURNS: u32 = 32;

#[async_trait]
impl PluginRun for TreeRun {
    async fn rewrite_context(
        &mut self,
        view: &ContextView<'_>,
        ctx: &PluginCtx,
    ) -> Result<Option<Rewrite>, PluginError> {
        match view.trigger {
            Trigger::TurnEnd | Trigger::Start => {
                let due = view.turn >= self.retry_at
                    && self.window.is_some_and(|window| {
                        should_compact(
                            view.tokens,
                            window,
                            &self.settings.thresholds(),
                        )
                    });
                if !due {
                    return Ok(None);
                }
                let result = self.compact(view, ctx).await;
                match &result {
                    Err(_) => {
                        self.failures += 1;
                        let wait = 2u32
                            .saturating_pow(self.failures)
                            .min(MAX_BACKOFF_TURNS);
                        self.retry_at = view.turn.saturating_add(wait);
                    }
                    Ok(_) => self.failures = 0,
                }
                Ok(result?)
            }
            Trigger::Overflow => Ok(self.compact(view, ctx).await?),
            // An idle run is tau-compaction's to summarize, in its own
            // conversation, while its cache lasts.
            Trigger::Idle => Ok(None),
        }
    }
}

impl From<BuildError> for PluginError {
    fn from(error: BuildError) -> Self {
        Self::other(error)
    }
}

impl TreeRun {
    /// Folds the messages before the kept tail into the history and
    /// puts the view in their place. `None` when the tail is all there
    /// is.
    async fn compact(
        &mut self,
        view: &ContextView<'_>,
        ctx: &PluginCtx,
    ) -> Result<Option<Rewrite>, BuildError> {
        let transcript = view.transcript;
        let mut history = self.shared.lock().expect("not poisoned").clone();
        // The transcript opens with the last view, unless another
        // plugin's rewrite replaced it since; then whatever it opens with
        // is folded like any other message.
        let opens = self.timestamp.is_some_and(|timestamp| {
            transcript.first() == Some(&view_message(&history, timestamp))
        });
        let offset = usize::from(opens);
        let rest = &transcript[offset..];
        let kept_from = offset
            + find_cut_point(rest, self.settings.keep_recent_tokens)
                .first_kept_index;
        if kept_from <= offset {
            return Ok(None);
        }
        let first = history.len();
        for message in &transcript[offset..kept_from] {
            for entry in entries(message) {
                history.push(entry);
            }
        }
        let before: Vec<Node> = history.nodes().map(|(node, _)| node).collect();
        // On a failure the history stays as it was: the next try folds
        // the same messages again.
        let grown = self
            .builder
            .grow(&mut history, self.settings.view_bytes, ctx)
            .await?;
        let timestamp = ctx.now();
        let nodes: Vec<(Node, String)> = history
            .nodes()
            .filter(|(node, _)| !before.contains(node))
            .map(|(node, text)| (node, text.to_owned()))
            .collect();
        let folded = Record::Folded {
            first,
            entries: history.entries()[first..].to_vec(),
            nodes,
        };
        if let Err(error) = ctx.record(&folded).await {
            eprintln!(
                "{NAME}: the folded history could not be stored: {error}"
            );
        }
        ctx.publish(&Record::Compacted {
            messages: kept_from - offset,
            entries: history.len(),
            lines: history.view().len(),
            bytes: history.view_bytes(0),
            asked: grown.asked,
            tokens_before: view.tokens,
        })
        .await;
        let messages = std::iter::once(view_message(&history, timestamp))
            .chain(transcript[kept_from..].iter().cloned())
            .collect();
        let details = Details {
            view: history.view().to_vec(),
            entries: history.len(),
            tokens_before: view.tokens,
            timestamp,
        };
        *self.shared.lock().expect("not poisoned") = history;
        self.timestamp = Some(timestamp);
        Ok(Some(Rewrite {
            messages,
            details: serde_json::to_value(&details).expect("details serialize"),
            // The folded messages leave the context: only zoom reaches
            // them.
            drops_conversation: true,
        }))
    }
}

/// What the view message says before the view.
pub const VIEW_PREFIX: &str = "\
The conversation before this point was compacted into the lines below: \
all of it, oldest first, as one-line summaries.

Each line is `id+n|text`: the n messages from message id on, summarized, \
newlines shown as spaces. A summary tags each item with its kind: user \
(the user's words), talk (your replies), tool (your tool calls) or echo \
(their results). A short message is its own line, word for word. Recent \
lines cover one message each; the older the messages, the more a line \
covers. The messages after this one are as they were.

zoom(id, n) opens line id+n into the two lines of n/2 messages it was \
made from; zoom(id, 1) gives message id in full. Zoom whenever a line \
only mentions something you need, such as the user's exact words, a \
decision, a past attempt or where a file is, before you act, guess or ask.

<chat>
";

/// What closes the view message.
pub const VIEW_SUFFIX: &str = "\n</chat>";

/// The message that stands for everything folded: the view.
pub fn view_message(history: &History, timestamp: Timestamp) -> Message {
    Message::User(UserMessage {
        content: UserContent::Text(format!(
            "{VIEW_PREFIX}{}{VIEW_SUFFIX}",
            history.render()
        )),
        timestamp,
    })
}

/// `zoom`: opens a line of a history's view. Tree compaction adds one
/// to each run; anything that shows a model a view can give it one.
pub struct Zoom(pub Arc<Mutex<History>>);

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ZoomArgs {
    /// The line's first message: the `id` of `id+n`.
    pub id: usize,
    /// How many messages the line covers: the `n` of `id+n`.
    pub n: usize,
}

#[async_trait]
impl TypedTool for Zoom {
    type Args = ZoomArgs;

    const NAME: &'static str = "zoom";
    const DESCRIPTION: &'static str = "Open the line id+n of the compacted \
        conversation (<chat>) into the two lines of n/2 under it; n = 1 \
        gives the message whole.";

    async fn call(
        &self,
        args: ZoomArgs,
        _ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let history = self.0.lock().expect("not poisoned");
        if history.is_empty() {
            return Err(ToolError::Message(
                "Nothing has been compacted yet: there is no line to open."
                    .into(),
            ));
        }
        history
            .zoom(args.id, args.n)
            .map(ToolOutput::text)
            .map_err(ToolError::Message)
    }
}
