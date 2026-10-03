//! The agent half: the `ask` tool, which holds its call open until the
//! person answers, and the plugin that adds it.
//!
//! The host keeps one [`Waiting`] for every run: the calls waiting for an
//! answer, by run and call id. The tool publishes [`Record::Asked`] and
//! waits; the UI's answer reaches [`Waiting::answer`] through the
//! plugin's `act`, and the tool returns it. A cancelled run ends the wait
//! and publishes [`Record::Closed`].

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use serde_json::Value;
use tau_agent::{
    error::ToolError,
    plugin::{Plugin, PluginCtx, PluginError, PluginRun, RunPlan},
    tool::{AgentTool, Exposure, ToolCtx, ToolOutput},
};
use tokio::sync::oneshot;

use crate::{Ask, NAME, Record, Reply, TOOL};

/// What the tool tells the model about itself.
pub const DESCRIPTION: &str = "Ask the person one to four questions and wait for \
their answers. Use it when you need a decision only they can make: a preference, \
a trade-off, which of several plans to follow, or something the request leaves \
open that you cannot settle from the code or a sensible default. Do not use it to \
ask whether to go on or whether a plan is ready.

Each question has a short header (at most 12 characters, shown as a tab) and two \
to four choices, each a short label and a description. The person can always \
write their own answer instead, so never offer an \"Other\". If you recommend a \
choice, put it first and end its label with \"(Recommended)\". Set multi_select \
when several choices can hold at once. A choice may carry a preview (Markdown, \
shown in a monospace box) when the person should compare concrete things, such as \
code or layouts; previews are only for questions where one choice is picked.

The result gives each question's answer and any note the person added to it, or \
says they declined.";

/// The calls waiting for an answer, by run and call id.
#[derive(Clone, Default)]
pub struct Waiting(Arc<Mutex<BTreeMap<(String, String), Pending>>>);

/// A call waiting: what it asked, and where its reply goes.
struct Pending {
    ask: Ask,
    reply: oneshot::Sender<Reply>,
}

/// Why an answer could not be given.
#[derive(Debug)]
pub struct NotWaiting;

impl std::fmt::Display for NotWaiting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "This question is no longer waiting: its run ended or was cancelled.",
        )
    }
}

impl std::error::Error for NotWaiting {}

impl Waiting {
    fn wait(
        &self,
        run: &str,
        call: &str,
        ask: Ask,
    ) -> oneshot::Receiver<Reply> {
        let (reply, receive) = oneshot::channel();
        self.lock()
            .insert((run.to_owned(), call.to_owned()), Pending { ask, reply });
        receive
    }

    fn forget(&self, run: &str, call: &str) {
        self.lock().remove(&(run.to_owned(), call.to_owned()));
    }

    /// Gives the call `call` of `run` its reply, if the reply answers
    /// what it asked.
    pub fn answer(
        &self,
        run: &str,
        call: &str,
        reply: Reply,
    ) -> anyhow::Result<()> {
        let mut waiting = self.lock();
        let key = (run.to_owned(), call.to_owned());
        let pending = waiting.get(&key).ok_or(NotWaiting)?;
        reply.check(&pending.ask).map_err(anyhow::Error::msg)?;
        let pending = waiting.remove(&key).expect("found above");
        pending.reply.send(reply).map_err(|_| NotWaiting)?;
        Ok(())
    }

    /// Whether the call `call` of `run` waits.
    pub fn waits(&self, run: &str, call: &str) -> bool {
        self.lock().contains_key(&(run.to_owned(), call.to_owned()))
    }

    fn lock(
        &self,
    ) -> std::sync::MutexGuard<'_, BTreeMap<(String, String), Pending>> {
        self.0.lock().expect("not poisoned")
    }
}

/// What `ask` answers on a sub-agent, which no one watches.
pub const NO_ONE_TO_ASK: &str = "A sub-agent has no one to ask: decide \
    yourself, and say in your answer what you assumed and why.";

/// tau-ask's agent plugin: the `ask` tool, and at a run's start, the
/// calls an earlier run left waiting closed.
pub struct AskPlugin {
    waiting: Waiting,
    refuses: bool,
}

impl AskPlugin {
    pub fn new(waiting: Waiting) -> Self {
        Self {
            waiting,
            refuses: false,
        }
    }

    /// For a sub-agent, which has no one to ask: `ask` is declared all
    /// the same, so the run's tools match its caller's and it can read
    /// their prompt cache (ADR 0022), and every call fails with
    /// [`NO_ONE_TO_ASK`].
    pub fn refusing(mut self) -> Self {
        self.refuses = true;
        self
    }
}

#[async_trait]
impl Plugin for AskPlugin {
    fn name(&self) -> &str {
        NAME
    }

    fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        let mut tool = AskTool::new(self.waiting.clone());
        tool.refuses = self.refuses;
        vec![Arc::new(tool)]
    }

    /// Closes the calls the run's history left waiting: a run that went
    /// on from there cannot answer them any more.
    async fn start(
        &self,
        plan: &mut RunPlan,
        ctx: &PluginCtx,
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        let mut open: Vec<String> = Vec::new();
        for record in plan.records().iter().filter_map(Record::parse) {
            match record {
                Record::Asked { call, .. } => open.push(call),
                Record::Answered { call, .. } | Record::Closed { call } => {
                    open.retain(|c| *c != call)
                }
            }
        }
        for call in open {
            ctx.publish(&Record::Closed { call }).await;
        }
        Ok(Box::new(()))
    }
}

/// The `ask` tool.
pub struct AskTool {
    waiting: Waiting,
    parameters: Value,
    /// Every call fails with [`NO_ONE_TO_ASK`].
    refuses: bool,
}

impl AskTool {
    pub fn new(waiting: Waiting) -> Self {
        let parameters = serde_json::to_value(schemars::schema_for!(Ask))
            .expect("a generated schema is valid JSON");
        Self {
            waiting,
            parameters,
            refuses: false,
        }
    }
}

/// While a call waits: forgets it when the tool's future ends, however
/// it ends, and closes it unless the call said how it ended. A future
/// dropped mid-wait (its run aborted) closes it too: the report goes out
/// now, and the record is stored in the background.
struct Waits<'a> {
    waiting: &'a Waiting,
    plugin: PluginCtx,
    run: String,
    call: String,
    ended: bool,
}

impl Waits<'_> {
    async fn end(&mut self, record: Record) {
        self.ended = true;
        self.plugin.publish(&record).await;
    }
}

impl Drop for Waits<'_> {
    fn drop(&mut self) {
        self.waiting.forget(&self.run, &self.call);
        if self.ended {
            return;
        }
        let closed = Record::Closed {
            call: self.call.clone(),
        }
        .to_value();
        self.plugin.report(closed.clone());
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let plugin = self.plugin.clone();
            runtime.spawn(async move {
                if let Err(error) = plugin.record(&closed).await {
                    tracing::warn!(%error, "tau-ask could not store a closed call");
                }
            });
        }
    }
}

#[async_trait]
impl AgentTool for AskTool {
    fn name(&self) -> &str {
        TOOL
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    /// Only the model asks: a script's call would be dropped when the
    /// script ends, with the person still answering.
    fn exposure(&self) -> Exposure {
        Exposure::ModelOnly
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        if self.refuses {
            return Err(NO_ONE_TO_ASK.into());
        }
        let ask: Ask = serde_json::from_value(args)?;
        ask.check().map_err(ToolError::Message)?;
        let Some(plugin) = ctx.plugin() else {
            return Err(
                "internal error: ask was called outside tau-ask's run".into()
            );
        };
        let (run, call) = (ctx.run.0.to_string(), ctx.call_id().to_owned());
        let answer = self.waiting.wait(&run, &call, ask.clone());
        let mut waits = Waits {
            waiting: &self.waiting,
            plugin: plugin.clone(),
            run,
            call: call.clone(),
            ended: false,
        };
        plugin
            .publish(&Record::Asked {
                call: call.clone(),
                ask: ask.clone(),
            })
            .await;
        // A plugin's reports go out with the run's next event, and none
        // comes while the call waits: this update carries the question
        // to the interface now.
        ctx.updates
            .send(ToolOutput::text("Waiting for the person to answer."));
        let reply = tokio::select! {
            reply = answer => reply.ok(),
            () = ctx.cancel.cancelled() => None,
        };
        let Some(reply) = reply else {
            waits.end(Record::Closed { call }).await;
            return Err(
                "The run was cancelled before the person answered.".into()
            );
        };
        waits
            .end(Record::Answered {
                call,
                reply: reply.clone(),
            })
            .await;
        let details = serde_json::to_value(&reply)?;
        let mut output = ToolOutput::text(reply.text(&ask));
        output.details = Some(details.clone());
        output.structured = Some(details);
        Ok(output)
    }
}
