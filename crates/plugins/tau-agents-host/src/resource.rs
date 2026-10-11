//! What an agent's `mcp` triggers keep and fire (ADR 0037): a trigger
//! watches one MCP resource, reads it each time its server says it
//! changed, and fires when what it reads differs from what it read last.
//!
//! The first read only records what is there, as a poll's first call
//! does; after that, each read that changes fires one event, its id the
//! resource and a digest of what it read, its data what it read.

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tau_agents::TriggerKind;

/// What tau keeps of one `mcp` trigger across restarts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Watched {
    /// What it watches: a trigger that changes starts over.
    pub kind: TriggerKind,
    /// The digest of what it read last, once it read.
    #[serde(default)]
    pub digest: Option<String>,
    /// Why it last failed: its watch, or a read.
    #[serde(default)]
    pub error: Option<String>,
}

impl Watched {
    /// A trigger of `kind` that has not read yet.
    pub fn new(kind: TriggerKind) -> Self {
        Self {
            kind,
            digest: None,
            error: None,
        }
    }
}

/// What a `resources/read` gives an event: each content's text, as
/// JSON when its type says it is; a blob as its type and size. One
/// content is itself, more a list.
pub fn contents(read: &Value) -> Value {
    let contents: Vec<Value> = read
        .get("contents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(content)
        .collect();
    match <[Value; 1]>::try_from(contents) {
        Ok([one]) => one,
        Err(many) => Value::Array(many),
    }
}

fn content(item: &Value) -> Value {
    let mime = item.get("mimeType").and_then(Value::as_str);
    if let Some(text) = item.get("text").and_then(Value::as_str) {
        let json = mime.is_some_and(|mime| mime.contains("json"));
        return json
            .then(|| serde_json::from_str(text).ok())
            .flatten()
            .unwrap_or_else(|| Value::String(text.to_owned()));
    }
    let bytes = item
        .get("blob")
        .and_then(Value::as_str)
        .and_then(|blob| {
            base64::engine::general_purpose::STANDARD.decode(blob).ok()
        })
        .map_or(0, |bytes| bytes.len());
    json!({ "mimeType": mime, "bytes": bytes })
}

/// The digest of `contents`: whether a read differs from the last.
pub fn digest(contents: &Value) -> String {
    let hash = Sha256::digest(contents.to_string().as_bytes());
    hash.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The id of the event a read of `uri` with `digest` fires.
pub fn event_id(uri: &str, digest: &str) -> String {
    format!("{uri}:{}", &digest[..digest.len().min(12)])
}
