//! The watcher on the host: the agent plugin that asks (ADR 0032), and
//! the host half that builds it for the runs it is on for.

use async_trait::async_trait;
use tau_agent::{
    error::PluginError,
    plugin::{AskError, Plugin, PluginCtx, PluginRun, RequestView, RunPlan},
};
use tau_ai::{
    message::{Message, StopReason, UserContent, UserMessage, text_of},
    responses::request::{ReasoningEffort, Settings as Request},
};
use tau_ui_plugin::{HostCx, HostHalf, PluginInfo, RunCtx, RunKind, Seam};

use crate::{
    prompt,
    record::{NAME, Record},
    reply::{self, Reply},
    state::State,
    ui::{WatcherUi, settings::Settings},
};

/// How much of a tool's result the side request reads.
const RESULT_CHARS: usize = 2_000;

/// The watcher for the runs it is switched on for.
#[derive(Debug, Clone, Default)]
pub struct WatcherPlugin {
    /// The model that reads the run; the run's own when none.
    model: Option<String>,
}

impl WatcherPlugin {
    /// A watcher whose side requests go to `model`, or to each run's own.
    pub fn new(model: Option<String>) -> Self {
        Self { model }
    }
}

#[async_trait]
impl Plugin for WatcherPlugin {
    fn name(&self) -> &str {
        NAME
    }

    /// A message written while a note waits is written past it.
    async fn start(
        &self,
        plan: &mut RunPlan,
        ctx: &PluginCtx,
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        let state = State::from_records(plan.records());
        if state.unanswered() {
            ctx.publish(&Record::TypedPast).await;
        }
        Ok(Box::new(WatcherRun {
            model: self
                .model
                .clone()
                .unwrap_or_else(|| plan.model().to_owned()),
            last_check: None,
            noted: false,
        }))
    }
}

/// One run's part: the check at every 6th step.
struct WatcherRun {
    model: String,
    /// The step of the last check this run made, whatever it found.
    last_check: Option<u32>,
    /// Whether this run made a note.
    noted: bool,
}

/// The step the next request is: the assistant messages before it, one
/// more. Counted from the transcript, so it goes on across a chat's
/// runs.
pub fn steps_of(transcript: &[Message]) -> u32 {
    let replies = transcript
        .iter()
        .filter(|message| matches!(message, Message::Assistant(_)))
        .count();
    u32::try_from(replies).unwrap_or(u32::MAX).saturating_add(1)
}

#[async_trait]
impl PluginRun for WatcherRun {
    async fn before_request(
        &mut self,
        view: &RequestView<'_>,
        ctx: &PluginCtx,
    ) -> Result<Option<ReasoningEffort>, PluginError> {
        let steps = steps_of(view.transcript);
        // Cheap tests first: most requests are not a 6th step.
        if self.noted || !steps.is_multiple_of(crate::cadence::EVERY) {
            return Ok(None);
        }
        // As stored now: the person may have answered meanwhile.
        let state = match ctx.records().await {
            Ok(bodies) => State::from_records(&bodies),
            Err(error) => {
                eprintln!("{NAME}: records could not be read: {error}");
                return Ok(None);
            }
        };
        let last = self.last_check.max(state.last_check);
        let due =
            !state.waiting() && crate::cadence::due(steps, last, state.ignored);
        if !due {
            return Ok(None);
        }
        self.last_check = Some(steps);
        let request = Request {
            model: self.model.clone(),
            instructions: Some(prompt::INSTRUCTIONS.to_owned()),
            ..Request::default()
        };
        let input = [Message::User(UserMessage {
            content: UserContent::Text(prompt::input(
                &conversation(view.transcript),
                &state.seen(),
                &state.known(),
            )),
            timestamp: ctx.now(),
        })];
        let answer = match ctx.ask(request, &input).await {
            Ok(answer) if answer.stop_reason == StopReason::Stop => answer,
            Ok(answer) => {
                dropped(
                    ctx,
                    steps,
                    format!("the request ended: {:?}", answer.stop_reason),
                )
                .await;
                return Ok(None);
            }
            Err(AskError::Cancelled) => return Ok(None),
            Err(error) => {
                dropped(ctx, steps, error.to_string()).await;
                return Ok(None);
            }
        };
        match reply::parse(&answer.text()) {
            Ok(Reply::Nothing) => {}
            Ok(Reply::Note { tag, line, explain }) => {
                self.noted = true;
                ctx.publish(&Record::Noted {
                    step: steps,
                    tag,
                    line,
                    explain,
                })
                .await;
            }
            Err(error) => dropped(ctx, steps, error.to_string()).await,
        }
        Ok(None)
    }
}

async fn dropped(ctx: &PluginCtx, step: u32, reason: String) {
    ctx.publish(&Record::Dropped { step, reason }).await;
}

/// The conversation as the side request reads it: flat text, tool
/// results cut to their first [`RESULT_CHARS`] characters.
pub fn conversation(transcript: &[Message]) -> String {
    let mut parts = Vec::new();
    for message in transcript {
        match message {
            Message::User(user) => {
                let text = user.content.text();
                if !text.is_empty() {
                    parts.push(format!("[Person]: {text}"));
                }
            }
            Message::Assistant(assistant) => {
                let text = assistant.text();
                if !text.is_empty() {
                    parts.push(format!("[Agent]: {text}"));
                }
                for call in assistant.tool_calls() {
                    let arguments = call
                        .arguments
                        .iter()
                        .map(|(key, value)| format!("{key}={value}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    parts.push(format!(
                        "[Agent ran]: {}({arguments})",
                        call.name
                    ));
                }
            }
            Message::ToolResult(result) => {
                let text = text_of(&result.content);
                if text.is_empty() {
                    continue;
                }
                let cut: String = text.chars().take(RESULT_CHARS).collect();
                let more = text.chars().count().saturating_sub(RESULT_CHARS);
                parts.push(if more > 0 {
                    format!("[Result]: {cut}\n[... {more} more characters]")
                } else {
                    format!("[Result]: {cut}")
                });
            }
        }
    }
    parts.join("\n\n")
}

/// The watcher on the host.
#[derive(Debug, Clone, Copy, Default)]
pub struct WatcherHost;

impl HostHalf for WatcherHost {
    type Plugin = WatcherUi;
    type Host = ();

    /// Off unless switched on, and for a run of the person's, not a
    /// sub-agent's.
    async fn agent_plugins(
        &self,
        _host: &(),
        run: &RunCtx,
        settings: &Settings,
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        if !settings.enabled || run.kind == RunKind::SubAgent {
            return Ok(Vec::new());
        }
        Ok(vec![Box::new(WatcherPlugin::new(settings.model()))])
    }

    async fn catalog(
        &self,
        _host: &(),
        _cx: &HostCx,
        settings: &Settings,
    ) -> PluginInfo {
        PluginInfo {
            group: tau_ui_plugin::Group::Context,
            description: "Every 6th step a side request reads the run and, \
                          rarely, offers one note you likely missed"
                .into(),
            seams: vec![Seam::Start],
            note: Some(tau_ui_plugin::Note::new(
                if settings.enabled { "on" } else { "off" },
                tau_ui_kit::theme::Tone::Quiet,
            )),
            page: None,
            ..Default::default()
        }
    }
}
