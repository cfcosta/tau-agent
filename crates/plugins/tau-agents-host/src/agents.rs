//! The agents on this host (ADR 0037): what wakes each one, the events
//! that wait for its next turn, and the turns that hear them.
//!
//! Every second each agent's schedules that came due fire, and its
//! queue is looked at. Its events wait a coalescing window from the
//! oldest, then go to its conversation as one message from tau, unless
//! a turn of it is going (they wait for the next), the person paused it,
//! or it spent its day's cap. An event leaves the queue once the turn
//! that hears it started: tau closed before then delivers it again.
//!
//! Polls run beside the ticks, each in a task of its own, so a slow one
//! holds up nothing else; what one finds joins the queue when it ends.
//! Streams are kept open in tasks of their own (`streams`), one per
//! stream a live version declares, and so are watches on MCP resources
//! (`resources`), one per `mcp` trigger.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use serde_json::{Value, json};
use tau_agents::{
    Act,
    Agent,
    Last,
    Overview,
    SKILL,
    SecretState,
    Settings,
    Status,
    Trigger,
    TriggerKind,
    event::{Batch, Event, Kind},
};
use tau_luau_plugins_host::runtime::{evaluate, long_string};
use tau_ui_plugin::{By, HostCx, PluginHost, Turn};
use tokio::{
    sync::{Mutex, mpsc},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

use crate::{
    NAME,
    ROOT,
    clock::{self, Clock, SystemClock},
    env::{Env, HostEnv},
    poll::{self, Polled},
    registry::{Read, Registry},
    schedule::Schedule,
    state::{Due, Kept, Store},
};

pub(crate) mod talk;

mod resources;
mod streams;

/// How often schedules and queues are looked at.
const TICK: Duration = Duration::from_secs(1);

/// How long a turn that did not start waits to be tried again.
const RETRY: i64 = 30;

/// How long a day's spend, as counted, is good for.
const SPEND_FOR: i64 = 30;

/// How long reading a poll's items in its agent's Luau may take.
const ITEMS_LIMIT: Duration = Duration::from_secs(2);

/// tau-agents on the host. Clones share it.
#[derive(Clone)]
pub struct Agents(Arc<Inner>);

struct Inner {
    /// Where the agents' repositories are: one directory each.
    root: PathBuf,
    registry: Registry,
    env: Arc<dyn Env>,
    clock: Arc<dyn Clock>,
    store: Store,
    /// What is kept of each agent, read from its file the first time.
    kept: Mutex<BTreeMap<String, Kept>>,
    /// Each agent's spend today, as last counted, and when.
    spend: Mutex<BTreeMap<String, (f64, i64)>>,
    /// What the registry tells agents: their versions that failed.
    told: Mutex<mpsc::UnboundedReceiver<(String, Event)>>,
    /// Makes each command's event id its own.
    commands: AtomicU64,
    /// The polls going now, by agent and trigger.
    polling: Mutex<BTreeSet<(String, String)>>,
    /// The polls that ran since tau started: one that has not runs at
    /// once.
    polled: Mutex<BTreeSet<(String, String)>>,
    /// The polls' tasks.
    tasks: Mutex<JoinSet<()>>,
    /// Where each agent's home is kept, one directory each.
    homes: PathBuf,
    /// The `bwrap` its commands run in; with none, they have no command tools.
    bwrap: Option<PathBuf>,
    /// The streams kept open, by agent and trigger.
    streams: streams::Streams,
    /// The MCP resources watched, by agent and trigger.
    resources: resources::Resources,
}

impl PluginHost for Agents {
    async fn new(cx: &HostCx) -> anyhow::Result<Self> {
        // The skill an agent's conversation reads to write itself, as
        // this build has it.
        let skills = cx.dir.join(tau_skills::BUILTIN_DIR);
        tokio::task::spawn_blocking(move || crate::skill::install(&skills))
            .await??;
        let agents = Self::at(
            cx.dir.join(ROOT),
            cx.plugin_dir(NAME),
            Arc::new(HostEnv(cx.clone())),
            Arc::new(SystemClock),
        );
        let (watcher, ticker) = (agents.registry().clone(), agents.clone());
        cx.runtime.spawn(async move { watcher.watch().await });
        cx.runtime.spawn(async move {
            loop {
                ticker.tick().await;
                tokio::time::sleep(TICK).await;
            }
        });
        Ok(agents)
    }
}

impl Agents {
    /// The agents under `root`, keeping what it must in `dir`, reaching
    /// the host through `env`. Nothing runs until [`Self::tick`] and
    /// the registry's scans are called.
    pub fn at(
        root: PathBuf,
        dir: PathBuf,
        env: Arc<dyn Env>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let (tell, told) = mpsc::unbounded_channel();
        let refresher = env.clone();
        let registry = Registry::at(
            root.clone(),
            dir.join("versions.json"),
            clock.clone(),
            move || refresher.refresh(),
            move |agent, event| {
                let _ = tell.send((agent.to_owned(), event));
            },
        );
        Self(Arc::new(Inner {
            root,
            registry,
            env,
            clock,
            store: Store::at(dir.join("agents")),
            kept: Mutex::default(),
            spend: Mutex::default(),
            told: Mutex::new(told),
            commands: AtomicU64::new(0),
            polling: Mutex::default(),
            polled: Mutex::default(),
            tasks: Mutex::default(),
            homes: dir.join("homes"),
            bwrap: crate::sandbox::find_bwrap(),
            streams: streams::Streams::default(),
            resources: resources::Resources::default(),
        }))
    }

    /// The same agents, their commands run in `bwrap`, or with no
    /// command tools when there is none. Before it is shared.
    pub fn with_bwrap(mut self, bwrap: Option<PathBuf>) -> Self {
        Arc::get_mut(&mut self.0)
            .expect("set before the agents are shared")
            .bwrap = bwrap;
        self
    }

    /// What the commands of the repository at `checkout` run in, when it
    /// is an agent's: its sandbox, or nothing when there is no `bwrap`
    /// (its runs then have no `exec_command`).
    pub fn sandbox(
        &self,
        name: &str,
        checkout: &std::path::Path,
    ) -> Option<crate::sandbox::Sandbox> {
        if checkout.parent() != Some(self.0.root.as_path()) {
            return None;
        }
        Some(crate::sandbox::Sandbox {
            bwrap: self.0.bwrap.clone()?,
            home: self.0.homes.join(name),
        })
    }

    /// Why an agent's commands cannot run, if they cannot.
    pub fn sandbox_problem(&self) -> Option<String> {
        self.0.bwrap.is_none().then(|| {
            "commands are off: bwrap (bubblewrap) is not installed, and an \
             agent's commands never run outside it"
                .to_owned()
        })
    }

    /// Each agent's versions.
    pub fn registry(&self) -> &Registry {
        &self.0.registry
    }

    /// Fires what came due, and starts the turns whose events waited
    /// long enough; redraws the agents when something changed.
    pub async fn tick(&self) {
        let now = self.0.clock.now();
        let mut changed = false;
        let told: Vec<(String, Event)> = {
            let mut told = self.0.told.lock().await;
            std::iter::from_fn(|| told.try_recv().ok()).collect()
        };
        for (agent, event) in told {
            self.enqueue(&agent, event).await;
            changed = true;
        }
        // Let go of the polls that ended.
        while self.0.tasks.lock().await.try_join_next().is_some() {}
        let (mut streams, mut watches) = (Vec::new(), Vec::new());
        for (name, read) in self.0.registry.agents().await {
            streams.push((name.clone(), streams::declared(&read)));
            watches.push((name.clone(), resources::declared(&read)));
            changed |= self.fire_schedules(&name, &read, now).await;
            changed |= self.start_polls(&name, &read, now).await;
            let counted = self.0.spend.lock().await.get(&name).copied();
            if counted.is_none_or(|(_, at)| now - at >= SPEND_FOR) {
                let before = counted.map(|(spend, _)| spend);
                changed |= Some(self.count_spend(&name, now).await) != before;
            }
            changed |= self.deliver(&name, now).await;
        }
        changed |= self.keep_streams(streams).await;
        changed |= self.keep_watches(watches).await;
        if changed {
            self.0.env.refresh();
        }
    }

    /// Runs `change` on what is kept of `agent`, read the first time,
    /// and keeps what it leaves.
    async fn with_kept<T>(
        &self,
        agent: &str,
        change: impl FnOnce(&mut Kept) -> T,
    ) -> T {
        let mut all = self.0.kept.lock().await;
        if !all.contains_key(agent) {
            let kept = self.0.store.load(agent).await;
            all.insert(agent.to_owned(), kept);
        }
        let kept = all.get_mut(agent).expect("just read");
        let before = kept.clone();
        let answer = change(kept);
        if *kept != before
            && let Err(error) = self.0.store.save(agent, kept).await
        {
            eprintln!("{NAME}: cannot keep {agent}'s events: {error:#}");
        }
        answer
    }

    /// Puts `event` in `agent`'s queue, unless the same one waits there.
    async fn enqueue(&self, agent: &str, event: Event) {
        self.with_kept(agent, |kept| {
            let known = kept.queue.iter().any(|queued| {
                queued.trigger == event.trigger && queued.id == event.id
            });
            if !known {
                kept.fired.insert(event.trigger.clone(), event.at);
                kept.queue.push(event);
            }
        })
        .await;
    }

    /// Keeps `agent`'s schedules as its live version declares them, and
    /// queues those that came due. Whether anything changed.
    async fn fire_schedules(&self, agent: &str, read: &Read, now: i64) -> bool {
        let declared: Vec<(String, TriggerKind, Schedule)> = read
            .live
            .iter()
            .flat_map(|live| &live.declared.triggers)
            .filter_map(|trigger| {
                Schedule::of(&trigger.kind).ok().map(|schedule| {
                    (trigger.name.clone(), trigger.kind.clone(), schedule)
                })
            })
            .collect();
        let clock = self.0.clock.clone();
        self.with_kept(agent, |kept| {
            let before = kept.clone();
            kept.schedules.retain(|name, _| {
                declared.iter().any(|(known, ..)| known == name)
            });
            for (name, kind, schedule) in &declared {
                let current = kept
                    .schedules
                    .get(name)
                    .is_some_and(|due| due.kind == *kind);
                if !current {
                    kept.schedules.insert(
                        name.clone(),
                        Due {
                            kind: kind.clone(),
                            next: schedule.first_after(now, clock.as_ref()),
                        },
                    );
                }
                let due = kept.schedules.get_mut(name).expect("just kept");
                if due.next <= now {
                    let fired = due.next;
                    due.next =
                        schedule.after_firing(fired, now, clock.as_ref());
                    kept.fired.insert(name.clone(), now);
                    kept.queue.push(Event {
                        trigger: name.clone(),
                        kind: Kind::Schedule,
                        id: format!("{name}:{fired}"),
                        at: now,
                        data: json!({ "due": fired }),
                    });
                }
            }
            *kept != before
        })
        .await
    }

    /// Waits for every poll going to end: for tests, which tick by
    /// hand.
    pub async fn polls_settled(&self) {
        let mut tasks = self.0.tasks.lock().await;
        while tasks.join_next().await.is_some() {}
    }

    /// Keeps `agent`'s polls as its live version declares them, and
    /// starts those that are due and not going. Whether anything
    /// changed.
    async fn start_polls(&self, agent: &str, read: &Read, now: i64) -> bool {
        let declared: Vec<(String, TriggerKind, i64)> = read
            .live
            .iter()
            .flat_map(|live| &live.declared.triggers)
            .filter_map(|trigger| match &trigger.kind {
                TriggerKind::Poll { every, .. } => {
                    poll::every(every).ok().map(|every| {
                        (trigger.name.clone(), trigger.kind.clone(), every)
                    })
                }
                _ => None,
            })
            .collect();
        let fresh: Vec<bool> = {
            let mut polled = self.0.polled.lock().await;
            declared
                .iter()
                .map(|(name, ..)| {
                    polled.insert((agent.to_owned(), name.clone()))
                })
                .collect()
        };
        let (changed, due) = self
            .with_kept(agent, |kept| {
                let before = kept.polls.clone();
                kept.polls.retain(|name, _| {
                    declared.iter().any(|(known, ..)| known == name)
                });
                let mut due = Vec::new();
                for ((name, kind, every), fresh) in declared.iter().zip(fresh) {
                    let same = kept
                        .polls
                        .get(name)
                        .is_some_and(|polled| polled.kind == *kind);
                    // New, or polling something else: from scratch, now.
                    if !same {
                        kept.polls.insert(
                            name.clone(),
                            Polled::new(kind.clone(), now),
                        );
                    }
                    let polled = kept.polls.get_mut(name).expect("just kept");
                    // Every poll runs at once when tau starts.
                    if fresh {
                        polled.next = polled.next.min(now);
                    }
                    if polled.next <= now {
                        due.push((name.clone(), kind.clone(), *every));
                    }
                }
                (kept.polls != before, due)
            })
            .await;
        for (trigger, kind, every) in due {
            let key = (agent.to_owned(), trigger.clone());
            if !self.0.polling.lock().await.insert(key) {
                continue;
            }
            let (agents, agent) = (self.clone(), agent.to_owned());
            self.0.tasks.lock().await.spawn(async move {
                agents.run_poll(&agent, &trigger, &kind, every).await;
            });
        }
        changed
    }

    /// Polls `agent`'s `trigger` once: queues what it found that is new,
    /// or keeps why it failed.
    async fn run_poll(
        &self,
        agent: &str,
        trigger: &str,
        kind: &TriggerKind,
        every: i64,
    ) {
        let result = match kind {
            TriggerKind::Poll {
                tool: Some(tool),
                args,
                ..
            } => {
                let args = args.clone().unwrap_or_else(|| json!({}));
                let checkout = self.0.root.join(agent);
                self.0.env.call_tool(&checkout, tool, args).await
            }
            TriggerKind::Poll {
                http: Some(spec), ..
            } => self.0.env.fetch(spec.clone()).await,
            _ => Err("this poll names no tool and no request".to_owned()),
        };
        let items = match result {
            Ok(result) => self.poll_items(agent, trigger, &result).await,
            Err(error) => Err(error),
        };
        let now = self.0.clock.now();
        self.with_kept(agent, |kept| {
            let Some(polled) = kept.polls.get_mut(trigger) else {
                return;
            };
            // It changed while it polled: the new one polls afresh.
            if polled.kind != *kind {
                return;
            }
            match items {
                Ok(items) => {
                    let events = polled.found(trigger, items, every, now);
                    if !events.is_empty() {
                        kept.fired.insert(trigger.to_owned(), now);
                    }
                    for event in events {
                        let known = kept.queue.iter().any(|queued| {
                            queued.trigger == event.trigger
                                && queued.id == event.id
                        });
                        if !known {
                            kept.queue.push(event);
                        }
                    }
                }
                Err(error) => {
                    eprintln!("{NAME}: {agent}'s poll {trigger}: {error}");
                    polled.failed(error, every, now);
                }
            }
        })
        .await;
        self.0
            .polling
            .lock()
            .await
            .remove(&(agent.to_owned(), trigger.to_owned()));
        self.0.env.refresh();
    }

    /// The items, with their keys, `result` holds for `agent`'s poll
    /// `trigger`, as its live version's `key` and `items` read them.
    async fn poll_items(
        &self,
        agent: &str,
        trigger: &str,
        result: &Value,
    ) -> Result<Vec<(String, Value)>, String> {
        let live = self
            .0
            .registry
            .live(agent)
            .await
            .ok_or("no version of it is live")?;
        let result = result.to_string();
        if result.len() > tau_tools_host::http::MAX_JSON_BYTES {
            return Err("the answer is longer than a poll reads".into());
        }
        let script = format!(
            "local tau = require(\"tau\")\n\
             return tau._poll_items(require(\"agent\"), {trigger}, \
             json.decode({result}))\n",
            trigger = long_string(trigger),
            result = long_string(&result),
        );
        let found = evaluate(
            agent,
            live.modules.clone(),
            script,
            ITEMS_LIMIT,
            CancellationToken::new(),
        )
        .await?;
        poll::items(found)
    }

    /// Counts what `agent` spent today: everything in its repository
    /// since the host's local midnight.
    async fn count_spend(&self, agent: &str, now: i64) -> f64 {
        let since = clock::midnight(self.0.clock.as_ref(), now);
        let spend = match self.0.env.spent_since(agent, since).await {
            Ok(spend) => spend,
            Err(error) => {
                eprintln!("{NAME}: cannot count {agent}'s spend: {error:#}");
                return self
                    .0
                    .spend
                    .lock()
                    .await
                    .get(agent)
                    .map_or(0.0, |(spend, _)| *spend);
            }
        };
        self.0
            .spend
            .lock()
            .await
            .insert(agent.to_owned(), (spend, now));
        spend
    }

    /// Starts a turn of `agent` on the events that waited long enough,
    /// if it may take one now. Whether anything changed.
    async fn deliver(&self, agent: &str, now: i64) -> bool {
        let settings = self.0.env.settings(agent);
        let window = settings.window_secs as i64;
        let ready = self
            .with_kept(agent, |kept| {
                let oldest = kept.queue.iter().map(|event| event.at).min()?;
                let waited = now >= oldest + window;
                let retry = kept.retry_at.is_none_or(|at| now >= at);
                (waited && retry && !kept.paused).then_some(())
            })
            .await;
        if ready.is_none() {
            return false;
        }
        let Some(conversations) = self.0.env.conversations() else {
            return false;
        };
        if conversations.running(agent) {
            return false;
        }
        let spend = self.count_spend(agent, now).await;
        let day = clock::day(self.0.clock.as_ref(), now);
        if spend >= settings.cap_on(&day) {
            return false;
        }
        let batch = self.with_kept(agent, |kept| kept.queue.clone()).await;
        let text = Batch {
            events: batch.clone(),
        }
        .message(self.0.clock.offset(now));
        let started = conversations
            .turn(Turn {
                repo: agent.to_owned(),
                text,
                by: By::Tau,
                model: settings.model(),
                effort: settings.effort().to_owned(),
            })
            .await;
        self.with_kept(agent, |kept| match started {
            Ok(()) => {
                kept.queue.retain(|event| {
                    !batch.iter().any(|heard| {
                        heard.trigger == event.trigger && heard.id == event.id
                    })
                });
                kept.retry_at = None;
                kept.last = Some(Last {
                    at: now,
                    line: heard(&batch),
                });
            }
            Err(error) => {
                eprintln!("{NAME}: {agent}'s turn did not start: {error:#}");
                kept.retry_at = Some(now + RETRY);
            }
        })
        .await;
        true
    }

    /// What the person set for `agent`: its model, effort and cap.
    pub fn settings_of(&self, agent: &str) -> Settings {
        self.0.env.settings(agent)
    }

    /// The agents as every interface shows them.
    pub async fn overview(&self) -> Overview {
        let now = self.0.clock.now();
        let day = clock::day(self.0.clock.as_ref(), now);
        let conversations = self.0.env.conversations();
        // Names only: the values stay in the store.
        let stored =
            self.0.env.secrets().names().await.unwrap_or_else(|error| {
                eprintln!("{NAME}: cannot read tau's secret store: {error}");
                Vec::new()
            });
        let open = self.0.streams.open().await;
        let watching = self.0.resources.open().await;
        let mut agents = Vec::new();
        for (name, read) in self.0.registry.agents().await {
            let settings = self.0.env.settings(&name);
            let kept = self.with_kept(&name, |kept| kept.clone()).await;
            let spend = self
                .0
                .spend
                .lock()
                .await
                .get(&name)
                .map_or(0.0, |(spend, _)| *spend);
            let cap = settings.cap_on(&day);
            let running = conversations
                .as_ref()
                .is_some_and(|conversations| conversations.running(&name));
            let status = match (&read.live, &read.error) {
                (None, Some(error)) => Status::Broken {
                    error: error.clone(),
                },
                _ if kept.paused => Status::Paused,
                _ if running => Status::Working,
                _ if spend >= cap => Status::AtCap,
                _ => Status::Waiting,
            };
            let triggers = read
                .live
                .iter()
                .flat_map(|live| &live.declared.triggers)
                .map(|declared| Trigger {
                    name: declared.name.clone(),
                    kind: declared.kind.clone(),
                    last: kept.fired.get(&declared.name).copied(),
                    next: kept
                        .schedules
                        .get(&declared.name)
                        .map(|due| due.next)
                        .or_else(|| {
                            kept.polls
                                .get(&declared.name)
                                .map(|polled| polled.next)
                        }),
                    error: kept
                        .polls
                        .get(&declared.name)
                        .and_then(|polled| polled.error.clone())
                        .or_else(|| {
                            kept.streams
                                .get(&declared.name)
                                .and_then(|streamed| streamed.error.clone())
                        })
                        .or_else(|| {
                            kept.resources
                                .get(&declared.name)
                                .and_then(|watched| watched.error.clone())
                        }),
                    live: match declared.kind {
                        TriggerKind::Stream { .. } => Some(&open),
                        TriggerKind::Mcp { .. } => Some(&watching),
                        _ => None,
                    }
                    .map(|open| {
                        open.contains(&(name.clone(), declared.name.clone()))
                    }),
                })
                .collect();
            let main = conversations
                .as_ref()
                .and_then(|conversations| conversations.main_of(&name));
            let secrets = read
                .live
                .iter()
                .flat_map(|live| &live.declared.secrets)
                .map(|secret| SecretState {
                    name: secret.clone(),
                    set: stored.contains(secret),
                })
                .collect();
            agents.push(Agent {
                main,
                secrets,
                sandbox: self.sandbox_problem(),
                notices: kept.notices.clone(),
                name,
                purpose: read.purpose.clone(),
                status,
                triggers,
                versions: read.versions.clone(),
                spend_today: spend,
                cap_today: cap,
                queued: kept.queue.len(),
                last: kept.last.clone(),
                model: settings.model(),
                effort: settings.effort().to_owned(),
            });
        }
        Overview { agents }
    }

    /// Carries out what the interface asked.
    pub async fn act(&self, act: Act) -> anyhow::Result<Option<Value>> {
        let now = self.0.clock.now();
        match act {
            Act::Create {
                name,
                purpose,
                model,
                daily_cap,
                window_secs,
                servers,
            } => {
                self.create(
                    &name,
                    &purpose,
                    model,
                    daily_cap,
                    window_secs,
                    &servers,
                )
                .await?;
                return Ok(Some(json!({ "repo": name })));
            }
            Act::AllowMore { agent, amount } => {
                anyhow::ensure!(
                    amount.is_finite() && amount > 0.0,
                    "Allow a positive amount, not {amount}"
                );
                let day = clock::day(self.0.clock.as_ref(), now);
                let mut settings = self.0.env.settings(&agent);
                if settings.extra_day.as_deref() == Some(day.as_str()) {
                    settings.extra_today += amount;
                } else {
                    settings.extra_day = Some(day);
                    settings.extra_today = amount;
                }
                self.0.env.save_settings(&agent, settings).await?;
            }
            Act::SetSecret { name, value } => {
                anyhow::ensure!(
                    tau_tools_host::secrets::valid_name(&name),
                    "`{name}` cannot name a secret: letters, digits, `_`, `-` and `.`"
                );
                self.0.env.secrets().set(&name, &value).await?;
            }
            Act::RemoveSecret { name } => {
                self.0.env.secrets().remove(&name).await?;
            }
            Act::Answer {
                agent,
                notice,
                answer,
            } => self.answer(&agent, &notice, &answer).await?,
            Act::Dismiss { agent, notice } => {
                self.dismiss(&agent, &notice).await?;
            }
            Act::Pause { agent } => {
                self.with_kept(&agent, |kept| kept.paused = true).await;
            }
            Act::Resume { agent } => {
                self.with_kept(&agent, |kept| {
                    kept.paused = false;
                    kept.retry_at = None;
                })
                .await;
            }
            Act::Command {
                agent,
                command,
                args,
                from,
            } => {
                let live =
                    self.0.registry.live(&agent).await.ok_or_else(|| {
                        anyhow::anyhow!("{agent} has no version that loads yet")
                    })?;
                let trigger = live
                    .declared
                    .triggers
                    .iter()
                    .find(|trigger| {
                        matches!(
                            &trigger.kind,
                            TriggerKind::Command { command: named, .. }
                                if *named == command
                        )
                    })
                    .ok_or_else(|| {
                        anyhow::anyhow!("{agent} has no command /{command}")
                    })?;
                let id = format!(
                    "cmd:{:x}{:02x}",
                    now,
                    self.0.commands.fetch_add(1, Ordering::Relaxed) % 256
                );
                self.enqueue(
                    &agent,
                    Event {
                        trigger: trigger.name.clone(),
                        kind: Kind::Command,
                        id,
                        at: now,
                        data: json!({ "args": args, "from": from }),
                    },
                )
                .await;
            }
        }
        self.0.env.refresh();
        Ok(None)
    }

    /// Makes the agent `name`, keeps what the person set for it, and
    /// starts its conversation with their words, as theirs.
    async fn create(
        &self,
        name: &str,
        purpose: &str,
        model: Option<String>,
        daily_cap: Option<f64>,
        window_secs: Option<u64>,
        servers: &[String],
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            tau_agents::valid_name(name),
            "An agent's name is lower-case letters, digits and dashes, \
             starting with a letter: {name} is not"
        );
        anyhow::ensure!(!purpose.trim().is_empty(), "Say what it is for");
        let defaults = Settings::default();
        let settings = Settings {
            model: model.filter(|model| !model.trim().is_empty()),
            daily_cap: daily_cap.unwrap_or(defaults.daily_cap),
            window_secs: window_secs.unwrap_or(defaults.window_secs),
            ..defaults
        };
        anyhow::ensure!(
            settings.daily_cap.is_finite() && settings.daily_cap >= 0.0,
            "A cap is a number of dollars, not {}",
            settings.daily_cap
        );
        let conversations = self
            .0
            .env
            .conversations()
            .ok_or_else(|| anyhow::anyhow!("This host makes no agents"))?;
        conversations.list_agent(name).await?;
        self.0.env.save_settings(name, settings.clone()).await?;
        let mut text = format!("/{SKILL} {}", purpose.trim());
        if !servers.is_empty() {
            text.push_str(&format!(
                "\n\nOf their MCP servers, the person chose these for you to \
                 start with: {}.",
                servers.join(", ")
            ));
        }
        conversations
            .turn(Turn {
                repo: name.to_owned(),
                text,
                by: By::Person,
                model: settings.model(),
                effort: settings.effort().to_owned(),
            })
            .await
    }
}

/// What a turn heard, in a line: the triggers its events came from.
fn heard(batch: &[Event]) -> String {
    let mut triggers: Vec<&str> =
        batch.iter().map(|event| event.trigger.as_str()).collect();
    triggers.dedup();
    match batch.len() {
        1 => format!("heard {}", triggers[0]),
        n => format!("heard {n} events: {}", triggers.join(", ")),
    }
}
