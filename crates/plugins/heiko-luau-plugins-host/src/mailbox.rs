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
//!
//! The VM's part of a step runs on a thread that may block, with the
//! actor's lock held (ADR 0028): a plugin that keeps the CPU keeps that
//! thread and its own next steps, and nothing else of the host.

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
    /// What a step did, once its commands were given; what the model's
    /// drawing drew of it, when it changed the model and the mailbox
    /// draws; and where the call it is of places what it keeps.
    async fn stepped(
        &self,
        _step: &Step,
        _drawn: Option<Result<Value, String>>,
        _place: Option<&str>,
    ) {
    }
    /// Whether the mailbox draws after a step that changed the model.
    fn draws(&self) -> bool {
        true
    }
    /// The model, with no hook under way: what a restart starts from.
    async fn quiet(&self, _model: &Value) {}
    /// A step ended with a hook under way: what it took is a restart's to
    /// take again.
    async fn busy(&self) {}
    /// The commands a step gave could not be carried out.
    async fn not_given(&self, _why: &str) {}
}

/// Keeps nothing.
pub struct KeepNothing;

impl Keep for KeepNothing {}

/// Keeps the lines steps log, for whoever called to take.
#[derive(Default)]
pub struct KeepLogs(std::sync::Mutex<Vec<String>>);

impl KeepLogs {
    /// The lines logged since last taken.
    pub fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().expect("not poisoned"))
    }
}

#[async_trait]
impl Keep for KeepLogs {
    async fn stepped(
        &self,
        step: &Step,
        _drawn: Option<Result<Value, String>>,
        _place: Option<&str>,
    ) {
        self.0
            .lock()
            .expect("not poisoned")
            .extend(step.logs.iter().cloned());
    }
}

/// A model's actor, and the hooks it drives.
pub struct Mailbox {
    /// Owned by the lock: a step takes its guard to a thread that may
    /// block, as ADR 0028 has a blocking resource with state.
    actor: Arc<tokio::sync::Mutex<Actor>>,
    keep: Arc<dyn Keep>,
    next: AtomicU64,
    /// What it draws after a step that changed the model, and the `env`
    /// the drawing gets.
    drawing: Option<Arc<(Vec<String>, Value)>>,
    /// Where each call under way places what it keeps, by hook id.
    places: std::sync::Mutex<std::collections::HashMap<String, String>>,
}

/// How long a drawing may take.
const DRAW_LIMIT: Duration = Duration::from_millis(200);

/// Runs `work` on the actor on a thread that may block, and gives the
/// actor back with what it made: a VM is busy with the CPU while it
/// runs, for as long as its deadline lets it, and the runtime's threads
/// are for what waits (ADR 0028). The lock stays held meanwhile, so
/// steps still go one at a time, in the order they asked.
async fn on_vm<T: Send + 'static>(
    actor: tokio::sync::OwnedMutexGuard<Actor>,
    work: impl FnOnce(&Actor) -> T + Send + 'static,
) -> Result<(tokio::sync::OwnedMutexGuard<Actor>, T), String> {
    tokio::task::spawn_blocking(move || {
        let made = work(&actor);
        (actor, made)
    })
    .await
    .map_err(|error| format!("the plugin's VM stopped: {error}"))
}

impl std::fmt::Debug for Mailbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mailbox").finish_non_exhaustive()
    }
}

impl Mailbox {
    pub fn new(actor: Actor, keep: Arc<dyn Keep>) -> Self {
        Self {
            actor: Arc::new(tokio::sync::Mutex::new(actor)),
            keep,
            next: AtomicU64::new(1),
            drawing: None,
            places: Default::default(),
        }
    }

    /// Draws `path` with `env` after each step that changes the model.
    pub fn drawing(mut self, path: Vec<String>, env: Value) -> Self {
        self.drawing = Some(Arc::new((path, env)));
        self
    }

    /// Runs the hook at `path` with `input` and `env` to its end: each of
    /// its steps may run for `limit`, and what it awaits is carried out by
    /// `executor`. A cancel delivers itself to the hook where it awaits.
    #[allow(clippy::too_many_arguments, reason = "one hook of one model")]
    pub async fn call(
        &self,
        path: Vec<String>,
        input: Value,
        env: Value,
        executor: &Executor,
        limit: Duration,
        cancel: &CancellationToken,
        place: Option<&str>,
    ) -> Ended {
        self.call_kept(path, input, env, executor, limit, cancel, place)
            .await
            .0
    }

    /// [`Self::call`], with the model as the step that ended the hook
    /// left it, when a step did: what this hook kept, whatever another
    /// message changes right after.
    #[allow(clippy::too_many_arguments, reason = "one hook of one model")]
    pub async fn call_kept(
        &self,
        path: Vec<String>,
        input: Value,
        env: Value,
        executor: &Executor,
        limit: Duration,
        cancel: &CancellationToken,
        place: Option<&str>,
    ) -> (Ended, Option<Value>) {
        let id = format!("h{}", self.next.fetch_add(1, Ordering::SeqCst));
        if let Some(place) = place {
            self.places
                .lock()
                .expect("not poisoned")
                .insert(id.clone(), place.to_owned());
        }
        let message = Message::Call {
            id: id.clone(),
            path,
            input,
            env,
        };
        let ended = self.drive(&id, message, executor, limit, cancel).await;
        self.places.lock().expect("not poisoned").remove(&id);
        ended
    }

    /// Takes `message` for the hook `id`, then what it awaits, to its end.
    async fn drive(
        &self,
        id: &str,
        mut message: Message,
        executor: &Executor,
        limit: Duration,
        cancel: &CancellationToken,
    ) -> (Ended, Option<Value>) {
        let id = id.to_owned();
        loop {
            let waits = match self.step(&message, executor, limit).await {
                Ok(mut step) => {
                    if let Some(ended) = step.ended.remove(&id) {
                        return (ended, Some(step.model));
                    }
                    // What it gave did not go in: its await raises why,
                    // rather than going on as if it had.
                    if let (Some(why), true) =
                        (&step.not_given, step.waits.contains_key(&id))
                    {
                        message = Answer::failed(why.clone()).to(&id);
                        continue;
                    }
                    step.waits.get(&id).cloned()
                }
                Err(error) => return (Ended::Error { error }, None),
            };
            let Some(command) = waits else {
                let error = "the hook neither ended nor awaits".into();
                return (Ended::Error { error }, None);
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
        let actor = self.actor.clone().lock_owned().await;
        self.keep.taken(message).await;
        // The VM's part of the step, whole, off the runtime's threads:
        // the message taken, the model drawn if it changed, and whether
        // a hook is still under way.
        let drawing = self.drawing.clone().filter(|_| self.keep.draws());
        let taken = message.clone();
        let (_actor, ran) = on_vm(actor, move |actor| {
            let step = actor.deliver(&taken, limit)?;
            let drawn = drawing.filter(|_| step.changed).map(|drawing| {
                let (path, env) = &*drawing;
                actor.draw(path, None, env, DRAW_LIMIT)
            });
            let quiet = actor.waiting()?.is_empty();
            Ok::<_, String>((step, drawn, quiet))
        })
        .await?;
        let (mut step, drawn, quiet) = ran?;
        if let Err(why) = executor.give(&step.given).await {
            self.keep.not_given(&why).await;
            // A step is one with its writes (ADR 0056): a hook it ended
            // did not end well when they did not go in.
            let why = format!("what the step gave was not carried out: {why}");
            for ended in step.ended.values_mut() {
                *ended = Ended::Error { error: why.clone() };
            }
            step.not_given = Some(why);
        }
        let place = self
            .places
            .lock()
            .expect("not poisoned")
            .get(message.id())
            .cloned();
        self.keep.stepped(&step, drawn, place.as_deref()).await;
        if quiet {
            self.keep.quiet(&step.model).await;
        } else {
            self.keep.busy().await;
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
        let actor = self.actor.clone().lock_owned().await;
        let (path, input, env) = (path.to_vec(), input.cloned(), env.clone());
        let (_actor, drawn) = on_vm(actor, move |actor| {
            actor.draw(&path, input.as_ref(), &env, limit)
        })
        .await?;
        drawn
    }

    /// The model as it is.
    pub async fn model(&self) -> Result<Value, String> {
        let actor = self.actor.clone().lock_owned().await;
        on_vm(actor, |actor| actor.model()).await?.1
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
        let actor = self.actor.clone().lock_owned().await;
        on_vm(actor, |actor| actor.waiting()).await?.1
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
        self.drive(id, answer.to(id), executor, limit, cancel)
            .await
            .0
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
