//! tau-luau-plugins: plugins written in Luau, run in codemode's sandbox,
//! as one tau plugin
//! ([ADR 0027](../../../docs/decisions/0027-luau-plugins.md)). This is
//! its interface half: what every interface shows of a plugin. Loading
//! plugins and running their hooks is `tau-luau-plugins-host`'s
//! (ADR 0030).
//!
//! A Luau plugin is a package (ADR 0051): `tau.toml`, `src/plugin.luau`,
//! which returns `tau.plugin { ... }`, modules beside it, tests under
//! `tests/` and
//! a README. The host reads what it declares ([`Declaration`]) and runs
//! each of its hooks in a fresh codemode VM.
//!
//! - [`Declaration`], [`Uses`], [`ToolSpec`], [`Hooks`]: what a plugin
//!   says it is, as every interface shows it.

pub mod names;
pub mod packages;
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

/// The `tau_test` module the tests run with: `tau.test`, apart from
/// `tau`, which every hook loads.
pub const TAU_TEST_MODULE: &str = include_str!("tau_test.luau");

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
    /// The hosts its HTTP requests may reach, by name: `ntfy.sh`.
    #[serde(default)]
    pub network: Vec<String>,
    /// The secrets those requests may name, kept for the plugin alone.
    #[serde(default)]
    pub secrets: Vec<String>,
    /// The model `infer` asks in hooks outside any run; `title` asks the
    /// run's own when it names none.
    #[serde(default)]
    pub model: Option<String>,
    /// A store of its own in each repository (`ctx.store`).
    #[serde(default)]
    pub store: bool,
    /// The run's conversation, read whole (`ctx.transcript()`).
    #[serde(default)]
    pub transcript: bool,
    /// Files of what it cuts from the context, which only their owner
    /// can read (`ctx.archive`), for the model to read back.
    #[serde(default)]
    pub archive: bool,
    /// Its jobs wake chats (`ctx.chats.wake`), and reach its store in
    /// every repository (ADR 0050, gap D).
    #[serde(default)]
    pub chats: bool,
    /// The folders its tools read, read-only (`ctx.files`, ADR 0050, gap
    /// G): [`WORKSPACE`] for the run's own, `~` or `~/x` under the home
    /// folder, [`PACKAGE`] or [`DATA`] and folders in them (gap K), or an
    /// absolute path.
    #[serde(default)]
    pub files: Vec<String>,
    /// The folders of [`Self::files`] it writes too (`{ "folder", write
    /// = true }`, ADR 0050, gap L): its own in tau's folders,
    /// [`own_folders`], with no Allow; any other with an Allow of its own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub writes: Vec<String>,
}

/// What `uses.files` names the run's workspace by.
pub const WORKSPACE: &str = "workspace";

/// What `uses.files` names the plugin's own package by, wherever it is
/// installed (ADR 0050, gap K); `package/x` is a folder in it.
pub const PACKAGE: &str = "package";

/// What `uses.files` names tau's data folder by; `data/x` is a folder in
/// it.
pub const DATA: &str = "data";

/// What `uses.files` names the person's skills folder by, as the host has
/// it (`~/.agents/skills`).
pub const SKILLS: &str = "skills";

/// What `uses.files` names tau's own directory for the run's repository
/// by, beside its rules and archive (ADR 0050, gap L); `repository/x` is
/// a folder in it.
pub const REPOSITORY: &str = "repository";

/// The folders `plugin` writes with no Allow: its own, in tau's data
/// folder and in tau's directory for a repository, and the folders in
/// them.
pub fn own_folders(plugin: &str) -> [String; 2] {
    [format!("{DATA}/{plugin}"), format!("{REPOSITORY}/{plugin}")]
}

/// Whether `folder` of `uses.files` is one of `plugin`'s own, or in one.
pub fn is_own_folder(plugin: &str, folder: &str) -> bool {
    !plugin.is_empty()
        && own_folders(plugin).iter().any(|own| {
            folder == own
                || folder
                    .strip_prefix(own.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
        })
}

/// What writing a folder of `uses.files` is, in words for the Allow.
pub fn write_words(folder: &str) -> String {
    folder_words(folder).replacen("reading", "writing", 1)
}

/// What a folder of `uses.files` is, in words for the Allow.
pub fn folder_words(folder: &str) -> String {
    let under = |root: &str| {
        folder
            .strip_prefix(root)
            .and_then(|rest| rest.strip_prefix('/'))
    };
    if folder == WORKSPACE {
        "reading the run's workspace".to_owned()
    } else if folder == PACKAGE || under(PACKAGE).is_some() {
        "reading its own package".to_owned()
    } else if folder == SKILLS {
        "reading your skills folder".to_owned()
    } else if folder == DATA {
        "reading tau's data folder".to_owned()
    } else if let Some(rest) = under(DATA) {
        format!("reading `{rest}` in tau's data folder")
    } else if folder == REPOSITORY {
        "reading tau's folder for the repository".to_owned()
    } else if let Some(rest) = under(REPOSITORY) {
        format!("reading `{rest}` in tau's folder for the repository")
    } else {
        format!("reading files in `{folder}`")
    }
}

impl Uses {
    /// What `self` reaches that `before` did not, in words: what the
    /// person allows before a version of `plugin` that reaches it
    /// activates. Writing its own folders is no new reach (ADR 0050, gap
    /// L).
    pub fn grown_from(&self, before: &Uses, plugin: &str) -> Vec<String> {
        let mut grown: Vec<String> = self
            .tools
            .iter()
            .filter(|tool| !before.tools.contains(tool))
            .map(|tool| format!("the `{tool}` tool"))
            .collect();
        if self.archive && !before.archive {
            grown.push("files of what it cuts".into());
        }
        if self.jev && !before.jev {
            grown.push("Jev".into());
        }
        if self.infer && !before.infer {
            grown.push("`infer`".into());
        }
        if self.store && !before.store {
            grown.push("a store of its own".into());
        }
        if self.transcript && !before.transcript {
            grown.push("reading the run's conversation".into());
        }
        grown.extend(
            self.files
                .iter()
                .filter(|folder| !before.files.contains(folder))
                .map(|folder| folder_words(folder)),
        );
        grown.extend(
            self.writes
                .iter()
                .filter(|folder| {
                    !before.writes.contains(folder)
                        && !is_own_folder(plugin, folder)
                })
                .map(|folder| write_words(folder)),
        );
        if self.chats && !before.chats {
            grown.push("waking chats".into());
        }
        if self.change_args && !before.change_args {
            grown.push("changing a tool call's arguments".into());
        }
        grown.extend(
            self.network
                .iter()
                .filter(|host| !before.network.contains(host))
                .map(|host| format!("reaching `{host}`")),
        );
        grown.extend(
            self.secrets
                .iter()
                .filter(|secret| !before.secrets.contains(secret))
                .map(|secret| format!("the secret `{secret}`")),
        );
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
    /// The boolean setting it is offered while it is on: the model sees
    /// it only then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
    /// Only the model calls it, never a script: a call that waits for
    /// the person (`tau.ask`) would end with the script.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub model_only: bool,
}

impl ToolSpec {
    /// Whether a run on `settings` offers it.
    pub fn offered(&self, settings: &Value) -> bool {
        self.when
            .as_ref()
            .is_none_or(|key| settings[key.as_str()] == Value::Bool(true))
    }
}

/// Which hooks a plugin has.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize,
)]
pub struct Hooks {
    #[serde(default)]
    pub start: bool,
    #[serde(default)]
    pub before_tool: bool,
    #[serde(default)]
    pub before_request: bool,
    #[serde(default)]
    pub after_tool_result: bool,
    #[serde(default)]
    pub rewrite_context: bool,
    #[serde(default)]
    pub rewritten: bool,
    #[serde(default)]
    pub before_stop: bool,
    #[serde(default)]
    pub turn_end: bool,
    #[serde(default)]
    pub on_event: bool,
    /// `title`: a run's title, for `tau.title`.
    #[serde(default)]
    pub title: bool,
    /// `notify`: where tau's notices go, for `tau.notify`.
    #[serde(default)]
    pub notify: bool,
    /// `repo_data`: what it keeps of a repository, for its sidebar entry
    /// and pages there.
    #[serde(default)]
    pub repo_data: bool,
    /// `recover`: what it lets go of for runs tau found cut off as it
    /// started.
    #[serde(default)]
    pub recover: bool,
    #[serde(default)]
    pub run_end: bool,
    #[serde(default)]
    pub view: bool,
    /// `settings.view`: a settings page of its own. It reaches nothing.
    #[serde(default)]
    pub settings_view: bool,
    /// `app`: what it keeps of the whole app, its tools' folders read
    /// (ADR 0050, gap K).
    #[serde(default)]
    pub app: bool,
    /// `page`: the page its `nav` opens, drawn of what `app` keeps.
    #[serde(default)]
    pub page: bool,
    /// `setup`: its step of first-run setup, drawn of what `app` keeps
    /// (ADR 0050, gap E); one of tau's own only.
    #[serde(default)]
    pub setup: bool,
    /// `models`: its section of the Models screen, drawn of what `app`
    /// keeps.
    #[serde(default)]
    pub models: bool,
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
        // They change what the model reads.
        if self.after_tool_result && !before.after_tool_result {
            grown.push("`after_tool_result`".into());
        }
        if self.start && !before.start {
            grown.push("`start`".into());
        }
        if self.rewrite_context && !before.rewrite_context {
            grown.push("`rewrite_context`".into());
        }
        grown
    }
}

/// The run's events `on_event` may hear, as `events` names them.
pub const EVENTS: [&str; 8] = [
    "turn_start",
    "tool_start",
    "tool_end",
    "retry",
    "steered",
    "continued",
    "context_rewritten",
    "plugin_error",
];

/// The slots a Luau plugin may fill (ADR 0045), as `fills` names them.
pub const SLOTS: [&str; 4] = [
    tau_ui_plugin::title::TITLE.name(),
    tau_ui_plugin::compaction::COMPACTION.name(),
    tau_ui_plugin::notify::NOTIFY.name(),
    tau_ui_plugin::forge::FORGE.name(),
];

/// One of its slash commands: run on the run it is typed in, as its
/// action `/name`, with the text after it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandSpec {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// What follows the name, as the composer's menu shows it.
    #[serde(default)]
    pub args: String,
    /// One of the interface's icons, by name; empty for the plugin mark.
    #[serde(default)]
    pub icon: String,
    /// What the composer's popover shows while it is written.
    #[serde(default)]
    pub popover: Option<CommandPopover>,
}

/// A command's popover: its pieces with a Jev key saved and without.
/// Their inputs' drafts reach the command's `run` as `ctx.drafts`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandPopover {
    pub jev: Value,
    pub no_jev: Value,
}

/// Its entry under each repository in the sidebar, which opens one of
/// its pages for that repository.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoNav {
    pub label: String,
    /// The page it opens, one of `pages`.
    pub page: String,
    #[serde(default)]
    pub icon: String,
}

/// What each Luau plugin's `repo_data` gave of one repository, by plugin:
/// what its sidebar entry and pages there read.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RepoValues(pub BTreeMap<String, Value>);

/// One of its jobs: work it does by itself, every `every` seconds, while
/// tau runs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobSpec {
    pub name: String,
    pub every: u64,
    /// What it does, as the jobs it shows say.
    #[serde(default)]
    pub label: String,
}

/// The shortest a job may wait between runs, in seconds: a tick, for
/// what comes due by the second.
pub const SHORTEST_EVERY: u64 = 1;

/// A job that waits less than this between runs is a tick: it runs
/// outside the person's jobs, and keeps when it ran in memory alone.
pub const TICK_UNDER: u64 = 60;

/// Its entry in the sidebar, which opens its page: the page `page` of
/// the app, of `pages`, when it names one (ADR 0050, gap L).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Nav {
    pub label: String,
    /// One of the interface's icons, by its file's name; a plug when it
    /// names none it knows.
    #[serde(default)]
    pub icon: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
}

/// The forge's hooks, as its `forge` table names them.
pub const FORGE_HOOKS: [&str; 5] =
    ["account", "repos", "can_push", "open_pull", "checks"];

/// What a plugin that fills `tau.forge` says of the forge: its name, its
/// clones' directory, the host repositories clone from, the secret that
/// signs in, and which of [`FORGE_HOOKS`] it has.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgeFill {
    pub name: String,
    pub id: String,
    pub host: String,
    pub token: String,
    #[serde(default)]
    pub hooks: Vec<String>,
}

/// What a plugin that fills `tau.compaction` says: the plugins whose
/// rewrites run, besides the Luau plugins' own.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionFill {
    #[serde(default)]
    pub rewriters: Vec<String>,
}

/// What a plugin declares, read without running its hooks.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Declaration {
    /// From its `tau.toml`, as the description is (ADR 0051).
    #[serde(default)]
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
    /// The run's events `on_event` hears, of [`EVENTS`].
    #[serde(default)]
    pub events: Vec<String>,
    /// The slots it fills, of [`SLOTS`].
    #[serde(default)]
    pub fills: Vec<String>,
    /// What it fills `tau.compaction` with.
    #[serde(default)]
    pub compaction: Option<CompactionFill>,
    /// What it fills `tau.forge` with.
    #[serde(default)]
    pub forge: Option<ForgeFill>,
    /// Its entry in the sidebar.
    #[serde(default)]
    pub nav: Option<Nav>,
    /// Its slash commands.
    #[serde(default)]
    pub commands: Vec<CommandSpec>,
    /// Its jobs.
    #[serde(default)]
    pub jobs: Vec<JobSpec>,
    /// Its pages of a repository, by name (`pages`).
    #[serde(default)]
    pub pages: Vec<String>,
    /// Its entry under each repository in the sidebar.
    #[serde(default)]
    pub repo_nav: Option<RepoNav>,
    /// What the effort picker's Auto says, with a Jev key saved and
    /// without (`picker_auto`).
    #[serde(default)]
    pub picker_auto: Option<PickerAuto>,
    /// Times its `before_stop` may keep a run going (`continues`), when
    /// it says.
    #[serde(default)]
    pub continues: Option<u32>,
    /// Its `commands` are `{ list, run }`: listed from what `app` keeps,
    /// each run as the action `/name` (ADR 0050, gap K).
    #[serde(default)]
    pub listed_commands: bool,
    /// Its entry in each repository's menu, opening one of its pages of
    /// that repository (`repo_menu`, ADR 0050, gap L).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_menu: Option<RepoNav>,
    /// The inputs its pages read as typed (`drafts`): each page is drawn
    /// again as what one holds changes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drafts: Vec<String>,
    /// The keys it binds, of [`KEYS`], each to one of its pages of the
    /// repository the person is in (`keys`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub keys: BTreeMap<String, String>,
}

/// The keys tau leaves to Luau plugins (ADR 0050, gap L): the first
/// active plugin by name that binds one has it; tau-memory binds
/// `ctrl-2` to the repository's notes.
pub const KEYS: &[&str] = &["ctrl-2"];

/// The words under the effort picker's Auto, as a plugin that picks the
/// effort says them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PickerAuto {
    /// With a Jev key saved.
    pub jev: String,
    /// Without one.
    pub no_jev: String,
}

impl Declaration {
    /// What `self` reaches that `before` did not: its new tools to call,
    /// Jev, `infer`, and the seams that block or continue. Empty when a
    /// version may activate by itself.
    pub fn grown_from(&self, before: &Declaration) -> Vec<String> {
        let mut grown = self.uses.grown_from(&before.uses, &self.name);
        grown.extend(self.hooks.grown_from(&before.hooks));
        // A job works with no one watching: always new reach.
        grown.extend(
            self.jobs
                .iter()
                .filter(|job| !before.jobs.iter().any(|b| b.name == job.name))
                .map(|job| format!("the job `{}`", job.name)),
        );
        // Filling a slot is always new reach.
        grown.extend(
            self.fills
                .iter()
                .filter(|slot| !before.fills.contains(slot))
                .map(|slot| format!("filling `{slot}`")),
        );
        grown
    }

    /// The seams its tools and hooks step in at, as the Plugins screen
    /// lists them.
    pub fn seams(&self) -> Vec<Seam> {
        let hooks = &self.hooks;
        [
            (!self.tools.is_empty(), Seam::Tools),
            (hooks.start, Seam::Start),
            (hooks.before_tool, Seam::BeforeTool),
            (hooks.before_request, Seam::BeforeRequest),
            (hooks.after_tool_result, Seam::AfterTool),
            (hooks.rewrite_context, Seam::Rewrite),
            (hooks.rewrite_context, Seam::StartIdle),
            (hooks.rewritten, Seam::Rewritten),
            (hooks.before_stop, Seam::BeforeStop),
            (hooks.turn_end || hooks.on_event, Seam::OnEvent),
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
    /// What its `tau.toml` says, when it was read; a digest names a version.
    #[serde(default)]
    pub version: String,
    pub standing: Standing,
    /// What it declares at trunk, when it loads.
    pub declaration: Option<Declaration>,
    pub tests: Vec<TestResult>,
    /// An earlier version is active while this one is not.
    pub keeps_earlier: bool,
    /// It comes with tau, not from the plugins repository: always active,
    /// and its name is no repository plugin's.
    #[serde(default)]
    pub built_in: bool,
    /// What it keeps of the whole app, as the host last read it (ADR 0050,
    /// gap K); none for a plugin with no `app`.
    #[serde(default)]
    pub app: Option<AppView>,
}

/// What a plugin's `app` keeps, and what it draws and lists of it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AppView {
    /// What `app` gave; its `nav` gives the sidebar entry's `detail` and
    /// `badge`.
    pub value: Value,
    /// What `commands.list` gave of it.
    #[serde(default)]
    pub commands: Vec<CommandSpec>,
    /// What `page` drew of it: the page `nav` opens.
    #[serde(default)]
    pub page: Option<Value>,
    /// What `setup` drew: `{ step, at, title, heading, detail, body }`.
    #[serde(default)]
    pub setup: Option<Value>,
    /// What `models` drew: its section of the Models screen.
    #[serde(default)]
    pub models: Option<Value>,
    /// The folders `uses.files` names, where they are: what
    /// `tau.interface("reveal")` may show.
    #[serde(default)]
    pub roots: Vec<String>,
    /// Why `app` failed, when it did.
    #[serde(default)]
    pub error: Option<String>,
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
    /// Keeps `value` as `plugin`'s secret `name`, which only its HTTP
    /// requests reach.
    SetSecret {
        plugin: String,
        name: String,
        value: String,
    },
    /// Draws `plugin`'s page `page` of `params`: of the repository `repo`
    /// among them, or of the app without one (ADR 0050, gap L);
    /// the answer is a fetched tree.
    Page {
        plugin: String,
        page: String,
        params: BTreeMap<String, String>,
        /// Whether a Jev key is saved, as the interface sees it.
        #[serde(default)]
        jev: bool,
        /// What the inputs its `drafts` names hold, by id.
        #[serde(default)]
        drafts: BTreeMap<String, String>,
    },
    /// Draws `plugin`'s step of first-run setup with the repositories
    /// `picked` so far, `{ name, folder, remote }` each (ADR 0050, gap
    /// L); the answer is a fetched tree, the step's body.
    Setup { plugin: String, picked: Vec<Value> },
    /// Carries out `plugin`'s `actions[action]` with `args` from one of
    /// its pages of the repository `repo`: no run's state, its store
    /// there.
    RepoAction {
        plugin: String,
        repo: String,
        action: String,
        args: Value,
    },
    /// Carries out `plugin`'s `actions[action]` with `args` from its page
    /// of the app (ADR 0050, gap K): no run, no repository.
    AppAction {
        plugin: String,
        action: String,
        args: Value,
    },
    /// The bytes of `plugin`'s blob `blob` in the repository `repo`, for
    /// a `ui.image` the interface draws: `{ data }` in base64, or
    /// `{ error }`.
    Blob {
        plugin: String,
        repo: String,
        blob: Value,
    },
    /// The person's answer to what the call `call` of `run` asked them
    /// to allow (`tau.allow`): from whichever device answers first.
    Answer {
        run: String,
        call: String,
        allowed: bool,
        #[serde(default)]
        reason: String,
    },
    /// Carries out `plugin`'s `actions[action]` with `args`, on the state
    /// it has in `run`, under the person's `settings` for it; the answer
    /// is an [`ActionDone`], which the interface records with the run.
    Action {
        plugin: String,
        action: String,
        args: Value,
        run: String,
        /// The repository the run works in, for the plugin's store.
        #[serde(default)]
        repo: Option<String>,
        state: Value,
        settings: Option<Value>,
        /// Whether the run is going on, as the interface sees it.
        #[serde(default)]
        live: bool,
        /// Whether a Jev key is saved, as the interface sees it.
        #[serde(default)]
        jev: bool,
    },
}

/// What a plugin's action left, for the interface to record with the
/// run it was carried out on: the state, the view drawn from it, the
/// lines it logged, and why it failed, if it did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionDone {
    pub plugin: String,
    pub run: String,
    /// The state the action was carried out on.
    #[serde(default)]
    pub on: Value,
    pub state: Value,
    /// Whether the action changed the state. One that did not records
    /// neither it nor its view: the run may have moved on meanwhile, as
    /// when the action gave a waiting call its answers.
    #[serde(default)]
    pub changed: bool,
    pub view: Option<Value>,
    pub logs: Vec<String>,
    pub error: Option<(String, String)>,
    /// What the interface does once it is done (`tau.interface`):
    /// `{ action, args }`, one of tau's button actions.
    #[serde(default)]
    pub interface: Option<Value>,
}

impl ActionDone {
    /// The records to store with the run, in order.
    pub fn records(&self) -> Vec<Record> {
        let plugin = || self.plugin.clone();
        let mut records = Vec::new();
        if self.changed {
            records.push(Record::Acted {
                plugin: plugin(),
                on: self.on.clone(),
                state: self.state.clone(),
                view: self.view.clone(),
            });
        }
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
        records
    }
}

/// Whether an action carried out `on` a state still applies to a plugin
/// whose state is `current`. No state yet is the empty one an action is
/// given then.
pub fn still_on(current: Option<&Value>, on: &Value) -> bool {
    let empty = |state: &Value| {
        state.is_null() || state.as_object().is_some_and(|all| all.is_empty())
    };
    match current {
        Some(current) if !empty(current) => current == on,
        _ => empty(on),
    }
}

/// Each plugin's last state as `records` leave it, oldest first: what a
/// run reads back of what it and its interfaces stored.
pub fn last_states<'a>(
    records: impl IntoIterator<Item = &'a Value>,
) -> std::collections::HashMap<String, Value> {
    let mut last = std::collections::HashMap::new();
    for record in records {
        match serde_json::from_value::<Record>(record.clone()) {
            Ok(Record::State { plugin, state }) => {
                last.insert(plugin, state);
            }
            Ok(Record::Acted {
                plugin, on, state, ..
            }) if still_on(last.get(&plugin), &on) => {
                last.insert(plugin, state);
            }
            _ => {}
        }
    }
    last
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
    /// An action's result: its state, and what its `view` drew from it,
    /// taken only if the plugin's state is still the one it was carried
    /// out `on`. Otherwise something moved the run on meanwhile, as when
    /// another device's answers reached a waiting call, and this is
    /// dropped: every reader of the run's records drops it alike.
    Acted {
        plugin: String,
        on: Value,
        state: Value,
        view: Option<Value>,
    },
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
    /// A call of its tool asks the person to allow what it would do
    /// (`tau.allow`): a card says `title` and `text`.
    Asked {
        plugin: String,
        call_id: String,
        title: String,
        text: String,
    },
    /// What came of the ask.
    Answered {
        plugin: String,
        call_id: String,
        outcome: Outcome,
    },
}

/// What came of what a call asked the person to allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Still waiting for them.
    Waiting,
    Allowed,
    Denied,
    /// No one can answer it any more: its run ended first.
    Withdrawn,
}

/// A call's ask, as its records leave it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ask {
    pub plugin: String,
    pub title: String,
    pub text: String,
    pub outcome: Outcome,
}

/// `text` as a `&'static str`, each once: the few names a setup step or
/// a forge gives as the interface wants them.
pub fn interned(text: &str) -> &'static str {
    static SEEN: std::sync::Mutex<Vec<&'static str>> =
        std::sync::Mutex::new(Vec::new());
    let mut seen = SEEN.lock().expect("not poisoned");
    if let Some(known) = seen.iter().find(|known| **known == text) {
        return known;
    }
    let leaked: &'static str = Box::leak(text.to_owned().into_boxed_str());
    seen.push(leaked);
    leaked
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
                network: Vec::new(),
                secrets: Vec::new(),
                model: None,
                store: false,
                transcript: false,
                archive: false,
                chats: false,
                files: Vec::new(),
                writes: Vec::new(),
            },
            settings: None,
            tools: Vec::new(),
            hooks: Hooks {
                before_tool,
                ..Hooks::default()
            },
            actions: Vec::new(),
            events: Vec::new(),
            fills: Vec::new(),
            compaction: None,
            forge: None,
            nav: None,
            commands: Vec::new(),
            jobs: Vec::new(),
            pages: Vec::new(),
            repo_nav: None,
            picker_auto: None,
            continues: None,
            listed_commands: false,
            repo_menu: None,
            drafts: Vec::new(),
            keys: BTreeMap::new(),
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
            start: true,
            before_tool: true,
            before_request: true,
            after_tool_result: true,
            rewrite_context: true,
            rewritten: true,
            before_stop: true,
            turn_end: true,
            on_event: true,
            title: true,
            notify: true,
            repo_data: true,
            recover: true,
            run_end: true,
            view: true,
            settings_view: true,
            app: false,
            page: false,
            setup: false,
            models: false,
        };
        assert_eq!(
            declaration.seams(),
            [
                Seam::Start,
                Seam::BeforeTool,
                Seam::BeforeRequest,
                Seam::AfterTool,
                Seam::Rewrite,
                Seam::StartIdle,
                Seam::Rewritten,
                Seam::BeforeStop,
                Seam::OnEvent,
                Seam::Finish
            ]
        );
    }
}
