//! The plugins tau comes with: the one place they are named (ADR 0045).
//! The kernel, tau-ui-remote, names none; the computer installs these,
//! and a phone has none of its own (ADR 0046).

use tau_ui_plugin::Registry;

/// Every plugin, in the order the host adds them to a run's agent, with
/// its UI and without its host half (ADR 0030).
pub fn plugins() -> Registry {
    Registry::new()
        // The tools' cards: the host builds the tools themselves on
        // each run's workspace.
        .with(tau_tools::ui::ToolsUi)
        .with(tau_vcs::ui::VcsUi)
        // Where repositories are hosted: clones, pushes and pull
        // requests.
        .with(tau_github::GitHubUi)
        .with(tau_reasoning::ReasoningPlugin)
        // Pruning first: it is cheaper than a summary, and summarizing
        // follows when pruning cannot help. The tree, when switched on,
        // folds before the summary, which steps in when the tree fails.
        .with(tau_fast_compaction::ui::FastCompactionUi)
        .with(tau_tree_compaction::ui::TreeCompactionUi)
        .with(tau_compaction::ui::CompactionUi)
        // The repository's rules check what the tools do; a goal's
        // hold of a stop comes after theirs.
        // Notes for what the tools do, and the repository's rules to
        // check it.
        .with(tau_memory::ui::MemoryUi)
        .with(tau_constitution::ui::ConstitutionUi)
        .with(tau_goal::GoalUi)
        // Notes on what the person likely missed, once switched on.
        .with(tau_watcher::WatcherUi)
        // Plugins the person wrote in Luau: after the rules and the
        // goal, so theirs hold first.
        .with(tau_luau_plugins::LuauPluginsUi)
        // Agents: their pages, and what their conversations show.
        .with(tau_agents::AgentsUi)
        // A chat's wake-ups, which it schedules for itself.
        .with(tau_schedule::ScheduleUi)
        // Questions the agent asks the person, answered in the
        // composer's place.
        .with(tau_ask::AskUi)
        // What the agent shows the person: images and documents, in a
        // pane beside the run.
        .with(tau_canvas::CanvasUi)
        // Agent commands in the repository's direnv environment, once
        // the person allows it.
        .with(tau_direnv::DirenvUi)
        // MCP servers' tools: their direct ones are added to the run's
        // plan in tau-mcp's `start`, so it comes before Codemode.
        .with(tau_mcp::McpUi)
        // Skills the model loads when a task calls for them.
        .with(tau_skills::SkillsUi)
        // Codemode's `start` lists the Luau signatures of the tools in
        // the run's plan when it runs. A plugin that adds tools in its
        // own `start` (tau-mcp, for its servers' direct tools) must be
        // registered before it, or scripts' signatures miss them.
        .with(tau_codemode::CodemodeUi)
        // Who fills each slot first when several plugins can, until the
        // person picks another on the Plugins screen (ADR 0045).
        .slot_order(&tau_ui_plugin::vcs::VCS, &[tau_vcs::ui::NAME])
        .slot_order(&tau_ui_plugin::forge::FORGE, &[tau_github::NAME])
}
