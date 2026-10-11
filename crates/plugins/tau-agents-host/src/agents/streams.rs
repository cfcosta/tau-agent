//! The streams agents keep open (ADR 0037): one task per stream that a
//! live version declares, which opens it, hands each frame to the
//! trigger's `parse`, queues what that gives, and opens it again when it
//! drops. The connection itself is `crate::stream`'s.

use std::collections::{BTreeMap, BTreeSet};

use super::*;
use crate::stream::{self, Connection, Frame, Read as Reading, Streamed};

/// How long `parse` may take on one frame.
const PARSE_LIMIT: Duration = Duration::from_secs(2);

/// The streams going, and those open now.
pub(super) struct Streams {
    going: Mutex<BTreeMap<(String, String), Going>>,
    open: Mutex<BTreeSet<(String, String)>>,
    /// How long a stream waits to open again: after its first failure,
    /// and at most.
    waits: (Duration, Duration),
}

impl Default for Streams {
    fn default() -> Self {
        Self {
            going: Mutex::default(),
            open: Mutex::default(),
            waits: (stream::FIRST_WAIT, stream::LONGEST_WAIT),
        }
    }
}

impl Streams {
    /// The streams open now, by agent and trigger.
    pub(super) async fn open(&self) -> BTreeSet<(String, String)> {
        self.open.lock().await.clone()
    }
}

/// A stream's task: what it reads, and how to stop it.
struct Going {
    kind: TriggerKind,
    cancel: CancellationToken,
}

/// The streams `read`'s live version declares, by trigger.
pub(super) fn declared(read: &Read) -> Vec<(String, TriggerKind)> {
    read.live
        .iter()
        .flat_map(|live| &live.declared.triggers)
        .filter(|trigger| matches!(trigger.kind, TriggerKind::Stream { .. }))
        .map(|trigger| (trigger.name.clone(), trigger.kind.clone()))
        .collect()
}

impl Agents {
    /// The same agents, their streams waiting from `first` to `longest`
    /// to open again. Before it is shared.
    pub fn with_stream_waits(
        mut self,
        first: Duration,
        longest: Duration,
    ) -> Self {
        Arc::get_mut(&mut self.0)
            .expect("set before the agents are shared")
            .streams
            .waits = (first, longest);
        self
    }

    /// Closes every stream, as tau does when it goes.
    pub async fn close_streams(&self) {
        let mut going = self.0.streams.going.lock().await;
        for going in going.values() {
            going.cancel.cancel();
        }
        going.clear();
    }

    /// Keeps a stream open for each one `declared` names, by agent, and
    /// only those: one removed or changed closes, one new opens. Whether
    /// anything changed.
    pub(super) async fn keep_streams(
        &self,
        declared: Vec<(String, Vec<(String, TriggerKind)>)>,
    ) -> bool {
        let mut changed = false;
        for (agent, streams) in &declared {
            changed |= self
                .with_kept(agent, |kept| {
                    let before = kept.streams.clone();
                    kept.streams.retain(|name, streamed| {
                        streams.iter().any(|(declared, kind)| {
                            declared == name && *kind == streamed.kind
                        })
                    });
                    for (name, kind) in streams {
                        kept.streams
                            .entry(name.clone())
                            .or_insert_with(|| Streamed::new(kind.clone()));
                    }
                    kept.streams != before
                })
                .await;
        }
        let wanted: BTreeMap<(String, String), TriggerKind> = declared
            .into_iter()
            .flat_map(|(agent, streams)| {
                streams
                    .into_iter()
                    .map(move |(name, kind)| ((agent.clone(), name), kind))
            })
            .collect();
        let mut going = self.0.streams.going.lock().await;
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
                agents.run_stream(&agent, &trigger, &kind, &cancel).await;
            });
            changed = true;
        }
        changed
    }

    /// Keeps `agent`'s stream `trigger` open until `cancel`: opens it,
    /// reads it, and opens it again when it drops.
    async fn run_stream(
        &self,
        agent: &str,
        trigger: &str,
        kind: &TriggerKind,
        cancel: &CancellationToken,
    ) {
        let key = (agent.to_owned(), trigger.to_owned());
        let (first, longest) = self.0.streams.waits;
        let mut failures = 0;
        loop {
            let read = tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                read = self.read_stream(agent, trigger, kind, &mut failures) => read,
            };
            self.0.streams.open.lock().await.remove(&key);
            if let Err(error) = read {
                failures += 1;
                eprintln!("{NAME}: {agent}'s stream {trigger}: {error}");
                self.with_kept(agent, |kept| {
                    if let Some(streamed) = kept.streams.get_mut(trigger)
                        && streamed.kind == *kind
                    {
                        streamed.error = Some(error);
                    }
                })
                .await;
            }
            self.0.env.refresh();
            let wait =
                stream::wait(failures.max(1), first, longest, stream::jitter());
            tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                () = tokio::time::sleep(wait) => {}
            }
        }
        self.0.streams.open.lock().await.remove(&key);
        self.0.env.refresh();
    }

    /// Opens `agent`'s stream `trigger` once and reads it to its end:
    /// `Ok` when the server closed it, else why it failed, scrubbed of
    /// every secret. Once it opens, `failures` starts over.
    async fn read_stream(
        &self,
        agent: &str,
        trigger: &str,
        kind: &TriggerKind,
        failures: &mut u32,
    ) -> Result<(), String> {
        let secrets = self
            .0
            .env
            .secrets(agent)
            .read()
            .await
            .map_err(|error| format!("tau's secret store: {error}"))?;
        let scrub = tau_tools_host::secrets::Scrub::new(&secrets);
        let last_id = self
            .with_kept(agent, |kept| {
                kept.streams
                    .get(trigger)
                    .and_then(|streamed| streamed.last_id.clone())
            })
            .await;
        let mut connection = Connection::open(kind, &secrets, last_id)
            .await
            .map_err(|error| scrub.text(&error))?;
        *failures = 0;
        self.0
            .streams
            .open
            .lock()
            .await
            .insert((agent.to_owned(), trigger.to_owned()));
        self.with_kept(agent, |kept| {
            if let Some(streamed) = kept.streams.get_mut(trigger) {
                streamed.error = None;
            }
        })
        .await;
        self.0.env.refresh();
        while let Some(read) = connection.next().await {
            match read.map_err(|error| scrub.text(&error))? {
                Reading::TooLarge => eprintln!(
                    "{NAME}: {agent}'s stream {trigger}: a frame longer than \
                     {} KiB was dropped",
                    stream::MAX_FRAME / 1024
                ),
                Reading::Frame(frame) => {
                    let frame = Frame {
                        value: scrub.value(&frame.value),
                        id: frame.id,
                    };
                    self.take_frame(agent, trigger, kind, frame).await;
                }
            }
        }
        Ok(())
    }

    /// Hands `frame` to `trigger`'s `parse`, keeps the state it leaves,
    /// and queues the event it gives unless that event fired lately. A
    /// `parse` that fails says why on the agent's page; the stream goes
    /// on.
    async fn take_frame(
        &self,
        agent: &str,
        trigger: &str,
        kind: &TriggerKind,
        frame: Frame,
    ) {
        let state = self
            .with_kept(agent, |kept| {
                kept.streams
                    .get(trigger)
                    .map(|streamed| streamed.state.clone())
            })
            .await
            .unwrap_or_else(|| json!({}));
        let parsed =
            self.parse_frame(agent, trigger, &frame.value, &state).await;
        if let Err(error) = &parsed {
            eprintln!("{NAME}: {agent}'s stream {trigger}: {error}");
        }
        let now = self.0.clock.now();
        self.with_kept(agent, |kept| {
            let Some(streamed) = kept.streams.get_mut(trigger) else {
                return;
            };
            // It changed while it read: the new one starts afresh.
            if streamed.kind != *kind {
                return;
            }
            if frame.id.is_some() {
                streamed.last_id.clone_from(&frame.id);
            }
            let event = match parsed {
                Ok((event, state)) => {
                    streamed.state = state;
                    streamed.error = None;
                    event.filter(|(id, _)| streamed.fresh(id))
                }
                Err(error) => {
                    streamed.error = Some(format!("`parse` failed: {error}"));
                    None
                }
            };
            if let Some((id, data)) = event {
                kept.fired.insert(trigger.to_owned(), now);
                let known = kept
                    .queue
                    .iter()
                    .any(|queued| queued.trigger == trigger && queued.id == id);
                if !known {
                    kept.queue.push(Event {
                        trigger: trigger.to_owned(),
                        kind: Kind::Stream,
                        id,
                        at: now,
                        data,
                    });
                }
            }
        })
        .await;
        self.0.env.refresh();
    }

    /// What `agent`'s live version's `parse` makes of `frame` for the
    /// stream `trigger`, given its `state`: the event, if any, as its id
    /// and data, and the state it leaves.
    async fn parse_frame(
        &self,
        agent: &str,
        trigger: &str,
        frame: &Value,
        state: &Value,
    ) -> Result<(Option<(String, Value)>, Value), String> {
        let live = self
            .0
            .registry
            .live(agent)
            .await
            .ok_or("no version of it is live")?;
        let (frame, state) = (frame.to_string(), state.to_string());
        if state.len() > tau_tools_host::http::MAX_JSON_BYTES {
            return Err("its state is longer than a stream keeps".into());
        }
        let script = format!(
            "local tau = require(\"tau\")\n\
             return tau._parse_frame(require(\"agent\"), {trigger}, \
             json.decode({frame}), json.decode({state}))\n",
            trigger = long_string(trigger),
            frame = long_string(&frame),
            state = long_string(&state),
        );
        let parsed = evaluate(
            agent,
            live.modules.clone(),
            script,
            PARSE_LIMIT,
            CancellationToken::new(),
        )
        .await?;
        let state = parsed.get("state").cloned().unwrap_or_else(|| json!({}));
        let event = match parsed.get("event") {
            None | Some(Value::Null) => None,
            Some(event) => Some((
                event
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or("an event from `parse` has no id")?
                    .to_owned(),
                event.get("data").cloned().unwrap_or(Value::Null),
            )),
        };
        Ok((event, state))
    }
}
