//! tau-luau-plugins: plugins written in Luau, run in codemode's sandbox,
//! as one tau plugin
//! ([ADR 0027](../../../docs/decisions/0027-luau-plugins.md)). This is
//! its interface half: what every interface shows of a plugin. Loading
//! plugins and running their hooks is `tau-luau-plugins-host`'s
//! (ADR 0030).
//!
//! A Luau plugin is a folder: `plugin.luau`, which returns
//! `tau.plugin { ... }`, modules under `lib/`, tests under `tests/` and
//! a README. The host reads what it declares ([`Declaration`]) and runs
//! each of its hooks in a fresh codemode VM.
//!
//! - [`Declaration`], [`Uses`], [`ToolSpec`], [`Hooks`]: what a plugin
//!   says it is, as every interface shows it.

pub mod pane;
pub mod repository;
pub mod settings;
pub mod ui;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_ui_plugin::Seam;
pub use ui::LuauPluginsUi;

/// The host plugin's name, for both its halves.
pub const NAME: &str = "tau-luau-plugins";

/// The name the host lists the plugins repository by.
pub const REPO: &str = "tau-plugins";

/// The kind of tau's own repository the plugins repository is
/// (`RepoKind::kind`).
pub const KIND: &str = "plugins";

/// The skill tau ships for writing plugins.
pub const SKILL: &str = "tau-plugins";

/// The `tau` module every plugin requires: the declaration, the
/// answers hooks give, the view pieces.
pub const TAU_MODULE: &str = include_str!("tau.luau");

/// What a plugin may reach. The host refuses anything else.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Uses {
    /// Tools its tool handlers may call, by name.
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub jev: bool,
    #[serde(default)]
    pub infer: bool,
    /// `before_tool` may change a call's arguments (`tau.change`).
    #[serde(default)]
    pub change_args: bool,
}

impl Uses {
    /// What `self` reaches that `before` did not, in words: what the
    /// person allows before a version that reaches it activates.
    pub fn grown_from(&self, before: &Uses) -> Vec<String> {
        let mut grown: Vec<String> = self
            .tools
            .iter()
            .filter(|tool| !before.tools.contains(tool))
            .map(|tool| format!("the `{tool}` tool"))
            .collect();
        if self.jev && !before.jev {
            grown.push("Jev".into());
        }
        if self.infer && !before.infer {
            grown.push("`infer`".into());
        }
        if self.change_args && !before.change_args {
            grown.push("changing a tool call's arguments".into());
        }
        grown
    }
}

/// A tool a plugin adds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// The JSON Schema of its arguments.
    pub parameters: Value,
    /// It draws its own card.
    #[serde(default)]
    pub card: bool,
}

/// Which hooks a plugin has.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize,
)]
pub struct Hooks {
    #[serde(default)]
    pub before_tool: bool,
    #[serde(default)]
    pub before_stop: bool,
    #[serde(default)]
    pub turn_end: bool,
    #[serde(default)]
    pub run_end: bool,
    #[serde(default)]
    pub view: bool,
    /// `settings.view`: a settings page of its own. It reaches nothing.
    #[serde(default)]
    pub settings_view: bool,
}

impl Hooks {
    /// The seams `self` has that `before` did not: a hook that can block
    /// a call or keep a run going reaches further.
    pub fn grown_from(&self, before: &Hooks) -> Vec<String> {
        let mut grown = Vec::new();
        if self.before_tool && !before.before_tool {
            grown.push("`before_tool`".into());
        }
        if self.before_stop && !before.before_stop {
            grown.push("`before_stop`".into());
        }
        grown
    }
}

/// What a plugin declares, read without running its hooks.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Declaration {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub uses: Uses,
    /// `{ schema, default }`, when it has settings.
    #[serde(default)]
    pub settings: Option<Value>,
    #[serde(default)]
    pub tools: Vec<ToolSpec>,
    #[serde(default)]
    pub hooks: Hooks,
    #[serde(default)]
    pub actions: Vec<String>,
}

impl Declaration {
    /// What `self` reaches that `before` did not: its new tools to call,
    /// Jev, `infer`, and the seams that block or continue. Empty when a
    /// version may activate by itself.
    pub fn grown_from(&self, before: &Declaration) -> Vec<String> {
        let mut grown = self.uses.grown_from(&before.uses);
        grown.extend(self.hooks.grown_from(&before.hooks));
        grown
    }

    /// The seams its tools and hooks step in at, as the Plugins screen
    /// lists them.
    pub fn seams(&self) -> Vec<Seam> {
        let hooks = &self.hooks;
        [
            (!self.tools.is_empty(), Seam::Tools),
            (hooks.before_tool, Seam::BeforeTool),
            (hooks.before_stop, Seam::BeforeStop),
            (hooks.turn_end, Seam::OnEvent),
            (hooks.run_end, Seam::Finish),
        ]
        .into_iter()
        .filter_map(|(has, seam)| has.then_some(seam))
        .collect()
    }

    /// A plugin named `name` that declares nothing: what one never
    /// allowed before may reach.
    pub fn named(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            ..Self::default()
        }
    }

    /// Its settings' defaults, or an empty object.
    pub fn default_settings(&self) -> Value {
        self.settings
            .as_ref()
            .and_then(|settings| settings.get("default"))
            .cloned()
            .unwrap_or_else(|| Value::Object(Default::default()))
    }
}

/// One case of a plugin's tests, as it came out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestResult {
    /// Its file under `tests/`, without `.luau`.
    pub file: String,
    pub name: String,
    pub passed: bool,
    #[serde(default)]
    pub error: Option<String>,
}

/// Where a plugin of the plugins repository stands (ADR 0027).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Standing {
    /// Its version at trunk is the one runs get.
    Active,
    /// It reaches further than the version the person allowed: it waits
    /// for them, and the version before stays active, if there is one.
    Waiting { grown: Vec<String> },
    /// Its tests fail at trunk: the version before stays active.
    Failing,
    /// It does not load at trunk: the version before stays active.
    Broken { error: String },
}

/// A plugin of the plugins repository, as the Plugins screen shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// Its folder.
    pub name: String,
    pub description: String,
    pub standing: Standing,
    /// What it declares at trunk, when it loads.
    pub declaration: Option<Declaration>,
    pub tests: Vec<TestResult>,
    /// An earlier version is active while this one is not.
    pub keeps_earlier: bool,
}

/// The plugins repository as the host last read it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Overview {
    /// Trunk's commit, once read.
    pub commit: Option<String>,
    pub plugins: Vec<Entry>,
    /// Why the repository could not be read, if it could not.
    pub error: Option<String>,
}

/// What the plugins page asks the host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "act", rename_all = "snake_case")]
pub enum Act {
    /// Lets `plugin`'s waiting version reach what it asks, and activates
    /// it.
    Allow { plugin: String },
    /// Draws `plugin`'s settings page (`settings.view`) of `settings`,
    /// its inputs' ids starting with `fields`; the answer is a fetched
    /// tree ([`pane::settings_view`]).
    SettingsView {
        plugin: String,
        settings: Value,
        fields: String,
    },
    /// Carries out `plugin`'s `actions[action]` with `args`, on the state
    /// it has in `run`, under the person's `settings` for it; the answer
    /// is an [`ActionDone`], which the interface records with the run.
    Action {
        plugin: String,
        action: String,
        args: Value,
        run: String,
        state: Value,
        settings: Option<Value>,
    },
}

/// What a plugin's action left, for the interface to record with the
/// run it was carried out on: the state, the view drawn from it, the
/// lines it logged, and why it failed, if it did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionDone {
    pub plugin: String,
    pub run: String,
    pub state: Value,
    pub view: Option<Value>,
    pub logs: Vec<String>,
    pub error: Option<(String, String)>,
}

impl ActionDone {
    /// The records to store with the run, in order.
    pub fn records(&self) -> Vec<Record> {
        let plugin = || self.plugin.clone();
        let mut records = vec![Record::State {
            plugin: plugin(),
            state: self.state.clone(),
        }];
        if !self.logs.is_empty() {
            records.push(Record::Log {
                plugin: plugin(),
                lines: self.logs.clone(),
            });
        }
        if let Some((hook, error)) = &self.error {
            records.push(Record::Error {
                plugin: plugin(),
                hook: hook.clone(),
                error: error.clone(),
            });
        }
        if let Some(view) = &self.view {
            records.push(Record::View {
                plugin: plugin(),
                view: view.clone(),
            });
        }
        records
    }
}

/// A Luau plugin's settings page as the host drew it, for the settings
/// it shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingsPage {
    pub plugin: String,
    pub settings: Value,
    /// The view tree, or why it could not be drawn.
    pub page: Result<Value, String>,
}

/// What the person set for each Luau plugin, by plugin: tau-luau-plugins'
/// own settings, kept everywhere or as a repository's copy (ADR 0029).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LuauSettings {
    #[serde(default)]
    pub plugins: BTreeMap<String, Value>,
}

/// What the host publishes about a run's Luau plugins, as tau-luau-plugins'
/// records: live and stored runs, on the computer and the phone, fold
/// them the same way.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Record {
    /// A plugin's state for the run, whole, after a hook changed it.
    State { plugin: String, state: Value },
    /// What its `view` drew from that state.
    View { plugin: String, view: Value },
    /// Lines it wrote with `ctx.log`.
    Log { plugin: String, lines: Vec<String> },
    /// A hook failed: it counted as allowing, or as stopping.
    Error {
        plugin: String,
        hook: String,
        error: String,
    },
    /// `before_tool` let a call through, flagged for review.
    Flag {
        plugin: String,
        call_id: String,
        reason: String,
    },
    /// The plugin is off for the rest of the run, and why.
    Off { plugin: String, why: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declaring(tools: &[&str], jev: bool, before_tool: bool) -> Declaration {
        Declaration {
            name: "p".into(),
            description: String::new(),
            uses: Uses {
                tools: tools.iter().map(|t| t.to_string()).collect(),
                jev,
                infer: false,
                change_args: false,
            },
            settings: None,
            tools: Vec::new(),
            hooks: Hooks {
                before_tool,
                ..Hooks::default()
            },
            actions: Vec::new(),
        }
    }

    /// A version reaches further when it calls a new tool, asks Jev, or
    /// gains a seam that blocks; dropping any of them is not growth.
    #[test]
    fn growth_is_what_a_version_newly_reaches() {
        let before = declaring(&["exec_command"], false, false);
        assert!(before.grown_from(&before).is_empty());
        let after = declaring(&["exec_command", "read"], true, true);
        assert_eq!(
            after.grown_from(&before),
            ["the `read` tool", "Jev", "`before_tool`"]
        );
        assert!(before.grown_from(&after).is_empty());
    }

    /// Each hook is the seam it steps in at, and a plugin's tools are
    /// `Tools`: `turn_end` hears the run's events.
    #[test]
    fn its_seams_are_its_tools_and_hooks() {
        let mut declaration = declaring(&[], false, false);
        assert!(declaration.seams().is_empty());
        declaration.hooks = Hooks {
            before_tool: true,
            before_stop: true,
            turn_end: true,
            run_end: true,
            view: true,
            settings_view: true,
        };
        assert_eq!(
            declaration.seams(),
            [
                Seam::BeforeTool,
                Seam::BeforeStop,
                Seam::OnEvent,
                Seam::Finish
            ]
        );
    }
}
