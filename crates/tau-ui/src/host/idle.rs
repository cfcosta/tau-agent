//! Compacting an idle chat before its prompt cache lapses
//! (`docs/reference/compaction.md`, "Idle compaction"): as Claude Code
//! does, a long chat left idle is summarized shortly before the cache
//! that holds it lapses, in its own conversation, so the summary reads
//! the chat from cache, and the person's next message starts from it
//! instead of resending everything uncached.

use std::time::Duration;

use tau_agent::agent::Compacted;
use tau_ai::{
    client::OpenAi,
    responses::request::Settings,
    ws::io::driver::CacheLapse,
};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::*;

/// When, between the last request and the lapse, an idle chat is
/// compacted: Claude Code's default.
const FIRE_AT: f64 = 0.9;

/// How late a timer may fire and still compact: past it, the cache may
/// have lapsed, and compacting would resend the chat uncached (Claude
/// Code's).
const LATE: Duration = Duration::from_secs(60);

/// What the host knows of a chat between its turns, to compact it while
/// it is idle.
#[derive(Debug, Default)]
pub(super) struct Idle {
    /// Counts the chat's turn ends: a timer set at an earlier one is
    /// stale.
    generation: u64,
    /// The settings its last request went with, which the compaction
    /// sends with to read its cache.
    request: Option<Settings>,
    /// The context its last request held, with what it answered.
    tokens: u64,
    /// The compaction going on: how to stop it, and what it holds until
    /// it stopped.
    compacting: Option<(CancellationToken, Arc<tokio::sync::Mutex<()>>)>,
}

/// When to compact an idle chat whose cache lapses as `lapse` says:
/// [`FIRE_AT`] of the way from its last request to the lapse.
pub fn fire_at(lapse: &CacheLapse) -> Instant {
    lapse.last_used
        + lapse
            .at
            .saturating_duration_since(lapse.last_used)
            .mul_f64(FIRE_AT)
}

/// Whether a chat whose last request held `tokens`, on a model with
/// `window`, is worth compacting while idle: half the window. A smaller
/// one costs little to resend, and would soon lose to a summary what
/// the person comes back for.
pub fn worth_compacting(tokens: u64, window: u64) -> bool {
    tokens >= window / 2
}

impl Host {
    /// Notes what a turn of `run` left: the context its request held,
    /// with the reply.
    pub(super) fn turn_ended(
        idle: &Mutex<HashMap<RunId, Idle>>,
        run: &RunId,
        usage: &tau_ai::message::Usage,
    ) {
        idle.lock()
            .expect("not poisoned")
            .entry(run.clone())
            .or_default()
            .tokens =
            usage.input + usage.cache_read + usage.cache_write + usage.output;
    }

    /// Notes the settings `run`'s last request went with.
    pub(super) fn run_ended(
        idle: &Mutex<HashMap<RunId, Idle>>,
        run: &RunId,
        request: Settings,
    ) {
        idle.lock()
            .expect("not poisoned")
            .entry(run.clone())
            .or_default()
            .request = Some(request);
    }

    /// Watches `run`, a chat whose turn just ended, while it is idle: if
    /// it is worth it, it is compacted shortly before its prompt cache
    /// lapses, unless it goes on first. A later turn's end starts a new
    /// watch.
    pub fn watch_idle(self: &Arc<Self>, run: &RunId) {
        if self.is_sub_agent(run) {
            return;
        }
        let generation = {
            let mut idle = self.idle.lock().expect("not poisoned");
            let entry = idle.entry(run.clone()).or_default();
            entry.generation += 1;
            entry.generation
        };
        let (host, run) = (Arc::downgrade(self), run.clone());
        self.runtime.spawn(async move {
            let fire = {
                let Some(host) = host.upgrade() else {
                    return;
                };
                host.settle(&run).await;
                match host.idle_lapse(&run, generation).await {
                    Some(lapse) => fire_at(&lapse),
                    None => return,
                }
            };
            // The host is not held while the chat sleeps.
            tokio::time::sleep_until(fire).await;
            let Some(host) = host.upgrade() else {
                return;
            };
            // Its cache must still be there, the chat idle since.
            let Some(lapse) = host.idle_lapse(&run, generation).await else {
                return;
            };
            if Instant::now() > fire + LATE || Instant::now() >= lapse.at {
                return;
            }
            if let Err(error) =
                host.compact_watched(&run, Some(generation)).await
            {
                eprintln!(
                    "tau-ui: an idle chat could not be compacted: {error:#}"
                );
            }
        });
    }

    /// When `run`'s cache lapses, while the watch set at `generation` is
    /// the latest, the chat is idle, still takes messages, and is worth
    /// compacting. `None` otherwise.
    async fn idle_lapse(
        &self,
        run: &RunId,
        generation: u64,
    ) -> Option<CacheLapse> {
        let worth = {
            let idle = self.idle.lock().expect("not poisoned");
            let entry = idle.get(run)?;
            let window = find(&entry.request.as_ref()?.model)?.context_window;
            entry.generation == generation
                && worth_compacting(entry.tokens, window)
        };
        if !worth
            || self.is_running(run)
            || self.ending_of(run).await.ok()?.is_some()
        {
            return None;
        }
        let client: OpenAi =
            self.client.lock().expect("not poisoned").clone()?;
        client.cache_lapse(&run.0).await.ok()?
    }

    /// Compacts `run`, an idle chat, in its own conversation, sent as its
    /// last request was so it reads that request's prompt cache: what
    /// the plugins that take part make of it ([`tau_agent::plugin::Plugin::start_idle`]).
    /// Its events go to the interface like a run's. A message for it
    /// stops it ([`Self::stop_compacting`]). `None` when no plugin
    /// compacted it, or nothing was known of its last request.
    pub async fn compact_idle(
        &self,
        run: &RunId,
    ) -> anyhow::Result<Option<Compacted>> {
        self.compact_watched(run, None).await
    }

    /// [`Self::compact_idle`], for the watch set at `generation`, if
    /// any: unless a message came since.
    async fn compact_watched(
        &self,
        run: &RunId,
        generation: Option<u64>,
    ) -> anyhow::Result<Option<Compacted>> {
        let Some(request) = self
            .idle
            .lock()
            .expect("not poisoned")
            .get(run)
            .and_then(|idle| idle.request.clone())
        else {
            return Ok(None);
        };
        let cancel = CancellationToken::new();
        let held = Arc::new(tokio::sync::Mutex::new(()));
        let _held = held.clone().lock_owned().await;
        {
            let mut idle = self.idle.lock().expect("not poisoned");
            let entry = idle.entry(run.clone()).or_default();
            // A message that came since stops it before it starts: it
            // moved the generation on under this lock.
            if entry.compacting.is_some()
                || generation.is_some_and(|at| at != entry.generation)
            {
                return Ok(None);
            }
            entry.compacting = Some((cancel.clone(), held));
        }
        let compacted = self.compact_with(run, request, cancel).await;
        if let Some(idle) = self.idle.lock().expect("not poisoned").get_mut(run)
        {
            idle.compacting = None;
        }
        compacted
    }

    async fn compact_with(
        &self,
        run: &RunId,
        request: Settings,
        cancel: CancellationToken,
    ) -> anyhow::Result<Option<Compacted>> {
        let repo = self.slot_of_run(run).await?;
        let choice =
            ModelChoice::new(request.model.clone(), self.choice_of(run).effort);
        let agent = {
            let base = self.base.lock().expect("not poisoned").clone();
            for_model(
                base,
                &choice,
                HostRecord {
                    repo: repo.name.clone(),
                    ..HostRecord::default()
                },
            )
        };
        let kind = if self.is_main(run) {
            tau_ui_plugin::RunKind::Main
        } else {
            tau_ui_plugin::RunKind::Chat
        };
        let registered = self.registered(&repo).await;
        let agent =
            registered(agent, kind, &choice, Services::default()).await?;
        // Its events go where a run's do; a stopped compaction's failures
        // are only that it stopped.
        let (sender, mut receiver) = mpsc::channel(64);
        let (events, stopped) = (self.events.clone(), cancel.clone());
        let forward = self.runtime.spawn(async move {
            while let Some(event) = receiver.recv().await {
                if stopped.is_cancelled()
                    && matches!(event, RunEvent::PluginError { .. })
                {
                    continue;
                }
                let _ = events.send(event);
            }
        });
        let compacted = agent
            .resume(run)
            .compact_idle(request, &self.store, Some(sender), cancel.clone())
            .await;
        let _ = forward.await;
        match compacted {
            Err(_) if cancel.is_cancelled() => Ok(None),
            compacted => Ok(compacted?),
        }
    }

    /// A message for `run`: the watch set at its last turn's end is
    /// stale, and an idle compaction under way stops, and is waited for,
    /// so the chat goes on from its transcript as that left it, stored or
    /// not.
    pub(super) async fn stop_compacting(&self, run: &RunId) {
        let compacting = {
            let mut idle = self.idle.lock().expect("not poisoned");
            let entry = idle.entry(run.clone()).or_default();
            entry.generation += 1;
            entry.compacting.clone()
        };
        if let Some((cancel, held)) = compacting {
            cancel.cancel();
            let _ = held.lock().await;
        }
    }
}
