//! What runs reach models with, as the host tells the interface: the
//! provider's label, why it stopped a run and its models (ADR 0045),
//! and the person's
//! choices.

use super::*;

impl Host {
    /// Why the provider last stopped a run, in its words, while runs use
    /// it: a usage limit, a sign-in it no longer takes, a restriction.
    pub fn stopped(&self) -> Option<tau_ui_plugin::provider::Stopped> {
        self.provider()?.stopped()
    }

    pub(super) fn access_label(&self) -> String {
        self.provider()
            .and_then(|provider| provider.label())
            .unwrap_or_else(|| "signed out".to_owned())
    }

    /// The models the picker offers, with what this sign-in can run, and
    /// the user's choices.
    pub fn models(&self) -> Models {
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
                signed_in: self.signed_in(),
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
        self.change_settings_now(change);
        self.persist_settings().await
    }

    /// Changes the user's model choices as kept now, at once: changes
    /// made in the order the window made them end as the window does.
    /// Saving them is [`Host::save_kept_settings`].
    pub fn change_settings_now(&self, change: impl FnOnce(&mut ModelSettings)) {
        change(&mut self.settings.lock().expect("not poisoned"));
    }

    /// Saves the user's model choices as kept now.
    pub async fn save_kept_settings(&self) -> anyhow::Result<()> {
        self.persist_settings().await
    }
}
