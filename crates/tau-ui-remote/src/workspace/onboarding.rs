//! Onboarding and accounts: GitHub, the ChatGPT sign-in and plan, the
//! TypeSafe key, and the first run.

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
        let signed_in =
            matches!(update, SetupUpdate::GitHub(GitHub::SignedIn { .. }));
        let connected =
            matches!(update, SetupUpdate::Model(ModelAccess::Connected { .. }));
        self.setup.update(update);
        let goal = self.setup_goal;
        match self.route {
            Route::Setup(SetupStep::GitHub | SetupStep::Token)
                if signed_in && goal == Some(SetupGoal::GitHub) =>
            {
                self.leave_setup(cx)
            }
            Route::Setup(SetupStep::GitHub | SetupStep::Token)
                if signed_in && goal == Some(SetupGoal::Repos) =>
            {
                self.navigate(Route::Setup(SetupStep::Repos), cx)
            }
            Route::Setup(SetupStep::GitHub | SetupStep::Token) if signed_in => {
                self.navigate(Route::Setup(SetupStep::Model), cx)
            }
            Route::Setup(SetupStep::Model)
                if connected && goal == Some(SetupGoal::Model) =>
            {
                self.leave_setup(cx)
            }
            // Onboarding stays to show the account and pick the default
            // model; "Continue" moves on.
            _ => cx.notify(),
        }
    }

    /// What onboarding showed when last drawn, and what changed.
    pub fn setup_motion(&self) -> &crate::motion::SetupMotion {
        &self.setup_motion
    }

    /// Leaves the model step once signed in: on to the repositories, or
    /// to a new run when there are none to pick.
    pub fn continue_from_model(&mut self, cx: &mut Context<Self>) {
        if self.setup_goal == Some(SetupGoal::Model) {
            self.leave_setup(cx);
        } else if self.setup.repos.is_empty() {
            self.finish_setup(cx);
        } else {
            self.navigate(Route::Setup(SetupStep::Repos), cx);
        }
    }

    /// Makes model `id`, one of the account's, the one runs start with:
    /// the coder's default, as the picker sets it.
    pub fn pick_setup_model(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(option) = self.catalog.models.find(id).cloned() else {
            return;
        };
        let choice = ModelChoice {
            model: option.id.clone(),
            ..self.catalog.models.settings.default_for("coder")
        }
        .fitted();
        if let ModelAccess::Connected { label } = &mut self.setup.model {
            *label = format!("{} · ChatGPT plan", option.id);
        }
        self.set_default_model("coder", choice, cx);
    }

    /// Stops waiting for the browser, back to the start of the model
    /// step.
    pub fn cancel_chatgpt_sign_in(&mut self, cx: &mut Context<Self>) {
        self.setup.model = ModelAccess::None;
        self.chatgpt_callback
            .update(cx, |input, cx| input.clear(cx));
        cx.emit(WorkspaceEvent::ChatGptCancel);
        cx.notify();
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

    /// Opens GitHub's sign-in from the app, coming back once signed in.
    pub fn connect_github(&mut self, cx: &mut Context<Self>) {
        self.sign_in_github(cx);
        self.setup_goal = Some(SetupGoal::GitHub);
    }

    pub fn sign_out_github(&mut self, cx: &mut Context<Self>) {
        cx.emit(WorkspaceEvent::GitHubSignOut);
    }

    /// Picks repositories to clone from GitHub, signing in first if
    /// needed, then comes back.
    pub fn pick_github_repos(&mut self, cx: &mut Context<Self>) {
        if self.setup.user().is_some() {
            self.navigate(Route::Setup(SetupStep::Repos), cx);
        } else {
            self.sign_in_github(cx);
        }
        self.setup_goal = Some(SetupGoal::Repos);
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

    pub fn sign_in_github(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.setup.github, GitHub::Waiting(_)) {
            self.setup.github = GitHub::SignedOut;
            cx.emit(WorkspaceEvent::GitHubSignIn);
        }
        self.navigate(Route::Setup(SetupStep::GitHub), cx);
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

    /// Checks the GitHub token typed in onboarding.
    pub(crate) fn submit_token(&mut self, cx: &mut Context<Self>) {
        let Some(token) = Self::take_field(&self.github_token, cx) else {
            return;
        };
        self.setup.github = GitHub::Checking;
        cx.emit(WorkspaceEvent::GitHubToken { token });
        cx.notify();
    }

    /// Opens the model step of onboarding from the Models screen, to
    /// sign in to ChatGPT, coming back once done.
    pub fn connect_model(&mut self, cx: &mut Context<Self>) {
        self.setup.model = ModelAccess::None;
        self.navigate(Route::Setup(SetupStep::Model), cx);
        self.setup_goal = Some(SetupGoal::Model);
    }

    /// Asks for the TypeSafe key.
    pub fn ask_for_jev_key(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.adding_jev_key = true;
        self.jev_key.update(cx, |input, cx| input.clear(cx));
        self.jev_key.read(cx).focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    /// Opens the TypeSafe key prompt from a plugin's click, which has no
    /// window to focus it in.
    pub(crate) fn ask_for_jev_key_later(&mut self, cx: &mut Context<Self>) {
        self.adding_jev_key = true;
        self.jev_key.update(cx, |input, cx| input.clear(cx));
        cx.notify();
    }

    /// Saves the TypeSafe key typed.
    pub(crate) fn submit_jev_key(&mut self, cx: &mut Context<Self>) {
        let Some(key) = Self::take_field(&self.jev_key, cx) else {
            return;
        };
        self.adding_jev_key = false;
        cx.emit(WorkspaceEvent::JevKey { key: Some(key) });
        cx.notify();
    }

    pub fn forget_jev_key(&mut self, cx: &mut Context<Self>) {
        cx.emit(WorkspaceEvent::JevKey { key: None });
    }

    /// The dialog that asks for the TypeSafe key.
    pub(super) fn jev_key_view(
        &self,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let actions = div()
            .flex()
            .gap(sp(2.))
            .child(
                div()
                    .id("jev-key-cancel")
                    .child(ui::button("Cancel", ButtonKind::Secondary, t))
                    .on_click(cx.listener(|ws, _, _, cx| {
                        ws.adding_jev_key = false;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("jev-key-save")
                    .child(ui::button("Use this key", ButtonKind::Primary, t))
                    .on_click(
                        cx.listener(|ws, _, _, cx| ws.submit_jev_key(cx)),
                    ),
            );
        ui::modal(
            ui::icon(Icon::Key, IconSize::LARGE, t.muted),
            "TypeSafe key",
            "tau-constitution asks Jev, TypeSafe's model, whether calls break \
             a repository's rules. The key stays in tau's config directory, \
             readable only by you.",
            Some(
                ui::field(&self.jev_key, true, t)
                    .flex_shrink_0()
                    .into_any_element(),
            ),
            actions,
            t,
        )
    }

    pub fn sign_out(&mut self, cx: &mut Context<Self>) {
        cx.emit(WorkspaceEvent::SignOut);
    }

    /// Signs in with ChatGPT in the browser: `account` again, or a new
    /// account with `None`; `consent` asks again for plan usage.
    pub fn sign_in_chatgpt(
        &mut self,
        account: Option<String>,
        consent: bool,
        cx: &mut Context<Self>,
    ) {
        self.setup.model = ModelAccess::SigningIn { url: None };
        self.chatgpt_callback
            .update(cx, |input, cx| input.clear(cx));
        cx.emit(WorkspaceEvent::ChatGptSignIn { account, consent });
        cx.notify();
    }

    /// Finishes the ChatGPT sign-in with a redirect URL pasted from the
    /// browser.
    pub(crate) fn submit_chatgpt_callback(&mut self, cx: &mut Context<Self>) {
        let Some(url) = Self::take_field(&self.chatgpt_callback, cx) else {
            return;
        };
        cx.emit(WorkspaceEvent::ChatGptCallback { url });
        cx.notify();
    }

    /// Signs in with this saved ChatGPT account from now on.
    pub fn switch_chatgpt(&mut self, account: &str, cx: &mut Context<Self>) {
        cx.emit(WorkspaceEvent::SwitchChatGpt {
            account: account.to_owned(),
        });
    }

    /// Asks again for plan usage on the active ChatGPT account, from the
    /// model setup, coming back once done.
    pub fn enable_plan_usage(&mut self, cx: &mut Context<Self>) {
        let account = self
            .catalog
            .models
            .access
            .active_account()
            .map(|account| account.id.clone());
        self.connect_model(cx);
        self.sign_in_chatgpt(account, true, cx);
    }

    /// Opens ChatGPT Settings → Usage, where the user reviews and limits
    /// what apps use of their plan.
    pub fn manage_usage(&mut self, cx: &mut Context<Self>) {
        cx.open_url(USAGE_SETTINGS_URL);
    }

    /// A run stopped on the ChatGPT plan: says what to do next. A
    /// temporary refusal, which the run already retried, says nothing
    /// more than the run's error.
    pub fn show_plan_refusal(
        &mut self,
        refusal: &tau_ai::refusal::Refusal,
        cx: &mut Context<Self>,
    ) {
        if let Some(alert) = PlanAlert::of(refusal) {
            self.plan_alert = Some(alert);
            cx.notify();
        }
    }

    pub fn plan_alert(&self) -> Option<&PlanAlert> {
        self.plan_alert.as_ref()
    }

    /// Carries out a button of the plan alert, and closes it.
    pub fn plan_action(&mut self, action: PlanAction, cx: &mut Context<Self>) {
        self.plan_alert = None;
        let active = self
            .catalog
            .models
            .access
            .active_account()
            .map(|account| account.id.clone());
        match action {
            PlanAction::ManageUsage => self.manage_usage(cx),
            PlanAction::SignInAgain => {
                self.connect_model(cx);
                self.sign_in_chatgpt(active, false, cx);
            }
            PlanAction::EnablePlanUsage => self.enable_plan_usage(cx),
            PlanAction::Close => {}
        }
        cx.notify();
    }

    /// The plan alert, over the app.
    pub(super) fn plan_alert_view(
        &self,
        alert: &PlanAlert,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (secondary, other) = alert.secondary();
        let actions = div()
            .flex()
            .gap(sp(2.))
            .child(
                div()
                    .id("plan-alert-secondary")
                    .child(ui::button(secondary, ButtonKind::Secondary, t))
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.plan_action(other, cx)
                    })),
            )
            .children(alert.primary().map(|(label, action)| {
                div()
                    .id("plan-alert-primary")
                    .child(ui::button(label, ButtonKind::Primary, t))
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.plan_action(action, cx)
                    }))
            }));
        let glyph = match alert {
            PlanAlert::UsageLimit => {
                ui::icon(Icon::Warning, IconSize::LARGE, t.accent)
            }
            _ => ui::icon(Icon::Chat, IconSize::LARGE, t.muted),
        };
        ui::modal(glyph, alert.title(), alert.message(), None, actions, t)
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
                });
            }
        }
        if !repos.is_empty() {
            cx.emit(WorkspaceEvent::CloneRepos { repos });
        }
        // From the app, the clones show up in the sidebar; from
        // onboarding, the first run comes next.
        if self.setup_goal == Some(SetupGoal::Repos) {
            self.leave_setup(cx);
        } else {
            self.navigate(Route::Setup(SetupStep::Ready), cx);
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
