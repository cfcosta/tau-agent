//! The ChatGPT account runs use, its plan, and the models it offers.

use super::*;

/// Carries out onboarding when no model is configured: Sign in with
/// ChatGPT. Once a sign-in with plan use is saved, `ready` gets its
/// account to build a [`Host`] from, and the host handles sign-ins from
/// then on.
///
/// GitHub sign-ins work before a model is connected, so onboarding can
/// start with them.
pub fn onboard(
    workspace: &Entity<Workspace>,
    model: Option<String>,
    credentials: Credentials,
    cx: &mut App,
    ready: impl Fn(AccountId, &mut App) + 'static,
) {
    let done = std::rc::Rc::new(std::cell::Cell::new(false));
    let connected: accounts::Connected = {
        let done = done.clone();
        std::rc::Rc::new(move |account, cx| {
            // A sign-in without plan usage connects nothing yet.
            if let Some(account) = account {
                done.set(true);
                ready(account, cx)
            }
        })
    };
    let sign_ins = accounts::SignIns::default();
    let api = github::Api::default();
    github::restore(workspace, &credentials, &api, cx);
    cx.subscribe(workspace, move |workspace, event: &WorkspaceEvent, cx| {
        // Once a host runs, it answers.
        if done.get() {
            return;
        }
        let _ = sign_ins.handle(
            event,
            &workspace,
            &credentials,
            model.as_deref(),
            &connected,
            cx,
        ) || github::handle(event, &workspace, &credentials, &api, cx);
    })
    .detach();
}

/// Checks on the host's runtime whether the account just signed in may
/// use its plan, then, if onboarding is on the model step and it may
/// not, says so. Nothing happens signed out.
pub(super) fn check_eligibility(
    host: &Arc<Host>,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    let Some(check) = host.check_eligibility() else {
        return;
    };
    let catalog = host.spawn(async move |host| {
        let _ = check.await;
        host.catalog().await
    });
    let (host, workspace) = (host.clone(), workspace.downgrade());
    cx.spawn(async move |cx| {
        let Ok(catalog) = catalog.await else {
            return;
        };
        let refused = host.not_eligible();
        let _ = workspace.update(cx, |ws, cx| {
            let account = catalog
                .models
                .access
                .active_account()
                .map(|account| account.label.clone())
                .unwrap_or_default();
            ws.apply(HostUpdate::catalog(catalog), cx);
            // Onboarding just signed in: say the account cannot share its
            // plan, rather than showing it as connected.
            if let (Some(detail), Route::Setup(SetupStep::Model)) =
                (refused, ws.route())
            {
                ws.update_setup(
                    SetupUpdate::Model(ModelAccess::NotEligible {
                        account,
                        detail,
                    }),
                    cx,
                );
            }
        });
    })
    .detach();
}

/// The refusal's words when `error` says plan use is not available to
/// the account: `403 subscription_sharing_user_not_eligible · request
/// req_…`. `None` for any other failure.
pub(super) fn not_eligible(
    error: &tau_ai::chatgpt::ChatGptError,
) -> Option<String> {
    use tau_ai::chatgpt::ChatGptError;
    if error.recovery() != tau_ai::retry::Recovery::Restricted {
        return None;
    }
    Some(match error {
        ChatGptError::Api(api) => {
            let mut detail = api.status.to_string();
            if let Some(code) = api.code() {
                detail.push_str(&format!(" {code}"));
            }
            if let Some(id) = &api.request_id {
                detail.push_str(&format!(" · request {id}"));
            }
            detail
        }
        other => other.to_string(),
    })
}

impl Host {
    /// The ChatGPT account runs reach models with, if any.
    pub fn account(&self) -> Option<AccountId> {
        self.account.lock().expect("not poisoned").clone()
    }

    /// Why the ChatGPT plan last refused a run, while runs use it: a
    /// usage limit, a sign-in OpenAI no longer takes, a restriction.
    pub fn refusal(&self) -> Option<Refusal> {
        self.account()?;
        self.client
            .lock()
            .expect("not poisoned")
            .as_ref()?
            .refusal()
    }

    /// Checks, once per sign-in, whether the active account may use
    /// its plan here: `GET /v1/models`, whose listing is not shown (the
    /// picker offers the model table's, [`plan_models`]); only a
    /// restricted refusal matters. The check runs on the host's runtime
    /// and the returned task ends when it is saved; `None` means no
    /// account is active.
    pub fn check_eligibility(&self) -> Option<tokio::task::JoinHandle<()>> {
        let account = self.account()?;
        let chatgpt = self.config.credentials.chatgpt();
        let refused = self.not_eligible.clone();
        Some(self.runtime.spawn(async move {
            let checked = match chatgpt {
                Ok(chatgpt) => chatgpt.models(&account).await.map(drop),
                Err(error) => Err(error),
            };
            *refused.lock().expect("not poisoned") =
                checked.err().as_ref().and_then(not_eligible);
        }))
    }

    /// Why the last eligibility check said plan use is not available to
    /// the account, if it did.
    pub fn not_eligible(&self) -> Option<String> {
        self.not_eligible.lock().expect("not poisoned").clone()
    }

    pub(super) fn access_label(&self) -> &'static str {
        if self.account().is_some() {
            "ChatGPT plan"
        } else {
            "signed out"
        }
    }

    /// Runs started from now on reach models on `account`'s plan; runs
    /// going on keep theirs. `None` stops new runs until one is set.
    pub fn set_account(
        &self,
        account: Option<AccountId>,
    ) -> anyhow::Result<()> {
        if let Some(account) = &account {
            let (agent, client) = coder(
                &self.runtime,
                account,
                &self.config.credentials,
                &self.config.default_model(),
            )?;
            *self.base.lock().expect("not poisoned") = agent;
            *self.client.lock().expect("not poisoned") = Some(client);
        }
        *self.account.lock().expect("not poisoned") = account;
        Ok(())
    }

    /// The models the picker offers, with what this sign-in can run, and
    /// the user's choices.
    pub fn models(&self) -> Models {
        let account = self.account();
        let credentials = &self.config.credentials;
        Models {
            // The plan's models, from the model table, while signed in.
            options: if account.is_some() {
                plan_models()
            } else {
                Vec::new()
            },
            settings: self.settings.lock().expect("not poisoned").clone(),
            access: AccessInfo {
                label: self.access_label().into(),
                chatgpt: account.is_some(),
                jev: self.has_jev_key(),
                accounts: credentials.accounts(),
            },
            agents: vec![(
                "coder".into(),
                "Runs you start from the composer.".into(),
            )],
        }
    }

    /// Keeps and saves the user's model choices.
    pub async fn save_settings(
        &self,
        settings: ModelSettings,
    ) -> anyhow::Result<()> {
        self.change_settings(|saved| *saved = settings).await
    }

    /// Changes the user's model choices as kept now, and saves them.
    pub async fn change_settings(
        &self,
        change: impl FnOnce(&mut ModelSettings),
    ) -> anyhow::Result<()> {
        change(&mut self.settings.lock().expect("not poisoned"));
        self.persist_settings().await
    }
}
