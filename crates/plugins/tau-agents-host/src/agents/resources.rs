//! The MCP resources agents watch (ADR 0037): one task per `mcp`
//! trigger a live version declares, which watches its resource through
//! the host's servers, reads it when it changes, and queues an event
//! when what it reads differs from what it read last. A watch ends with
//! its connection; the task then watches again, reading once, so a
//! change missed in between still fires. What it keeps is
//! `crate::resource`'s.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use tau_ui_plugin::WatchError;

use super::*;
use crate::{
    resource::{self, Watched},
    stream,
};

/// The watches going, and those that hold a subscription now.
pub(super) struct Resources {
    going: Mutex<BTreeMap<(String, String), Going>>,
    open: Mutex<BTreeSet<(String, String)>>,
    /// How long a watch waits to start again: after its first failure,
    /// and at most.
    waits: (Duration, Duration),
}

impl Default for Resources {
    fn default() -> Self {
        Self {
            going: Mutex::default(),
            open: Mutex::default(),
            waits: (stream::FIRST_WAIT, stream::LONGEST_WAIT),
        }
    }
}

impl Resources {
    /// The watches that hold a subscription now, by agent and trigger.
    pub(super) async fn open(&self) -> BTreeSet<(String, String)> {
        self.open.lock().await.clone()
    }
}

/// A watch's task: what it watches, and how to stop it.
struct Going {
    kind: TriggerKind,
    cancel: CancellationToken,
}

/// The `mcp` triggers `read`'s live version declares, by trigger.
pub(super) fn declared(read: &Read) -> Vec<(String, TriggerKind)> {
    read.live
        .iter()
        .flat_map(|live| &live.declared.triggers)
        .filter(|trigger| matches!(trigger.kind, TriggerKind::Mcp { .. }))
        .map(|trigger| (trigger.name.clone(), trigger.kind.clone()))
        .collect()
}

impl Agents {
    /// The same agents, their watches waiting from `first` to `longest`
    /// to start again. Before it is shared.
    pub fn with_watch_waits(
        mut self,
        first: Duration,
        longest: Duration,
    ) -> Self {
        Arc::get_mut(&mut self.0)
            .expect("set before the agents are shared")
            .resources
            .waits = (first, longest);
        self
    }

    /// Stops every watch, as tau does when it goes.
    pub async fn close_watches(&self) {
        let mut going = self.0.resources.going.lock().await;
        for going in going.values() {
            going.cancel.cancel();
        }
        going.clear();
    }

    /// Keeps a watch on each resource `declared` names, by agent, and
    /// only those: one removed or changed stops, one new starts.
    /// Whether anything changed.
    pub(super) async fn keep_watches(
        &self,
        declared: Vec<(String, Vec<(String, TriggerKind)>)>,
    ) -> bool {
        let mut changed = false;
        for (agent, watches) in &declared {
            changed |= self
                .with_kept(agent, |kept| {
                    let before = kept.resources.clone();
                    kept.resources.retain(|name, watched| {
                        watches.iter().any(|(declared, kind)| {
                            declared == name && *kind == watched.kind
                        })
                    });
                    for (name, kind) in watches {
                        kept.resources
                            .entry(name.clone())
                            .or_insert_with(|| Watched::new(kind.clone()));
                    }
                    kept.resources != before
                })
                .await;
        }
        let wanted: BTreeMap<(String, String), TriggerKind> = declared
            .into_iter()
            .flat_map(|(agent, watches)| {
                watches
                    .into_iter()
                    .map(move |(name, kind)| ((agent.clone(), name), kind))
            })
            .collect();
        let mut going = self.0.resources.going.lock().await;
        going.retain(|key, going| {
            let keep = wanted.get(key) == Some(&going.kind);
            if !keep {
                going.cancel.cancel();
                changed = true;
            }
            keep
        });
        for (key, kind) in wanted {
            if going.contains_key(&key) {
                continue;
            }
            let cancel = CancellationToken::new();
            going.insert(
                key.clone(),
                Going {
                    kind: kind.clone(),
                    cancel: cancel.clone(),
                },
            );
            let agents = self.clone();
            tokio::spawn(async move {
                let (agent, trigger) = key;
                agents.run_watch(&agent, &trigger, &kind, &cancel).await;
            });
            changed = true;
        }
        changed
    }

    /// Watches `agent`'s resource `trigger` until `cancel`: watches it,
    /// and watches again when the watch ends.
    async fn run_watch(
        &self,
        agent: &str,
        trigger: &str,
        kind: &TriggerKind,
        cancel: &CancellationToken,
    ) {
        let TriggerKind::Mcp { server, uri } = kind else {
            return;
        };
        let key = (agent.to_owned(), trigger.to_owned());
        let (first, longest) = self.0.resources.waits;
        let mut failures = 0;
        loop {
            let watched = tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                watched = self.watch_once(agent, trigger, kind, server, uri, &mut failures) => watched,
            };
            self.0.resources.open.lock().await.remove(&key);
            // A server that cannot subscribe will not soon learn to: it
            // tries again when its connection changes, or after the
            // longest wait.
            let mut wait = None;
            let mut unsupported = false;
            if let Err(error) = watched {
                failures += 1;
                if matches!(error, WatchError::Unsupported(_)) {
                    wait = Some(longest);
                    unsupported = true;
                }
                eprintln!("{NAME}: {agent}'s watch {trigger}: {error}");
                self.with_kept(agent, |kept| {
                    if let Some(watched) = kept.resources.get_mut(trigger)
                        && watched.kind == *kind
                    {
                        watched.error = Some(error.to_string());
                    }
                })
                .await;
            }
            self.0.env.refresh();
            let wait = wait.unwrap_or_else(|| {
                stream::wait(failures.max(1), first, longest, stream::jitter())
            });
            let checkout = self.0.root.join(agent);
            let changed = async {
                if unsupported {
                    self.0.env.server_changed(&checkout, server).await;
                } else {
                    std::future::pending::<()>().await;
                }
            };
            tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                () = tokio::time::sleep(wait) => {}
                () = changed => {}
            }
        }
        self.0.resources.open.lock().await.remove(&key);
        self.0.env.refresh();
    }

    /// Watches `agent`'s resource `trigger` once, until the watch ends:
    /// reads it as the watch starts, then each time it changes. Once it
    /// watches, `failures` starts over.
    async fn watch_once(
        &self,
        agent: &str,
        trigger: &str,
        kind: &TriggerKind,
        server: &str,
        uri: &str,
        failures: &mut u32,
    ) -> Result<(), WatchError> {
        let checkout = self.0.root.join(agent);
        let mut watch =
            self.0.env.watch_resource(&checkout, server, uri).await?;
        *failures = 0;
        self.0
            .resources
            .open
            .lock()
            .await
            .insert((agent.to_owned(), trigger.to_owned()));
        self.with_kept(agent, |kept| {
            if let Some(watched) = kept.resources.get_mut(trigger) {
                watched.error = None;
            }
        })
        .await;
        self.0.env.refresh();
        // What changed while it did not watch fires now.
        self.read_watched(agent, trigger, kind, &checkout, server, uri)
            .await;
        while watch.changed().await {
            self.read_watched(agent, trigger, kind, &checkout, server, uri)
                .await;
        }
        Ok(())
    }

    /// Reads `agent`'s watched resource and keeps its digest; queues an
    /// event when it differs from the last read, but not on the first.
    /// A read that fails says why on the agent's page; the watch goes
    /// on.
    async fn read_watched(
        &self,
        agent: &str,
        trigger: &str,
        kind: &TriggerKind,
        checkout: &Path,
        server: &str,
        uri: &str,
    ) {
        let read = self.0.env.read_resource(checkout, server, uri).await;
        let now = self.0.clock.now();
        self.with_kept(agent, |kept| {
            let Some(watched) = kept.resources.get_mut(trigger) else {
                return;
            };
            // It changed while it read: the new one starts afresh.
            if watched.kind != *kind {
                return;
            }
            let read = match read {
                Ok(read) => read,
                Err(error) => {
                    eprintln!("{NAME}: {agent}'s watch {trigger}: {error}");
                    watched.error = Some(error);
                    return;
                }
            };
            watched.error = None;
            let data = resource::contents(&read);
            let digest = resource::digest(&data);
            let before = watched.digest.replace(digest.clone());
            if before.is_none() || before.as_deref() == Some(digest.as_str()) {
                return;
            }
            let id = resource::event_id(uri, &digest);
            kept.fired.insert(trigger.to_owned(), now);
            let known = kept
                .queue
                .iter()
                .any(|queued| queued.trigger == trigger && queued.id == id);
            if !known {
                kept.queue.push(Event {
                    trigger: trigger.to_owned(),
                    kind: Kind::Mcp,
                    id,
                    at: now,
                    data,
                });
            }
        })
        .await;
        self.0.env.refresh();
    }
}
