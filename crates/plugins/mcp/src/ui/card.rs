//! An MCP tool's card: the server and the tool, the tool's hints, and
//! what it returned, as structured JSON when the server gave some, else
//! as text.

use gpui::{Div, div, prelude::*, px};
use serde_json::Value;
use tau_ui_kit::{
    components::{code_block, heading, mono},
    theme::{Theme, Tone, Type, sp},
};
use tau_ui_plugin::{
    CallData,
    ViewCx,
    points::{AtCard, CardView},
};

use super::{
    McpUi,
    page::{hint_badge, hints},
};
use crate::connection::Annotations;

/// The most lines of a result's text a card shows.
const TEXT_LINES: usize = 40;

/// What a card shows of a call to an MCP tool.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Shown {
    pub server: Option<String>,
    pub tool: Option<String>,
    pub annotations: Annotations,
    /// The result's structured content, pretty.
    pub structured: Option<String>,
    /// The result's text, when it has no structured content.
    pub text: Option<String>,
    /// The first line of the result, when the call failed.
    pub failed: Option<String>,
}

/// What the card of `data` shows: from the result's details once the
/// call ended, else from what the repository's servers list
/// (`listed`: server, tool and hints), so a call going on names its
/// server too.
pub fn shown(
    data: &CallData,
    listed: Option<(String, String, Annotations)>,
) -> Shown {
    let details = data.result.as_ref().and_then(|r| r.details.as_ref());
    let field = |key: &str| {
        details
            .and_then(|details| details.get(key))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let (listed_server, listed_tool, listed_hints) = match listed {
        Some((server, tool, hints)) => (Some(server), Some(tool), Some(hints)),
        None => (None, None, None),
    };
    let annotations = details
        .and_then(|details| details.get("annotations"))
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .or(listed_hints)
        .unwrap_or_default();
    let structured = details
        .and_then(|details| details.get("structuredContent"))
        .map(|value| serde_json::to_string_pretty(value).unwrap_or_default());
    let result = data.result.as_ref();
    Shown {
        server: field("server").or(listed_server),
        tool: field("tool").or(listed_tool),
        annotations,
        text: result
            .filter(|_| structured.is_none())
            .map(|result| result.text.clone())
            .filter(|text| !text.is_empty()),
        structured,
        failed: result.filter(|result| result.error).map(|result| {
            result.text.lines().next().unwrap_or_default().to_owned()
        }),
    }
}

/// The card of a call to an MCP tool: any tool named `mcp__…`.
pub fn card(at: &AtCard, view: &mut ViewCx<'_, McpUi>) -> Option<CardView> {
    if !at.tool.starts_with("mcp__") {
        return None;
    }
    let t = view.theme().clone();
    let servers = view.repos.get(&at.run.repo).unwrap_or(view.data);
    let listed = servers.tool(&at.tool).map(|(server, tool)| {
        (server.name.clone(), tool.tool.clone(), tool.annotations)
    });
    let shown = shown(&at.data, listed);
    Some(CardView {
        head: Some(head(&shown, &at.summary, &t).into_any_element()),
        label: None,
        failed: shown.failed.clone(),
        edge: shown.failed.as_ref().map(|_| Tone::Danger),
        shape: None,
        body: Some(body(&at.data, &shown, &t).into_any_element()),
        folds: at.data.result.is_some(),
        inset: false,
    })
}

/// The server and the tool, and the tool's hints.
fn head(shown: &Shown, summary: &str, t: &Theme) -> Div {
    let named = match (&shown.server, &shown.tool) {
        (Some(server), Some(tool)) => format!("{server} · {tool}"),
        _ => summary.to_owned(),
    };
    div()
        .flex_1()
        .min_w(px(0.))
        .flex()
        .items_center()
        .gap(sp(1.5))
        .child(
            mono(named, Type::CAPTION, t.text_soft)
                .min_w(px(0.))
                .truncate(),
        )
        .children(
            hints(&shown.annotations)
                .into_iter()
                .map(|hint| hint_badge(hint, t)),
        )
}

/// What the call sent, and what it returned.
fn body(data: &CallData, shown: &Shown, t: &Theme) -> Div {
    let args = (!data.args.is_null()
        && data.args.as_object().is_none_or(|args| !args.is_empty()))
    .then(|| serde_json::to_string_pretty(&data.args).unwrap_or_default());
    div()
        .flex()
        .flex_col()
        .gap(sp(2.))
        .px(sp(3.))
        .py(sp(2.5))
        .when_some(args, |body, args| {
            body.child(heading("Arguments", t)).child(code_block(
                Some("json"),
                &args,
                t,
            ))
        })
        .when_some(shown.structured.clone(), |body, structured| {
            body.child(heading("Structured result", t))
                .child(code_block(Some("json"), &structured, t))
        })
        .when_some(shown.text.clone(), |body, text| {
            let lines: Vec<&str> = text.lines().collect();
            let more = lines.len().saturating_sub(TEXT_LINES);
            let color = if shown.failed.is_some() {
                t.red
            } else {
                t.muted
            };
            body.child(heading("Result", t)).child(
                mono(
                    lines[..lines.len() - more].join("\n"),
                    Type::CAPTION,
                    color,
                )
                .when(more > 0, |text| {
                    text.child(mono(
                        format!("… {more} more lines"),
                        Type::MICRO,
                        t.dim,
                    ))
                }),
            )
        })
        .when(data.result.is_none(), |body| {
            body.children(
                data.partial
                    .clone()
                    .map(|partial| mono(partial, Type::CAPTION, t.dim)),
            )
        })
}
