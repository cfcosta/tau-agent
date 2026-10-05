//! Servers' prompts as commands (`docs/reference/mcp.md`, "Prompts"). As
//! properties: `key=value` arguments, quoted or not, read back as they
//! were written; a prompt's arguments are accepted exactly when every
//! required one is given and none is unknown or given twice. Against the
//! in-process server: the commands of every page of prompts, getting one
//! with arguments, and the errors a person reads.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

mod common;

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use common::State as Fixture;
use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_mcp::{
    McpPlugin,
    config::{ServerConfig, Transport},
    info::{PromptArgument, PromptInfo},
    prompts::{
        arguments_hint,
        check_arguments,
        format_arguments,
        parse_arguments,
        prompt_text,
        usage,
    },
};
use tokio_util::sync::CancellationToken;

fn key() -> impl hegel::generators::PrintableGenerator<String> {
    gs::from_regex("[A-Za-z_][A-Za-z0-9_.-]{0,8}")
}

/// Any arguments, with any values (spaces, quotes, `\`, empty, any
/// script), written by `format_arguments`, read back the same, in order.
#[hegel::test(test_cases = 300)]
fn arguments_round_trip(tc: TestCase) {
    let map: BTreeMap<String, String> =
        tc.draw(gs::btree_maps(key(), gs::text().max_size(16)).max_size(5));
    let pairs: Vec<(String, String)> = map.into_iter().collect();
    let written = format_arguments(&pairs);
    assert_eq!(parse_arguments(&written).unwrap(), pairs, "{written}");
    // Whitespace around and between them changes nothing.
    let spaced = format!(
        "  {}\t\n",
        pairs
            .iter()
            .map(|pair| format_arguments(std::slice::from_ref(pair)))
            .collect::<Vec<_>>()
            .join(" \t  ")
    );
    assert_eq!(parse_arguments(&spaced).unwrap(), pairs, "{spaced}");
}

/// Quotes may cover part of a value, and `'` keeps `\` as it is.
#[test]
fn quotes_in_part_of_a_value() {
    assert_eq!(
        parse_arguments(r#"a=x"y z"w b='c\d' c="say \"hi\"" d="#).unwrap(),
        [
            ("a".into(), "xy zw".into()),
            ("b".into(), r"c\d".into()),
            ("c".into(), r#"say "hi""#.into()),
            ("d".into(), String::new()),
        ]
    );
    assert_eq!(
        parse_arguments("name").unwrap_err(),
        "`name` is not an argument: write key=value, quoting values with spaces"
    );
    assert_eq!(
        parse_arguments("=x").unwrap_err(),
        "an argument has no name before its `=`"
    );
    assert_eq!(
        parse_arguments("a=\"open").unwrap_err(),
        "the quote in the value of `a` is not closed"
    );
}

fn prompt(arguments: &[(&str, bool)]) -> PromptInfo {
    PromptInfo {
        name: "p".into(),
        arguments: arguments
            .iter()
            .map(|(name, required)| PromptArgument {
                name: (*name).into(),
                description: None,
                required: *required,
            })
            .collect(),
        ..PromptInfo::default()
    }
}

/// For any prompt and any arguments given, the check accepts them
/// exactly when none is unknown, none is given twice and every required
/// one is there, and then passes them on as given.
#[hegel::test(test_cases = 300)]
fn required_arguments_are_checked(tc: TestCase) {
    let names = ["a", "b", "c", "d"];
    let required: Vec<bool> =
        tc.draw(gs::vecs(gs::booleans()).min_size(4).max_size(4));
    let taken: usize = tc.draw(gs::integers::<usize>().max_value(4));
    let declared: Vec<(&str, bool)> = names[..taken]
        .iter()
        .zip(&required)
        .map(|(name, required)| (*name, *required))
        .collect();
    let info = prompt(&declared);
    let given: Vec<(String, String)> = tc.draw(
        gs::vecs(hegel::tuples!(
            gs::sampled_from(
                names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>()
            ),
            gs::text().max_size(4)
        ))
        .max_size(5),
    );

    let unknown = given
        .iter()
        .any(|(key, _)| !declared.iter().any(|(name, _)| name == key));
    let mut seen = std::collections::BTreeSet::new();
    let twice = given.iter().any(|(key, _)| !seen.insert(key.clone()));
    let missing = declared.iter().any(|(name, required)| {
        *required && !given.iter().any(|(k, _)| k == name)
    });
    let result = check_arguments("cmd", &info, &given);
    assert_eq!(result.is_ok(), !unknown && !twice && !missing, "{result:?}");
    if let Ok(map) = result {
        let expected: serde_json::Map<String, Value> =
            given.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
        assert_eq!(map, expected);
    } else if missing && !unknown && !twice {
        let error = result.unwrap_err();
        assert!(error.starts_with("/cmd needs the argument"), "{error}");
        assert!(error.ends_with(&usage("cmd", &info)), "{error}");
    }
}

#[test]
fn usage_and_hints_put_required_arguments_first() {
    let info = prompt(&[("style", false), ("name", true)]);
    assert_eq!(usage("x", &info), "/x name=<name> [style=<style>]");
    assert_eq!(arguments_hint(&info), "name=… [style=…]");
    assert_eq!(
        check_arguments("x", &info, &[]).unwrap_err(),
        "/x needs the argument `name`. Usage: /x name=<name> [style=<style>]"
    );
    assert_eq!(
        check_arguments("x", &info, &[("who".into(), "me".into())])
            .unwrap_err(),
        "/x has no argument `who`. Usage: /x name=<name> [style=<style>]"
    );
    assert_eq!(
        check_arguments("x", &prompt(&[]), &[("who".into(), "me".into())])
            .unwrap_err(),
        "/x takes no arguments, but `who` was given."
    );
}

/// A prompt's messages as the text a person sends.
#[test]
fn messages_as_text() {
    let result = json!({"messages": [
        {"role": "user", "content": {"type": "text", "text": "One."}},
        {"role": "assistant", "content": {"type": "text", "text": "Two."}},
        {"role": "user", "content": {"type": "image", "data": "aGk=", "mimeType": "image/png"}},
        {"role": "user", "content": {"type": "resource", "resource": {"uri": "file:///t", "text": "Embedded."}}},
        {"role": "user", "content": {"type": "resource", "resource": {"uri": "file:///b", "blob": "AA==", "mimeType": "application/zip"}}},
        {"role": "user", "content": {"type": "resource_link", "uri": "file:///l", "name": "l"}}
    ]});
    assert_eq!(
        prompt_text("srv", &result),
        "One.\n\nTwo.\n\n[Image (image/png)]\n\nEmbedded.\n\n[Resource file:///b (application/zip)]\n\n\
         [Resource file:///l \"l\". Read it with read_mcp_resource (server \"srv\")]"
    );
}

fn plugin(fixture: &Arc<Fixture>) -> McpPlugin {
    McpPlugin::builder()
        .env(Arc::new(|_| None))
        .home(None)
        .server(ServerConfig::new("srv", Transport::Stream(fixture.dial())))
        .startup_wait(Duration::from_secs(5))
        .build()
}

async fn ready(plugin: &McpPlugin) {
    for connection in plugin.connections() {
        connection.settled(&CancellationToken::new()).await;
    }
}

/// Every prompt of every page is a command; getting one checks its
/// arguments first and gives its messages as text.
#[tokio::test(flavor = "multi_thread")]
async fn prompts_are_commands() {
    let fixture = Fixture::with_features(false);
    let plugin = plugin(&fixture);
    ready(&plugin).await;
    let commands: Vec<String> =
        plugin.prompts().into_iter().map(|p| p.command).collect();
    assert_eq!(commands, ["mcp__srv__greet", "mcp__srv__summary"]);

    let cancel = CancellationToken::new();
    assert_eq!(
        plugin
            .get_prompt(
                "mcp__srv__greet",
                r#"name=Ada style="very warmly""#,
                &cancel
            )
            .await
            .unwrap(),
        "Say hello to Ada, very warmly."
    );
    let summary = plugin
        .get_prompt("mcp__srv__summary", "", &cancel)
        .await
        .unwrap();
    assert!(
        summary.starts_with("Summarize these.\n\nRemember the milk.\n\n[Resource file:///data.bin"),
        "{summary}"
    );
    assert!(summary.ends_with("[Image (image/png)]"), "{summary}");
    assert_eq!(
        plugin
            .get_prompt("mcp__srv__greet", "style=warmly", &cancel)
            .await
            .unwrap_err(),
        "/mcp__srv__greet needs the argument `name`. Usage: /mcp__srv__greet name=<name> \
         [style=<style>]"
    );
    assert_eq!(
        plugin
            .get_prompt("mcp__srv__nothing", "", &cancel)
            .await
            .unwrap_err(),
        "No MCP prompt /mcp__srv__nothing here."
    );
    plugin.shutdown().await;
}

/// A server without prompts has no commands.
#[tokio::test(flavor = "multi_thread")]
async fn no_commands_without_prompts() {
    let fixture = Fixture::new(false);
    let plugin = plugin(&fixture);
    ready(&plugin).await;
    assert!(plugin.prompts().is_empty());
    plugin.shutdown().await;
}
