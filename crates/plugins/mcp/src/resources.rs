//! Servers' resources as three tools, as pi and Codex have them
//! (`docs/reference/mcp.md`, "Resources"): `list_mcp_resources`,
//! `list_mcp_resource_templates` and `read_mcp_resource`.
//!
//! They are the plugin's, not a server's: one of each for every server
//! that offers resources. Their exposure is the widest among those
//! servers' ([`exposure`]): declared when one is `direct`, `Nested` when
//! the widest is `codemode`, absent when no server that is not hidden
//! offers resources. MCP apps' resources (`ui://`, or
//! `text/html;profile=mcp-app`) are left out everywhere ([`is_app`]).

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use futures_util::future::join_all;
use serde_json::{Value, json};
use tau_agent::tool::{
    AgentTool,
    Exposure as ToolExposure,
    ToolCtx,
    ToolError,
    ToolOutput,
};
use tokio_util::sync::CancellationToken;

use crate::{
    config::Exposure,
    connection::{Connection, State},
    results::{Spill, TEXT_LIMIT, resource_contents, text_block, truncate},
};

/// Lists servers' resources.
pub const LIST_RESOURCES: &str = "list_mcp_resources";
/// Lists servers' resource templates.
pub const LIST_TEMPLATES: &str = "list_mcp_resource_templates";
/// Reads one resource.
pub const READ_RESOURCE: &str = "read_mcp_resource";

/// The MIME type of an MCP app's interface.
const APP_MIME: &str = "text/html;profile=mcp-app";

/// Whether `uri` (or a template) with `mime` is an MCP app's: a `ui://`
/// URI, or the MCP app MIME type, spaces and case aside. tau shows no
/// MCP apps, so these are left out.
pub fn is_app(uri: &str, mime: Option<&str>) -> bool {
    let app_mime = mime.is_some_and(|mime| {
        let flat: String = mime
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_ascii_lowercase();
        flat == APP_MIME
    });
    uri.get(..5)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("ui://"))
        || app_mime
}

/// How wide an exposure is: hidden, then codemode, then direct.
fn width(exposure: Exposure) -> u8 {
    match exposure {
        Exposure::Hidden => 0,
        Exposure::Codemode => 1,
        Exposure::Direct => 2,
    }
}

/// The resource tools' exposure for servers given as (exposure, whether
/// it offers resources): the widest among those that offer them. `None`
/// when none does, or every one that does is hidden.
pub fn exposure(
    servers: impl IntoIterator<Item = (Exposure, bool)>,
) -> Option<Exposure> {
    servers
        .into_iter()
        .filter(|(_, offers)| *offers)
        .map(|(exposure, _)| exposure)
        .max_by_key(|exposure| width(*exposure))
        .filter(|exposure| *exposure != Exposure::Hidden)
}

/// Which of the three tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    List,
    Templates,
    Read,
}

impl Kind {
    pub const ALL: [Self; 3] = [Self::List, Self::Templates, Self::Read];

    pub fn name(self) -> &'static str {
        match self {
            Self::List => LIST_RESOURCES,
            Self::Templates => LIST_TEMPLATES,
            Self::Read => READ_RESOURCE,
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::List => {
                "Lists the resources MCP servers offer: files, documents, records. Each has its \
                 server and its URI, which read_mcp_resource reads. Give `server` for one \
                 server's only."
            }
            Self::Templates => {
                "Lists the resource templates MCP servers offer: URI templates (RFC 6570) whose \
                 filled-in URIs read_mcp_resource reads. Give `server` for one server's only."
            }
            Self::Read => {
                "Reads one resource of an MCP server by its URI, from list_mcp_resources, a \
                 filled-in resource template, or a resource link in a tool's result."
            }
        }
    }

    fn parameters(self) -> &'static Value {
        static LIST: OnceLock<Value> = OnceLock::new();
        static READ: OnceLock<Value> = OnceLock::new();
        match self {
            Self::List | Self::Templates => LIST.get_or_init(|| {
                json!({
                    "type": "object",
                    "properties": {
                        "server": {
                            "type": "string",
                            "description": "Only this server's. All servers' when left out."
                        }
                    }
                })
            }),
            Self::Read => READ.get_or_init(|| {
                json!({
                    "type": "object",
                    "properties": {
                        "server": {"type": "string", "description": "The server's name."},
                        "uri": {"type": "string", "description": "The resource's URI."}
                    },
                    "required": ["server", "uri"]
                })
            }),
        }
    }

    fn output_schema(self) -> &'static Value {
        static LIST: OnceLock<Value> = OnceLock::new();
        static TEMPLATES: OnceLock<Value> = OnceLock::new();
        static READ: OnceLock<Value> = OnceLock::new();
        let items = json!({"type": "array", "items": {"type": "object"}});
        match self {
            Self::List => LIST.get_or_init(|| {
                json!({
                    "type": "object",
                    "properties": {"resources": items},
                    "required": ["resources"]
                })
            }),
            Self::Templates => TEMPLATES.get_or_init(|| {
                json!({
                    "type": "object",
                    "properties": {"resourceTemplates": items},
                    "required": ["resourceTemplates"]
                })
            }),
            Self::Read => READ.get_or_init(|| {
                json!({
                    "type": "object",
                    "properties": {
                        "server": {"type": "string"},
                        "uri": {"type": "string"},
                        "contents": items
                    },
                    "required": ["server", "uri", "contents"]
                })
            }),
        }
    }
}

/// One of the resource tools, over the servers that are on and not
/// hidden.
#[derive(Clone)]
pub struct ResourceTool {
    kind: Kind,
    connections: Vec<Arc<Connection>>,
    exposure: ToolExposure,
    spill: Spill,
}

impl ResourceTool {
    pub(crate) fn new(
        kind: Kind,
        connections: Vec<Arc<Connection>>,
        exposure: ToolExposure,
        spill: Spill,
    ) -> Self {
        Self {
            kind,
            connections,
            exposure,
            spill,
        }
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// The same tool with another exposure.
    pub(crate) fn exposed(&self, exposure: ToolExposure) -> Self {
        Self {
            exposure,
            ..self.clone()
        }
    }

    /// The names of the servers that offer resources now.
    fn offering(&self) -> Vec<&str> {
        self.connections
            .iter()
            .filter(|c| c.offers_resources())
            .map(|c| c.name())
            .collect()
    }

    /// The servers `server` names (every one when `None`), after they
    /// finished connecting; a named one that dropped or failed connects
    /// again first.
    async fn targets(
        &self,
        server: Option<&str>,
        cancel: &CancellationToken,
    ) -> Result<Vec<Arc<Connection>>, String> {
        let targets: Vec<Arc<Connection>> = match server {
            None => self.connections.clone(),
            Some(name) => {
                let found = self
                    .connections
                    .iter()
                    .find(|c| c.name() == name)
                    .cloned()
                    .ok_or_else(|| self.unknown(name))?;
                if matches!(
                    found.status().state,
                    State::Disconnected | State::Failed
                ) {
                    found.connect();
                }
                vec![found]
            }
        };
        join_all(targets.iter().map(|c| c.settled(cancel))).await;
        if let Some(name) = server
            && !targets.iter().any(|c| c.offers_resources())
        {
            let status = targets.first().map(|c| c.status());
            return Err(match status {
                Some(status) if status.state != State::Connected => format!(
                    "MCP server {name} is not connected{}",
                    status.error.map(|e| format!(": {e}")).unwrap_or_default()
                ),
                _ => format!("MCP server {name} does not offer resources"),
            });
        }
        Ok(targets
            .into_iter()
            .filter(|c| c.offers_resources())
            .collect())
    }

    fn unknown(&self, name: &str) -> String {
        let offering = self.offering();
        if offering.is_empty() {
            format!("Unknown MCP server `{name}`")
        } else {
            format!(
                "Unknown MCP server `{name}`; servers with resources: {}",
                offering.join(", ")
            )
        }
    }

    async fn list(
        &self,
        args: &Value,
        cancel: &CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        let server = args.get("server").and_then(Value::as_str);
        let targets = self.targets(server, cancel).await?;
        let (key, items): (&str, Vec<Value>) = match self.kind {
            Kind::List => (
                "resources",
                targets
                    .iter()
                    .flat_map(|c| {
                        c.resources().into_iter().map(|resource| {
                            with_server(c.name(), json!(resource))
                        })
                    })
                    .collect(),
            ),
            _ => (
                "resourceTemplates",
                targets
                    .iter()
                    .flat_map(|c| {
                        c.templates().into_iter().map(|template| {
                            with_server(c.name(), json!(template))
                        })
                    })
                    .collect(),
            ),
        };
        let value = json!({ key: items });
        let text = serde_json::to_string_pretty(&value).unwrap_or_default();
        Ok(ToolOutput {
            content: vec![text_block(truncate(&text, TEXT_LIMIT, &self.spill))],
            details: None,
            structured: Some(value),
        })
    }

    async fn read(
        &self,
        args: &Value,
        cancel: &CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        let server = args
            .get("server")
            .and_then(Value::as_str)
            .ok_or("`server` is required: the name of the resource's server")?;
        let uri = args
            .get("uri")
            .and_then(Value::as_str)
            .ok_or("`uri` is required: the resource's URI")?;
        if is_app(uri, None) {
            return Err(format!(
                "{uri} is an MCP app's interface, which tau does not read"
            )
            .into());
        }
        let connection = self
            .connections
            .iter()
            .find(|c| c.name() == server)
            .ok_or_else(|| self.unknown(server))?;
        let result = connection.read_resource(uri, cancel).await?;
        let contents: Vec<Value> = result
            .get("contents")
            .and_then(Value::as_array)
            .map(|contents| {
                contents
                    .iter()
                    .filter(|content| {
                        !is_app(
                            content
                                .get("uri")
                                .and_then(Value::as_str)
                                .unwrap_or(uri),
                            content.get("mimeType").and_then(Value::as_str),
                        )
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let mut content = resource_contents(&contents, &self.spill);
        if content.is_empty() {
            content
                .push(text_block(format!("[Resource {uri} has no contents]")));
        }
        Ok(ToolOutput {
            content,
            details: Some(json!({ "server": server, "uri": uri })),
            structured: Some(json!({
                "server": server,
                "uri": uri,
                "contents": contents,
            })),
        })
    }
}

/// `item` with `server` first.
fn with_server(server: &str, item: Value) -> Value {
    let mut out = serde_json::Map::new();
    out.insert("server".into(), json!(server));
    if let Value::Object(fields) = item {
        out.extend(fields);
    }
    Value::Object(out)
}

#[async_trait]
impl AgentTool for ResourceTool {
    fn name(&self) -> &str {
        self.kind.name()
    }

    fn description(&self) -> &str {
        self.kind.description()
    }

    fn parameters(&self) -> &Value {
        self.kind.parameters()
    }

    fn exposure(&self) -> ToolExposure {
        self.exposure
    }

    fn output_schema(&self) -> Option<&Value> {
        Some(self.kind.output_schema())
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        match self.kind {
            Kind::List | Kind::Templates => self.list(&args, &ctx.cancel).await,
            Kind::Read => self.read(&args, &ctx.cancel).await,
        }
    }
}
