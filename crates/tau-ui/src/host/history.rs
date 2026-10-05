//! Stored runs, read back as views.

use super::*;
use crate::view::ChildRun;

/// One stored run, rebuilt as the interface shows it.
pub(super) async fn stored_view(
    store: &Store,
    record: &tau_store::RunRecord,
) -> anyhow::Result<RunView> {
    // The messages, and in place among them what plugins recorded: the
    // effort each message ran at, the checks and verdicts on the calls.
    let timeline: Vec<Stored> = store
        .timeline(&record.id)
        .await?
        .into_iter()
        .filter_map(|entry| match entry {
            Entry::Message { body, .. } => {
                serde_json::from_str(&body).ok().map(Stored::Message)
            }
            Entry::Plugin { plugin, body }
                if plugin == LANDING_RECORD
                    || crate::plugins::registry().get(&plugin).is_some() =>
            {
                serde_json::from_str(&body)
                    .ok()
                    .map(|body| Stored::Record { plugin, body })
            }
            // A rewrite carries its plugin's details, and what it did.
            Entry::Context {
                plugin,
                body,
                stats,
                ..
            } => serde_json::from_str(&body)
                .ok()
                .map(|body| Stored::Rewrite {
                    plugin,
                    body,
                    tokens: stats
                        .map(|stats| (stats.tokens_before, stats.tokens_after)),
                }),
            _ => None,
        })
        .collect();
    let prompt = first_prompt(store, &record.id).await?;
    let mut view = RunView::from_timeline(
        RunId(record.id.clone().into()),
        record
            .title
            .clone()
            .unwrap_or_else(|| crate::titles::placeholder(&prompt)),
        &record.agent,
        &record.model,
        &timeline,
    )
    .in_repo(stored_repo(store, &record.id).await.unwrap_or_default())
    .started(started(&record.created_at));
    let plugin_cost = store
        .plugin_costs(&record.id)
        .await?
        .iter()
        .map(|cost| cost.cost_usd)
        .sum();
    let stop = match record.status {
        Status::Done => Some(StopReason::Stop),
        Status::Cancelled => Some(StopReason::Cancelled),
        Status::Failed => Some(StopReason::Error(
            record.error.clone().unwrap_or_else(|| "failed".into()),
        )),
        Status::Limit => Some(StopReason::Error("stopped at a limit".into())),
        // tau closed while it ran; a run still `running` in the store
        // is one this tau does not run, so it was cut off too.
        Status::Interrupted | Status::Running => None,
    };
    match stop {
        Some(stop) => view.finish_stored(stop, record.cost_usd, plugin_cost),
        None => view.interrupted_stored(record.cost_usd, plugin_cost),
    }
    // The plan it started with, as it showed live.
    if let Some(start) = stored_start(store, &record.id).await {
        view.set_base_plan(
            &record.model,
            start.effort.as_deref().unwrap_or("auto"),
            start.access.as_deref().unwrap_or_default(),
            start.workspace.as_deref(),
        );
    }
    if let RunKind::Fork { parent, fork_seq } = &record.kind {
        let turn = store
            .plugin_entries(parent, WORKSPACE_PLUGIN)
            .await?
            .iter()
            .filter(|(seq, _)| seq <= fork_seq)
            .filter_map(|(_, body)| Link::parse(body))
            .map(|link| link.turn)
            .next_back()
            .unwrap_or(0);
        view = view.with_origin(Origin::Fork {
            from: RunId(parent.clone().into()),
            turn,
        });
    }
    Ok(view)
}

/// How `run` ended for good, if it did (see [`endings`]).
pub(super) async fn stored_ending(
    store: &Store,
    run: &str,
) -> anyhow::Result<Option<Ending>> {
    if !store.plugin_entries(run, DROPPED_RECORD).await?.is_empty() {
        return Ok(Some(Ending::Dropped));
    }
    let Some(record) = store.run(run).await? else {
        return Ok(None);
    };
    let parent = match record.kind {
        RunKind::Fork { parent, .. } | RunKind::Subagent { parent, .. } => {
            parent
        }
        RunKind::Root => return Ok(None),
    };
    let of_parent = |entries: Vec<(i64, String)>| -> Vec<(String, String)> {
        entries
            .into_iter()
            .map(|(_, body)| (parent.clone(), body))
            .collect()
    };
    let landings =
        of_parent(store.plugin_entries(&parent, LANDING_RECORD).await?);
    let links =
        of_parent(store.plugin_entries(&parent, WORKSPACE_PLUGIN).await?);
    Ok(landed(&landings, &links).remove(run))
}

/// How each run that ended for good ended: the chats that landed or
/// were dropped, by id. A chat landed when its parent recorded a landing
/// from it (`Host::land`), or a change it brought (a sub-agent landing
/// as it returns); it was dropped when it keeps a [`DROPPED_RECORD`].
pub(super) async fn endings(
    store: &Store,
) -> anyhow::Result<HashMap<String, Ending>> {
    let landings = store.plugin_entries_everywhere(LANDING_RECORD).await?;
    let links = store.plugin_entries_everywhere(WORKSPACE_PLUGIN).await?;
    let mut endings = landed(&landings, &links);
    for (run, _) in store.plugin_entries_everywhere(DROPPED_RECORD).await? {
        endings.insert(run, Ending::Dropped);
    }
    Ok(endings)
}

/// The chats that landed, from their parents' `(parent, body)` landing
/// records and links.
fn landed(
    landings: &[(String, String)],
    links: &[(String, String)],
) -> HashMap<String, Ending> {
    let mut landed: HashMap<String, Ending> = HashMap::new();
    for (parent, body) in links {
        let Some(from) = Link::parse(body).and_then(|link| link.from) else {
            continue;
        };
        let on = RunId(parent.as_str().into());
        match landed
            .entry(from)
            .or_insert(Ending::Landed { on, changes: 0 })
        {
            Ending::Landed { changes, .. } => *changes += 1,
            Ending::Dropped => {}
        }
    }
    // A landing's record counts its changes itself, and covers one that
    // brought none.
    for (parent, body) in landings {
        if let Ok(record) = serde_json::from_str::<LandingRecord>(body) {
            landed.insert(
                record.from,
                Ending::Landed {
                    on: RunId(parent.as_str().into()),
                    changes: record.landing.changes.len(),
                },
            );
        }
    }
    landed
}

/// The words a run was started with: its own first message, not one a
/// fork inherited.
pub(super) async fn first_prompt(
    store: &Store,
    run: &str,
) -> anyhow::Result<String> {
    let body = store.first_prompt(run).await?;
    let words = body
        .and_then(|body| serde_json::from_str::<Message>(&body).ok())
        .and_then(|message| match message {
            Message::User(user) => Some(crate::view::user_words(&user.content)),
            _ => None,
        });
    Ok(words.unwrap_or_default())
}

/// Past runs, rebuilt from their stored transcripts, each under the
/// repository it recorded: the latest, and the runs `pinned` however old
/// they are.
pub async fn history(
    store: &Store,
    pinned: &[String],
) -> anyhow::Result<Vec<RunView>> {
    let endings = endings(store).await?;
    let mut records = store.recent_runs(HISTORY).await?;
    for id in pinned {
        if !records.iter().any(|record| &record.id == id)
            && let Some(record) = store.run(id).await?
        {
            records.push(record);
        }
    }
    let mut views = Vec::with_capacity(records.len());
    for record in &records {
        views.push(stored_view(store, record).await?);
    }
    // Sub-agents are left out of the list: they come back under the
    // runs that called them, however they ended, newest first.
    let parents: Vec<RunId> =
        views.iter().map(|view| view.id.clone()).collect();
    for parent in parents {
        for record in store.subagents(&parent.0).await?.into_iter().rev() {
            let view = stored_view(store, &record).await?.with_origin(
                Origin::SubAgent {
                    parent: parent.clone(),
                },
            );
            if let Some(parent) =
                views.iter_mut().find(|view| view.id == parent)
            {
                let child = ChildRun::of(&view, ChildKind::SubAgent, None);
                parent.children.push(child);
            }
            views.push(view);
        }
    }
    // Each fork is listed under the run it came from, too. A finished
    // fork that has not landed waits in its parent's chat; a dropped one
    // is closed, and the chat leaves it out.
    let forks: Vec<(RunId, ChildRun)> = views
        .iter()
        .filter_map(|view| match &view.origin {
            Origin::Fork { from, .. } => {
                Some((from.clone(), ChildRun::of(view, ChildKind::Fork, None)))
            }
            _ => None,
        })
        .collect();
    for view in &mut views {
        view.ending = endings.get(&*view.id.0).cloned();
    }
    for (parent, fork) in forks {
        if let Some(view) = views.iter_mut().find(|view| view.id == parent) {
            if !fork.status.is_live() {
                view.fork_finished(&fork.id);
            }
            view.children.push(fork);
        }
    }
    Ok(views)
}

/// The date `days` ago, as `YYYY-MM-DD`: a prefix the store's times
/// (`2026-09-28T14:03:11.402Z`) compare after when they are on or after
/// that day.
pub(super) fn days_ago(days: u64) -> String {
    let today = tau_ai::time::now_seconds() / 86_400;
    tau_ai::time::date(today - days.min(today))
}

pub(super) fn started(created_at: &str) -> String {
    created_at.get(..16).map_or_else(
        || created_at.to_owned(),
        |minute| minute.replace('T', " "),
    )
}

impl Host {
    /// Runs from earlier sessions, newest first, rebuilt from the store.
    /// The view of `repo`'s main chat, as history shows it.
    pub fn main_view(&self, repo: &Repo) -> anyhow::Result<Option<RunView>> {
        let Some(main) = &repo.main else {
            return Ok(None);
        };
        self.runtime.block_on(async {
            match self.store.run(&main.0).await? {
                Some(record) => {
                    Ok(Some(stored_view(&self.store, &record).await?))
                }
                None => Ok(None),
            }
        })
    }

    pub fn history(&self) -> anyhow::Result<Vec<RunView>> {
        self.runtime.block_on(history(&self.store, &self.mains()))
    }
}
