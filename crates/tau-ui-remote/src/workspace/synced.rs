//! What every interface of one tau must show alike: the computer's
//! window and each phone (decision 0013). The rest of a workspace is
//! the device's own: where it is, what is typed, what is open.

use std::collections::{HashMap, HashSet};

use gpui::Context;
use serde::{Deserialize, Serialize};
use tau_agent::tool::RunId;

use super::Workspace;
use crate::{
    catalog::Catalog,
    pull_request::PullRequest,
    push::PushState,
    view::{CodeState, RunStatus, RunView},
};

/// The part of a [`Workspace`] the host's updates decide. Two
/// interfaces of one tau that have had the same updates are equal here.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Synced {
    pub runs: Vec<RunView>,
    pub catalog: Catalog,
    pub queued: HashMap<RunId, Vec<String>>,
    pub resuming: HashMap<RunId, RunStatus>,
    pub closed: HashSet<RunId>,
    pub kept_branch: Option<RunId>,
    pub pushes: HashMap<String, PushState>,
    pub proposed: HashSet<RunId>,
    pub pull_requests: HashMap<RunId, PullRequest>,
    #[serde(with = "pairs")]
    pub branch_code: HashMap<(RunId, RunId), CodeState>,
    pub query_result: Option<Result<tau_store::Table, String>>,
}

impl Workspace {
    /// Shows what another interface shows alike: `synced`, from the host,
    /// in place of what this one had. The screen stays if its run is
    /// still there.
    pub(crate) fn restore(&mut self, synced: Synced, cx: &mut Context<Self>) {
        let Synced {
            runs,
            catalog,
            queued,
            resuming,
            closed,
            kept_branch,
            pushes,
            proposed,
            pull_requests,
            branch_code,
            query_result,
        } = synced;
        self.set_catalog(catalog, cx);
        self.queued = queued;
        self.resuming = resuming;
        self.closed = closed;
        self.kept_branch = kept_branch;
        self.pushes = pushes;
        self.proposed = proposed;
        self.pull_requests = pull_requests;
        self.branch_code = branch_code;
        self.query_result = query_result;
        self.replace_runs(runs, cx);
    }

    /// What this interface shows that every other must show too.
    pub fn synced(&self) -> Synced {
        // Every field is named, so a new one is put on one side or the
        // other before this compiles.
        let Self {
            runs,
            catalog,
            queued,
            resuming,
            closed,
            kept_branch,
            pushes,
            proposed,
            pull_requests,
            branch_code,
            query_result,
            // The device's own.
            name: _,
            route: _,
            back_stack: _,
            current: _,
            composer: _,
            history_filter: _,
            query: _,
            attachments: _,
            searching: _,
            search: _,
            events_open: _,
            sheet_open: _,
            // What this person has read here.
            seen: _,
            hovered_run: _,
            forking: _,
            // Shown on each, dismissed on each.
            dialog: _,
            // A landing's card, as this device asked, waits and puts it
            // away; what the host did to the chat is in its run.
            landings: _,
            plan_alert: _,
            next_model: _,
            next_model_picked: _,
            fork_model: _,
            picker: _,
            model_search: _,
            // Picked here for the next message, which carries it.
            run_models: _,
            inspector_shown: _,
            // How this window is laid out and filtered.
            details_open: _,
            zoom: _,
            interface_settings: _,
            runs_filter: _,
            history_repo: _,
            open_notes: _,
            open_cards: _,
            // Onboarding and pairing are each device's own.
            setup: _,
            pairing: _,
            phones: _,
            pair_address: _,
            pair_code: _,
            phone_name: _,
            github_token: _,
            chatgpt_callback: _,
            repo_filter: _,
            first_task: _,
            pr_title: _,
            reviewers: _,
            repo: _,
            open_repos: _,
            all_runs: _,
            folded: _,
            hovered_repo: _,
            repo_menu: _,
            sidebar_filter: _,
            setup_goal: _,
            setup_motion: _,
            reduce_motion: _,
            adding_jev_key: _,
            jev_key: _,
            slash_selected: _,
            slash_dismissed: _,
            slash_seen: _,
            transcript: _,
            listed: _,
            transcript_width: _,
            follow: _,
            focus: _,
            replays: _,
            phone_preview: _,
            mirrored: _,
            frame: _,
            width: _,
            _subscriptions: _,
            weak: _,
            // Plugins keep what interfaces share in their records, which
            // are folded into the runs.
            plugin_ui: _,
            plugin_requests: _,
            plugin_focus: _,
            composer_replaced: _,
            composer_back: _,
        } = self;
        Synced {
            runs: runs.clone(),
            catalog: catalog.clone(),
            queued: queued.clone(),
            resuming: resuming.clone(),
            closed: closed.clone(),
            kept_branch: kept_branch.clone(),
            pushes: pushes.clone(),
            proposed: proposed.clone(),
            pull_requests: pull_requests.clone(),
            branch_code: branch_code.clone(),
            query_result: query_result.clone(),
        }
    }
}

/// A map with keys JSON cannot have, as a list of pairs.
mod pairs {
    use std::{collections::HashMap, hash::Hash};

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<K, V, S>(
        map: &HashMap<K, V>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        K: Serialize,
        V: Serialize,
        S: Serializer,
    {
        serializer.collect_seq(map.iter())
    }

    pub fn deserialize<'de, K, V, D>(
        deserializer: D,
    ) -> Result<HashMap<K, V>, D::Error>
    where
        K: Deserialize<'de> + Eq + Hash,
        V: Deserialize<'de>,
        D: Deserializer<'de>,
    {
        Ok(Vec::<(K, V)>::deserialize(deserializer)?
            .into_iter()
            .collect())
    }
}
