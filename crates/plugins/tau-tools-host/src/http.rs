//! `http`: an agent's HTTP requests (ADR 0037), a nested tool that
//! codemode's `http.request` calls. Only an agent's runs have it.
//!
//! A request names secrets and never holds them: a header or the body
//! may be `{ "$secret": name, "prefix"?: text }` (what `http.secret`
//! and `tau.secret` make), and [`Request::ready`] puts the value from
//! the [`SecretStore`] in as it leaves. Everything that comes back
//! (status line, headers, body, errors) is scrubbed of every secret's
//! value ([`Scrub`]). Redirects are not followed, so a secret never
//! reaches a host the request did not name.
//!
//! [`fetch`] is the same request for the host itself, without a run:
//! what an agent's polls call.

use std::{collections::BTreeMap, io::Cursor, time::Duration};

use async_trait::async_trait;
use serde_json::{Map, Value, json};
use tau_agent::{
    error::ToolError,
    tool::{AgentTool, Exposure, ToolCtx, ToolOutput},
};
use tau_artifacts::Bytes;
use tokio_util::sync::CancellationToken;

use crate::{
    ABORTED,
    artifact_grant::publish_artifact,
    secrets::{Scrub, SecretStore},
};

/// How long a request may take when it does not say.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// The longest a request may ask for.
pub const MAX_TIMEOUT: Duration = Duration::from_secs(60);

/// The most of a body read at all; the rest is cut.
pub const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// The most of a body a script gets inline; a longer one is an artifact
/// when the run has artifact storage, else cut here.
pub const INLINE_BODY_BYTES: usize = 64 * 1024;

/// The longest body decoded as JSON: codemode's `json.decode` bound.
pub const MAX_JSON_BYTES: usize = 1024 * 1024;

/// The key a secret's marker holds its name under.
pub const SECRET_KEY: &str = "$secret";

/// The methods a request may use.
const METHODS: [&str; 7] =
    ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"];

/// The client every request goes through: tau's TLS, no redirects.
pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .tls_backend_preconfigured(tau_ai::http::tls_config())
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("a static client configuration builds")
}

/// A secret's marker: its name, and text to put before its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    pub name: String,
    pub prefix: String,
}

impl Marker {
    /// `value` as a marker, if it is one: an object with `$secret` and
    /// at most `prefix` besides.
    pub fn of(value: &Value) -> Option<Self> {
        let fields = value.as_object()?;
        let name = fields.get(SECRET_KEY)?.as_str()?.to_owned();
        let prefix = match fields.get("prefix") {
            None | Some(Value::Null) => String::new(),
            Some(Value::String(prefix)) => prefix.clone(),
            Some(_) => return None,
        };
        fields
            .keys()
            .all(|key| key == SECRET_KEY || key == "prefix")
            .then_some(Self { name, prefix })
    }

    fn resolve(
        &self,
        secrets: &BTreeMap<String, String>,
    ) -> Result<String, String> {
        match secrets.get(&self.name) {
            Some(value) => Ok(format!("{}{value}", self.prefix)),
            None => Err(format!(
                "The secret `{}` is not in tau's secret store: add it on \
                 the agent's page.",
                self.name
            )),
        }
    }
}

/// Every secret `value` names, anywhere in it, in order, once each.
pub fn secret_names(value: &Value) -> Vec<String> {
    fn walk(value: &Value, names: &mut Vec<String>) {
        if let Some(marker) = Marker::of(value) {
            if !names.contains(&marker.name) {
                names.push(marker.name);
            }
            return;
        }
        match value {
            Value::Array(items) => {
                items.iter().for_each(|item| walk(item, names))
            }
            Value::Object(fields) => {
                fields.values().for_each(|value| walk(value, names))
            }
            _ => {}
        }
    }
    let mut names = Vec::new();
    walk(value, &mut names);
    names
}

/// A request as a script or an agent's poll gives it.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub method: reqwest::Method,
    pub url: reqwest::Url,
    /// Each a string or a secret's marker.
    pub headers: Vec<(String, Value)>,
    /// A string goes as it is; a marker as its value; anything else as
    /// JSON, its markers put in.
    pub body: Option<Value>,
    pub timeout: Duration,
}

impl Request {
    /// Reads `{ method?, url, headers?, body?, timeout? }`; `timeout` is
    /// in seconds.
    pub fn parse(spec: &Value) -> Result<Self, String> {
        let fields = spec
            .as_object()
            .ok_or("an HTTP request is a table: { url = ..., ... }")?;
        if let Some(key) = fields.keys().find(|key| {
            !matches!(
                key.as_str(),
                "method" | "url" | "headers" | "body" | "timeout"
            )
        }) {
            return Err(format!("an HTTP request has no `{key}`"));
        }
        let method = match fields.get("method") {
            None | Some(Value::Null) => "GET".to_owned(),
            Some(Value::String(method)) => method.to_ascii_uppercase(),
            Some(_) => {
                return Err("`method` is a string, such as \"POST\"".into());
            }
        };
        if !METHODS.contains(&method.as_str()) {
            return Err(format!(
                "`{method}` is not a method HTTP requests use here"
            ));
        }
        let method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|error| error.to_string())?;
        let url = fields
            .get("url")
            .and_then(Value::as_str)
            .ok_or("an HTTP request needs a `url`")?;
        let url = reqwest::Url::parse(url)
            .map_err(|error| format!("`{url}` is not a URL: {error}"))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(format!(
                "`{}` is not an http or https URL",
                url.scheme()
            ));
        }
        let headers = match fields.get("headers") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Object(headers)) => headers
                .iter()
                .map(|(name, value)| match value {
                    Value::String(_) => Ok((name.clone(), value.clone())),
                    _ if Marker::of(value).is_some() => {
                        Ok((name.clone(), value.clone()))
                    }
                    _ => Err(format!(
                        "the header `{name}` is a string or a secret"
                    )),
                })
                .collect::<Result<_, _>>()?,
            Some(_) => {
                return Err("`headers` is a table of names to values".into());
            }
        };
        let timeout = match fields.get("timeout") {
            None | Some(Value::Null) => DEFAULT_TIMEOUT,
            Some(seconds) => {
                let seconds = seconds
                    .as_f64()
                    .filter(|seconds| *seconds > 0.0)
                    .ok_or("`timeout` is a number of seconds")?;
                let timeout = Duration::from_secs_f64(seconds);
                if timeout > MAX_TIMEOUT {
                    return Err(format!(
                        "`timeout` is at most {} seconds",
                        MAX_TIMEOUT.as_secs()
                    ));
                }
                timeout
            }
        };
        Ok(Self {
            method,
            url,
            headers,
            body: fields.get("body").filter(|body| !body.is_null()).cloned(),
            timeout,
        })
    }

    /// The request as it leaves, its secrets' values put in.
    pub fn ready(
        &self,
        secrets: &BTreeMap<String, String>,
    ) -> Result<Ready, String> {
        let mut headers = reqwest::header::HeaderMap::new();
        for (name, value) in &self.headers {
            let value = match Marker::of(value) {
                Some(marker) => marker.resolve(secrets)?,
                None => value.as_str().unwrap_or_default().to_owned(),
            };
            let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| format!("`{name}` cannot name a header"))?;
            let mut value = reqwest::header::HeaderValue::from_str(&value)
                .map_err(|_| {
                    format!("the header `{name}` holds what a header cannot")
                })?;
            value.set_sensitive(true);
            headers.insert(name, value);
        }
        let body = match &self.body {
            None => None,
            Some(Value::String(text)) => Some(text.clone().into_bytes()),
            Some(body) => match Marker::of(body) {
                Some(marker) => Some(marker.resolve(secrets)?.into_bytes()),
                None => {
                    if !headers.contains_key(reqwest::header::CONTENT_TYPE) {
                        headers.insert(
                            reqwest::header::CONTENT_TYPE,
                            reqwest::header::HeaderValue::from_static(
                                "application/json",
                            ),
                        );
                    }
                    let body = put_in(body, secrets)?;
                    Some(
                        serde_json::to_vec(&body)
                            .map_err(|error| error.to_string())?,
                    )
                }
            },
        };
        Ok(Ready {
            method: self.method.clone(),
            url: self.url.clone(),
            headers,
            body,
            timeout: self.timeout,
        })
    }
}

/// `value` with each marker in it replaced by its secret's value.
fn put_in(
    value: &Value,
    secrets: &BTreeMap<String, String>,
) -> Result<Value, String> {
    if let Some(marker) = Marker::of(value) {
        return marker.resolve(secrets).map(Value::String);
    }
    Ok(match value {
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| put_in(item, secrets))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, value)| Ok((key.clone(), put_in(value, secrets)?)))
                .collect::<Result<Map<_, _>, String>>()?,
        ),
        other => other.clone(),
    })
}

/// A request with its secrets in: it never leaves this module but to
/// the network.
pub struct Ready {
    method: reqwest::Method,
    url: reqwest::Url,
    headers: reqwest::header::HeaderMap,
    body: Option<Vec<u8>>,
    timeout: Duration,
}

impl std::fmt::Debug for Ready {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ready")
            .field("method", &self.method)
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

/// What came back, before scrubbing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
    /// The body went past [`MAX_BODY_BYTES`] and was cut there.
    pub cut: bool,
}

impl Response {
    /// Whether its body says it is JSON.
    pub fn is_json(&self) -> bool {
        self.headers.get("content-type").is_some_and(|kind| {
            let kind = kind.to_ascii_lowercase();
            kind.contains("/json") || kind.contains("+json")
        })
    }
}

/// Sends `ready` and reads the answer, up to [`MAX_BODY_BYTES`].
pub async fn send(
    client: &reqwest::Client,
    ready: Ready,
    cancel: &CancellationToken,
) -> Result<Response, String> {
    let work = async {
        let mut request = client
            .request(ready.method, ready.url)
            .headers(ready.headers)
            .timeout(ready.timeout);
        if let Some(body) = ready.body {
            request = request.body(body);
        }
        let mut response = request.send().await.map_err(error_text)?;
        let status = response.status().as_u16();
        let mut headers = BTreeMap::new();
        for (name, value) in response.headers() {
            let value = String::from_utf8_lossy(value.as_bytes()).into_owned();
            headers
                .entry(name.as_str().to_owned())
                .and_modify(|before: &mut String| {
                    before.push_str(", ");
                    before.push_str(&value);
                })
                .or_insert(value);
        }
        let mut body = Vec::new();
        let mut cut = false;
        while let Some(chunk) = response.chunk().await.map_err(error_text)? {
            let room = MAX_BODY_BYTES - body.len();
            if chunk.len() > room {
                body.extend_from_slice(&chunk[..room]);
                cut = true;
                break;
            }
            body.extend_from_slice(&chunk);
        }
        Ok(Response {
            status,
            headers,
            body,
            cut,
        })
    };
    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(ABORTED.to_owned()),
        result = work => result,
    }
}

/// reqwest's error, with its causes, never its request's headers.
fn error_text(error: reqwest::Error) -> String {
    let mut text = if error.is_timeout() {
        "the request timed out".to_owned()
    } else if error.is_connect() {
        "could not connect".to_owned()
    } else {
        "the request failed".to_owned()
    };
    let mut cause = std::error::Error::source(&error);
    while let Some(error) = cause {
        text.push_str(": ");
        text.push_str(&error.to_string());
        cause = error.source();
    }
    text
}

/// What a request the host makes for itself got back, scrubbed: an
/// agent's poll's answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Fetched {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: String,
    /// The body decoded, when it says it is JSON and is not too long.
    pub json: Option<Value>,
    pub cut: bool,
}

/// Sends the request `spec` names, its secrets from `store`, and reads
/// the answer whole (to [`MAX_BODY_BYTES`]), scrubbed. Errors are
/// scrubbed too.
pub async fn fetch(
    client: &reqwest::Client,
    store: &SecretStore,
    spec: &Value,
    cancel: &CancellationToken,
) -> Result<Fetched, String> {
    let request = Request::parse(spec)?;
    let secrets = store
        .read()
        .await
        .map_err(|error| format!("tau's secret store: {error}"))?;
    let scrub = Scrub::new(&secrets);
    let ready = request.ready(&secrets)?;
    let response = send(client, ready, cancel)
        .await
        .map_err(|error| scrub.text(&error))?;
    let body = scrub.text(&String::from_utf8_lossy(&response.body));
    let json = (response.is_json() && body.len() <= MAX_JSON_BYTES)
        .then(|| serde_json::from_str(&body).ok())
        .flatten();
    Ok(Fetched {
        status: response.status,
        headers: scrub_headers(&scrub, &response.headers),
        body,
        json,
        cut: response.cut,
    })
}

fn scrub_headers(
    scrub: &Scrub,
    headers: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    headers
        .iter()
        .map(|(name, value)| (scrub.text(name), scrub.text(value)))
        .collect()
}

/// The `http` tool: one request, from a script.
pub struct Http {
    store: SecretStore,
    client: reqwest::Client,
    artifacts: Option<Bytes>,
    parameters: Value,
    output: Value,
}

impl std::fmt::Debug for Http {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Http")
            .field("store", &self.store)
            .field("artifacts", &self.artifacts.is_some())
            .finish_non_exhaustive()
    }
}

impl Http {
    pub fn new(store: SecretStore) -> Self {
        let text_or_secret = json!({
            "anyOf": [
                {"type": "string"},
                {
                    "type": "object",
                    "required": [SECRET_KEY],
                    "properties": {
                        SECRET_KEY: {"type": "string"},
                        "prefix": {"type": "string"}
                    },
                    "additionalProperties": false
                }
            ]
        });
        Self {
            store,
            client: client(),
            artifacts: None,
            parameters: json!({
                "type": "object",
                "required": ["url"],
                "additionalProperties": false,
                "properties": {
                    "method": {"type": "string", "description": "GET when left out; also HEAD, POST, PUT, PATCH, DELETE, OPTIONS."},
                    "url": {"type": "string", "description": "An http or https URL."},
                    "headers": {
                        "type": "object",
                        "additionalProperties": text_or_secret,
                        "description": "Each a string, or a secret: http.secret(name, prefix?)."
                    },
                    "body": {
                        "description": "A string as it is, a secret, or any JSON value, sent as application/json."
                    },
                    "timeout": {
                        "type": "number",
                        "exclusiveMinimum": 0,
                        "maximum": MAX_TIMEOUT.as_secs(),
                        "description": "Seconds; 30 when left out."
                    }
                }
            }),
            output: json!({
                "type": "object",
                "required": ["status", "headers", "body", "json", "artifact", "truncated"],
                "properties": {
                    "status": {"type": "integer"},
                    "headers": {"type": "object", "additionalProperties": {"type": "string"}},
                    "body": {"type": ["string", "null"]},
                    "json": {},
                    "artifact": {
                        "type": ["object", "null"],
                        "properties": {
                            "id": {"type": "string"},
                            "digest": {"type": "string"},
                            "size_bytes": {"type": "integer"}
                        }
                    },
                    "truncated": {"type": "boolean"}
                }
            }),
        }
    }

    /// Bodies longer than [`INLINE_BODY_BYTES`] become artifacts here.
    pub fn with_artifacts(mut self, bytes: Bytes) -> Self {
        self.artifacts = Some(bytes);
        self
    }
}

#[async_trait]
impl AgentTool for Http {
    fn name(&self) -> &str {
        "http"
    }

    fn description(&self) -> &str {
        "Send one HTTP request and get { status, headers, body, json, artifact, truncated } back. \
         Headers and the body name secrets from tau's secret store with http.secret(name, prefix?); \
         their values go in as the request leaves and are scrubbed from what comes back. \
         Redirects are not followed. A body over 64 KiB is an artifact, read with artifact_read."
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn output_schema(&self) -> Option<&Value> {
        Some(&self.output)
    }

    fn exposure(&self) -> Exposure {
        Exposure::Nested
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let request = Request::parse(&args).map_err(ToolError::from)?;
        let secrets = self.store.read().await.map_err(|error| {
            ToolError::from(format!("tau's secret store: {error}"))
        })?;
        let scrub = Scrub::new(&secrets);
        let ready = request.ready(&secrets).map_err(ToolError::from)?;
        let response = send(&self.client, ready, &ctx.cancel)
            .await
            .map_err(|error| ToolError::from(scrub.text(&error)))?;
        let headers = scrub_headers(&scrub, &response.headers);
        let size = response.body.len();
        let body = scrub.bytes(&response.body);
        let json_wanted = response.is_json();
        let (body, artifact, truncated) = if body.len() <= INLINE_BODY_BYTES {
            (
                Some(String::from_utf8_lossy(&body).into_owned()),
                None,
                response.cut,
            )
        } else if let Some(bytes) = &self.artifacts {
            let source = format!("http {} {}", request.method, request.url);
            let grant =
                publish_artifact(bytes, Cursor::new(body), &source, &ctx)
                    .await?;
            let artifact = json!({
                "id": grant.artifact.id,
                "digest": grant.artifact.digest,
                "size_bytes": grant.artifact.size_bytes,
            });
            (None, Some(artifact), response.cut)
        } else {
            // Cut on a character's edge when the body is text.
            let end = match std::str::from_utf8(&body[..INLINE_BODY_BYTES]) {
                Err(error) if error.error_len().is_none() => {
                    error.valid_up_to()
                }
                _ => INLINE_BODY_BYTES,
            };
            (
                Some(String::from_utf8_lossy(&body[..end]).into_owned()),
                None,
                true,
            )
        };
        let json = body
            .as_deref()
            .filter(|body| json_wanted && body.len() <= MAX_JSON_BYTES)
            .and_then(|body| serde_json::from_str::<Value>(body).ok())
            .unwrap_or(Value::Null);
        let shown = match &artifact {
            Some(artifact) => format!(
                "HTTP {} · {size} bytes, in artifact {}",
                response.status, artifact["id"]
            ),
            None => format!("HTTP {} · {size} bytes", response.status),
        };
        let mut output = ToolOutput::text(shown);
        output.structured = Some(json!({
            "status": response.status,
            "headers": headers,
            "body": body,
            "json": json,
            "artifact": artifact,
            "truncated": truncated,
        }));
        Ok(output)
    }
}
