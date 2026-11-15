//! The plugins heiko comes with: the one place they are named (ADR 0045).
//! The kernel, heiko-ui-remote, names none; the computer installs these,
//! and a phone has none of its own (ADR 0046).

use heiko_ui_plugin::Registry;

/// Every plugin, with its UI and without its host half (ADR 0030). The
/// registry puts them in the order each declares (`after` / `before`),
/// the rest by name: the order here does not count.
pub fn plugins() -> Registry {
    Registry::new()
        .with(heiko_agents::AgentsUi)
        .with(heiko_chatgpt::ChatGptUi)
        .with(heiko_codemode::CodemodeUi)
        .with(heiko_direnv::DirenvUi)
        .with(heiko_decide::DecideUi)
        .with(heiko_luau_plugins::LuauPluginsUi)
        .with(heiko_mcp::McpUi)
        .with(heiko_phones::PhonesUi)
        .with(heiko_sqlite::SqliteUi)
        .with(heiko_tools::ui::ToolsUi)
        .with(heiko_vcs::ui::VcsUi)
        // Who fills each slot first when several plugins can, until the
        // person picks another on the Plugins screen (ADR 0045).
        .slot_order(&heiko_ui_plugin::vcs::VCS, &[heiko_vcs::ui::NAME])
        .slot_order(&heiko_ui_plugin::provider::PROVIDER, &[heiko_chatgpt::NAME])
        // heiko-titles, heiko-compaction-chain, heiko-compaction, heiko-github and
        // heiko-notify
        // are Luau (ADR 0050): the slots are heiko-luau-plugins', which
        // fills them for them.
        .slot_order(&heiko_ui_plugin::title::TITLE, &[heiko_luau_plugins::NAME])
        .slot_order(&heiko_ui_plugin::forge::FORGE, &[heiko_luau_plugins::NAME])
        .slot_order(&heiko_ui_plugin::notify::NOTIFY, &[heiko_luau_plugins::NAME])
        .slot_order(
            &heiko_ui_plugin::compaction::COMPACTION,
            &[heiko_luau_plugins::NAME],
        )
}
