//! The interface's state for plugins, kept by the host (ADR 0045): it
//! applies each change an interface makes, saves the result beside the
//! repository list, and gives it to the interface as it attaches.

use tau_ui_plugin::{UiEvent, UiState};

use super::*;

impl Host {
    /// Where the UI state is saved: `ui.json` beside `repos.json`.
    fn ui_state_path(config: &HostConfig) -> PathBuf {
        config.repo_list.with_file_name("ui.json")
    }

    /// The UI state saved last, or none when there is none that reads.
    pub(super) async fn load_ui_state(config: &HostConfig) -> UiState {
        tokio::fs::read_to_string(Self::ui_state_path(config))
            .await
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// The UI state as the host keeps it.
    pub fn ui_state(&self) -> UiState {
        self.ui_state.lock().expect("not poisoned").clone()
    }

    /// Applies `event` and saves the result (ADR 0028).
    pub async fn set_ui(&self, event: &UiEvent) -> anyhow::Result<()> {
        self.apply_ui(event);
        self.save_ui().await
    }

    /// Applies `event` to the UI state the host keeps, at once: events
    /// applied in the order the window made them end as the window does.
    pub fn apply_ui(&self, event: &UiEvent) {
        self.ui_state.lock().expect("not poisoned").apply(event);
        // A pick of who fills a slot holds from now on (ADR 0045).
        if event.plugin == tau_ui_plugin::slots::PICKS {
            self.slots.lock().expect("not poisoned").pick(
                &event.key,
                event.value.as_ref().and_then(serde_json::Value::as_str),
            );
            self.catalog_changed();
        }
    }

    /// Saves the UI state as it is now.
    pub async fn save_ui(&self) -> anyhow::Result<()> {
        let path = Self::ui_state_path(&self.config);
        self.ui_saving
            .save(
                || self.ui_state(),
                async move |state| {
                    if let Some(dir) = path.parent() {
                        tokio::fs::create_dir_all(dir).await?;
                    }
                    tokio::fs::write(&path, serde_json::to_string(&state)?)
                        .await?;
                    Ok(())
                },
            )
            .await
    }
}
