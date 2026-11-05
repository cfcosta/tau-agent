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
        .with(tau_ask::AskUi)
        .with(tau_canvas::CanvasUi)
        .with(tau_codemode::CodemodeUi)
        .with(tau_compaction::ui::CompactionUi)
        .with(tau_constitution::ui::ConstitutionUi)
        .with(tau_direnv::DirenvUi)
        .with(tau_fast_compaction::ui::FastCompactionUi)
        .with(tau_github::GitHubUi)
        .with(tau_goal::GoalUi)
        .with(tau_luau_plugins::LuauPluginsUi)
        .with(tau_mcp::McpUi)
        .with(tau_memory::ui::MemoryUi)
        .with(tau_reasoning::ReasoningPlugin)
        .with(tau_schedule::ScheduleUi)
        .with(tau_skills::SkillsUi)
        .with(tau_tools::ui::ToolsUi)
        .with(tau_tree_compaction::ui::TreeCompactionUi)
        .with(tau_vcs::ui::VcsUi)
        .with(tau_watcher::WatcherUi)
        // Who fills each slot first when several plugins can, until the
        // person picks another on the Plugins screen (ADR 0045).
        .slot_order(&tau_ui_plugin::vcs::VCS, &[tau_vcs::ui::NAME])
        .slot_order(&tau_ui_plugin::forge::FORGE, &[tau_github::NAME])
}
