//! Opening a run's work as a pull request, and the code a fork changed.

use super::*;

impl Workspace {
    /// The code of a comparison, as the host found it.
    pub fn set_branch_code(
        &mut self,
        main: &RunId,
        fork: &RunId,
        code: CodeState,
        cx: &mut Context<Self>,
    ) {
        self.branch_code.insert((main.clone(), fork.clone()), code);
        cx.notify();
    }

    pub fn branch_code(
        &self,
        main: &RunId,
        fork: &RunId,
    ) -> Option<&CodeState> {
        self.branch_code.get(&(main.clone(), fork.clone()))
    }

    pub fn pull_request(&self, run: &RunId) -> Option<&PullRequest> {
        self.pull_requests.get(run)
    }

    /// Opens the pull request screen for a run, asking the host for a
    /// draft if there is none yet.
    pub fn open_pull_request(&mut self, run: &RunId, cx: &mut Context<Self>) {
        match self.pull_requests.get(run) {
            Some(pr) => {
                let title = pr.title.clone();
                self.pr_title
                    .update(cx, |input, cx| input.set_text(title, cx));
            }
            None => {
                cx.emit(WorkspaceEvent::PreparePullRequest { run: run.clone() })
            }
        }
        self.navigate(Route::PullRequest(run.clone()), cx);
    }

    /// The draft the host wrote from a run.
    pub fn set_pull_request(
        &mut self,
        run: &RunId,
        pr: PullRequest,
        cx: &mut Context<Self>,
    ) {
        if self.route == Route::PullRequest(run.clone()) {
            let title = pr.title.clone();
            self.pr_title
                .update(cx, |input, cx| input.set_text(title, cx));
        }
        self.pull_requests.insert(run.clone(), pr);
        cx.notify();
    }

    /// What became of a pull request: creating, opened or failed.
    pub fn set_pull_request_state(
        &mut self,
        run: &RunId,
        state: PrState,
        cx: &mut Context<Self>,
    ) {
        if let Some(pr) = self.pull_requests.get_mut(run) {
            pr.state = state;
            cx.notify();
        }
    }

    pub(crate) fn toggle_pr_option(
        &mut self,
        run: &RunId,
        draft: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(pr) = self.pull_requests.get_mut(run) {
            if draft {
                pr.draft = !pr.draft;
            } else {
                pr.keep_pushing = !pr.keep_pushing;
            }
            cx.notify();
        }
    }

    pub fn create_pull_request(&mut self, run: &RunId, cx: &mut Context<Self>) {
        let title = self.pr_title.read(cx).text().trim().to_owned();
        let reviewers: Vec<String> = self
            .reviewers
            .read(cx)
            .text()
            .split([',', ' '])
            .map(|name| name.trim().trim_start_matches('@'))
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect();
        let Some(pr) = self.pull_requests.get_mut(run) else {
            return;
        };
        if !title.is_empty() {
            pr.title = title;
        }
        pr.state = PrState::Creating;
        let event = WorkspaceEvent::CreatePullRequest {
            run: run.clone(),
            title: pr.title.clone(),
            body: pr.body.clone(),
            draft: pr.draft,
            keep_pushing: pr.keep_pushing,
            reviewers,
        };
        cx.emit(event);
        cx.notify();
    }
}
