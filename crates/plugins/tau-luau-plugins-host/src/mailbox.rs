//! A model's mailbox (ADR 0056): its actor, taking one message at a
//! time, and the calls that drive hooks to their end.
//!
//! A call delivers its hook, then, while the hook awaits, carries out
//! the command it awaits outside the mailbox and delivers the answer, in
//! a later step. Between two of its steps any other message may be taken:
//! an action, another hook, a job's tick. What a step gives is carried
//! out before the next message is taken, so a step and its writes are
//! one. A call cancelled while its hook awaits delivers the cancel, which
//! the hook's await raises.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::{
    actor::{Actor, Answer, Ended, Message, Step},
    runtime::Executor,
};

/// What keeps what a mailbox does: each message as it is taken, each
/// step, and the model when no hook is under way.
#[async_trait]
pub trait Keep: Send + Sync {
    /// `message`, about to be taken.
    async fn taken(&self, _message: &Message) {}
    /// What a step did, once its commands were given.
    async fn stepped(&self, _step: &Step) {}
    /// The model, with no hook under way: what a restart starts from.
    async fn quiet(&self, _model: &Value) {}
    /// The commands a step gave could not be carried out.
    async fn not_given(&self, _why: &str) {}
}

/// Keeps nothing.
pub struct KeepNothing;

impl Keep for KeepNothing {}

/// A model's actor, and the hooks it drives.
pub struct Mailbox {
    actor: tokio::sync::Mutex<Actor>,
    keep: Arc<dyn Keep>,
    next: AtomicU64,
}

impl std::fmt::Debug for Mailbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mailbox").finish_non_exhaustive()
    }
}

impl Mailbox {
    pub fn new(actor: Actor, keep: Arc<dyn Keep>) -> Self {
        Self {
            actor: tokio::sync::Mutex::new(actor),
            keep,
            next: AtomicU64::new(1),
        }
    }

    /// Runs the hook at `path` with `input` and `env` to its end: each of
    /// its steps may run for `limit`, and what it awaits is carried out by
    /// `executor`. A cancel delivers itself to the hook where it awaits.
    pub async fn call(
        &self,
        path: Vec<String>,
        input: Value,
        env: Value,
        executor: &Executor,
        limit: Duration,
        cancel: &CancellationToken,
    ) -> Ended {
        let id = format!("h{}", self.next.fetch_add(1, Ordering::SeqCst));
        let message = Message::Call {
            id: id.clone(),
            path,
            input,
            env,
        };
        self.drive(&id, message, executor, limit, cancel).await
    }

    /// Takes `message` for the hook `id`, then what it awaits, to its end.
    async fn drive(
        &self,
        id: &str,
        mut message: Message,
        executor: &Executor,
        limit: Duration,
        cancel: &CancellationToken,
    ) -> Ended {
        let id = id.to_owned();
        loop {
            let waits = match self.step(&message, executor, limit).await {
                Ok(step) => {
                    if let Some(ended) = step.ended.get(&id) {
                        return ended.clone();
                    }
                    step.waits.get(&id).cloned()
                }
                Err(error) => return Ended::Error { error },
            };
            let Some(command) = waits else {
                return Ended::Error {
                    error: "the hook neither ended nor awaits".into(),
                };
            };
            message = tokio::select! {
                biased;
                () = cancel.cancelled() => Message::Cancel {
                    id: id.clone(),
                    why: "it was cancelled".into(),
                },
                answer = executor.run(&command) => answer.to(&id),
            };
        }
    }

    /// Takes `message` in one step, and gives what it gave.
    async fn step(
        &self,
        message: &Message,
        executor: &Executor,
        limit: Duration,
    ) -> Result<Step, String> {
        let actor = self.actor.lock().await;
        self.keep.taken(message).await;
        let step = actor.deliver(message, limit)?;
        if let Err(why) = executor.give(&step.given).await {
            self.keep.not_given(&why).await;
        }
        self.keep.stepped(&step).await;
        if actor.waiting()?.is_empty() {
            self.keep.quiet(&step.model).await;
        }
        Ok(step)
    }

    /// What the drawing at `path` draws from the model as it is.
    pub async fn draw(
        &self,
        path: &[String],
        input: Option<&Value>,
        env: &Value,
        limit: Duration,
    ) -> Result<Value, String> {
        self.actor.lock().await.draw(path, input, env, limit)
    }

    /// The model as it is.
    pub async fn model(&self) -> Result<Value, String> {
        self.actor.lock().await.model()
    }

    /// The mailbox of `actor`, made from the model it kept when no hook
    /// was under way, after taking again each of `log`, the messages taken
    /// since, in order: the same steps, so the hooks under way then are
    /// under way again, awaiting the same commands. What those steps gave
    /// was carried out already, and is not again; what they did is not
    /// kept again.
    pub fn replayed(
        actor: Actor,
        log: &[Message],
        keep: Arc<dyn Keep>,
        limit: Duration,
    ) -> Result<Self, String> {
        let mut highest = 0;
        for message in log {
            actor.deliver(message, limit)?;
            if let Some(n) = message
                .id()
                .strip_prefix('h')
                .and_then(|n| n.parse::<u64>().ok())
            {
                highest = highest.max(n);
            }
        }
        let mailbox = Self::new(actor, keep);
        mailbox.next.store(highest + 1, Ordering::SeqCst);
        Ok(mailbox)
    }

    /// The hooks under way, by id, and what each awaits.
    pub async fn waiting(
        &self,
    ) -> Result<std::collections::BTreeMap<String, crate::actor::Command>, String>
    {
        self.actor.lock().await.waiting()
    }

    /// Goes on with the hook `id`, under way with no call to drive it (as
    /// after a replay): carries out what it awaits, and what it awaits
    /// next, to its end.
    pub async fn resume(
        &self,
        id: &str,
        executor: &Executor,
        limit: Duration,
        cancel: &CancellationToken,
    ) -> Ended {
        let Some(command) = self
            .waiting()
            .await
            .ok()
            .and_then(|mut waiting| waiting.remove(id))
        else {
            return Ended::Error {
                error: format!("no hook {id} awaits"),
            };
        };
        let answer = executor.run(&command).await;
        self.drive(id, answer.to(id), executor, limit, cancel).await
    }

    /// The answer `answer` to what `id` awaits, from outside a call: a
    /// person's answers to a form a hook asked, say.
    pub async fn answer(
        &self,
        id: &str,
        answer: Answer,
        executor: &Executor,
        limit: Duration,
    ) -> Result<Step, String> {
        self.step(&answer.to(id), executor, limit).await
    }
}
