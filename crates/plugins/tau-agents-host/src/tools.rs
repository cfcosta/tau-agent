//! The tools an agent's runs have, and no other run (ADR 0037): `notify`
//! and `ask`, to reach the person without waiting for them,
//! `pending_asks`, what of that still waits for them, `agent_send`, to
//! reach another agent, and `schedule`, `scheduled` and `unschedule`,
//! to wake itself later.
//!
//! An agent works unattended, so its `ask` does not hold the turn open
//! as tau-ask's does (the host leaves tau-ask out of an agent's runs):
//! the question waits on its page, and the answer comes back as an
//! event in a later turn.

use std::sync::{
    Arc,
    OnceLock,
    atomic::{AtomicU64, Ordering},
};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tau_agent::{
    error::ToolError,
    plugin::{Plugin, PluginCtx, PluginError, PluginRun, RunPlan},
    tool::{AgentTool, ToolCtx, ToolOutput},
};

use crate::{Agents, MAX_SCHEDULED, NAME, Plan, hops_of};

/// The tools of one run of the agent `agent`.
pub struct AgentTools {
    agents: Agents,
    agent: String,
    /// How far a message this run sends goes ([`hops_of`]): set as it
    /// starts, from what woke it.
    hops: Arc<AtomicU64>,
}

impl AgentTools {
    pub fn new(agents: Agents, agent: String) -> Self {
        Self {
            agents,
            agent,
            hops: Arc::new(AtomicU64::new(1)),
        }
    }
}

#[async_trait]
impl Plugin for AgentTools {
    fn name(&self) -> &str {
        NAME
    }

    fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        let (agents, agent) = (self.agents.clone(), self.agent.clone());
        vec![
            Arc::new(Notify {
                agents: agents.clone(),
                agent: agent.clone(),
            }),
            Arc::new(Ask {
                agents: agents.clone(),
                agent: agent.clone(),
            }),
            Arc::new(Pending {
                agents: agents.clone(),
                agent: agent.clone(),
            }),
            Arc::new(Send {
                agents: agents.clone(),
                agent: agent.clone(),
                hops: self.hops.clone(),
            }),
            Arc::new(Schedule {
                agents: agents.clone(),
                agent: agent.clone(),
            }),
            Arc::new(Listed {
                agents: agents.clone(),
                agent: agent.clone(),
            }),
            Arc::new(Unschedule { agents, agent }),
        ]
    }

    /// Reads what woke the run: a turn that only messages woke sends its
    /// own one hop further.
    async fn start(
        &self,
        plan: &mut RunPlan,
        _ctx: &PluginCtx,
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        self.hops.store(hops_of(&plan.input), Ordering::Relaxed);
        Ok(Box::new(()))
    }
}

/// A tool's parameters, as JSON Schema, made once.
fn schema(
    cell: &'static OnceLock<Value>,
    make: fn() -> Value,
) -> &'static Value {
    cell.get_or_init(make)
}

/// `notify { text }`.
struct Notify {
    agents: Agents,
    agent: String,
}

#[derive(Deserialize)]
struct NotifyArgs {
    text: String,
}

#[async_trait]
impl AgentTool for Notify {
    fn name(&self) -> &str {
        "notify"
    }

    fn description(&self) -> &str {
        "Leave the person a note: something they should know, found while \
         they were away. It shows on your page and in their sidebar, and \
         as a notification on their desktop and phone, until they dismiss \
         it. Say it in a sentence or two; use it for what matters, not for \
         every step."
    }

    fn parameters(&self) -> &Value {
        static CELL: OnceLock<Value> = OnceLock::new();
        schema(&CELL, || {
            json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "The note." },
                },
                "required": ["text"],
                "additionalProperties": false,
            })
        })
    }

    async fn call(
        &self,
        args: Value,
        _ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let args: NotifyArgs = serde_json::from_value(args)?;
        self.agents
            .notify(&self.agent, &args.text)
            .await
            .map_err(|error| ToolError::Message(format!("{error:#}")))?;
        Ok(ToolOutput::text("Left for the person."))
    }
}

/// `ask { question, choices? }`.
struct Ask {
    agents: Agents,
    agent: String,
}

#[derive(Deserialize)]
struct AskArgs {
    question: String,
    #[serde(default)]
    choices: Vec<String>,
}

#[async_trait]
impl AgentTool for Ask {
    fn name(&self) -> &str {
        "ask"
    }

    fn description(&self) -> &str {
        "Ask the person something only they can decide. It does not wait: \
         the question shows on your page and as a notification, and their \
         answer reaches you later as an event (`ask · answer`, with the \
         question and the answer), in a turn of its own. Offer `choices` \
         when the answer is one of a few; leave them out for any answer. \
         Go on with what does not depend on it, or end your turn."
    }

    fn parameters(&self) -> &Value {
        static CELL: OnceLock<Value> = OnceLock::new();
        schema(&CELL, || {
            json!({
                "type": "object",
                "properties": {
                    "question": { "type": "string" },
                    "choices": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "The answers to offer, if it is \
                                        one of a few.",
                    },
                },
                "required": ["question"],
                "additionalProperties": false,
            })
        })
    }

    async fn call(
        &self,
        args: Value,
        _ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let args: AskArgs = serde_json::from_value(args)?;
        if let Some(id) = self.agents.asked(&self.agent, &args.question).await {
            return Ok(ToolOutput::text(format!(
                "You asked this already ({id}); it still waits for the \
                 person. Their answer comes as an event `ask · answer · {id}`."
            )));
        }
        let id = self
            .agents
            .ask(&self.agent, &args.question, args.choices)
            .await
            .map_err(|error| ToolError::Message(format!("{error:#}")))?;
        Ok(ToolOutput::text(format!(
            "Asked. The answer comes as an event `ask · answer · {id}`."
        )))
    }
}

/// `pending_asks {}`.
struct Pending {
    agents: Agents,
    agent: String,
}

#[async_trait]
impl AgentTool for Pending {
    fn name(&self) -> &str {
        "pending_asks"
    }

    fn description(&self) -> &str {
        "What you left the person that they have not dealt with yet: your \
         questions still waiting for an answer, and your notes they have \
         not dismissed. Look before you ask or tell them something again."
    }

    fn parameters(&self) -> &Value {
        static CELL: OnceLock<Value> = OnceLock::new();
        schema(&CELL, || {
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            })
        })
    }

    async fn call(
        &self,
        _args: Value,
        _ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let waiting = self.agents.waiting(&self.agent).await;
        if waiting.is_empty() {
            return Ok(ToolOutput::text("Nothing waits for the person."));
        }
        let lines: Vec<String> = waiting
            .iter()
            .map(|notice| match &notice.choices {
                Some(choices) if !choices.is_empty() => format!(
                    "- {} · question · {} (choices: {})",
                    notice.id,
                    notice.text,
                    choices.join(" / ")
                ),
                Some(_) => {
                    format!("- {} · question · {}", notice.id, notice.text)
                }
                None => format!("- {} · note · {}", notice.id, notice.text),
            })
            .collect();
        Ok(ToolOutput::text(lines.join("\n")))
    }
}

/// `agent_send { to, text }`.
struct Send {
    agents: Agents,
    agent: String,
    hops: Arc<AtomicU64>,
}

#[derive(Deserialize)]
struct SendArgs {
    to: String,
    text: String,
}

#[async_trait]
impl AgentTool for Send {
    fn name(&self) -> &str {
        "agent_send"
    }

    fn description(&self) -> &str {
        "Send another agent a message. It reaches that agent as a `message` \
         event, with the others in its next turn, under its own spend cap; \
         only an agent whose agent.luau declares a `message = true` trigger \
         hears messages. A chain of messages between agents that nothing \
         else wakes stops after a few hops: do not use messages to keep \
         each other going."
    }

    fn parameters(&self) -> &Value {
        static CELL: OnceLock<Value> = OnceLock::new();
        schema(&CELL, || {
            json!({
                "type": "object",
                "properties": {
                    "to": { "type": "string", "description": "The agent's name." },
                    "text": { "type": "string" },
                },
                "required": ["to", "text"],
                "additionalProperties": false,
            })
        })
    }

    async fn call(
        &self,
        args: Value,
        _ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let args: SendArgs = serde_json::from_value(args)?;
        let hops = self.hops.load(Ordering::Relaxed);
        self.agents
            .send(&self.agent, &args.to, &args.text, hops)
            .await
            .map_err(ToolError::Message)?;
        Ok(ToolOutput::text(format!("Sent to {}.", args.to)))
    }
}

/// `schedule { name, note, in | on | every | at, days? }`.
struct Schedule {
    agents: Agents,
    agent: String,
}

#[async_trait]
impl AgentTool for Schedule {
    fn name(&self) -> &str {
        "schedule"
    }

    fn description(&self) -> &str {
        "Wake yourself later, with a note to yourself for then: once (`in` \
         a while, or `on` a local date and time), or again and again \
         (`every` interval, or `at` a local time on some `days`). It \
         reaches you as an event from the trigger `name`, of kind \
         `scheduled`, with your note. Scheduling a name again replaces it. \
         Use it for what comes up: following something up, checking back, \
         a routine you took on. What should always wake you belongs in \
         agent.luau. The person sees what you scheduled on your page and \
         may cancel it."
    }

    fn parameters(&self) -> &Value {
        static CELL: OnceLock<Value> = OnceLock::new();
        schema(&CELL, || {
            json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Lower-case letters, digits, `-` \
                                        and `_`: `check-pr-412`.",
                    },
                    "note": {
                        "type": "string",
                        "description": "What to do then, for you to read \
                                        when it comes.",
                    },
                    "in": {
                        "type": "string",
                        "description": "Once, after this long: `90m`, \
                                        `2h`, `3d`.",
                    },
                    "on": {
                        "type": "string",
                        "description": "Once, at this local date and \
                                        time: `2026-10-13 09:00`.",
                    },
                    "every": {
                        "type": "string",
                        "description": "Again and again, every interval: \
                                        `15m`, `6h`.",
                    },
                    "at": {
                        "type": "string",
                        "description": "Again and again, at this local \
                                        time: `09:00`.",
                    },
                    "days": {
                        "type": "array",
                        "items": {
                            "enum": ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
                        },
                        "description": "With `at`, the days; none is \
                                        every day.",
                    },
                },
                "required": ["name", "note"],
                "additionalProperties": false,
            })
        })
    }

    async fn call(
        &self,
        args: Value,
        _ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let plan: Plan = serde_json::from_value(args)?;
        let made = self
            .agents
            .schedule(&self.agent, plan)
            .await
            .map_err(ToolError::Message)?;
        let scheduled = &made.scheduled;
        Ok(ToolOutput::text(format!(
            "{} `{}`: {}, next {}. It comes as an event `{} · scheduled`.",
            if made.replaced {
                "Replaced"
            } else {
                "Scheduled"
            },
            scheduled.name,
            scheduled.when.label(),
            self.agents.local_time(scheduled.next),
            scheduled.name,
        )))
    }
}

/// `scheduled {}`.
struct Listed {
    agents: Agents,
    agent: String,
}

#[async_trait]
impl AgentTool for Listed {
    fn name(&self) -> &str {
        "scheduled"
    }

    fn description(&self) -> &str {
        "What you scheduled for yourself with `schedule` that is still to \
         come, soonest first, and the host's local time now. Look before \
         you schedule something again."
    }

    fn parameters(&self) -> &Value {
        static CELL: OnceLock<Value> = OnceLock::new();
        schema(&CELL, || {
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            })
        })
    }

    async fn call(
        &self,
        _args: Value,
        _ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let scheduled = self.agents.scheduled(&self.agent).await;
        let now = self.agents.local_time(self.agents.now());
        if scheduled.is_empty() {
            return Ok(ToolOutput::text(format!(
                "It is {now}. Nothing is scheduled."
            )));
        }
        let mut lines = vec![format!(
            "It is {now}. {} of the {MAX_SCHEDULED} you may have:",
            scheduled.len()
        )];
        lines.extend(scheduled.iter().map(|scheduled| {
            format!(
                "- {} · {} · next {} · {}",
                scheduled.name,
                scheduled.when.label(),
                self.agents.local_time(scheduled.next),
                scheduled.note.lines().next().unwrap_or_default(),
            )
        }));
        Ok(ToolOutput::text(lines.join("\n")))
    }
}

/// `unschedule { name }`.
struct Unschedule {
    agents: Agents,
    agent: String,
}

#[derive(Deserialize)]
struct UnscheduleArgs {
    name: String,
}

#[async_trait]
impl AgentTool for Unschedule {
    fn name(&self) -> &str {
        "unschedule"
    }

    fn description(&self) -> &str {
        "Cancel what you scheduled for yourself as `name`."
    }

    fn parameters(&self) -> &Value {
        static CELL: OnceLock<Value> = OnceLock::new();
        schema(&CELL, || {
            json!({
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "required": ["name"],
                "additionalProperties": false,
            })
        })
    }

    async fn call(
        &self,
        args: Value,
        _ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let args: UnscheduleArgs = serde_json::from_value(args)?;
        Ok(ToolOutput::text(
            if self.agents.unschedule(&self.agent, &args.name).await {
                format!("Cancelled `{}`.", args.name)
            } else {
                format!("Nothing is scheduled as `{}`.", args.name)
            },
        ))
    }
}
