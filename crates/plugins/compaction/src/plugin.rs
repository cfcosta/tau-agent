//! Compaction as a plugin: the part that makes requests and keeps state.
//!
//! The rules are the crate root's pure functions; this module asks for
//! summaries and turns them into a [`Rewrite`]. Add it to an agent after
//! every other context plugin, so cheaper rewrites (pruning) get the
//! first chance.

use async_trait::async_trait;
use tau_agent::{
    error::PluginError,
    plugin::{
        ContextView,
        Plugin,
        PluginCtx,
        PluginRun,
        Rewrite,
        RunPlan,
        Trigger,
    },
};
use tau_ai::{
    message::{Message, UserContent, UserMessage},
    model,
    responses::request::{ReasoningEffort, Settings},
};

use crate::{
    Compaction,
    CompactionError,
    Record,
    SUMMARIZATION_SYSTEM_PROMPT,
    build_summary_request,
    build_turn_prefix_summary_request,
    check_summary,
    format_file_operations,
    merge_split_turn_summary,
    plan,
    should_compact,
};

/// The name compaction goes by in events and stored rewrites.
pub const NAME: &str = "compaction";

#[async_trait]
impl Plugin for Compaction {
    fn name(&self) -> &str {
        NAME
    }

    async fn start(
        &self,
        plan: &mut RunPlan,
        _ctx: &PluginCtx,
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        let known = model::find(plan.model());
        let compacted = plan
            .last_rewrite()
            .map(|details| serde_json::from_value(details.clone()))
            .transpose()?;
        Ok(Box::new(CompactionRun {
            settings: *self,
            model: plan.model().to_owned(),
            reasoning: plan.reasoning,
            window: self
                .context_window
                .or(known.map(|model| model.context_window)),
            compacted,
            off: false,
        }))
    }
}

struct CompactionRun {
    settings: Compaction,
    model: String,
    reasoning: Option<ReasoningEffort>,
    /// The context window to compact against, if known. Without one,
    /// only an overflow compacts.
    window: Option<u64>,
    /// The latest compaction, whose summary opens the transcript.
    compacted: Option<Record>,
    /// A summary past the threshold failed: the run goes on without
    /// compacting, so a failing summary is not paid for every turn.
    off: bool,
}

#[async_trait]
impl PluginRun for CompactionRun {
    async fn rewrite_context(
        &mut self,
        view: &ContextView<'_>,
        ctx: &PluginCtx,
    ) -> Result<Option<Rewrite>, PluginError> {
        match view.trigger {
            Trigger::TurnEnd => {
                let due = !self.off
                    && self.window.is_some_and(|window| {
                        should_compact(view.tokens, window, &self.settings)
                    });
                if !due {
                    return Ok(None);
                }
                let result = self.compact(view, ctx).await;
                if result.is_err() {
                    self.off = true;
                }
                Ok(result?)
            }
            Trigger::Overflow => Ok(self.compact(view, ctx).await?),
        }
    }
}

impl CompactionRun {
    /// Replaces the transcript's older messages with a summary, followed
    /// by the kept messages. `None` when the cut keeps everything.
    async fn compact(
        &mut self,
        view: &ContextView<'_>,
        ctx: &PluginCtx,
    ) -> Result<Option<Rewrite>, CompactionError> {
        let transcript = view.transcript;
        // The transcript opens with the latest summary, unless another
        // plugin's rewrite has replaced it since.
        let summarized =
            usize::from(self.compacted.as_ref().is_some_and(|record| {
                transcript.first() == Some(&record.message())
            }));
        let Some(plan) =
            plan(transcript, summarized, self.settings.keep_recent_tokens)
        else {
            return Ok(None);
        };
        let previous = self
            .compacted
            .as_ref()
            .filter(|_| summarized == 1)
            .map(|record| record.summary.as_str());
        let mut files = self
            .compacted
            .as_ref()
            .filter(|_| summarized == 1)
            .map(Record::files)
            .unwrap_or_default();
        let history = &transcript[plan.history.clone()];
        files.extract_from_messages(history);
        let mut summary = if history.is_empty() {
            previous.unwrap_or("No prior history.").to_owned()
        } else {
            let request = build_summary_request(history, previous, None);
            self.summarize(request, ctx).await?
        };
        if let Some(prefix) = plan.turn_prefix.clone() {
            let prefix = &transcript[prefix];
            files.extract_from_messages(prefix);
            let request = build_turn_prefix_summary_request(prefix);
            let text = self.summarize(request, ctx).await?;
            summary = merge_split_turn_summary(&summary, &text);
        }
        let (read_files, modified_files) = files.file_lists();
        summary.push_str(&format_file_operations(&read_files, &modified_files));
        let record = Record {
            summary,
            tokens_before: view.tokens,
            read_files,
            modified_files,
            timestamp: ctx.now(),
        };
        let messages = std::iter::once(record.message())
            .chain(transcript[plan.kept_from..].iter().cloned())
            .collect();
        let details = serde_json::to_value(&record).expect("records serialize");
        self.compacted = Some(record);
        Ok(Some(Rewrite { messages, details }))
    }

    /// One summary request through [`PluginCtx::ask`]: its own session,
    /// the run's retry policy, its usage charged to the run.
    async fn summarize(
        &self,
        request: String,
        ctx: &PluginCtx,
    ) -> Result<String, CompactionError> {
        let settings = Settings {
            model: self.model.clone(),
            instructions: Some(SUMMARIZATION_SYSTEM_PROMPT.to_owned()),
            reasoning: self.reasoning,
            ..Settings::default()
        };
        let input = [Message::User(UserMessage {
            content: UserContent::Text(request),
            timestamp: ctx.now(),
        })];
        let message = ctx.ask(settings, &input).await?;
        check_summary(&message)
    }
}
