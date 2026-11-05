//! The slots a Luau plugin fills (ADR 0045). tau-luau-plugins fills each
//! once, as tau starts, with a stand-in that asks the active plugin that
//! fills it on every call: the person's plugins change while tau runs. A
//! slot no active plugin fills answers as an empty filler would, and a
//! hook that fails or runs out of time answers nothing.

use serde_json::json;
use tau_ui_plugin::{compaction::Compaction, title::Title};
use tokio_util::sync::CancellationToken;

use crate::{
    registry::Registry,
    runtime::{Context, Hook, NoReach},
};

/// What a hook outside any run has as its `ctx`: no run, no state.
fn context(registry: &Registry, active: &crate::agent::Active) -> Context {
    Context {
        run: json!({}),
        now: crate::agent::now(),
        settings: active.settings.clone(),
        state: json!({}),
        secrets: registry.secrets_of(&active.loaded.declaration.name),
    }
}

/// `tau.title` from the active plugin that fills it.
pub struct LuauTitle(pub Registry);

impl Title for LuauTitle {
    fn write<'a>(
        &'a self,
        task: &'a str,
        run_model: &'a str,
    ) -> tau_ui_plugin::title::Written<'a> {
        Box::pin(async move {
            let Some(active) =
                self.0.filler(tau_ui_plugin::title::TITLE.name()).await
            else {
                return Ok(None);
            };
            let outcome = active
                .loaded
                .call(
                    &Hook::Title,
                    json!({ "task": task, "model": run_model }),
                    &context(&self.0, &active),
                    std::sync::Arc::new(NoReach),
                    CancellationToken::new(),
                )
                .await;
            if let Some(error) = outcome.error {
                anyhow::bail!(
                    "{}'s title: {error}",
                    active.loaded.declaration.name
                );
            }
            Ok(outcome
                .value
                .as_str()
                .map(str::trim)
                .filter(|title| !title.is_empty())
                .map(|title| {
                    title
                        .chars()
                        .take(tau_ui_plugin::title::MAX_CHARS)
                        .collect()
                }))
        })
    }
}

/// `tau.compaction` from the active plugin that fills it: the Luau
/// plugins' own rewrites, and the plugins it names.
pub struct LuauCompaction(pub Registry);

impl Compaction for LuauCompaction {
    fn rewriters(&self) -> Vec<String> {
        let mut rewriters = vec![tau_luau_plugins::NAME.to_owned()];
        let filler = self
            .0
            .filler_now(tau_ui_plugin::compaction::COMPACTION.name());
        if let Some(fill) =
            filler.and_then(|active| active.loaded.declaration.compaction)
        {
            rewriters.extend(fill.rewriters);
        }
        rewriters
    }
}
