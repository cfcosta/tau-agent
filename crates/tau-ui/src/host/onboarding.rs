//! The ChatGPT account runs use, its plan, and the models it offers.

use super::*;

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
    /// The ChatGPT account the plan's runs use, if any.
    pub fn account(&self) -> Option<AccountId> {
        self.config.credentials.plan_account()
    }

    /// Why the provider last refused a run, while runs use it: a usage
    /// limit, a sign-in it no longer takes, a restriction.
    pub fn refusal(&self) -> Option<Refusal> {
        self.provider()?.refusal()
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

    pub(super) fn access_label(&self) -> String {
        self.provider()
            .and_then(|provider| provider.label())
            .unwrap_or_else(|| "signed out".to_owned())
    }

    /// The models the picker offers, with what this sign-in can run, and
    /// the user's choices.
    pub fn models(&self) -> Models {
        let credentials = &self.config.credentials;
        let provider = self.provider();
        Models {
            // The provider's models, while there is one to run on.
            options: provider
                .as_ref()
                .map(|provider| provider.models())
                .unwrap_or_default(),
            settings: self.settings.lock().expect("not poisoned").clone(),
            access: AccessInfo {
                label: self.access_label(),
                chatgpt: self.signed_in(),
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
