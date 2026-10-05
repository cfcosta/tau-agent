//! Servers' resources (`docs/reference/mcp.md`, "Resources"): against
//! the in-process server, on 2026-07-28 and 2025-11-25, listing every
//! page of resources and templates with MCP apps' left out, reading text
//! and blobs, `list_changed`, a read that is tried again once, and a
//! server without the capability. As properties, the resource tools'
//! exposure is the widest among the servers that offer resources, and
//! MCP apps are recognized whatever the case and spacing of their type.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

mod common;

use std::{
    os::unix::fs::PermissionsExt,
    sync::Arc,
    time::{Duration, Instant},
};

use base64::Engine;
use common::{LOGO, State as Fixture};
use hegel::{TestCase, generators as gs};
use serde_json::json;
use tau_ai::message::InputBlock;
use tau_mcp::{
    config::{Exposure, Origin, ServerConfig, Transport},
    connection::{Connection, Environment, State},
    resources::{exposure, is_app},
    results::resource_contents,
};
use tokio_util::sync::CancellationToken;

fn environment() -> Environment {
    Environment {
        env: Arc::new(|_| None),
        home: None,
        repo: None,
        auth: None,
        launcher: Default::default(),
    }
}

async fn connected(fixture: &Arc<Fixture>) -> Arc<Connection> {
    let config = ServerConfig::new("srv", Transport::Stream(fixture.dial()));
    let connection = Connection::new(config, Origin::User, environment());
    connection.connect();
    connection.settled(&CancellationToken::new()).await;
    assert_eq!(connection.status().state, State::Connected);
    connection
}

/// Waits up to 5 s for `check`.
async fn eventually(mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn uris(connection: &Connection) -> Vec<String> {
    connection.resources().into_iter().map(|r| r.uri).collect()
}

async fn lists_resources_templates_and_prompts(legacy: bool) {
    let fixture = Fixture::with_features(legacy);
    let connection = connected(&fixture).await;
    assert!(connection.offers_resources() && connection.offers_prompts());
    // Every page, MCP apps' left out.
    assert_eq!(
        uris(&connection),
        [
            "file:///notes.txt",
            "file:///logo.png",
            "file:///data.bin",
            "file:///flaky"
        ]
    );
    let notes = &connection.resources()[0];
    assert_eq!(
        (notes.title.as_deref(), notes.mime_type.as_deref()),
        (Some("Notes"), Some("text/plain"))
    );
    assert_eq!(connection.resources()[2].size, Some(3));
    let templates = connection.templates();
    assert_eq!(templates.len(), 1);
    assert_eq!(templates[0].uri_template, "file:///notes/{name}");
    assert_eq!(templates[0].description.as_deref(), Some("A note by name."));
    let prompts = connection.prompts();
    assert_eq!(
        prompts.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        ["greet", "summary"]
    );
    let greet = &prompts[0];
    assert_eq!(
        greet
            .arguments
            .iter()
            .map(|a| (a.name.as_str(), a.required))
            .collect::<Vec<_>>(),
        [("name", true), ("style", false)]
    );

    // A list change lists them again.
    let generation = connection.generation();
    fixture
        .add_resource(json!({"uri": "file:///new.txt", "name": "new"}))
        .await;
    eventually(|| uris(&connection).contains(&"file:///new.txt".to_owned()))
        .await;
    fixture
        .add_prompt(json!({"name": "later", "description": "Added later."}))
        .await;
    eventually(|| connection.prompts().iter().any(|p| p.name == "later")).await;
    assert!(connection.generation() > generation);
    connection.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn lists_resources_templates_and_prompts_2026() {
    lists_resources_templates_and_prompts(false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn lists_resources_templates_and_prompts_2025() {
    lists_resources_templates_and_prompts(true).await;
}

/// Reading a resource gives its `ReadResourceResult`: text, blobs, and a
/// filled-in template's. An unknown URI fails with the server's message.
#[tokio::test(flavor = "multi_thread")]
async fn reads_text_and_blobs() {
    let fixture = Fixture::with_features(false);
    let connection = connected(&fixture).await;
    let cancel = CancellationToken::new();
    let notes = connection
        .read_resource("file:///notes.txt", &cancel)
        .await
        .unwrap();
    assert_eq!(notes["contents"][0]["text"], json!("Remember the milk."));
    let logo = connection
        .read_resource("file:///logo.png", &cancel)
        .await
        .unwrap();
    let blob = logo["contents"][0]["blob"].as_str().unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(blob)
            .unwrap(),
        LOGO
    );
    let note = connection
        .read_resource("file:///notes/milk", &cancel)
        .await
        .unwrap();
    assert_eq!(note["contents"][0]["text"], json!("note milk"));
    let missing = connection
        .read_resource("file:///nothing", &cancel)
        .await
        .unwrap_err();
    assert!(
        missing
            .starts_with("MCP server srv failed the read of file:///nothing:")
            && missing.contains("no resource file:///nothing"),
        "{missing}"
    );
    connection.shutdown().await;
}

/// Reads are read-only, so one the connection drops under is sent again,
/// once, on a new connection.
#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_read_is_tried_again_once() {
    let fixture = Fixture::with_features(false);
    let connection = connected(&fixture).await;
    let read = connection
        .read_resource("file:///flaky", &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(read["contents"][0]["text"], json!("steady"));
    assert_eq!(fixture.reads("file:///flaky"), 2);
    assert_eq!(fixture.dials(), 2);
    connection.shutdown().await;
}

/// A server without resources or prompts lists none, and reading or
/// getting fails saying so, without a request.
#[tokio::test(flavor = "multi_thread")]
async fn a_server_without_the_capabilities_has_none() {
    let fixture = Fixture::new(false);
    let connection = connected(&fixture).await;
    assert!(!connection.offers_resources() && !connection.offers_prompts());
    assert!(connection.resources().is_empty());
    assert!(connection.templates().is_empty());
    assert!(connection.prompts().is_empty());
    let cancel = CancellationToken::new();
    assert_eq!(
        connection
            .read_resource("file:///notes.txt", &cancel)
            .await
            .unwrap_err(),
        "MCP server srv does not offer resources"
    );
    assert_eq!(
        connection
            .get_prompt("greet", serde_json::Map::new(), &cancel)
            .await
            .unwrap_err(),
        "MCP server srv does not offer prompts"
    );
    assert_eq!(fixture.reads("file:///notes.txt"), 0);
    connection.shutdown().await;
}

/// `prompts/get` with arguments gives the server's messages; a missing
/// argument the server needs fails with its message.
#[tokio::test(flavor = "multi_thread")]
async fn gets_prompts_with_arguments() {
    let fixture = Fixture::with_features(true);
    let connection = connected(&fixture).await;
    let cancel = CancellationToken::new();
    let mut arguments = serde_json::Map::new();
    arguments.insert("name".into(), json!("Ada"));
    arguments.insert("style".into(), json!("warmly"));
    let greet = connection
        .get_prompt("greet", arguments, &cancel)
        .await
        .unwrap();
    assert_eq!(
        greet["messages"][0]["content"]["text"],
        json!("Say hello to Ada, warmly.")
    );
    let error = connection
        .get_prompt("greet", serde_json::Map::new(), &cancel)
        .await
        .unwrap_err();
    assert!(error.contains("greet needs a name"), "{error}");
    connection.shutdown().await;
}

/// A resource's contents for the model: text as text, an image as an
/// image, other binary in a 0600 file named by its path, MCP apps' left
/// out.
#[test]
fn contents_for_the_model() {
    let dir = tempfile::tempdir().unwrap();
    let spill = tau_agent::output::Spill::new(dir.path(), "tau-mcp");
    let encode =
        |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
    let contents = [
        json!({"uri": "file:///a", "text": "hello"}),
        json!({"uri": "file:///i", "mimeType": "image/png", "blob": encode(LOGO)}),
        json!({"uri": "file:///z", "mimeType": "application/zip", "blob": encode(b"\x00zip")}),
        json!({"uri": "ui://app", "text": "<app>"}),
        json!({"uri": "file:///p", "mimeType": "text/html; profile=mcp-app", "text": "<app>"}),
    ];
    let blocks = resource_contents(&contents, &spill);
    assert_eq!(blocks.len(), 3);
    assert!(matches!(&blocks[0], InputBlock::Text(t) if t.text == "hello"));
    assert!(matches!(
        &blocks[1],
        InputBlock::Image(image) if image.mime_type == "image/png" && image.data == encode(LOGO)
    ));
    let InputBlock::Text(saved) = &blocks[2] else {
        panic!("a text block")
    };
    let path = saved
        .text
        .strip_prefix("[Resource file:///z (application/zip) saved to ")
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap();
    assert_eq!(std::fs::read(path).unwrap(), b"\x00zip");
    let mode = std::fs::metadata(path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

/// The widest exposure among servers that offer resources, by a
/// reference that ranks them by hand: direct if any such server is
/// direct, else codemode if any is, else none.
#[hegel::test(test_cases = 300)]
fn exposure_is_the_widest_among_servers_with_resources(tc: TestCase) {
    let drawn: Vec<(usize, bool)> = tc.draw(gs::vecs(hegel::tuples!(
        gs::integers::<usize>().max_value(2),
        gs::booleans()
    )));
    let servers: Vec<(Exposure, bool)> = drawn
        .into_iter()
        .map(|(exposure, offers)| (Exposure::ALL[exposure], offers))
        .collect();
    let offering = |wanted: Exposure| {
        servers
            .iter()
            .any(|(exposure, offers)| *offers && *exposure == wanted)
    };
    let expected = if offering(Exposure::Direct) {
        Some(Exposure::Direct)
    } else if offering(Exposure::Codemode) {
        Some(Exposure::Codemode)
    } else {
        None
    };
    assert_eq!(exposure(servers.clone()), expected);
    // The order the servers come in does not matter.
    let mut reversed = servers;
    reversed.reverse();
    assert_eq!(exposure(reversed), expected);
}

/// `ui://` URIs and the MCP app type are apps whatever their case and
/// spacing; a type that only starts like it, or a URI that has `ui://`
/// past its start, is not.
#[hegel::test(test_cases = 200)]
fn mcp_apps_are_recognized(tc: TestCase) {
    let rest: String = tc.draw(gs::from_regex("[a-z/]{0,12}"));
    let upper: bool = tc.draw(gs::booleans());
    let scheme = if upper { "UI://" } else { "ui://" };
    assert!(is_app(&format!("{scheme}{rest}"), None));
    assert!(!is_app(&format!("file:///{rest}ui://"), None));
    let spaces: String = tc.draw(gs::from_regex("[ \t]{0,3}"));
    let mime = format!("Text/HTML;{spaces}profile=MCP-app");
    assert!(is_app("file:///x", Some(&mime)));
    assert!(!is_app("file:///x", Some("text/html")));
    assert!(!is_app(
        "file:///x",
        Some("text/html;profile=mcp-application")
    ));
}
