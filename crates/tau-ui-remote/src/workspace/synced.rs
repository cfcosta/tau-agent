//! What every interface of one tau must show alike: the computer's
//! window and each phone (decision 0013). The rest of a workspace is
//! the device's own: where it is, what is typed, what is open.

use std::collections::{HashMap, HashSet};

use tau_agent::tool::RunId;

use super::{LandingState, Workspace};
use crate::{
    catalog::Catalog,
    pull_request::PullRequest,
    push::PushState,
    view::{CodeState, RunStatus, RunView},
};

/// The part of a [`Workspace`] the host's updates decide. Two
/// interfaces of one tau that have had the same updates are equal here.
#[derive(Debug, Clone, PartialEq)]
pub struct Synced {
    pub runs: Vec<RunView>,
    pub catalog: Catalog,
    pub queued: HashMap<RunId, Vec<String>>,
    pub resuming: HashMap<RunId, RunStatus>,
    pub closed: HashSet<RunId>,
    pub kept_branch: Option<RunId>,
    pub landings: HashMap<RunId, LandingState>,
    pub pushes: HashMap<String, PushState>,
    pub proposed: HashSet<RunId>,
    pub pull_requests: HashMap<RunId, PullRequest>,
    pub branch_code: HashMap<(RunId, RunId), CodeState>,
    pub query_result: Option<Result<tau_store::Table, String>>,
}

impl Workspace {
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
            landings,
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
            landings: landings.clone(),
            pushes: pushes.clone(),
            proposed: proposed.clone(),
            pull_requests: pull_requests.clone(),
            branch_code: branch_code.clone(),
            query_result: query_result.clone(),
        }
    }
}
