//! tau-mcp's UI (ADR 0017): the Servers page, where servers are added,
//! edited, approved and reconnected, with their tools, resources,
//! templates and prompts; the cards of MCP tools; its row under each
//! repository in the sidebar; its line in a run's plugin list; and the
//! servers' prompts as composer commands, `/mcp__<server>__<prompt>
//! key=value ...`, whose messages the host fetches into the composer.
//!
//! ## On the host
//!
//! Connections outlive runs ([`McpPlugin`]). The user's servers (the
//! user's file, the settings, and the host's own) run once for every
//! repository, in a shared [`Pool`]; a repository's servers, and a user
//! server whose `cwd` is relative ([`ServerConfig::per_repo`]), run in
//! the repository's own pool. The host keeps one plugin per **scope**
//! (a repository, or the user's servers alone) over the connections it
//! uses, built the first time something needs them: a run in the
//! repository, or Connect on the page. Until then the page shows the
//! servers as configured, and a shared server as another scope started
//! it.
//!
//! Each time a scope is used (a run starts, the catalog is drawn, an
//! action runs) its servers are read again, from the user's file, the
//! plugin's settings and the repository's file. When they differ from
//! what its plugin was built from, the plugin is built again over the
//! pools, which keep every connection whose entry did not change: only
//! new and changed servers connect, and removed, changed or disabled
//! ones close once the runs going on that use them end. Files are not
//! watched: an edit by hand shows the next time the scope is used.
//!
//! Every run gets a wrapper ([`RunServers`]) around its scope's plugin:
//! its `start` and tool source. When the host is dropped, every
//! connection is closed.
//!
//! ## Actions
//!
//! The page asks through [`Act`]. The host half reads the plugin's
//! settings, changes them ([`apply`]), and saves them, so an approval
//! or a new server holds for the next run. It refuses what the page
//! should not do: a name the user's file has, an entry that does not
//! parse, approving an entry other than the one the page showed.

pub mod card;
pub mod page;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use futures_util::future::join_all;
use gpui::{AppContext as _, Context};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tau_agent::{
    plugin::{Plugin, PluginCtx, PluginError, PluginRun, RunPlan},
    tool::ToolSource,
};
use tau_ui_kit::{assets::Icon, input::TextInput, theme::Tone};
use tau_ui_plugin::{
    Handle,
    HostCx,
    Link,
    ListedCommand,
    Manifest,
    NavEntry,
    Page,
    PluginHost,
    PluginInfo,
    PluginStatus,
    PluginUi,
    RepoCtx,
    RunCtx,
    Seam,
    UiPlugin,
    ViewCx,
    points::{self, AtRepo, AtRun},
};

use crate::{
    McpPlugin,
    NAME,
    auth::{Grant, GrantKey, SIGN_IN_TIMEOUT, TokenStore},
    config::{
        ConfigError,
        McpConfig,
        Off,
        Origin,
        PendingApproval,
        Read,
        ServerConfig,
        Settings,
        Sources,
        Transport,
        USER_FILE,
        repo_key,
        valid_name,
    },
    connection::{
        Annotations,
        Connection,
        Environment,
        PromptArgument,
        ResourceInfo,
        TemplateInfo,
    },
    pool::Pool,
    prompts::{arguments_hint, check_arguments, parse_arguments},
};

/// tau-mcp with its UI.
#[derive(Debug, Clone, Copy, Default)]
pub struct McpUi;

/// Where a set of servers is read: a repository's (the user's, the
/// settings' and its own file), or the user's and the settings' alone.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Scope {
    User,
    Repo(PathBuf),
}

impl Scope {
    fn of(repo: Option<&Path>) -> Self {
        repo.map_or(Self::User, |repo| Self::Repo(repo.to_owned()))
    }

    fn repo(&self) -> Option<&Path> {
        match self {
            Self::User => None,
            Self::Repo(repo) => Some(repo),
        }
    }
}

/// What a scope's plugin was built from: when it changes, the plugin is
/// built again, over the connections of the servers that did not change.
type Print = (
    Vec<(Origin, ServerConfig)>,
    Vec<ConfigError>,
    Vec<PendingApproval>,
);

fn print(sources: &Sources) -> Print {
    (
        sources.servers.clone(),
        sources.errors.clone(),
        sources.pending.clone(),
    )
}

/// A scope's servers as read now, and the servers every repository
/// shares.
struct Loaded {
    /// The scope's: the user's file, the settings, the host's and, for
    /// a repository, its file.
    sources: Sources,
    /// The user's file's, the settings' and the host's servers that do
    /// not run per repository, as the shared pool runs them.
    shared: Vec<(Origin, ServerConfig)>,
}

/// A scope's plugin, and the connections it does not share.
struct Built {
    print: Print,
    plugin: McpPlugin,
    /// The repository's servers, and the user's that run per
    /// repository.
    pool: Pool,
}

/// How long the page waits after a connection changes before it asks
/// for a redraw, so a burst of changes makes one.
pub const REFRESH_COALESCE: std::time::Duration =
    std::time::Duration::from_millis(50);

/// The plugin on the host: one pool of connections shared by every
/// repository for the user's servers, one pool per scope for the rest,
/// and one [`McpPlugin`] per scope over them, built when first needed
/// and shared by every run in it.
pub struct Host {
    runtime: tokio::runtime::Handle,
    /// `~/.config/tau`, whose `mcp.json` is the user's file.
    user_dir: Option<PathBuf>,
    /// Servers added after the files and the settings, as the settings'.
    servers: Mutex<Vec<ServerConfig>>,
    /// The user's servers, one connection each for every repository.
    shared: Pool,
    scopes: Mutex<BTreeMap<Scope, Built>>,
    /// The sign-ins waiting for the browser, by grant: a new one for the
    /// same grant ends the old.
    signing_in: Mutex<BTreeMap<GrantKey, tokio::task::AbortHandle>>,
    /// Asks the interface to draw the page again.
    refresh: Option<Refresh>,
    /// The connections whose changes reach `refresh` already.
    watched: Mutex<Vec<std::sync::Weak<Connection>>>,
}

/// Asks the interface to draw the catalog again.
pub type Refresh = Arc<dyn Fn() + Send + Sync>;

impl Host {
    /// A host whose user file is in `user_dir`, connecting on `runtime`.
    pub fn new(
        runtime: tokio::runtime::Handle,
        user_dir: Option<PathBuf>,
    ) -> Self {
        let auth = user_dir.as_deref().map(TokenStore::in_dir);
        Self {
            runtime,
            user_dir,
            servers: Mutex::default(),
            shared: Pool::new(Environment::process(None).with_auth(auth)),
            scopes: Mutex::default(),
            signing_in: Mutex::default(),
            refresh: None,
            watched: Mutex::default(),
        }
    }

    /// The same host, asking the interface to draw the page again
    /// whenever a connection it started changes: its state, or what the
    /// server lists. Without it, the page shows a connection as it was
    /// when the page last asked, `connecting` for good after Connect.
    pub fn with_refresh(mut self, refresh: Refresh) -> Self {
        self.refresh = Some(refresh);
        self
    }

    /// Sends `connection`'s changes to the interface, once per
    /// connection. The task ends with the connection.
    fn watch(&self, connection: &Arc<Connection>) {
        let Some(refresh) = self.refresh.clone() else {
            return;
        };
        let mut watched = self.watched.lock().expect("not poisoned");
        watched.retain(|weak| weak.strong_count() > 0);
        if watched
            .iter()
            .any(|weak| std::ptr::eq(weak.as_ptr(), Arc::as_ptr(connection)))
        {
            return;
        }
        watched.push(Arc::downgrade(connection));
        let mut changes = connection.watch();
        self.runtime.spawn(async move {
            while changes.changed().await.is_ok() {
                // Changes come in bursts (connected, then the lists):
                // one redraw for each.
                tokio::time::sleep(REFRESH_COALESCE).await;
                changes.borrow_and_update();
                refresh();
            }
        });
    }

    /// `~/.config/tau/mcp-auth.json`, where sign-ins are kept; none
    /// without a configuration directory.
    pub fn token_store(&self) -> Option<TokenStore> {
        self.user_dir.as_deref().map(TokenStore::in_dir)
    }

    /// Servers every scope gets after the files and the settings, as the
    /// settings', replacing the ones set before: an in-process server's,
    /// through [`Transport::Stream`], which no file can name.
    pub fn set_servers(&self, servers: Vec<ServerConfig>) {
        *self.servers.lock().expect("not poisoned") = servers;
    }

    /// Reads the scope's servers, and the shared ones, now.
    fn load(&self, scope: &Scope, settings: &Settings) -> Loaded {
        let user = Read::user(self.user_dir.as_deref());
        let added = self.servers.lock().expect("not poisoned").clone();
        let with_added = |mut sources: Sources, repo: Option<&Path>| {
            for server in &added {
                sources.add(server.clone());
            }
            sources.disable(settings, repo);
            sources
        };
        let shared = with_added(
            Sources::from_reads(&user, settings, &Read::default()),
            None,
        );
        let sources = match scope.repo() {
            None => shared.clone(),
            Some(repo) => with_added(
                Sources::from_reads(&user, settings, &Read::repo(Some(repo))),
                Some(repo),
            ),
        };
        Loaded {
            shared: shared
                .servers
                .into_iter()
                .filter(|(_, server)| !server.per_repo())
                .collect(),
            sources,
        }
    }

    fn sources(&self, scope: &Scope, settings: &Settings) -> Sources {
        self.load(scope, settings).sources
    }

    /// Whether the scope's server runs in the shared pool: it is the
    /// user's, does not run per repository, and is the same as the
    /// shared pool's.
    fn shares(&self, origin: Origin, server: &ServerConfig) -> bool {
        origin != Origin::Repo
            && !server.per_repo()
            && self.shared.get(&server.name).is_some_and(|shared| {
                shared.origin() == origin && shared.config() == server
            })
    }

    /// The names the user's file gives servers, valid entries or not:
    /// the page refuses them for a server of its own.
    pub fn user_names(&self) -> BTreeSet<String> {
        self.user_dir
            .as_deref()
            .map(user_file_names)
            .unwrap_or_default()
    }

    /// The plugin of `repo` (or of the user's servers alone), built now
    /// if it was not, or again if its servers changed since.
    pub fn plugin(
        &self,
        repo: Option<&Path>,
        settings: &Settings,
    ) -> McpPlugin {
        let scope = Scope::of(repo);
        let loaded = self.load(&scope, settings);
        self.fresh(scope, &loaded, true)
            .expect("built when asked to")
    }

    /// The scope's plugin: built again when its servers changed, and
    /// built at all only when `build`. Building starts the connections
    /// it uses that never started; the others keep going as they were.
    fn fresh(
        &self,
        scope: Scope,
        loaded: &Loaded,
        build: bool,
    ) -> Option<McpPlugin> {
        // Connections start, and the ones let go close once their last
        // run lets go of them, on the host's runtime.
        let _runtime = self.runtime.enter();
        self.shared.update(loaded.shared.clone());
        let print = print(&loaded.sources);
        let mut scopes = self.scopes.lock().expect("not poisoned");
        match scopes.get(&scope) {
            Some(built) if built.print == print => {
                return Some(built.plugin.clone());
            }
            None if !build => return None,
            _ => {}
        }
        let pool = scopes.remove(&scope).map_or_else(
            || {
                Pool::new(
                    Environment::process(scope.repo().map(Path::to_owned))
                        .with_auth(self.token_store()),
                )
            },
            |built| built.pool,
        );
        let servers = &loaded.sources.servers;
        let own: Vec<(Origin, ServerConfig)> = servers
            .iter()
            .filter(|(origin, server)| {
                !self.shares(*origin, server)
                    && (scope.repo().is_some() || !server.per_repo())
            })
            .cloned()
            .collect();
        pool.update(own);
        let connections: Vec<Arc<Connection>> = servers
            .iter()
            .filter_map(|(origin, server)| match self.shares(*origin, server) {
                true => self.shared.get(&server.name),
                false => pool.get(&server.name),
            })
            .collect();
        let plugin = McpPlugin::from_connections(
            connections,
            loaded.sources.errors.clone(),
            loaded.sources.pending.clone(),
        );
        for connection in plugin.connections() {
            self.watch(connection);
            connection.start();
        }
        scopes.insert(
            scope,
            Built {
                print,
                plugin: plugin.clone(),
                pool,
            },
        );
        Some(plugin)
    }

    /// What the page shows of `repo`'s servers (or of the user's alone):
    /// as configured, and as connected where they started, for this
    /// scope or, for a shared server, for any.
    pub fn servers(&self, repo: Option<&Path>, settings: &Settings) -> Servers {
        let scope = Scope::of(repo);
        let loaded = self.load(&scope, settings);
        let built = self.fresh(scope, &loaded, false);
        let started = built.is_some();
        let sources = &loaded.sources;
        let shared: BTreeSet<String> = sources
            .servers
            .iter()
            .filter(|(origin, server)| self.shares(*origin, server))
            .map(|(_, server)| server.name.clone())
            .collect();
        // Before the scope is built, the shared servers that started for
        // another.
        let plugin = built.unwrap_or_else(|| {
            McpPlugin::from_connections(
                shared
                    .iter()
                    .filter_map(|name| self.shared.get(name))
                    .filter(|connection| connection.started())
                    .collect(),
                Vec::new(),
                Vec::new(),
            )
        });
        servers_view(
            sources,
            &plugin,
            started,
            &shared,
            settings,
            self.user_names(),
            self.user_dir.as_deref(),
            repo,
        )
    }

    /// Every connection signing in with `key`, in every pool.
    fn signed_with(&self, key: &GrantKey) -> Vec<Arc<Connection>> {
        let mut connections = self.shared.connections();
        for built in self.scopes.lock().expect("not poisoned").values() {
            connections.extend(built.pool.connections());
        }
        connections
            .into_iter()
            .filter(|c| c.oauth().is_some_and(|(_, k)| k == *key))
            .collect()
    }

    /// The connection of `server` in `repo`'s scope (or the user's
    /// alone), its scope built if it was not.
    fn connection(
        &self,
        repo: Option<&Path>,
        settings: &Settings,
        server: &str,
    ) -> Result<Arc<Connection>, String> {
        self.plugin(repo, settings)
            .connections()
            .iter()
            .find(|c| c.name() == server && c.config().enabled)
            .cloned()
            .ok_or_else(|| format!("No server `{server}` is on here."))
    }

    /// Starts signing in to `server` in `repo`'s scope: finds its
    /// authorization server, registers a client if it must, and returns
    /// the URL to open. The browser has [`SIGN_IN_TIMEOUT`] to come
    /// back; then the grant is saved, every connection that uses it
    /// connects again, and `done` hears how it went. A sign-in already
    /// waiting for the same grant is dropped.
    pub fn sign_in(
        &self,
        repo: Option<&Path>,
        settings: &Settings,
        server: &str,
        done: impl FnOnce(Result<Grant, String>) + Send + 'static,
    ) -> Result<String, String> {
        let connection = self.connection(repo, settings, server)?;
        let (store, request) = connection.sign_in_request()?;
        let key = request.key();
        let (sender, answer) = std::sync::mpsc::channel();
        self.runtime.spawn(async move {
            let begun = tokio::time::timeout(
                SIGN_IN_WAIT,
                crate::auth::begin(&store, request),
            )
            .await
            .unwrap_or_else(|_| {
                Err(format!(
                    "the authorization server gave no answer in {} s",
                    SIGN_IN_WAIT.as_secs()
                ))
            });
            let _ = sender.send(begun);
        });
        let sign_in = answer
            .recv()
            .unwrap_or_else(|_| Err("the host stopped".to_owned()))?;
        let url = sign_in.url().to_owned();
        let mut connections = self.signed_with(&key);
        if !connections.iter().any(|c| Arc::ptr_eq(c, &connection)) {
            connections.push(connection);
        }
        let task = self.runtime.spawn(async move {
            let finished =
                tokio::time::timeout(SIGN_IN_TIMEOUT, sign_in.finish())
                    .await
                    .unwrap_or_else(|_| {
                        Err(format!(
                            "the browser did not come back in {} minutes",
                            SIGN_IN_TIMEOUT.as_secs() / 60
                        ))
                    });
            if finished.is_ok() {
                for connection in &connections {
                    connection.restart();
                }
            }
            done(finished);
        });
        if let Some(old) = self
            .signing_in
            .lock()
            .expect("not poisoned")
            .insert(key, task.abort_handle())
        {
            old.abort();
        }
        Ok(url)
    }

    /// Signs out of `server` in `repo`'s scope: forgets the grant's
    /// tokens, keeping its client, and connects every connection that
    /// used it again. Whether it was signed in.
    pub fn sign_out(
        &self,
        repo: Option<&Path>,
        settings: &Settings,
        server: &str,
    ) -> Result<bool, String> {
        let connection = self.connection(repo, settings, server)?;
        let (store, key) = connection
            .oauth()
            .ok_or_else(|| format!("`{server}` has no sign-in."))?;
        let was = store.sign_out(&key).map_err(|error| {
            format!("cannot save {}: {error}", store.path().display())
        })?;
        let _runtime = self.runtime.enter();
        for connection in self.signed_with(&key) {
            connection.restart();
        }
        Ok(was)
    }
}

/// How long the host waits for discovery and registration, at most.
pub const SIGN_IN_WAIT: std::time::Duration =
    std::time::Duration::from_secs(60);

impl Drop for Host {
    /// Closes every connection, runs going on included: the host is
    /// going away.
    fn drop(&mut self) {
        let scopes = std::mem::take(
            &mut *self.scopes.lock().unwrap_or_else(|e| e.into_inner()),
        );
        let mut connections = self.shared.connections();
        for built in scopes.values() {
            connections.extend(built.pool.connections());
            connections.extend(built.plugin.connections().iter().cloned());
        }
        let _runtime = self.runtime.enter();
        self.runtime.spawn(async move {
            join_all(connections.iter().map(|c| c.shutdown())).await;
        });
        drop(scopes);
    }
}

/// The names in `<dir>/mcp.json`'s `mcpServers`, whether their entries
/// are valid or not.
pub fn user_file_names(dir: &Path) -> BTreeSet<String> {
    std::fs::read_to_string(dir.join(USER_FILE))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| {
            value.get("mcpServers")?.as_object().map(|servers| {
                servers.keys().cloned().collect::<BTreeSet<String>>()
            })
        })
        .unwrap_or_default()
}

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
    fn of(origin: Origin) -> Self {
        match origin {
            Origin::User => Self::User,
            Origin::Settings => Self::Settings,
            Origin::Repo => Self::Repo,
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

/// The sign-in of `config` as the page shows it, when OAuth applies.
fn auth_row(
    store: Option<&TokenStore>,
    config: &ServerConfig,
    connection: Option<&Arc<Connection>>,
) -> Option<AuthRow> {
    let Transport::Http(http) = &config.transport else {
        return None;
    };
    let store = store.filter(|_| http.uses_oauth())?;
    let key = GrantKey {
        url: http.url.clone(),
        client: http.oauth.as_ref().and_then(|o| o.client_id.clone()),
    };
    let grant = store
        .get(&key)
        .ok()
        .flatten()
        .filter(Grant::is_signed_in)
        .unwrap_or_default();
    Some(AuthRow {
        signed_in: grant.is_signed_in(),
        expires_at: grant.expires_at(),
        refreshes: grant.can_refresh(),
        account: grant.account,
        scopes: grant.scopes,
        issuer: grant.issuer,
        wants_scope: connection
            .and_then(|c| c.auth_need())
            .and_then(|need| need.scope),
    })
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
    pub fn info(&self) -> crate::connection::PromptInfo {
        crate::connection::PromptInfo {
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

/// How tau reaches a server, in a line: the command and its arguments,
/// or the URL. Headers and variables show by name only: their values
/// may hold tokens.
pub fn transport(server: &ServerConfig) -> String {
    match &server.transport {
        Transport::Stdio(stdio) => {
            let mut line = std::iter::once(stdio.command.as_str())
                .chain(stdio.args.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" ");
            if !stdio.env.is_empty() {
                let names: Vec<&str> =
                    stdio.env.iter().map(|(name, _)| name.as_str()).collect();
                line.push_str(&format!(" (env {})", names.join(", ")));
            }
            line
        }
        Transport::Http(http) => {
            let mut line = http.url.clone();
            if !http.headers.is_empty() {
                let names: Vec<&str> = http
                    .headers
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect();
                line.push_str(&format!(" (headers {})", names.join(", ")));
            }
            line
        }
        Transport::Stream(_) => "in-process".to_owned(),
    }
}

#[allow(clippy::too_many_arguments)]
fn servers_view(
    sources: &Sources,
    plugin: &McpPlugin,
    started: bool,
    shared: &BTreeSet<String>,
    settings: &Settings,
    user_names: BTreeSet<String>,
    user_dir: Option<&Path>,
    repo: Option<&Path>,
) -> Servers {
    let tools = plugin.tools();
    let prompts = plugin.prompts();
    let servers = sources
        .servers
        .iter()
        .map(|(origin, config)| {
            let connection = plugin
                .connections()
                .iter()
                .find(|c| c.name() == config.name && c.started());
            let status = connection.map(|connection| connection.status());
            let listed = connection.map(|c| c.tools()).unwrap_or_default();
            let resources =
                connection.map(|c| c.resources()).unwrap_or_default();
            let templates =
                connection.map(|c| c.templates()).unwrap_or_default();
            let prompts: Vec<PromptRow> = prompts
                .iter()
                .filter(|prompt| prompt.connection.name() == config.name)
                .map(|prompt| PromptRow {
                    command: prompt.command.clone(),
                    name: prompt.info.name.clone(),
                    title: prompt.info.title.clone(),
                    description: prompt.info.description.clone(),
                    arguments: prompt.info.arguments.clone(),
                })
                .collect();
            ServerRow {
                name: config.name.clone(),
                defined: Defined::of(*origin),
                transport: transport(config),
                exposure: config.exposure.to_string(),
                enabled: config.enabled,
                description: config.description.clone(),
                // A disabled server is never connected.
                state: match &status {
                    _ if !config.enabled => Some("disabled".to_owned()),
                    status => {
                        status.as_ref().map(|status| status.state.to_string())
                    }
                },
                shared: shared.contains(&config.name),
                off: sources.off.get(&config.name).copied(),
                error: status.and_then(|status| status.error),
                tools: listed
                    .into_iter()
                    .map(|info| ToolRow {
                        name: tools
                            .iter()
                            .find(|tool| {
                                tool.server() == config.name
                                    && tool.info().name == info.name
                            })
                            .map(|tool| {
                                tau_agent::tool::AgentTool::name(&**tool)
                                    .to_owned()
                            }),
                        exposure: config.exposure_of(&info.name).to_string(),
                        description: info.description.clone(),
                        annotations: info.annotations,
                        tool: info.name,
                    })
                    .collect(),
                resources,
                templates,
                prompts,
                entry: (*origin == Origin::Settings)
                    .then(|| settings.servers.get(&config.name).cloned())
                    .flatten(),
                auth: auth_row(
                    user_dir.map(TokenStore::in_dir).as_ref(),
                    config,
                    connection,
                ),
            }
        })
        .collect();
    Servers {
        started,
        servers,
        pending: sources
            .pending
            .iter()
            .map(|pending| PendingRow {
                name: pending.server.name.clone(),
                transport: transport(&pending.server),
                entry: pending.server.to_entry().unwrap_or(Value::Null),
                hash: pending.hash.clone(),
            })
            .collect(),
        errors: sources.errors.iter().map(ToString::to_string).collect(),
        user_names,
        user_file: user_dir
            .map(|dir| dir.join(USER_FILE).display().to_string()),
        repo_file: repo.map(|repo| {
            repo.join(crate::config::REPO_FILE).display().to_string()
        }),
    }
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

/// How long the host waits for a prompt, at most: a connect and the
/// server's own timeout fit in it.
pub const PROMPT_WAIT: std::time::Duration =
    std::time::Duration::from_secs(120);

impl Host {
    /// Gets the prompt whose command is `command` in `repo`'s scope, and
    /// waits for it on the host's runtime.
    pub fn prompt(
        &self,
        repo: Option<&Path>,
        settings: &Settings,
        command: &str,
        arguments: &str,
    ) -> Result<String, String> {
        let plugin = self.plugin(repo, settings);
        let (command, arguments) = (command.to_owned(), arguments.to_owned());
        let (sender, answer) = std::sync::mpsc::channel();
        self.runtime.spawn(async move {
            let cancel = tokio_util::sync::CancellationToken::new();
            let got = tokio::time::timeout(
                PROMPT_WAIT,
                plugin.get_prompt(&command, &arguments, &cancel),
            )
            .await
            .unwrap_or_else(|_| {
                Err(format!(
                    "/{command} gave no answer in {} s",
                    PROMPT_WAIT.as_secs()
                ))
            });
            let _ = sender.send(got);
        });
        answer
            .recv()
            .unwrap_or_else(|_| Err("the host stopped".to_owned()))
    }
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

/// The settings after `act`, or why it is refused. `user_names` are the
/// names the user's file has; `pending` the servers of the act's
/// repository waiting for approval, as the host reads them now;
/// `repo_key` the act's repository's [`repo_key`].
pub fn apply(
    settings: &Settings,
    user_names: &BTreeSet<String>,
    pending: &[PendingApproval],
    repo_key: Option<&str>,
    act: &Act,
) -> Result<Settings, String> {
    let mut next = settings.clone();
    let settings_entry = |name: &str| {
        settings
            .servers
            .get(name)
            .ok_or_else(|| format!("No server `{name}` was added here."))
    };
    match act {
        Act::Approve { server, hash, .. } => {
            let waiting = pending
                .iter()
                .find(|pending| pending.server.name == *server)
                .ok_or_else(|| {
                    format!("`{server}` is not waiting for approval.")
                })?;
            if waiting.hash != *hash {
                return Err(format!(
                    "The entry for `{server}` changed since the page showed \
                     it. Look at it again before approving."
                ));
            }
            next.approved.insert(hash.clone());
        }
        Act::Add { name, entry } => {
            if user_names.contains(name) {
                return Err(format!(
                    "Your file mcp.json has a server named `{name}`: edit it \
                     there, or pick another name."
                ));
            }
            if settings.servers.contains_key(name) {
                return Err(format!(
                    "A server named `{name}` was added already: edit it."
                ));
            }
            let entry = server_entry(name, entry)?;
            let namespace = crate::names::namespace(name);
            if let Some(other) = user_names
                .iter()
                .chain(settings.servers.keys())
                .find(|other| crate::names::namespace(other) == namespace)
            {
                return Err(format!(
                    "`{name}` clashes with `{other}`: names that differ only \
                     in `-` and `_` share a namespace."
                ));
            }
            next.servers.insert(name.clone(), entry);
        }
        Act::Edit { name, entry } => {
            settings_entry(name)?;
            next.servers
                .insert(name.clone(), server_entry(name, entry)?);
        }
        Act::Remove { name } => {
            settings_entry(name)?;
            next.servers.remove(name);
        }
        Act::Enable { name, enabled, .. } => {
            if !valid_name(name) {
                return Err(format!("`{name}` is not a server name."));
            }
            next.disabled.set(repo_key, name, !enabled);
        }
        Act::Reconnect { .. }
        | Act::Prompt { .. }
        | Act::SignIn { .. }
        | Act::SignOut { .. } => {}
    }
    Ok(next)
}

/// Whether the page may turn `name` on or off in `sources`, a
/// repository's (`in_repo`) or the user's alone: it must be one of
/// them, and turning it on must turn it on. One its entry turns off is
/// turned on there; one turned off for every repository, on the page of
/// the user's servers.
fn enabling(
    sources: &Sources,
    in_repo: bool,
    name: &str,
    enabled: bool,
) -> Result<(), String> {
    if !sources
        .servers
        .iter()
        .any(|(_, server)| server.name == name)
    {
        return Err(format!("No server `{name}` here."));
    }
    match sources.off.get(name) {
        Some(Off::Entry) if enabled => Err(format!(
            "`{name}` says `\"enabled\": false` in its entry: turn it on there."
        )),
        Some(Off::Everywhere) if enabled && in_repo => Err(format!(
            "`{name}` is turned off for every repository: turn it on in your \
             servers."
        )),
        _ => Ok(()),
    }
}

/// The repository listed as `name`.
fn repo<'a>(cx: &'a HostCx, name: &str) -> anyhow::Result<&'a RepoCtx> {
    cx.repo(name)
        .ok_or_else(|| anyhow::anyhow!("No repository {name}"))
}

/// A run's servers: its scope's plugin, shared with every run in it.
/// Dropped on the host's runtime, so the plugin's connections close
/// there once its last run ends after it was replaced.
pub struct RunServers {
    plugin: Option<McpPlugin>,
    runtime: tokio::runtime::Handle,
}

impl Drop for RunServers {
    fn drop(&mut self) {
        let _runtime = self.runtime.enter();
        self.plugin.take();
    }
}

#[async_trait]
impl Plugin for RunServers {
    fn name(&self) -> &str {
        NAME
    }

    fn tool_source(&self) -> Option<Arc<dyn ToolSource>> {
        self.plugin.as_ref()?.tool_source()
    }

    async fn start(
        &self,
        plan: &mut RunPlan,
        ctx: &PluginCtx,
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        match &self.plugin {
            Some(plugin) => plugin.start(plan, ctx).await,
            None => Ok(Box::new(())),
        }
    }
}

impl UiPlugin for McpUi {
    type State = ();
    type Data = Servers;
    type RepoData = Servers;
    type Settings = Settings;
    type Host = Host;
    type Ui = page::Ui;

    fn name(&self) -> &'static str {
        NAME
    }

    /// The repository's servers, started on its first run, when it has
    /// any that would connect.
    fn agent_plugins(
        &self,
        host: &Host,
        run: &RunCtx,
        settings: &Settings,
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        let scope = Scope::of(Some(&run.repo.checkout));
        let loaded = host.load(&scope, settings);
        if !loaded
            .sources
            .servers
            .iter()
            .any(|(_, server)| server.enabled)
        {
            return Ok(Vec::new());
        }
        let plugin = host.fresh(scope, &loaded, true);
        Ok(vec![Box::new(RunServers {
            plugin,
            runtime: host.runtime.clone(),
        })])
    }

    fn catalog(
        &self,
        host: &Host,
        cx: &HostCx,
        settings: &Settings,
    ) -> PluginInfo {
        // Servers by name, connected where any repository connected it.
        let mut names = BTreeSet::new();
        let mut connected = BTreeSet::new();
        let mut sign_in = BTreeSet::new();
        let mut started = false;
        let mut pending = 0;
        let scopes = std::iter::once(None)
            .chain(cx.repos.iter().map(|repo| Some(repo.checkout.as_path())));
        for repo in scopes {
            let servers = host.servers(repo, settings);
            started |= servers.started;
            pending += servers.pending.len();
            for server in &servers.servers {
                names.insert(server.name.clone());
                if server.state.as_deref() == Some("connected") {
                    connected.insert(server.name.clone());
                }
                if server.state.as_deref() == Some(NEEDS_AUTH) {
                    sign_in.insert(server.name.clone());
                }
            }
        }
        let description = if names.is_empty() && pending == 0 {
            "Connects to MCP servers and adds their tools".to_owned()
        } else {
            format!(
                "MCP servers: {}",
                summary_with_sign_in(
                    names.len(),
                    started.then_some(connected.len()),
                    sign_in.len(),
                    pending
                )
            )
        };
        PluginInfo {
            description,
            seams: vec![Seam::Start, Seam::Tools],
            page: Some(Link::page("servers").param("repo", "")),
            ..Default::default()
        }
    }

    fn data(&self, host: &Host, cx: &HostCx) -> Servers {
        host.servers(None, &cx.settings(NAME))
    }

    fn repo_data(&self, host: &Host, repo: &RepoCtx, cx: &HostCx) -> Servers {
        host.servers(Some(&repo.checkout), &cx.settings(NAME))
    }

    fn act(
        &self,
        host: &Host,
        action: Value,
        cx: &HostCx,
    ) -> anyhow::Result<Option<Value>> {
        let act: Act = serde_json::from_value(action)?;
        let settings: Settings = cx.settings(NAME);
        match &act {
            Act::Prompt {
                repo: name,
                command,
                arguments,
            } => {
                let checkout = match name {
                    Some(name) => Some(repo(cx, name)?.checkout.clone()),
                    None => None,
                };
                let reply = match host.prompt(
                    checkout.as_deref(),
                    &settings,
                    command,
                    arguments,
                ) {
                    Ok(text) => Reply::Prompt { text },
                    Err(error) => Reply::PromptFailed {
                        error,
                        command: format!("/{command} {arguments}")
                            .trim_end()
                            .to_owned(),
                    },
                };
                return Ok(Some(serde_json::to_value(reply)?));
            }
            Act::SignIn { repo: name, server } => {
                let checkout = match name {
                    Some(name) => Some(repo(cx, name)?.checkout.clone()),
                    None => None,
                };
                let (done_cx, name) = (cx.clone(), server.clone());
                let url = host
                    .sign_in(
                        checkout.as_deref(),
                        &settings,
                        server,
                        move |done| {
                            if let Err(error) = done {
                                done_cx.alert(
                                    format!(
                                        "Signing in to {name} did not finish"
                                    ),
                                    error,
                                );
                            }
                            done_cx.refresh();
                        },
                    )
                    .map_err(anyhow::Error::msg)?;
                cx.refresh();
                let reply = Reply::SignIn {
                    server: server.clone(),
                    url,
                };
                return Ok(Some(serde_json::to_value(reply)?));
            }
            Act::SignOut { repo: name, server } => {
                let checkout = match name {
                    Some(name) => Some(repo(cx, name)?.checkout.clone()),
                    None => None,
                };
                host.sign_out(checkout.as_deref(), &settings, server)
                    .map_err(anyhow::Error::msg)?;
            }
            Act::Reconnect { repo: name, server } => {
                let checkout = match name {
                    Some(name) => Some(repo(cx, name)?.checkout.clone()),
                    None => None,
                };
                let plugin = host.plugin(checkout.as_deref(), &settings);
                let _runtime = host.runtime.enter();
                match server {
                    Some(server) => plugin.reconnect(server),
                    None => {
                        for connection in plugin.connections() {
                            plugin.reconnect(connection.name());
                        }
                    }
                }
            }
            act => {
                let checkout = match act {
                    Act::Approve { repo: name, .. }
                    | Act::Enable {
                        repo: Some(name), ..
                    } => Some(repo(cx, name)?.checkout.clone()),
                    _ => None,
                };
                let scope = Scope::of(checkout.as_deref());
                let sources = host.sources(&scope, &settings);
                if let Act::Enable { name, enabled, .. } = act {
                    enabling(&sources, scope.repo().is_some(), name, *enabled)
                        .map_err(anyhow::Error::msg)?;
                }
                let next = apply(
                    &settings,
                    &host.user_names(),
                    &sources.pending,
                    checkout.as_deref().map(repo_key).as_deref(),
                    act,
                )
                .map_err(anyhow::Error::msg)?;
                cx.save_settings(NAME, &next)?;
            }
        }
        cx.refresh();
        Ok(None)
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

impl PluginHost for Host {
    fn new(cx: &HostCx) -> anyhow::Result<Self> {
        let refresher = cx.clone();
        Ok(
            Host::new(cx.runtime.clone(), cx.config_dir().map(Path::to_owned))
                .with_refresh(Arc::new(move || refresher.refresh())),
        )
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
