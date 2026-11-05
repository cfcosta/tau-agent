//! Onboarding and accounts: the ChatGPT sign-in and plan, plugins'
//! steps around the repositories and the first run.

use tau_ui_plugin::provider::{StopAction, Stopped};

use super::*;

impl Workspace {
    pub fn setup(&self) -> &Setup {
        &self.setup
    }

    /// Replaces what onboarding knows, as when resuming it.
    pub fn set_setup(&mut self, setup: Setup, cx: &mut Context<Self>) {
        self.setup = setup;
        cx.notify();
    }

    /// Opens onboarding at `step`, with no way back to the runs until it
    /// is done.
    pub fn start_setup(&mut self, step: SetupStep, cx: &mut Context<Self>) {
        self.back_stack.clear();
        self.route = Route::Setup(step);
        self.entered(cx);
    }

    /// Records what the host learned, and moves on when a step is done.
    pub fn update_setup(
        &mut self,
        update: SetupUpdate,
        cx: &mut Context<Self>,
    ) {
        self.setup.update(update);
        cx.notify();
    }

    /// What onboarding showed when last drawn, and what changed.
    pub fn setup_motion(&self) -> &crate::motion::SetupMotion {
        &self.setup_motion
    }

    /// Leaves the welcome for setup's first step: the provider's
    /// sign-in, when a plugin offers one.
    pub fn start_setup_steps(&mut self, cx: &mut Context<Self>) {
        self.setup_next(SetupStep::Welcome, cx);
    }

    /// Plugins' steps of first-run setup for the setup at hand
    /// (`points::SETUP`), in order, each with what it draws.
    pub fn plugin_setup_steps(
        &self,
    ) -> Vec<(SetupStep, tau_ui_plugin::points::SetupView)> {
        let at = tau_ui_plugin::points::AtSetup {
            repos: self.setup.picked(),
        };
        self.contributions_by(tau_ui_plugin::points::SETUP, &at)
            .into_iter()
            .map(|(plugin, view)| {
                let step = SetupStep::Plugin {
                    plugin,
                    step: view.step,
                };
                (step, view)
            })
            .collect()
    }

    /// The step bar's stages: tau's, with plugins' steps first and
    /// around the repositories.
    pub fn setup_stages(&self) -> Vec<Stage> {
        Stage::all(
            self.plugin_setup_steps()
                .into_iter()
                .map(|(step, view)| (step, view.title, view.at)),
        )
    }

    /// Setup's steps, in order: plugins' first (the model's), plugins'
    /// before the repositories, the repositories, plugins' after them.
    fn setup_sequence(&self) -> Vec<SetupStep> {
        self.setup_stages()
            .into_iter()
            .map(|stage| stage.step)
            .filter(|step| *step != SetupStep::Ready)
            .collect()
    }

    /// Whether the repositories' step has something to pick: repositories
    /// listed, a forge signed in to list them, or the person asked for it.
    fn repos_to_pick(&self) -> bool {
        !self.setup.repos.is_empty()
            || self.catalog.pull_requests
            || self.setup_goal == Some(SetupGoal::Repos)
    }

    /// Goes on from `from`, the welcome, the repositories or a plugin's
    /// step: to the next step, skipping the repositories when there are
    /// none to pick, else to the end of setup. A plugin's step that is
    /// gone goes on from after the repositories.
    fn setup_next(&mut self, from: SetupStep, cx: &mut Context<Self>) {
        let sequence = self.setup_sequence();
        let after_repos = sequence
            .iter()
            .position(|step| *step == SetupStep::Repos)
            .map_or(0, |n| n + 1);
        let start = match from {
            SetupStep::Welcome => 0,
            _ => sequence
                .iter()
                .position(|step| *step == from)
                .map_or(after_repos, |n| n + 1),
        };
        let repos_to_pick = self.repos_to_pick();
        let next = sequence
            .into_iter()
            .skip(start)
            .find(|step| *step != SetupStep::Repos || repos_to_pick);
        match next {
            Some(step) => self.navigate(Route::Setup(step), cx),
            None => self.end_setup_steps(cx),
        }
    }

    /// Past the last step: the host hears setup is done, once, and the
    /// clones and the first run's prompt show, or with nothing cloned, a
    /// new run.
    fn end_setup_steps(&mut self, cx: &mut Context<Self>) {
        if !self.setup_finished {
            self.setup_finished = true;
            cx.emit(WorkspaceEvent::SetupFinished {
                repos: self.setup.picked(),
            });
        }
        if self.setup.clones.is_empty() {
            self.finish_setup(cx);
        } else {
            self.navigate(Route::Setup(SetupStep::Ready), cx);
        }
    }

    /// Ends the plugin's step on screen, from the plugin
    /// (`Request::SetupDone`) or the person skipping it: on to the next.
    pub fn plugin_setup_step_done(
        &mut self,
        plugin: &str,
        cx: &mut Context<Self>,
    ) {
        if let Route::Setup(step @ SetupStep::Plugin { plugin: on, .. }) =
            self.route
            && on == plugin
        {
            self.setup_next(step, cx);
        }
    }

    /// Goes back to the screen an onboarding step was opened from.
    pub fn leave_setup(&mut self, cx: &mut Context<Self>) {
        let mut route = self.back_stack.pop().unwrap_or(Route::Home);
        while let Route::Setup(_) = route {
            route = self.back_stack.pop().unwrap_or(Route::Home);
        }
        self.route = route;
        self.entered(cx);
    }

    /// Picks repositories to clone from the forge, through plugins' steps
    /// before the repositories (its sign-in) while the forge is not
    /// signed in, then comes back.
    pub fn pick_repos(&mut self, cx: &mut Context<Self>) {
        // The first step placed before the repositories: the model's
        // sign-in comes before them in setup, but runs are signed in by now.
        let first =
            self.plugin_setup_steps()
                .into_iter()
                .find_map(|(step, view)| {
                    (view.at
                        == tau_ui_plugin::points::SetupAt::BeforeRepositories)
                        .then_some(step)
                });
        let step = match first {
            Some(step) if !self.catalog.pull_requests => step,
            _ => SetupStep::Repos,
        };
        self.setup_goal = Some(SetupGoal::Repos);
        self.navigate(Route::Setup(step), cx);
    }

    /// Leaves onboarding for a new run.
    pub fn finish_setup(&mut self, cx: &mut Context<Self>) {
        self.back_stack.clear();
        self.route = if self.runs.is_empty() {
            Route::NewRun
        } else {
            Route::Home
        };
        self.entered(cx);
    }

    /// What `field` holds, trimmed, taken out of it; none when it is
    /// empty.
    pub(super) fn take_field(
        field: &Entity<TextInput>,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let text = field.read(cx).text().trim().to_owned();
        if text.is_empty() {
            return None;
        }
        field.update(cx, |input, cx| input.clear(cx));
        Some(text)
    }

    /// A run stopped on the provider: says what to do next, as the
    /// provider words it (`Provider::stopped`).
    pub fn show_stopped(&mut self, stopped: Stopped, cx: &mut Context<Self>) {
        self.plan_alert = Some(stopped);
        cx.notify();
    }

    pub fn plan_alert(&self) -> Option<&Stopped> {
        self.plan_alert.as_ref()
    }

    /// Carries out the alert's button, `action`, or closes it with none.
    pub fn plan_action(
        &mut self,
        action: Option<StopAction>,
        cx: &mut Context<Self>,
    ) {
        self.plan_alert = None;
        match action {
            Some(StopAction::Open(url)) => cx.open_url(&url),
            // The provider's section of the Models screen signs in.
            Some(StopAction::Models) => self.navigate(Route::Models, cx),
            None => {}
        }
        cx.notify();
    }

    /// The provider's alert, over the app.
    pub(super) fn plan_alert_view(
        &self,
        alert: &Stopped,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let actions = div()
            .flex()
            .gap(sp(2.))
            .child(
                div()
                    .id("plan-alert-secondary")
                    .child(ui::button("Close", ButtonKind::Secondary, t))
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.plan_action(None, cx)
                    })),
            )
            .children(alert.action.clone().map(|(label, action)| {
                div()
                    .id("plan-alert-primary")
                    .child(ui::button(label, ButtonKind::Primary, t))
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.plan_action(Some(action.clone()), cx)
                    }))
            }));
        ui::modal(
            ui::icon(Icon::Warning, IconSize::LARGE, t.accent),
            alert.title.clone(),
            alert.message.clone(),
            None,
            actions,
            t,
        )
    }

    /// Shows every repository in the picker, or only the first ones.
    pub fn toggle_repos_expanded(&mut self, cx: &mut Context<Self>) {
        self.repos_expanded = !self.repos_expanded;
        cx.notify();
    }

    /// Whether the picker shows every repository.
    pub fn repos_expanded(&self) -> bool {
        self.repos_expanded
    }

    pub fn toggle_repo(&mut self, name: &str, cx: &mut Context<Self>) {
        self.setup.toggle(name);
        cx.notify();
    }

    /// Clones the picked repositories and opens the first run's screen.
    pub fn clone_selected(&mut self, cx: &mut Context<Self>) {
        let repos: Vec<String> = self
            .setup
            .selected()
            .map(|repo| repo.name.clone())
            .collect();
        for name in &repos {
            if !self.setup.clones.iter().any(|clone| &clone.name == name) {
                self.setup.clones.push(RepoClone {
                    name: name.clone(),
                    state: CloneState::Cloning {
                        share: 0.0,
                        detail: "starting".into(),
                    },
                    id: None,
                });
            }
        }
        if !repos.is_empty() {
            cx.emit(WorkspaceEvent::CloneRepos { repos });
        }
        // From the app, the clones show up in the sidebar; from
        // onboarding, plugins' steps and the first run come next.
        if self.setup_goal == Some(SetupGoal::Repos) {
            self.leave_setup(cx);
        } else {
            self.setup_next(SetupStep::Repos, cx);
        }
    }

    /// Starts the first run, on the task typed at onboarding's end.
    pub(crate) fn start_first_run(&mut self, cx: &mut Context<Self>) {
        let Some(task) = Self::take_field(&self.first_task, cx) else {
            return;
        };
        self.finish_setup(cx);
        self.start_in_repo(task, cx);
    }
}
