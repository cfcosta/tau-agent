//! The plugin (`docs/reference/mcp.md`, "Tools" and "The server list"):
//! direct tools added to each run, codemode tools through a tool source,
//! and the `<mcp_servers>` block in each run's context.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use futures_util::future::join_all;
use tau_agent::{
    output::Spill,
    plugin::{Plugin, PluginCtx, PluginError, PluginRun, RunPlan},
    tool::{AgentTool, Exposure as ToolExposure, Namespace, ToolSource},
};
use tokio_util::sync::CancellationToken;

use crate::{
    NAME,
    auth::TokenStore,
    config::{
        ConfigError,
        EnvLookup,
        Exposure,
        PendingApproval,
        ServerConfig,
        Settings,
        Sources,
    },
    connection::{Connection, Environment, State},
    names::tool_names,
    prompts,
    resources::{self, Kind, ResourceTool},
    results::temp_spill,
    tool::McpTool,
};

/// How long `start` waits, at most, for servers that may have direct
/// tools and are still connecting.
pub const STARTUP_WAIT: Duration = Duration::from_secs(10);

/// The most characters the `<mcp_servers>` block takes.
pub const SERVERS_LIMIT: usize = 4096;

/// The most characters a server's description takes in the block.
pub const DESCRIPTION_LIMIT: usize = 250;

/// What the block says before the servers.
pub const SERVERS_INTRO: &str = "MCP servers connected to this agent. Their direct tools are declared to you. Call the tools of `codemode` servers from codemode scripts: find them with `search_tools(query, { namespace = name })` and read a server's instructions and tool names with `describe_namespace(name)`.";

/// Builds an [`McpPlugin`] from where servers come from.
pub struct McpPluginBuilder {
    user_dir: Option<PathBuf>,
    repo: Option<PathBuf>,
    settings: Settings,
    env: Option<EnvLookup>,
    home: Option<Option<PathBuf>>,
    servers: Vec<ServerConfig>,
    startup_wait: Duration,
    spill: Spill,
}

impl McpPluginBuilder {
    /// The user's configuration directory, `~/.config/tau`, whose
    /// `mcp.json` is read and where `mcp-auth.json` keeps sign-ins. None
    /// by default: then OAuth does not apply.
    pub fn user_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.user_dir = Some(dir.into());
        self
    }

    /// The repository: its `.tau/mcp.json` is read, relative `cwd`s start
    /// from it, and servers get it as their root. None by default.
    pub fn repo(mut self, dir: impl Into<PathBuf>) -> Self {
        self.repo = Some(dir.into());
        self
    }

    /// The plugin's settings: the servers added in the interface, and the
    /// approved repository servers.
    pub fn settings(mut self, settings: Settings) -> Self {
        self.settings = settings;
        self
    }

    /// Where `${VAR}` reads variables; the process's environment by
    /// default.
    pub fn env(mut self, env: EnvLookup) -> Self {
        self.env = Some(env);
        self
    }

    /// The home directory for `~/`; `$HOME` by default.
    pub fn home(mut self, home: Option<PathBuf>) -> Self {
        self.home = Some(home);
        self
    }

    /// A server added after every file and the settings, replacing one
    /// of the same name: an in-process server's, through
    /// [`Transport::Stream`](crate::config::Transport::Stream). It counts
    /// as the settings'.
    pub fn server(mut self, server: ServerConfig) -> Self {
        self.servers.push(server);
        self
    }

    /// How long `start` waits for servers with direct tools;
    /// [`STARTUP_WAIT`] by default.
    pub fn startup_wait(mut self, wait: Duration) -> Self {
        self.startup_wait = wait;
        self
    }

    /// Where long or binary results are written; `$TMPDIR` by default.
    pub fn spill_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.spill = Spill::new(dir, "tau-mcp");
        self
    }

    /// Reads the files, merges the servers and starts connecting every
    /// enabled, approved one in the background.
    ///
    /// # Panics
    ///
    /// Outside a Tokio runtime, which the connections run on.
    pub fn build(self) -> McpPlugin {
        let mut sources = Sources::load(
            self.user_dir.as_deref(),
            &self.settings,
            self.repo.as_deref(),
        );
        for server in self.servers {
            sources.add(server);
        }
        sources.disable(&self.settings, self.repo.as_deref());
        let process = Environment::process(self.repo.clone());
        let environment = Environment {
            env: self.env.unwrap_or(process.env),
            home: self.home.unwrap_or(process.home),
            repo: self.repo,
            // Sign-ins are kept next to the user's file.
            auth: self.user_dir.as_deref().map(TokenStore::in_dir),
        };
        let connections: Vec<Arc<Connection>> = sources
            .servers
            .into_iter()
            .map(|(origin, config)| {
                Connection::new(config, origin, environment.clone())
            })
            .collect();
        for connection in &connections {
            connection.connect();
        }
        McpPlugin::from_connections(
            connections,
            sources.errors,
            sources.pending,
        )
        .with_startup_wait(self.startup_wait)
        .with_spill(self.spill)
    }
}

/// Connects the agent to MCP servers and adds their tools. Add it with
/// `Agent::plugin`; build it with [`McpPlugin::builder`].
///
/// - `direct` tools are added to each run in `start`, after waiting up
///   to [`STARTUP_WAIT`] for servers that may have some and are still
///   connecting.
/// - Every tool that is not `hidden` is in the plugin's tool source, as
///   `Nested`, so Codemode scripts call it, and reach a server that
///   connected after the run started.
/// - Servers' resources are read through three tools of the plugin's
///   ([`crate::resources`]), as exposed as the widest server that offers
///   resources: added in `start` when that one is `direct`, in the tool
///   source as `Nested` whenever there is one.
/// - The connections are the agent's: shared by every run, and closed
///   once nothing holds them (the plugin dropped, and the runs that used
///   them ended), or by [`McpPlugin::shutdown`].
#[derive(Clone)]
pub struct McpPlugin {
    shared: Arc<Shared>,
}

/// Every tool that is not hidden, named.
type Tools = Arc<Vec<Arc<McpTool>>>;

struct Shared {
    connections: Vec<Arc<Connection>>,
    errors: Vec<ConfigError>,
    pending: Vec<PendingApproval>,
    startup_wait: Duration,
    spill: Spill,
    /// Every tool that is not hidden, named, by the generations of the
    /// connections they were built from.
    tools: Mutex<Option<(Vec<u64>, Tools)>>,
}

impl McpPlugin {
    pub fn builder() -> McpPluginBuilder {
        McpPluginBuilder {
            user_dir: None,
            repo: None,
            settings: Settings::default(),
            env: None,
            home: None,
            servers: Vec::new(),
            startup_wait: STARTUP_WAIT,
            spill: temp_spill(),
        }
    }

    /// A plugin over connections someone else keeps, such as the host's
    /// pools: it starts none of them.
    pub(crate) fn from_connections(
        connections: Vec<Arc<Connection>>,
        errors: Vec<ConfigError>,
        pending: Vec<PendingApproval>,
    ) -> Self {
        Self {
            shared: Arc::new(Shared {
                connections,
                errors,
                pending,
                startup_wait: STARTUP_WAIT,
                spill: temp_spill(),
                tools: Mutex::default(),
            }),
        }
    }

    fn with_startup_wait(mut self, wait: Duration) -> Self {
        Arc::get_mut(&mut self.shared)
            .expect("not shared yet")
            .startup_wait = wait;
        self
    }

    fn with_spill(mut self, spill: Spill) -> Self {
        Arc::get_mut(&mut self.shared)
            .expect("not shared yet")
            .spill = spill;
        self
    }

    /// Every configured server's connection, enabled or not, in merge
    /// order: their states, errors, tools and instructions.
    pub fn connections(&self) -> &[Arc<Connection>] {
        &self.shared.connections
    }

    /// The entries and files that were skipped.
    pub fn config_errors(&self) -> &[ConfigError] {
        &self.shared.errors
    }

    /// The repository servers waiting for the user's approval.
    pub fn pending_approvals(&self) -> &[PendingApproval] {
        &self.shared.pending
    }

    /// Every tool the servers offer now that is not `hidden`, with its
    /// name, as tools call them.
    pub fn tools(&self) -> Tools {
        self.shared.tools()
    }

    /// The tool named `name`, with its annotations.
    pub fn tool(&self, name: &str) -> Option<Arc<McpTool>> {
        self.shared
            .tools()
            .iter()
            .find(|tool| tool.name() == name)
            .cloned()
    }

    /// The resource tools as the servers offer resources now: none when
    /// no server that is not hidden does, else all three, `Nested`. Their
    /// exposure is [`McpPlugin::resource_exposure`].
    pub fn resource_tools(&self) -> Vec<Arc<ResourceTool>> {
        self.shared.resource_tools()
    }

    /// The resource tools' exposure: the widest among the servers that
    /// offer resources, `None` without any ([`resources::exposure`]).
    pub fn resource_exposure(&self) -> Option<Exposure> {
        self.shared.resource_exposure()
    }

    /// Every prompt the servers that are on offer now, with the command
    /// that gets it.
    pub fn prompts(&self) -> Vec<prompts::Prompt> {
        let enabled: Vec<Arc<Connection>> =
            self.shared.enabled().cloned().collect();
        prompts::prompts(&enabled)
    }

    /// Gets the prompt whose command is `command` with the `key=value`
    /// pairs of `arguments`, and gives its messages as text. A command
    /// not listed waits for the servers still connecting first.
    pub async fn get_prompt(
        &self,
        command: &str,
        arguments: &str,
        cancel: &CancellationToken,
    ) -> Result<String, String> {
        let find = || {
            self.prompts()
                .into_iter()
                .find(|prompt| prompt.command == command)
        };
        // Right after the servers start, their prompts may not be listed
        // yet.
        let prompt = match find() {
            Some(prompt) => prompt,
            None => {
                join_all(self.shared.enabled().map(|c| c.settled(cancel)))
                    .await;
                find()
                    .ok_or_else(|| format!("No MCP prompt /{command} here."))?
            }
        };
        prompt.get(arguments, cancel).await
    }

    /// The `<mcp_servers>` block as a run would get it now.
    pub fn servers_block(&self) -> Option<String> {
        self.shared.servers_block()
    }

    /// Starts connecting the server `name` again, if it is not connected
    /// or connecting.
    pub fn reconnect(&self, name: &str) {
        if let Some(connection) =
            self.shared.connections.iter().find(|c| c.name() == name)
        {
            connection.connect();
        }
    }

    /// Closes every connection and waits for them to end.
    pub async fn shutdown(&self) {
        join_all(self.shared.connections.iter().map(|c| c.shutdown())).await;
    }
}

impl Shared {
    fn enabled(&self) -> impl Iterator<Item = &Arc<Connection>> {
        self.connections.iter().filter(|c| c.config().enabled)
    }

    fn tools(&self) -> Tools {
        let key: Vec<u64> =
            self.connections.iter().map(|c| c.generation()).collect();
        let mut cache = self.tools.lock().expect("tools lock");
        if let Some((cached, tools)) = &*cache
            && *cached == key
        {
            return tools.clone();
        }
        let listed: Vec<(&Arc<Connection>, crate::connection::ToolInfo)> = self
            .enabled()
            .flat_map(|connection| {
                connection
                    .tools()
                    .into_iter()
                    .map(move |info| (connection, info))
            })
            .collect();
        let pairs: Vec<(&str, &str)> = listed
            .iter()
            .map(|(connection, info)| (connection.name(), info.name.as_str()))
            .collect();
        let names = tool_names(&pairs);
        let tools: Vec<Arc<McpTool>> = listed
            .iter()
            .zip(names)
            .filter(|((connection, info), _)| {
                connection.config().exposure_of(&info.name) != Exposure::Hidden
            })
            .map(|((connection, info), name)| {
                Arc::new(McpTool::new(
                    name,
                    (*connection).clone(),
                    info.clone(),
                    ToolExposure::Nested,
                    self.spill.clone(),
                ))
            })
            .collect();
        let tools = Arc::new(tools);
        *cache = Some((key, tools.clone()));
        tools
    }

    fn resource_exposure(&self) -> Option<Exposure> {
        resources::exposure(
            self.enabled()
                .map(|c| (c.config().exposure, c.offers_resources())),
        )
    }

    fn resource_tools(&self) -> Vec<Arc<ResourceTool>> {
        if self.resource_exposure().is_none() {
            return Vec::new();
        }
        let connections: Vec<Arc<Connection>> = self
            .enabled()
            .filter(|c| c.config().exposure != Exposure::Hidden)
            .cloned()
            .collect();
        Kind::ALL
            .into_iter()
            .map(|kind| {
                Arc::new(ResourceTool::new(
                    kind,
                    connections.clone(),
                    ToolExposure::Nested,
                    self.spill.clone(),
                ))
            })
            .collect()
    }

    /// A server's description: its entry's, else the first line of its
    /// instructions.
    fn description(connection: &Connection) -> String {
        connection
            .config()
            .description
            .clone()
            .or_else(|| {
                connection.instructions().and_then(|text| {
                    text.lines()
                        .map(str::trim)
                        .find(|line| !line.is_empty())
                        .map(str::to_owned)
                })
            })
            .unwrap_or_default()
    }

    fn servers_block(&self) -> Option<String> {
        let servers: Vec<(String, Exposure, String)> = self
            .enabled()
            .filter(|c| c.config().exposure != Exposure::Hidden)
            .map(|c| {
                (
                    c.config().namespace(),
                    c.config().exposure,
                    Self::description(c),
                )
            })
            .collect();
        servers_block(&servers)
    }

    /// Waits, up to the startup wait, for enabled servers that may have
    /// direct tools and are still connecting.
    async fn wait_for_direct(&self, cancel: &CancellationToken) {
        let waiting: Vec<&Arc<Connection>> = self
            .enabled()
            .filter(|c| {
                c.config().may_have_direct()
                    && c.status().state == State::Connecting
            })
            .collect();
        if waiting.is_empty() {
            return;
        }
        let settled = join_all(waiting.iter().map(|c| c.settled(cancel)));
        let _ = tokio::time::timeout(self.startup_wait, settled).await;
    }
}

/// Shortens `text` to at most `limit` characters, ending with `…` when
/// cut.
fn shorten(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut short: String =
        text.chars().take(limit.saturating_sub(1)).collect();
    short.push('…');
    short
}

/// The `<mcp_servers>` block for `servers` (namespace, exposure,
/// description), at most [`SERVERS_LIMIT`] characters: each description
/// on one line and at most [`DESCRIPTION_LIMIT`]; the servers that do
/// not fit are left out, counted in a last line. `None` without servers.
pub fn servers_block(servers: &[(String, Exposure, String)]) -> Option<String> {
    if servers.is_empty() {
        return None;
    }
    let head = format!("<mcp_servers>\n{SERVERS_INTRO}\n");
    let tail = "</mcp_servers>";
    let lines: Vec<String> = servers
        .iter()
        .map(|(namespace, exposure, description)| {
            let flat =
                description.split_whitespace().collect::<Vec<_>>().join(" ");
            let description = shorten(&flat, DESCRIPTION_LIMIT);
            if description.is_empty() {
                format!("- {namespace} ({exposure})\n")
            } else {
                format!("- {namespace} ({exposure}): {description}\n")
            }
        })
        .collect();
    let overflow = |left: usize| {
        format!(
            "- … {left} more servers; find their tools with search_tools()\n"
        )
    };
    let chars = |text: &str| text.chars().count();
    let fixed = chars(&head) + chars(tail);
    let all: usize = lines.iter().map(|line| chars(line)).sum();
    let mut block = head;
    if fixed + all <= SERVERS_LIMIT {
        lines.iter().for_each(|line| block.push_str(line));
    } else {
        let room = SERVERS_LIMIT - fixed - chars(&overflow(servers.len()));
        let mut used = 0;
        let mut kept = 0;
        for line in &lines {
            if used + chars(line) > room {
                break;
            }
            used += chars(line);
            kept += 1;
            block.push_str(line);
        }
        block.push_str(&overflow(servers.len() - kept));
    }
    block.push_str(tail);
    Some(block)
}

/// The plugin's tool source: every tool that is not hidden, `Nested`,
/// and a namespace per server.
struct Source(Arc<Shared>);

#[async_trait]
impl ToolSource for Source {
    fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        let resource_tools = self.0.resource_tools();
        self.0
            .tools()
            .iter()
            .map(|tool| tool.clone() as Arc<dyn AgentTool>)
            .chain(
                resource_tools
                    .into_iter()
                    .map(|tool| tool as Arc<dyn AgentTool>),
            )
            .collect()
    }

    fn namespaces(&self) -> Vec<Namespace> {
        let tools = self.0.tools();
        self.0
            .enabled()
            .filter_map(|connection| {
                let names: Vec<String> = tools
                    .iter()
                    .filter(|tool| Arc::ptr_eq(tool.connection(), connection))
                    .map(|tool| tool.name().to_owned())
                    .collect();
                if names.is_empty()
                    && connection.config().exposure == Exposure::Hidden
                {
                    return None;
                }
                Some(Namespace {
                    name: connection.config().namespace(),
                    description: Shared::description(connection),
                    instructions: connection.instructions(),
                    tools: names,
                })
            })
            .collect()
    }

    /// Waits for the named servers (by namespace or by name) to finish
    /// connecting, or for all of them. A named server that dropped or
    /// failed is connected again first.
    async fn ready(
        &self,
        namespaces: Option<&[String]>,
        cancel: &CancellationToken,
    ) {
        let targets: Vec<&Arc<Connection>> = self
            .0
            .enabled()
            .filter(|connection| match namespaces {
                None => true,
                Some(names) => names.iter().any(|name| {
                    *name == connection.config().namespace()
                        || crate::names::namespace(name)
                            == connection.config().namespace()
                }),
            })
            .collect();
        if namespaces.is_some() {
            for connection in &targets {
                if matches!(
                    connection.status().state,
                    State::Disconnected | State::Failed
                ) {
                    connection.connect();
                }
            }
        }
        join_all(targets.iter().map(|c| c.settled(cancel))).await;
    }
}

#[async_trait]
impl Plugin for McpPlugin {
    fn name(&self) -> &str {
        NAME
    }

    fn tool_source(&self) -> Option<Arc<dyn ToolSource>> {
        Some(Arc::new(Source(self.shared.clone())))
    }

    async fn start(
        &self,
        plan: &mut RunPlan,
        ctx: &PluginCtx,
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        self.shared.wait_for_direct(&ctx.cancel).await;
        for tool in self.shared.tools().iter() {
            if tool.configured_exposure() == Exposure::Direct {
                plan.add_tool(Arc::new(tool.exposed(ToolExposure::Direct)));
            }
        }
        if self.shared.resource_exposure() == Some(Exposure::Direct) {
            for tool in self.shared.resource_tools() {
                plan.add_tool(Arc::new(tool.exposed(ToolExposure::Direct)));
            }
        }
        if let Some(block) = self.shared.servers_block() {
            plan.context.push(block);
        }
        Ok(Box::new(()))
    }
}
