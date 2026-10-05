//! tau-mcp's UI (ADR 0017): the Servers page, where servers are added,
//! edited, approved and reconnected, with their tools, resources,
//! templates and prompts; the cards of MCP tools; its row under each
//! repository in the sidebar; its line in a run's plugin list; and the
//! servers' prompts as composer commands, `/mcp__<server>__<prompt>
//! key=value ...`, whose messages the host fetches into the composer.
//! The connections behind them are `tau-mcp-host`'s (ADR 0030).

pub mod card;
pub mod page;

use std::collections::BTreeSet;

use gpui::{AppContext as _, Context};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tau_ui_kit::{assets::Icon, input::TextInput, theme::Tone};
use tau_ui_plugin::{
    Handle,
    Link,
    ListedCommand,
    Manifest,
    NavEntry,
    Page,
    PluginStatus,
    PluginUi,
    UiPlugin,
    ViewCx,
    points::{self, AtRepo, AtRun},
};

use crate::{
    NAME,
    config::{McpConfig, Off, ServerConfig, Settings, valid_name},
    info::{Annotations, PromptArgument, ResourceInfo, TemplateInfo},
    prompts::{arguments_hint, check_arguments, parse_arguments},
};

/// tau-mcp with its UI.
#[derive(Debug, Clone, Copy, Default)]
pub struct McpUi;

/// Where a server's entry came from, as the page says it.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Defined {
    /// `~/.config/tau/mcp.json`.
    #[default]
    User,
    /// Added on the page: the only ones it edits.
    Settings,
    /// `<repository>/.tau/mcp.json`, approved.
    Repo,
}

impl Defined {
    /// Where an entry of `origin` is defined.
    pub fn of(origin: crate::config::Origin) -> Self {
        match origin {
            crate::config::Origin::User => Self::User,
            crate::config::Origin::Settings => Self::Settings,
            crate::config::Origin::Repo => Self::Repo,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::User => "user file",
            Self::Settings => "settings",
            Self::Repo => "repository file",
        }
    }
}

/// One server, as the page lists it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerRow {
    pub name: String,
    pub defined: Defined,
    /// The command and its arguments, or the URL. Never a header's or a
    /// variable's value.
    pub transport: String,
    /// `direct`, `codemode` or `hidden`.
    pub exposure: String,
    pub enabled: bool,
    pub description: Option<String>,
    /// `connecting`, `connected`, `disconnected`, `failed` or `closed`,
    /// or `disabled`; `None` until it starts.
    pub state: Option<String>,
    /// One connection shared by every repository: a server of the
    /// user's file or the settings that does not run per repository.
    pub shared: bool,
    /// Why it is off, when it is.
    pub off: Option<Off>,
    pub error: Option<String>,
    pub tools: Vec<ToolRow>,
    /// Its resources and templates, but MCP apps', and its prompts, as
    /// last listed.
    pub resources: Vec<ResourceInfo>,
    pub templates: Vec<TemplateInfo>,
    pub prompts: Vec<PromptRow>,
    /// The entry, as the page edits it: only a settings server's.
    pub entry: Option<Value>,
    /// Its sign-in, when OAuth applies to it.
    pub auth: Option<AuthRow>,
}

/// A server's sign-in, as the page shows it. Never a token.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthRow {
    pub signed_in: bool,
    /// Who signed in, when the authorization server said.
    pub account: Option<String>,
    /// The scopes granted.
    pub scopes: Vec<String>,
    /// Where it signed in: the authorization server's issuer.
    pub issuer: Option<String>,
    /// Unix seconds the access token expires, when known.
    pub expires_at: Option<u64>,
    /// Whether the access token can be refreshed.
    pub refreshes: bool,
    /// The scopes the server asks for beyond those granted.
    pub wants_scope: Option<String>,
}

/// One of a server's prompts, and the composer command that gets it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PromptRow {
    /// Without the slash: `mcp__<server>__<prompt>`.
    pub command: String,
    /// As the server lists it.
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub arguments: Vec<PromptArgument>,
}

impl PromptRow {
    pub fn info(&self) -> crate::info::PromptInfo {
        crate::info::PromptInfo {
            name: self.name.clone(),
            title: self.title.clone(),
            description: self.description.clone(),
            arguments: self.arguments.clone(),
        }
    }

    /// What the composer's menu says it does.
    pub fn hint(&self, server: &str) -> String {
        self.description
            .as_deref()
            .or(self.title.as_deref())
            .and_then(|text| text.lines().next())
            .map_or_else(
                || format!("MCP prompt {} from {server}", self.name),
                str::to_owned,
            )
    }
}

/// One of a server's tools.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolRow {
    /// As tools call it, `mcp__<server>__<tool>`; `None` for a hidden
    /// tool, which nothing calls.
    pub name: Option<String>,
    /// As the server lists it.
    pub tool: String,
    pub description: Option<String>,
    pub exposure: String,
    pub annotations: Annotations,
}

/// A repository server waiting for the user's approval.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PendingRow {
    pub name: String,
    pub transport: String,
    /// The entry as approving it approves it, whole.
    pub entry: Value,
    /// What approving it saves.
    pub hash: String,
}

/// The servers of a repository, or the user's alone: the plugin's data
/// for each repository, and across them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Servers {
    /// Whether their connections are started: on the first run in the
    /// repository, or Connect on the page.
    pub started: bool,
    pub servers: Vec<ServerRow>,
    pub pending: Vec<PendingRow>,
    /// Entries and files that were skipped, each with where and why.
    pub errors: Vec<String>,
    /// The names the user's file has, which the page refuses.
    pub user_names: BTreeSet<String>,
    pub user_file: Option<String>,
    pub repo_file: Option<String>,
}

impl Servers {
    /// The prompt whose command is `command`, with its server.
    pub fn prompt(&self, command: &str) -> Option<(&ServerRow, &PromptRow)> {
        self.servers.iter().find_map(|server| {
            server
                .prompts
                .iter()
                .find(|prompt| prompt.command == command)
                .map(|prompt| (server, prompt))
        })
    }

    /// Every prompt of a server that is on, as a composer command.
    pub fn commands(&self) -> Vec<ListedCommand> {
        self.servers
            .iter()
            .filter(|server| server.enabled)
            .flat_map(|server| {
                server.prompts.iter().map(|prompt| ListedCommand {
                    name: prompt.command.clone(),
                    hint: prompt.hint(&server.name),
                    args: arguments_hint(&prompt.info()),
                    icon: Icon::Plug,
                })
            })
            .collect()
    }

    pub fn connected(&self) -> usize {
        self.servers
            .iter()
            .filter(|server| server.state.as_deref() == Some("connected"))
            .count()
    }

    /// Servers waiting for a sign-in.
    pub fn needs_sign_in(&self) -> usize {
        self.servers
            .iter()
            .filter(|server| server.state.as_deref() == Some(NEEDS_AUTH))
            .count()
    }

    pub fn failed(&self) -> usize {
        self.servers
            .iter()
            .filter(|server| server.state.as_deref() == Some("failed"))
            .count()
    }

    /// The tool tools call `name`, with its server.
    pub fn tool(&self, name: &str) -> Option<(&ServerRow, &ToolRow)> {
        self.servers.iter().find_map(|server| {
            server
                .tools
                .iter()
                .find(|tool| tool.name.as_deref() == Some(name))
                .map(|tool| (server, tool))
        })
    }

    /// `3 servers · 2 connected · 1 needs sign-in · 1 needs approval`.
    pub fn summary(&self) -> String {
        summary_with_sign_in(
            self.servers.len(),
            self.started.then(|| self.connected()),
            self.needs_sign_in(),
            self.pending.len(),
        )
    }
}

/// A server's state while it waits for a sign-in.
pub const NEEDS_AUTH: &str = "needs-auth";

/// `3 servers · 2 connected · 1 needs approval`; connected is left out
/// before the servers start (`None`), and approval when none waits.
pub fn summary(
    servers: usize,
    connected: Option<usize>,
    pending: usize,
) -> String {
    summary_with_sign_in(servers, connected, 0, pending)
}

/// [`summary`] with the servers waiting for a sign-in:
/// `3 servers · 2 connected · 1 needs sign-in · 1 needs approval`.
pub fn summary_with_sign_in(
    servers: usize,
    connected: Option<usize>,
    sign_in: usize,
    pending: usize,
) -> String {
    let mut parts = vec![match servers {
        0 => "no servers".to_owned(),
        1 => "1 server".to_owned(),
        n => format!("{n} servers"),
    }];
    if let Some(connected) = connected.filter(|_| servers > 0) {
        parts.push(format!("{connected} connected"));
    }
    match sign_in {
        0 => {}
        1 => parts.push("1 needs sign-in".to_owned()),
        n => parts.push(format!("{n} need sign-in")),
    }
    match pending {
        0 => {}
        1 => parts.push("1 needs approval".to_owned()),
        n => parts.push(format!("{n} need approval")),
    }
    parts.join(" · ")
}

/// What the page asks the host half to do.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "act")]
pub enum Act {
    /// Approves the repository server `server` of `repo`, as the page
    /// showed it: `hash` is its entry's.
    Approve {
        repo: String,
        server: String,
        hash: String,
    },
    /// Adds a server to the settings.
    Add { name: String, entry: Value },
    /// Replaces a settings server's entry.
    Edit { name: String, entry: Value },
    /// Removes a settings server.
    Remove { name: String },
    /// Turns any server on or off in `repo`, or one of the user's
    /// servers in every repository without one, in the settings: its
    /// entry is not touched.
    Enable {
        #[serde(default)]
        repo: Option<String>,
        name: String,
        enabled: bool,
    },
    /// Starts the servers of `repo` (or the user's alone) if they are
    /// not, and connects `server` again, or every server.
    Reconnect {
        repo: Option<String>,
        server: Option<String>,
    },
    /// Starts signing in to `server` of `repo` (or of the user's servers
    /// alone). The answer is [`Reply::SignIn`], the page to open.
    SignIn {
        #[serde(default)]
        repo: Option<String>,
        server: String,
    },
    /// Signs out of `server`: its tokens are forgotten.
    SignOut {
        #[serde(default)]
        repo: Option<String>,
        server: String,
    },
    /// Gets the prompt whose command is `command`, in `repo` (or the
    /// user's servers alone), with the `key=value` pairs of `arguments`.
    /// The answer is a [`Reply`].
    Prompt {
        #[serde(default)]
        repo: Option<String>,
        command: String,
        #[serde(default)]
        arguments: String,
    },
}

/// What the host half answers the page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "reply")]
pub enum Reply {
    /// The prompt's messages, for the composer.
    Prompt { text: String },
    /// Why the prompt could not be had, and the command as it was
    /// written, for the composer to hold again.
    PromptFailed { error: String, command: String },
    /// The page to open in the browser to sign in to `server`.
    SignIn { server: String, url: String },
}

/// The entry for a server named `name`, checked as the files are and
/// printed as tau prints it, defaults left out.
pub fn server_entry(name: &str, entry: &Value) -> Result<Value, String> {
    if !valid_name(name) {
        return Err(format!(
            "`{name}` is not a server name: use letters, digits, `-` and `_`."
        ));
    }
    let (config, errors) =
        McpConfig::from_value(&json!({ "mcpServers": { name: entry } }));
    if let Some(error) = errors.first() {
        return Err(error.message.clone());
    }
    config
        .servers
        .first()
        .and_then(ServerConfig::to_entry)
        .ok_or_else(|| format!("`{name}` has no entry"))
}

impl UiPlugin for McpUi {
    type State = ();
    type Data = Servers;
    type RepoData = Servers;
    type Settings = Settings;
    type Ui = page::Ui;

    fn name(&self) -> &'static str {
        NAME
    }

    /// A prompt goes to the composer; one that failed says why and puts
    /// the command back.
    fn reply(
        &self,
        ui: &mut page::Ui,
        reply: Value,
        cx: &mut Context<page::Ui>,
    ) {
        if let Ok(reply) = serde_json::from_value::<Reply>(reply) {
            ui.answered(reply, cx);
        }
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .settings(page::settings_pane)
            .page(
                Page::new("servers", page::render)
                    .title(|_| "MCP servers".to_owned()),
            )
            .contribute(points::SIDEBAR_REPO, sidebar)
            .contribute(points::CARD, card::card)
            .contribute(points::STATUS, status)
            .listed_commands(commands, run_prompt)
    }
}

impl PluginUi for page::Ui {
    fn new(handle: Handle, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| TextInput::new("linear", cx).keep_on_submit());
        let entry = cx.new(|cx| {
            TextInput::new(
                "{ \"command\": \"uvx\", \"args\": [\"mcp-server-git\"] }",
                cx,
            )
            .multiline()
            .keep_on_submit()
        });
        cx.observe(&name, |_, _, cx| cx.notify()).detach();
        cx.observe(&entry, |_, _, cx| cx.notify()).detach();
        page::Ui::new(handle, name, entry)
    }
}

/// The prompts of the servers where the composer is, as commands: the
/// repository's, or the user's alone outside one.
pub fn commands(data: &Servers, repo: Option<&Servers>) -> Vec<ListedCommand> {
    repo.unwrap_or(data).commands()
}

/// `/mcp__<server>__<prompt> key=value ...`: checks the arguments here,
/// then asks the host for the prompt, whose messages fill the composer.
/// A mistake says what is wrong and leaves the command to fix.
pub fn run_prompt(
    command: &str,
    arguments: &str,
    repo: Option<&str>,
    view: &mut ViewCx<'_, McpUi>,
) {
    let servers = repo.and_then(|repo| view.repo(repo)).unwrap_or(view.data);
    let written = format!("/{command} {arguments}");
    let checked = servers.prompt(command).map(|(_, prompt)| {
        parse_arguments(arguments)
            .and_then(|pairs| check_arguments(command, &prompt.info(), &pairs))
    });
    match checked {
        Some(Err(error)) => {
            view.handle.alert("The prompt needs fixing", error, view.cx);
            view.handle.composer(written, view.cx);
        }
        _ => view.handle.act(
            Act::Prompt {
                repo: repo
                    .filter(|repo| view.repo(repo).is_some())
                    .map(str::to_owned),
                command: command.to_owned(),
                arguments: arguments.to_owned(),
            },
            view.cx,
        ),
    }
}

/// The repository's servers in the sidebar: how many, and how many wait
/// for approval.
pub fn sidebar(at: &AtRepo, view: &mut ViewCx<'_, McpUi>) -> Option<NavEntry> {
    let servers = view.repo(&at.repo).cloned().unwrap_or_default();
    let waiting = servers.pending.len();
    Some(
        NavEntry::new(
            "MCP",
            Icon::Plug,
            Link::page("servers").param("repo", at.repo.clone()),
        )
        .detail(match servers.servers.len() {
            1 => "1 server".to_owned(),
            n => format!("{n} servers"),
        })
        .badge((waiting > 0).then(|| (waiting.to_string(), Tone::Warn))),
    )
}

/// The plugin's line in a run's plugin list: its repository's servers.
pub fn status(
    at: &AtRun,
    view: &mut ViewCx<'_, McpUi>,
) -> Option<PluginStatus> {
    let servers = view.repo(&at.run.repo)?;
    if servers.servers.is_empty() && servers.pending.is_empty() {
        return None;
    }
    let tone = if !servers.pending.is_empty() || servers.needs_sign_in() > 0 {
        Tone::Warn
    } else if servers.failed() > 0 {
        Tone::Danger
    } else {
        Tone::Quiet
    };
    Some(PluginStatus {
        name: NAME.into(),
        state: servers.summary(),
        tone,
    })
}
