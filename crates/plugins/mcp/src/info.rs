//! What a server lists of itself, as tau keeps it: its tools' hints, its
//! resources and templates, and its prompts with their arguments. The page and the cards read them,
//! with or without the host half.

use serde::{Deserialize, Serialize};

/// The MCP hints a tool gives about itself, kept for the constitution
/// and the interface. `None` is a hint the tool did not give.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "camelCase")]
pub struct Annotations {
    #[serde(rename = "readOnlyHint", skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
    #[serde(
        rename = "destructiveHint",
        skip_serializing_if = "Option::is_none"
    )]
    pub destructive: Option<bool>,
    #[serde(
        rename = "idempotentHint",
        skip_serializing_if = "Option::is_none"
    )]
    pub idempotent: Option<bool>,
    #[serde(rename = "openWorldHint", skip_serializing_if = "Option::is_none")]
    pub open_world: Option<bool>,
}

/// A prompt as its server lists it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PromptInfo {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub arguments: Vec<PromptArgument>,
}

/// One of a prompt's arguments.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PromptArgument {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub required: bool,
}

/// A resource as its server lists it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ResourceInfo {
    pub uri: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

/// A resource template as its server lists it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TemplateInfo {
    pub uri_template: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}
