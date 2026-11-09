//! The plugins tau comes with: the one place they are named (ADR 0045).
//! The kernel, tau-ui-remote, names none; the computer installs these,
//! and a phone has none of its own (ADR 0046).

use tau_ui_plugin::Registry;

/// Every plugin, with its UI and without its host half (ADR 0030). The
/// registry puts them in the order each declares (`after` / `before`),
/// the rest by name: the order here does not count.
pub fn plugins() -> Registry {
    Registry::new()
        .with(tau_agents::AgentsUi)
        .with(tau_canvas::CanvasUi)
        .with(tau_chatgpt::ChatGptUi)
        .with(tau_codemode::CodemodeUi)
        .with(tau_direnv::DirenvUi)
        .with(tau_github::GitHubUi)
        .with(tau_jev::JevUi)
        .with(tau_luau_plugins::LuauPluginsUi)
        .with(tau_mcp::McpUi)
        .with(tau_memory::ui::MemoryUi)
        .with(tau_notify::NotifyUi)
        .with(tau_phones::PhonesUi)
        .with(tau_skills::SkillsUi)
        .with(tau_sqlite::SqliteUi)
        .with(tau_tools::ui::ToolsUi)
        .with(tau_vcs::ui::VcsUi)
        // Who fills each slot first when several plugins can, until the
        // person picks another on the Plugins screen (ADR 0045).
        .slot_order(&tau_ui_plugin::vcs::VCS, &[tau_vcs::ui::NAME])
        .slot_order(&tau_ui_plugin::forge::FORGE, &[tau_github::NAME])
        .slot_order(&tau_ui_plugin::provider::PROVIDER, &[tau_chatgpt::NAME])
        // tau-titles, tau-compaction-chain and tau-compaction are Luau
        // (ADR 0050): the slots are tau-luau-plugins', which fills them
        // for them.
        .slot_order(&tau_ui_plugin::title::TITLE, &[tau_luau_plugins::NAME])
        .slot_order(&tau_ui_plugin::notify::NOTIFY, &[tau_notify::NAME])
        .slot_order(
            &tau_ui_plugin::compaction::COMPACTION,
            &[tau_luau_plugins::NAME],
        )
}
