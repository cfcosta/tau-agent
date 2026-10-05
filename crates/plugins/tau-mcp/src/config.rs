//! Server configuration (`docs/reference/mcp.md`, "Configuration"):
//! the `mcpServers` format, its validation, the merge of the three
//! places servers come from, `${VAR}` and `~/` expansion, and the
//! approval of a repository's servers.
//!
//! Nothing here touches the network or the file system but
//! [`Sources::load`], which reads the two documented files.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::Path,
    sync::Arc,
    time::Duration,
};

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncWrite};

/// Seconds a call may take when an entry does not say.
pub const DEFAULT_TIMEOUT_SECS: f64 = 60.0;

/// The refusal of `type: "sse"`.
pub const SSE_REFUSED: &str =
    "SSE servers are not supported; use the server's streamable HTTP endpoint";

/// The user's file, under the user's configuration directory
/// (`~/.config/tau`).
pub const USER_FILE: &str = "mcp.json";

/// The repository's file, under the repository's directory.
pub const REPO_FILE: &str = ".tau/mcp.json";

/// Who sees a server's or a tool's tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Exposure {
    /// Declared to the model from a run's start, and callable from
    /// Codemode.
    #[default]
    Direct,
    /// Callable from Codemode only.
    Codemode,
    /// Neither.
    Hidden,
}

impl Exposure {
    /// Every exposure, in the order the format lists them.
    pub const ALL: [Self; 3] = [Self::Direct, Self::Codemode, Self::Hidden];

    /// The exposure a config value names.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "direct" => Some(Self::Direct),
            "codemode" => Some(Self::Codemode),
            "hidden" => Some(Self::Hidden),
            _ => None,
        }
    }

    /// The name the format uses.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Codemode => "codemode",
            Self::Hidden => "hidden",
        }
    }
}

impl fmt::Display for Exposure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A stdio server: a process tau starts.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StdioConfig {
    /// One executable, not a shell line. `~/` is expanded.
    pub command: String,
    /// `~/` is expanded in each.
    pub args: Vec<String>,
    /// Values expand `${VAR}`.
    pub env: Vec<(String, String)>,
    /// Relative to the repository. `~/` is expanded.
    pub cwd: Option<String>,
}

/// A streamable HTTP server.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HttpConfig {
    /// http or https.
    pub url: String,
    /// Values expand `${VAR}`.
    pub headers: Vec<(String, String)>,
    /// How to sign in, when the server asks: only for a server without
    /// an `Authorization` header. Without it, sign-in registers a client
    /// of its own and asks for the scopes the server names.
    pub oauth: Option<OAuthConfig>,
}

impl HttpConfig {
    /// Whether OAuth applies: the entry sends no `Authorization` header
    /// of its own.
    pub fn uses_oauth(&self) -> bool {
        !self
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("authorization"))
    }
}

/// An HTTP server's `oauth` block (`docs/reference/mcp.md`, "Signing
/// in"). Every field is optional; an empty string or `null` is the same
/// as leaving it out.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct OAuthConfig {
    /// A client registered with the authorization server beforehand;
    /// without one, sign-in registers a client (RFC 7591).
    pub client_id: Option<String>,
    /// The registered client's secret. Expands `${VAR}`; never printed
    /// by `Debug`.
    pub client_secret: Option<String>,
    /// The loopback port the browser comes back to; any free port when
    /// neither this nor `callback_url` gives one.
    pub callback_port: Option<u16>,
    /// The whole redirect URI, on a loopback host
    /// ([`callback_address`]).
    pub callback_url: Option<String>,
    /// The scopes to ask for, separated by spaces; without them, the
    /// ones the server names.
    pub scope: Option<String>,
    /// The name a registered client goes by; `tau` by default.
    pub client_name: Option<String>,
    /// The authorization server's metadata, when discovery from the
    /// server cannot find it.
    pub auth_server_metadata_url: Option<String>,
}

impl fmt::Debug for OAuthConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuthConfig")
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "[redacted]"),
            )
            .field("callback_port", &self.callback_port)
            .field("callback_url", &self.callback_url)
            .field("scope", &self.scope)
            .field("client_name", &self.client_name)
            .field("auth_server_metadata_url", &self.auth_server_metadata_url)
            .finish()
    }
}

/// The path the browser comes back to when `callbackUrl` does not name
/// one.
pub const CALLBACK_PATH: &str = "/callback";

/// Where the browser comes back to after signing in: a loopback address
/// tau listens on, and the redirect URI it registers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallbackAddress {
    /// What to listen on: `127.0.0.1`, or `::1`.
    pub ip: std::net::IpAddr,
    /// 0 for any free port.
    pub port: u16,
    /// As the redirect URI writes it: `127.0.0.1`, `localhost` or
    /// `[::1]`.
    pub host: String,
    /// Starts with `/`.
    pub path: String,
}

impl CallbackAddress {
    /// The redirect URI once listening on `port`.
    pub fn redirect_uri(&self, port: u16) -> String {
        format!("http://{}:{port}{}", self.host, self.path)
    }
}

/// The callback address an `oauth` block asks for: its `callbackUrl`,
/// which must be plain `http` on a loopback host (`127.0.0.0/8`,
/// `localhost` or `[::1]`) with no user, query or fragment, else
/// `http://127.0.0.1:<callbackPort>/callback`. A port in the URL and a
/// `callbackPort` must agree; without either, any free port.
pub fn callback_address(
    callback_url: Option<&str>,
    callback_port: Option<u16>,
) -> Result<CallbackAddress, String> {
    let Some(text) = callback_url else {
        return Ok(CallbackAddress {
            ip: std::net::Ipv4Addr::LOCALHOST.into(),
            port: callback_port.unwrap_or(0),
            host: "127.0.0.1".into(),
            path: CALLBACK_PATH.into(),
        });
    };
    let url = url::Url::parse(text)
        .map_err(|error| format!("`callbackUrl` is not a URL: {error}"))?;
    if url.scheme() != "http" {
        return Err("`callbackUrl` must be an http:// URL: tau listens on \
             it without TLS"
            .into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("`callbackUrl` must not carry a user or password".into());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("`callbackUrl` must not carry a query or fragment".into());
    }
    let (ip, host): (std::net::IpAddr, String) = match url.host() {
        Some(url::Host::Ipv4(ip)) if ip.is_loopback() => {
            (ip.into(), ip.to_string())
        }
        Some(url::Host::Ipv6(ip)) if ip.is_loopback() => {
            (ip.into(), format!("[{ip}]"))
        }
        Some(url::Host::Domain(domain))
            if domain.eq_ignore_ascii_case("localhost") =>
        {
            (std::net::Ipv4Addr::LOCALHOST.into(), "localhost".into())
        }
        _ => {
            return Err("`callbackUrl` must be on a loopback host: \
                 127.0.0.1, localhost or [::1]"
                .into());
        }
    };
    // `Url` drops the port when it is http's default, 80.
    let port = url.port().or_else(|| {
        let authority = text.split_once("://")?.1.split('/').next()?;
        authority.ends_with(":80").then_some(80)
    });
    let port = match (port, callback_port) {
        (Some(a), Some(b)) if a != b => {
            return Err(format!(
                "`callbackUrl` says port {a} and `callbackPort` says {b}"
            ));
        }
        (Some(port), _) | (None, Some(port)) => port,
        (None, None) => {
            return Err("`callbackUrl` must name its port, or give \
                 `callbackPort`"
                .into());
        }
    };
    Ok(CallbackAddress {
        ip,
        port,
        host,
        path: url.path().to_owned(),
    })
}

/// A byte stream to a server, for [`Transport::Stream`].
pub trait ServerStream:
    AsyncRead + AsyncWrite + Send + Unpin + 'static
{
}

impl<T: AsyncRead + AsyncWrite + Send + Unpin + 'static> ServerStream for T {}

/// What [`Dial::open`] gives.
pub type OpenStream =
    BoxFuture<'static, std::io::Result<Box<dyn ServerStream>>>;

/// Opens a new stream to a server, once per connection: an in-process
/// server's end of a `tokio::io::duplex`, for example.
#[derive(Clone)]
pub struct Dial(Arc<dyn Fn() -> OpenStream + Send + Sync>);

impl Dial {
    pub fn new<F, Fut, S>(open: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = std::io::Result<S>> + Send + 'static,
        S: ServerStream,
    {
        Self(Arc::new(move || {
            let future = open();
            Box::pin(async move {
                future
                    .await
                    .map(|stream| Box::new(stream) as Box<dyn ServerStream>)
            })
        }))
    }

    /// Opens a stream.
    pub fn open(&self) -> OpenStream {
        (self.0)()
    }
}

impl PartialEq for Dial {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// How tau reaches a server.
#[derive(Clone, PartialEq)]
pub enum Transport {
    Stdio(StdioConfig),
    Http(HttpConfig),
    /// A stream the host opens itself, such as an in-process server's.
    /// Never read from or written to a file: [`McpConfig::to_value`]
    /// leaves these servers out.
    Stream(Dial),
}

impl fmt::Debug for Transport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stdio(stdio) => f.debug_tuple("Stdio").field(stdio).finish(),
            Self::Http(http) => f.debug_tuple("Http").field(http).finish(),
            Self::Stream(_) => f.write_str("Stream"),
        }
    }
}

/// One `mcpServers` entry, validated.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerConfig {
    /// Matches `^[A-Za-z0-9_-]+$`.
    pub name: String,
    pub transport: Transport,
    pub exposure: Exposure,
    /// Tool name or `*` glob to exposure, in the file's order.
    pub tool_exposure: Vec<(String, Exposure)>,
    pub description: Option<String>,
    pub enabled: bool,
    /// Seconds per call; a progress notification starts it again.
    pub timeout: f64,
}

impl ServerConfig {
    /// An enabled, `direct` server with the default timeout.
    pub fn new(name: impl Into<String>, transport: Transport) -> Self {
        Self {
            name: name.into(),
            transport,
            exposure: Exposure::Direct,
            tool_exposure: Vec::new(),
            description: None,
            enabled: true,
            timeout: DEFAULT_TIMEOUT_SECS,
        }
    }

    /// The exposure of the server's tool `tool`: an exact name in
    /// `toolExposure`, else the first pattern that matches, in order,
    /// else the server's.
    pub fn exposure_of(&self, tool: &str) -> Exposure {
        exposure_of(&self.tool_exposure, self.exposure, tool)
    }

    /// Whether any of the server's tools can be `direct`, before it has
    /// listed them.
    pub fn may_have_direct(&self) -> bool {
        self.exposure == Exposure::Direct
            || self
                .tool_exposure
                .iter()
                .any(|(_, exposure)| *exposure == Exposure::Direct)
    }

    /// Whether the server depends on the repository it runs for: a
    /// stdio server whose `cwd` is relative (not absolute, not `~/`)
    /// starts in the repository. Such a server from the user's file or
    /// the settings runs once per repository; any other is shared by
    /// all of them.
    pub fn per_repo(&self) -> bool {
        match &self.transport {
            Transport::Stdio(StdioConfig { cwd: Some(cwd), .. }) => {
                !cwd.starts_with('~') && Path::new(cwd).is_relative()
            }
            _ => false,
        }
    }

    /// The time a call may take.
    pub fn timeout(&self) -> Duration {
        Duration::from_secs_f64(self.timeout)
    }

    /// The server's namespace, `mcp__<name with - as _>`.
    pub fn namespace(&self) -> String {
        format!("mcp__{}", self.name.replace('-', "_"))
    }

    /// The entry as the format writes it, defaults left out; `None` for
    /// a [`Transport::Stream`] server.
    pub fn to_entry(&self) -> Option<Value> {
        let mut entry = Map::new();
        match &self.transport {
            Transport::Stdio(stdio) => {
                entry.insert("command".into(), json!(stdio.command));
                if !stdio.args.is_empty() {
                    entry.insert("args".into(), json!(stdio.args));
                }
                if !stdio.env.is_empty() {
                    entry.insert("env".into(), pairs_to_object(&stdio.env));
                }
                if let Some(cwd) = &stdio.cwd {
                    entry.insert("cwd".into(), json!(cwd));
                }
            }
            Transport::Http(http) => {
                entry.insert("url".into(), json!(http.url));
                if !http.headers.is_empty() {
                    entry.insert(
                        "headers".into(),
                        pairs_to_object(&http.headers),
                    );
                }
                if let Some(oauth) = &http.oauth {
                    entry.insert("oauth".into(), oauth_to_value(oauth));
                }
            }
            Transport::Stream(_) => return None,
        }
        if self.exposure != Exposure::Direct {
            entry.insert("exposure".into(), json!(self.exposure.as_str()));
        }
        if !self.tool_exposure.is_empty() {
            let map: Map<String, Value> = self
                .tool_exposure
                .iter()
                .map(|(tool, exposure)| {
                    (tool.clone(), json!(exposure.as_str()))
                })
                .collect();
            entry.insert("toolExposure".into(), Value::Object(map));
        }
        if let Some(description) = &self.description {
            entry.insert("description".into(), json!(description));
        }
        if !self.enabled {
            entry.insert("enabled".into(), json!(false));
        }
        if self.timeout != DEFAULT_TIMEOUT_SECS {
            entry.insert("timeout".into(), json!(self.timeout));
        }
        Some(Value::Object(entry))
    }

    /// The hash a repository server's approval is saved under: the
    /// SHA-256, in hex, of the name, a NUL and the entry as
    /// [`Self::to_entry`] prints it. Any change to the entry changes it;
    /// formatting the file does not.
    pub fn approval_hash(&self) -> String {
        let entry = self.to_entry().unwrap_or(Value::Null);
        let mut hasher = Sha256::new();
        hasher.update(self.name.as_bytes());
        hasher.update([0]);
        hasher.update(entry.to_string().as_bytes());
        hex(&hasher.finalize())
    }
}

fn oauth_to_value(oauth: &OAuthConfig) -> Value {
    let mut object = Map::new();
    let mut put = |key: &str, value: Option<Value>| {
        if let Some(value) = value {
            object.insert(key.into(), value);
        }
    };
    put("clientId", oauth.client_id.as_ref().map(|v| json!(v)));
    put(
        "clientSecret",
        oauth.client_secret.as_ref().map(|v| json!(v)),
    );
    put("callbackPort", oauth.callback_port.map(|v| json!(v)));
    put("callbackUrl", oauth.callback_url.as_ref().map(|v| json!(v)));
    put("scope", oauth.scope.as_ref().map(|v| json!(v)));
    put("clientName", oauth.client_name.as_ref().map(|v| json!(v)));
    put(
        "authServerMetadataUrl",
        oauth.auth_server_metadata_url.as_ref().map(|v| json!(v)),
    );
    Value::Object(object)
}

fn pairs_to_object(pairs: &[(String, String)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| (key.clone(), json!(value)))
            .collect(),
    )
}

/// The exposure of `tool` under `rules`: an exact name wins, then the
/// first pattern with a `*` that matches, in order, then `default`.
pub fn exposure_of(
    rules: &[(String, Exposure)],
    default: Exposure,
    tool: &str,
) -> Exposure {
    if let Some((_, exposure)) = rules.iter().find(|(key, _)| key == tool) {
        return *exposure;
    }
    rules
        .iter()
        .find(|(key, _)| key.contains('*') && glob_matches(key, tool))
        .map_or(default, |(_, exposure)| *exposure)
}

/// Whether `pattern`, where `*` stands for any run of characters,
/// matches all of `text`.
pub fn glob_matches(pattern: &str, text: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or_default();
    let Some(mut rest) = text.strip_prefix(first) else {
        return false;
    };
    let parts: Vec<&str> = parts.collect();
    let Some((last, middle)) = parts.split_last() else {
        // No `*`: the whole text must be the pattern.
        return rest.is_empty();
    };
    for part in middle {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    rest.len() >= last.len() && rest.ends_with(last)
}

/// Where a server's entry came from, in merge order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Origin {
    /// The user's file, `~/.config/tau/mcp.json`.
    User,
    /// The plugin's settings, which tau-ui's page edits.
    Settings,
    /// The repository's file, `<repository>/.tau/mcp.json`. Its servers
    /// wait for the user's approval.
    Repo,
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::User => "user file",
            Self::Settings => "settings",
            Self::Repo => "repository file",
        })
    }
}

/// An entry or a file that was skipped, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    pub origin: Option<Origin>,
    /// The server, when one entry is at fault.
    pub server: Option<String>,
    pub message: String,
}

impl ConfigError {
    fn server(server: &str, message: impl Into<String>) -> Self {
        Self {
            origin: None,
            server: Some(server.to_owned()),
            message: message.into(),
        }
    }

    fn file(message: impl Into<String>) -> Self {
        Self {
            origin: None,
            server: None,
            message: message.into(),
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(origin) = self.origin {
            write!(f, "{origin}: ")?;
        }
        if let Some(server) = &self.server {
            write!(f, "server `{server}`: ")?;
        }
        f.write_str(&self.message)
    }
}

impl std::error::Error for ConfigError {}

/// Whether `name` is a valid server name: `^[A-Za-z0-9_-]+$`.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
        })
}

/// A set of servers: one file's, or the settings'.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct McpConfig {
    pub servers: Vec<ServerConfig>,
}

impl McpConfig {
    /// Reads a file's text. A file that is not JSON is one error and no
    /// servers; an invalid entry is an error and is skipped, and the
    /// others are kept.
    pub fn parse(text: &str) -> (Self, Vec<ConfigError>) {
        match serde_json::from_str::<Value>(text) {
            Ok(value) => Self::from_value(&value),
            Err(error) => (
                Self::default(),
                vec![ConfigError::file(format!("not valid JSON: {error}"))],
            ),
        }
    }

    /// Reads `{ "mcpServers": { ... } }`. No `mcpServers` is no servers.
    pub fn from_value(value: &Value) -> (Self, Vec<ConfigError>) {
        let mut errors = Vec::new();
        let Some(object) = value.as_object() else {
            return (
                Self::default(),
                vec![ConfigError::file("the file must hold a JSON object")],
            );
        };
        let servers = match object.get("mcpServers") {
            None | Some(Value::Null) => return (Self::default(), errors),
            Some(Value::Object(servers)) => servers,
            Some(_) => {
                return (
                    Self::default(),
                    vec![ConfigError::file("`mcpServers` must be an object")],
                );
            }
        };
        let mut config = Self::default();
        for (name, entry) in servers {
            match parse_entry(name, entry) {
                Ok(server) => config.servers.push(server),
                Err(message) => errors.push(ConfigError::server(name, message)),
            }
        }
        (config, errors)
    }

    /// `{ "mcpServers": { ... } }`, every server but the
    /// [`Transport::Stream`] ones.
    pub fn to_value(&self) -> Value {
        let servers: Map<String, Value> = self
            .servers
            .iter()
            .filter_map(|server| {
                Some((server.name.clone(), server.to_entry()?))
            })
            .collect();
        json!({ "mcpServers": servers })
    }

    /// [`Self::to_value`], pretty-printed.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(&self.to_value())
            .expect("a JSON value always prints")
    }
}

fn parse_entry(name: &str, entry: &Value) -> Result<ServerConfig, String> {
    if !valid_name(name) {
        return Err(
            "a server name may hold only letters, digits, `-` and `_`".into()
        );
    }
    let Some(entry) = entry.as_object() else {
        return Err("the entry must be an object".into());
    };
    let kind = match entry.get("type") {
        None | Some(Value::Null) => None,
        Some(Value::String(kind)) => Some(kind.as_str()),
        Some(_) => return Err("`type` must be a string".into()),
    };
    let has_command = entry.contains_key("command");
    let has_url = entry.contains_key("url");
    let transport = match kind {
        Some("sse") => return Err(SSE_REFUSED.into()),
        Some("stdio") => stdio(entry)?,
        Some("http" | "streamable-http") => http(entry)?,
        Some(other) => {
            return Err(format!(
                "unknown `type` `{other}`; use `stdio`, `http` or `streamable-http`"
            ));
        }
        None => match (has_command, has_url) {
            (true, true) => {
                return Err("give either `command` or `url`, not both".into());
            }
            (true, false) => stdio(entry)?,
            (false, true) => http(entry)?,
            (false, false) => {
                return Err("give `command` for a stdio server or `url` for an HTTP one".into());
            }
        },
    };
    let exposure = match entry.get("exposure") {
        None | Some(Value::Null) => Exposure::Direct,
        Some(value) => exposure_value(value, "`exposure`")?,
    };
    let tool_exposure = match entry.get("toolExposure") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Object(map)) => map
            .iter()
            .map(|(tool, value)| {
                Ok((
                    tool.clone(),
                    exposure_value(value, &format!("`toolExposure.{tool}`"))?,
                ))
            })
            .collect::<Result<_, String>>()?,
        Some(_) => return Err("`toolExposure` must be an object".into()),
    };
    let description = optional_string(entry, "description")?;
    let enabled = match entry.get("enabled") {
        None | Some(Value::Null) => true,
        Some(Value::Bool(enabled)) => *enabled,
        Some(_) => return Err("`enabled` must be true or false".into()),
    };
    let timeout = match entry.get("timeout") {
        None | Some(Value::Null) => DEFAULT_TIMEOUT_SECS,
        Some(value) => match value.as_f64() {
            Some(secs) if secs > 0.0 && secs.is_finite() && secs <= 1e9 => secs,
            _ => {
                return Err(
                    "`timeout` must be a positive number of seconds".into()
                );
            }
        },
    };
    Ok(ServerConfig {
        name: name.to_owned(),
        transport,
        exposure,
        tool_exposure,
        description,
        enabled,
        timeout,
    })
}

fn exposure_value(value: &Value, what: &str) -> Result<Exposure, String> {
    value.as_str().and_then(Exposure::parse).ok_or_else(|| {
        format!("{what} must be `direct`, `codemode` or `hidden`")
    })
}

fn optional_string(
    entry: &Map<String, Value>,
    key: &str,
) -> Result<Option<String>, String> {
    match entry.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(format!("`{key}` must be a string")),
    }
}

fn string_map(
    entry: &Map<String, Value>,
    key: &str,
) -> Result<Vec<(String, String)>, String> {
    match entry.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Object(map)) => map
            .iter()
            .map(|(name, value)| match value {
                Value::String(value) => Ok((name.clone(), value.clone())),
                _ => Err(format!("`{key}.{name}` must be a string")),
            })
            .collect(),
        Some(_) => Err(format!("`{key}` must be an object of strings")),
    }
}

fn stdio(entry: &Map<String, Value>) -> Result<Transport, String> {
    if entry.contains_key("url") {
        return Err("a stdio server takes `command`, not `url`".into());
    }
    let command = match entry.get("command") {
        Some(Value::String(command)) if !command.trim().is_empty() => {
            command.clone()
        }
        _ => return Err("`command` must be a non-empty string".into()),
    };
    let args = match entry.get("args") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(args)) => args
            .iter()
            .map(|arg| {
                arg.as_str().map(str::to_owned).ok_or_else(|| {
                    "`args` must be an array of strings".to_owned()
                })
            })
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("`args` must be an array of strings".into()),
    };
    Ok(Transport::Stdio(StdioConfig {
        command,
        args,
        env: string_map(entry, "env")?,
        cwd: optional_string(entry, "cwd")?,
    }))
}

fn http(entry: &Map<String, Value>) -> Result<Transport, String> {
    if entry.contains_key("command") {
        return Err("an HTTP server takes `url`, not `command`".into());
    }
    let url = match entry.get("url") {
        Some(Value::String(url)) => url.clone(),
        _ => return Err("`url` must be a string".into()),
    };
    let lower = url.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"));
    if rest.is_none_or(str::is_empty) {
        return Err("`url` must be an http or https URL".into());
    }
    let headers = string_map(entry, "headers")?;
    let oauth = match entry.get("oauth") {
        None | Some(Value::Null) => None,
        Some(Value::Object(block)) => Some(oauth(block)?),
        Some(_) => return Err("`oauth` must be an object".into()),
    };
    let config = HttpConfig {
        url,
        headers,
        oauth,
    };
    if config.oauth.is_some() && !config.uses_oauth() {
        return Err("`oauth` is for a server without an `Authorization` \
             header: give one or the other"
            .into());
    }
    Ok(Transport::Http(config))
}

/// An `oauth` block, checked. Empty strings and nulls are absent.
fn oauth(block: &Map<String, Value>) -> Result<OAuthConfig, String> {
    let text = |key: &str| -> Result<Option<String>, String> {
        match block.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(text)) if text.is_empty() => Ok(None),
            Some(Value::String(text)) => Ok(Some(text.clone())),
            Some(_) => Err(format!("`oauth.{key}` must be a string")),
        }
    };
    let callback_port = match block.get("callbackPort") {
        None | Some(Value::Null) => None,
        Some(value) => match value.as_u64() {
            Some(port @ 1..=65535) => Some(port as u16),
            _ => {
                return Err(
                    "`oauth.callbackPort` must be a port, 1 to 65535".into()
                );
            }
        },
    };
    let config = OAuthConfig {
        client_id: text("clientId")?,
        client_secret: text("clientSecret")?,
        callback_port,
        callback_url: text("callbackUrl")?,
        scope: text("scope")?,
        client_name: text("clientName")?,
        auth_server_metadata_url: text("authServerMetadataUrl")?,
    };
    if config.client_secret.is_some() && config.client_id.is_none() {
        return Err("`oauth.clientSecret` needs `oauth.clientId`".into());
    }
    callback_address(config.callback_url.as_deref(), config.callback_port)
        .map_err(|error| format!("`oauth`: {error}"))?;
    if let Some(url) = &config.auth_server_metadata_url {
        match url::Url::parse(url) {
            Ok(parsed) if matches!(parsed.scheme(), "http" | "https") => {}
            _ => {
                return Err("`oauth.authServerMetadataUrl` must be an http \
                     or https URL"
                    .into());
            }
        }
    }
    Ok(config)
}

/// Servers merged from every place they come from, in order, a later
/// entry replacing an earlier one of the same name in its place. A name
/// that differs from an earlier one only in `-` and `_` clashes with it:
/// it is reported and skipped.
pub fn merge(
    layers: &[(Origin, &McpConfig)],
) -> (Vec<(Origin, ServerConfig)>, Vec<ConfigError>) {
    let mut merged: Vec<(Origin, ServerConfig)> = Vec::new();
    for (origin, config) in layers {
        for server in &config.servers {
            match merged.iter().position(|(_, kept)| kept.name == server.name) {
                Some(at) => merged[at] = (*origin, server.clone()),
                None => merged.push((*origin, server.clone())),
            }
        }
    }
    let mut kept: Vec<(Origin, ServerConfig)> = Vec::new();
    let mut errors = Vec::new();
    for (origin, server) in merged {
        let namespace = server.namespace();
        match kept.iter().find(|(_, other)| other.namespace() == namespace) {
            Some((_, other)) => errors.push(ConfigError {
                origin: Some(origin),
                server: Some(server.name.clone()),
                message: format!(
                    "clashes with server `{}`: names that differ only in `-` and `_` share the namespace `{namespace}`",
                    other.name
                ),
            }),
            None => kept.push((origin, server)),
        }
    }
    (kept, errors)
}

/// The plugin's settings, which tau-ui's page edits: the servers added
/// there, the repository servers the user approved, by
/// [`ServerConfig::approval_hash`], and the servers the page turned off.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Settings {
    #[serde(
        rename = "mcpServers",
        default,
        skip_serializing_if = "Map::is_empty"
    )]
    pub servers: Map<String, Value>,
    #[serde(
        rename = "approvedRepoServers",
        default,
        skip_serializing_if = "BTreeSet::is_empty"
    )]
    pub approved: BTreeSet<String>,
    #[serde(
        rename = "disabledServers",
        default,
        skip_serializing_if = "Disabled::is_empty"
    )]
    pub disabled: Disabled,
}

impl Settings {
    /// The settings' servers, validated.
    pub fn config(&self) -> (McpConfig, Vec<ConfigError>) {
        McpConfig::from_value(&json!({ "mcpServers": self.servers }))
    }
}

/// The servers the page turned off, by name, without touching their
/// entries: `user` for the user's servers (the user's file's, the
/// settings') in every repository, and `repos`, keyed by the
/// repository's directory ([`repo_key`]), for any server in that
/// repository alone.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Disabled {
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub user: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub repos: BTreeMap<String, BTreeSet<String>>,
}

impl Disabled {
    pub fn is_empty(&self) -> bool {
        self.user.is_empty() && self.repos.is_empty()
    }

    /// Whether `name` is turned off at `repo` (a [`repo_key`]), or for
    /// the user's servers everywhere without one.
    pub fn is_off(&self, repo: Option<&str>, name: &str) -> bool {
        match repo {
            None => self.user.contains(name),
            Some(repo) => self
                .repos
                .get(repo)
                .is_some_and(|names| names.contains(name)),
        }
    }

    /// Turns `name` off, or on again, at `repo` or everywhere. A
    /// repository left with nothing turned off is forgotten.
    pub fn set(&mut self, repo: Option<&str>, name: &str, off: bool) {
        let names = match repo {
            None => &mut self.user,
            Some(repo) => self.repos.entry(repo.to_owned()).or_default(),
        };
        if off {
            names.insert(name.to_owned());
        } else {
            names.remove(name);
        }
        self.repos.retain(|_, names| !names.is_empty());
    }

    /// Why the server `name` from `origin`, whose entry says `enabled`,
    /// is off in `repo` (a [`repo_key`]), or in the user's servers alone
    /// without one; `None` when it is on. Its entry's `enabled: false`
    /// comes first, then the user's servers turned off everywhere (which
    /// a repository's own server ignores), then the repository's.
    pub fn off(
        &self,
        origin: Origin,
        name: &str,
        enabled: bool,
        repo: Option<&str>,
    ) -> Option<Off> {
        if !enabled {
            Some(Off::Entry)
        } else if origin != Origin::Repo && self.is_off(None, name) {
            Some(Off::Everywhere)
        } else if repo.is_some() && self.is_off(repo, name) {
            Some(Off::Repo)
        } else {
            None
        }
    }
}

/// The key of `repo` in [`Disabled::repos`]: its directory, as written.
pub fn repo_key(repo: &Path) -> String {
    repo.display().to_string()
}

/// Why a server is off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Off {
    /// Its entry says `enabled: false`; only editing it turns it on.
    Entry,
    /// The page turned one of the user's servers off for every
    /// repository.
    Everywhere,
    /// The page turned it off in this repository.
    Repo,
}

/// A repository server waiting for the user's approval.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingApproval {
    pub server: ServerConfig,
    /// What to add to [`Settings::approved`] to approve it.
    pub hash: String,
}

/// Every server tau would connect to, after merging and approval.
#[derive(Debug, Clone, Default)]
pub struct Sources {
    /// The servers to run, enabled or not, in merge order.
    pub servers: Vec<(Origin, ServerConfig)>,
    /// Entries and files that were skipped.
    pub errors: Vec<ConfigError>,
    /// Repository servers that are not connected until approved.
    pub pending: Vec<PendingApproval>,
    /// Why each server that is off is off, by name.
    pub off: BTreeMap<String, Off>,
}

impl Sources {
    /// Merges the three layers. `user` and `repo` are the files' parsed
    /// contents, when they exist.
    pub fn merge(
        user: Option<&McpConfig>,
        settings: &Settings,
        repo: Option<&McpConfig>,
    ) -> Self {
        let (settings_config, mut errors) = settings.config();
        for error in &mut errors {
            error.origin = Some(Origin::Settings);
        }
        let empty = McpConfig::default();
        let (merged, merge_errors) = merge(&[
            (Origin::User, user.unwrap_or(&empty)),
            (Origin::Settings, &settings_config),
            (Origin::Repo, repo.unwrap_or(&empty)),
        ]);
        errors.extend(merge_errors);
        let mut servers = Vec::new();
        let mut pending = Vec::new();
        for (origin, server) in merged {
            let hash = server.approval_hash();
            if origin == Origin::Repo && !settings.approved.contains(&hash) {
                pending.push(PendingApproval { server, hash });
            } else {
                servers.push((origin, server));
            }
        }
        Self {
            servers,
            errors,
            pending,
            off: BTreeMap::new(),
        }
    }

    /// Turns off the servers the settings turn off in `repo`, or in the
    /// user's servers alone without one ([`Disabled::off`]): a server is
    /// on when its entry says so and nothing turned it off. Run after
    /// every server is added, once: a server it turned off reads as one
    /// whose entry is off. Approvals are not touched.
    pub fn disable(&mut self, settings: &Settings, repo: Option<&Path>) {
        let key = repo.map(repo_key);
        self.off.clear();
        for (origin, server) in &mut self.servers {
            let off = settings.disabled.off(
                *origin,
                &server.name,
                server.enabled,
                key.as_deref(),
            );
            server.enabled = off.is_none();
            if let Some(off) = off {
                self.off.insert(server.name.clone(), off);
            }
        }
    }

    /// Reads `<user_dir>/mcp.json` and `<repo>/.tau/mcp.json`, where they
    /// exist, and merges them with the settings' servers.
    pub fn load(
        user_dir: Option<&Path>,
        settings: &Settings,
        repo: Option<&Path>,
    ) -> Self {
        Self::from_reads(&Read::user(user_dir), settings, &Read::repo(repo))
    }

    /// [`Self::merge`] of files already read, their errors first.
    pub fn from_reads(user: &Read, settings: &Settings, repo: &Read) -> Self {
        let mut sources =
            Self::merge(user.config.as_ref(), settings, repo.config.as_ref());
        let mut errors: Vec<ConfigError> =
            user.errors.iter().chain(&repo.errors).cloned().collect();
        errors.append(&mut sources.errors);
        sources.errors = errors;
        sources
    }

    /// Adds `server` after every file and the settings, replacing one of
    /// the same name, as the settings'. A name that clashes with another
    /// server's namespace is reported and skipped.
    pub fn add(&mut self, server: ServerConfig) {
        let namespace = server.namespace();
        let clash = self.servers.iter().find(|(_, other)| {
            other.name != server.name && other.namespace() == namespace
        });
        if let Some((_, other)) = clash {
            self.errors.push(ConfigError {
                origin: Some(Origin::Settings),
                server: Some(server.name.clone()),
                message: format!(
                    "clashes with server `{}`: names that differ only in `-` and `_` share the namespace `{namespace}`",
                    other.name
                ),
            });
            return;
        }
        self.pending
            .retain(|pending| pending.server.name != server.name);
        match self.servers.iter().position(|(_, s)| s.name == server.name) {
            Some(at) => self.servers[at] = (Origin::Settings, server),
            None => self.servers.push((Origin::Settings, server)),
        }
    }
}

/// A file of servers as read: its servers, when it exists, and what was
/// wrong with it.
#[derive(Debug, Clone, Default)]
pub struct Read {
    pub config: Option<McpConfig>,
    pub errors: Vec<ConfigError>,
}

impl Read {
    /// Reads `path`, its errors marked as `origin`'s. A file that is not
    /// there is no servers and no error.
    pub fn file(path: &Path, origin: Origin) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Self::default();
            }
            Err(error) => {
                return Self {
                    config: None,
                    errors: vec![ConfigError {
                        origin: Some(origin),
                        server: None,
                        message: format!(
                            "cannot read {}: {error}",
                            path.display()
                        ),
                    }],
                };
            }
        };
        let (config, errors) = McpConfig::parse(&text);
        Self {
            config: Some(config),
            errors: errors
                .into_iter()
                .map(|mut error| {
                    error.origin = Some(origin);
                    error
                })
                .collect(),
        }
    }

    /// `<user_dir>/mcp.json`, when there is a user directory.
    pub fn user(user_dir: Option<&Path>) -> Self {
        user_dir.map_or_else(Self::default, |dir| {
            Self::file(&dir.join(USER_FILE), Origin::User)
        })
    }

    /// `<repo>/.tau/mcp.json`, when there is a repository.
    pub fn repo(repo: Option<&Path>) -> Self {
        repo.map_or_else(Self::default, |dir| {
            Self::file(&dir.join(REPO_FILE), Origin::Repo)
        })
    }
}

/// Looks a variable up in tau's environment.
pub type EnvLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// The process's environment.
pub fn process_env() -> EnvLookup {
    Arc::new(|name| std::env::var(name).ok())
}

/// Replaces each `${NAME}` (`NAME` a letter or `_`, then letters, digits
/// and `_`) with the variable's value. Anything else, a lone `$` or an
/// unclosed `${` included, stays as it is. A variable that is not set
/// is the error, by name.
pub fn expand_vars(
    text: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("${") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        let name_len = after
            .bytes()
            .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
            .count();
        let name = &after[..name_len];
        let valid = !name.is_empty()
            && !name.as_bytes()[0].is_ascii_digit()
            && after[name_len..].starts_with('}');
        if valid {
            out.push_str(&lookup(name).ok_or_else(|| name.to_owned())?);
            rest = &after[name_len + 1..];
        } else {
            out.push_str("${");
            rest = after;
        }
    }
    out.push_str(rest);
    Ok(out)
}

/// `~/` at the start of `text` becomes `home/`; `~` alone becomes
/// `home`. Without a home, the text stays as it is.
pub fn expand_home(text: &str, home: Option<&Path>) -> String {
    let Some(home) = home else {
        return text.to_owned();
    };
    if text == "~" {
        return home.display().to_string();
    }
    match text.strip_prefix("~/") {
        Some(rest) => home.join(rest).display().to_string(),
        None => text.to_owned(),
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob_matches("git_*", "git_push"));
        assert!(glob_matches("*", ""));
        assert!(glob_matches("a*b*c", "abc"));
        assert!(glob_matches("a*b*c", "a__b__c"));
        assert!(!glob_matches("a*b*c", "acb"));
        assert!(!glob_matches("ab*ba", "aba"));
        assert!(glob_matches("exact", "exact"));
        assert!(!glob_matches("exact", "exactly"));
    }

    #[test]
    fn sse_is_refused() {
        let (config, errors) = McpConfig::parse(
            r#"{"mcpServers": {"s": {"type": "sse", "url": "https://x"}, "ok": {"command": "x"}}}"#,
        );
        assert_eq!(config.servers.len(), 1);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].message, SSE_REFUSED);
        assert_eq!(errors[0].server.as_deref(), Some("s"));
    }

    #[test]
    fn missing_variable_is_named() {
        let lookup = |name: &str| (name == "A").then(|| "1".to_owned());
        assert_eq!(expand_vars("x${A}y", &lookup).unwrap(), "x1y");
        assert_eq!(expand_vars("${B}", &lookup).unwrap_err(), "B");
        assert_eq!(expand_vars("${1A} ${", &lookup).unwrap(), "${1A} ${");
    }

    #[test]
    fn home_is_expanded() {
        let home = Path::new("/h");
        assert_eq!(expand_home("~/bin/x", Some(home)), "/h/bin/x");
        assert_eq!(expand_home("a~/x", Some(home)), "a~/x");
        assert_eq!(expand_home("~/x", None), "~/x");
    }
}
