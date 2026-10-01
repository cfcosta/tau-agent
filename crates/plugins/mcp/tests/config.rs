//! The configuration's laws (`docs/reference/mcp.md`, "Tests"):
//! printing and parsing round-trips; merging is last-wins by name; names
//! that differ only in `-` and `_` clash; `${VAR}` expansion leaves text
//! without `${` unchanged; a tool's exposure follows its exact name, then
//! the first glob.

use std::collections::BTreeMap;

use hegel::{
    TestCase,
    generators::{self as gs, Generator},
};
use serde_json::json;
use tau_mcp::config::{
    ConfigError,
    Exposure,
    HttpConfig,
    McpConfig,
    Origin,
    SSE_REFUSED,
    ServerConfig,
    Settings,
    Sources,
    StdioConfig,
    Transport,
    expand_vars,
    exposure_of,
    merge,
};

#[hegel::composite]
fn exposure(tc: &TestCase) -> Exposure {
    let index: usize = tc.draw(gs::integers().max_value(2_usize));
    Exposure::ALL[index]
}

#[hegel::composite]
fn pairs(tc: &TestCase) -> Vec<(String, String)> {
    let map: BTreeMap<String, String> = tc.draw(
        gs::btree_maps(gs::text().max_size(8), gs::text().max_size(12))
            .max_size(3),
    );
    map.into_iter().collect()
}

#[hegel::composite]
fn transport(tc: &TestCase) -> Transport {
    if tc.draw(gs::booleans()) {
        Transport::Stdio(StdioConfig {
            command: tc.draw(gs::from_regex("[a-z~/._-]{0,10}[a-z]")),
            args: tc.draw(gs::vecs(gs::text().max_size(10)).max_size(3)),
            env: tc.draw(pairs()),
            cwd: tc.draw(gs::optional(gs::text().max_size(10))),
        })
    } else {
        Transport::Http(HttpConfig {
            url: tc.draw(gs::from_regex("https?://[a-z0-9.:/${}_]{1,20}")),
            headers: tc.draw(pairs()),
        })
    }
}

#[hegel::composite]
fn server(tc: &TestCase, name: String) -> ServerConfig {
    let rules: BTreeMap<String, usize> = tc.draw(
        gs::btree_maps(
            gs::from_regex("[a-z*_]{0,6}"),
            gs::integers().max_value(2_usize),
        )
        .max_size(3),
    );
    let timeout: f64 = tc.draw(hegel::one_of!(
        gs::just(60.0),
        gs::floats::<f64>()
            .min_value_exclusive(0.0)
            .max_value(1e9)
            .allow_nan(false)
            .allow_infinity(false)
    ));
    ServerConfig {
        name,
        transport: tc.draw_silent(transport()),
        exposure: tc.draw_silent(exposure()),
        tool_exposure: rules
            .into_iter()
            .map(|(pattern, index)| (pattern, Exposure::ALL[index]))
            .collect(),
        description: tc.draw(gs::optional(gs::text().max_size(20))),
        enabled: tc.draw(gs::booleans()),
        timeout,
    }
}

#[hegel::composite]
fn config(tc: &TestCase) -> McpConfig {
    let names: Vec<String> = tc.draw(
        gs::vecs(gs::from_regex("[A-Za-z0-9_-]{1,8}"))
            .max_size(4)
            .unique(true),
    );
    McpConfig {
        servers: names
            .into_iter()
            .map(|name| tc.draw_silent(server(name)))
            .collect(),
    }
}

/// Printing a config and parsing it gives the same servers, in order,
/// and no errors.
#[hegel::test(test_cases = 300)]
fn printing_and_parsing_round_trips(tc: TestCase) {
    let config = tc.draw(config().print_as_debug());
    let (parsed, errors) = McpConfig::parse(&config.to_json());
    assert_eq!(errors, Vec::<ConfigError>::new());
    assert_eq!(parsed, config);
}

/// Merging keeps one server per name: the last layer's entry, in the
/// place the name first appeared.
#[hegel::test(test_cases = 300)]
fn merging_is_last_wins_by_name(tc: TestCase) {
    // Names without `-` and `_`, so no two clash.
    let layer = || {
        hegel::compose!(|tc| {
            let names: Vec<String> = tc.draw(
                gs::vecs(gs::sampled_from(vec!["a", "b", "c", "d", "e"]))
                    .max_size(4)
                    .unique(true)
                    .map(|names| {
                        names.into_iter().map(str::to_owned).collect()
                    }),
            );
            McpConfig {
                servers: names
                    .into_iter()
                    .map(|name| tc.draw_silent(server(name)))
                    .collect(),
            }
        })
    };
    let user = tc.draw(layer().print_as_debug());
    let settings = tc.draw(layer().print_as_debug());
    let repo = tc.draw(layer().print_as_debug());
    let layers = [
        (Origin::User, &user),
        (Origin::Settings, &settings),
        (Origin::Repo, &repo),
    ];
    let (merged, errors) = merge(&layers);
    assert!(errors.is_empty());

    let mut order: Vec<String> = Vec::new();
    let mut last: BTreeMap<String, (Origin, ServerConfig)> = BTreeMap::new();
    for (origin, config) in layers {
        for server in &config.servers {
            if !order.contains(&server.name) {
                order.push(server.name.clone());
            }
            last.insert(server.name.clone(), (origin, server.clone()));
        }
    }
    let expected: Vec<(Origin, ServerConfig)> =
        order.iter().map(|name| last[name].clone()).collect();
    assert_eq!(merged, expected);
}

/// Of names that differ only in `-` and `_`, the first is kept and each
/// later one is reported as a clash; no two kept servers share a
/// namespace.
#[hegel::test(test_cases = 300)]
fn names_that_differ_in_dash_and_underscore_clash(tc: TestCase) {
    let names: Vec<String> = tc.draw(
        gs::vecs(gs::from_regex("[ab_-]{1,4}"))
            .min_size(1)
            .max_size(6)
            .unique(true),
    );
    let layer = McpConfig {
        servers: names
            .iter()
            .map(|name| {
                ServerConfig::new(
                    name.clone(),
                    Transport::Stdio(StdioConfig {
                        command: "x".into(),
                        ..StdioConfig::default()
                    }),
                )
            })
            .collect(),
    };
    let (merged, errors) = merge(&[(Origin::User, &layer)]);

    let mut seen: Vec<String> = Vec::new();
    let mut clashing: Vec<String> = Vec::new();
    for name in &names {
        let space = name.replace('-', "_");
        if seen.contains(&space) {
            clashing.push(name.clone());
        } else {
            seen.push(space);
        }
    }
    let kept: Vec<&str> = merged.iter().map(|(_, s)| s.name.as_str()).collect();
    let expected: Vec<&str> = names
        .iter()
        .filter(|name| !clashing.contains(name))
        .map(String::as_str)
        .collect();
    assert_eq!(kept, expected);
    let reported: Vec<String> =
        errors.iter().filter_map(|e| e.server.clone()).collect();
    assert_eq!(reported, clashing);
}

/// Text without `${` comes out of expansion unchanged, and no variable
/// is looked up.
#[hegel::test(test_cases = 500)]
fn expansion_leaves_text_without_a_reference_alone(tc: TestCase) {
    let mut text: String =
        tc.draw(gs::text().max_size(40).alphabet("ab${}_ \u{e9}"));
    while text.contains("${") {
        text = text.replace("${", "$");
    }
    let expanded = expand_vars(&text, &|name| panic!("looked up {name}"));
    assert_eq!(expanded.as_deref(), Ok(text.as_str()));
}

/// Literal pieces and `${NAME}` references expand to the pieces and the
/// values, in order.
#[hegel::test(test_cases = 300)]
fn expansion_substitutes_every_reference(tc: TestCase) {
    let pieces: Vec<(bool, String)> = tc.draw(
        gs::vecs(hegel::tuples!(
            gs::booleans(),
            gs::from_regex("[A-Z_][A-Z0-9_]{0,5}|[a-z }]{0,5}")
        ))
        .max_size(6),
    );
    let mut text = String::new();
    let mut expected = String::new();
    for (reference, piece) in &pieces {
        let is_name = piece
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase() || c == '_');
        if *reference && is_name {
            text.push_str(&format!("${{{piece}}}"));
            expected.push_str(&format!("<{piece}>"));
        } else {
            let literal = piece.replace('$', "");
            text.push_str(&literal);
            expected.push_str(&literal);
        }
    }
    let lookup = |name: &str| Some(format!("<{name}>"));
    assert_eq!(expand_vars(&text, &lookup), Ok(expected));
}

/// A glob as a regular expression: the reference for `exposure_of`.
fn glob_regex(pattern: &str) -> regex::Regex {
    let body: Vec<String> = pattern.split('*').map(regex::escape).collect();
    regex::Regex::new(&format!("^{}$", body.join(".*"))).unwrap()
}

/// A tool's exposure is its exact name's, else the first matching
/// glob's in order, else the server's.
#[hegel::test(test_cases = 500)]
fn exposure_follows_exact_name_then_first_glob(tc: TestCase) {
    let rules: Vec<(String, usize)> = tc.draw(
        gs::vecs(hegel::tuples!(
            gs::from_regex("[ab*]{0,4}"),
            gs::integers().max_value(2_usize)
        ))
        .max_size(5),
    );
    let rules: Vec<(String, Exposure)> = rules
        .into_iter()
        .map(|(pattern, index)| (pattern, Exposure::ALL[index]))
        .collect();
    let default = tc.draw_silent(exposure());
    let tool: String = tc.draw(gs::from_regex("[ab*]{0,4}"));

    let expected = rules
        .iter()
        .find(|(key, _)| *key == tool)
        .or_else(|| {
            rules.iter().find(|(key, _)| {
                key.contains('*') && glob_regex(key).is_match(&tool)
            })
        })
        .map_or(default, |(_, exposure)| *exposure);
    assert_eq!(exposure_of(&rules, default, &tool), expected);
}

#[test]
fn invalid_entries_are_reported_and_skipped() {
    let (config, errors) = McpConfig::parse(
        &json!({
            "mcpServers": {
                "bad name": {"command": "x"},
                "both": {"command": "x", "url": "https://x"},
                "neither": {},
                "sse": {"type": "sse", "url": "https://x"},
                "ftp": {"url": "ftp://x"},
                "exposure": {"command": "x", "exposure": "deferred"},
                "timeout": {"command": "x", "timeout": 0},
                "args": {"command": "x", "args": [1]},
                "good": {"url": "https://x", "headers": {"A": "${T}"}}
            }
        })
        .to_string(),
    );
    assert_eq!(
        config
            .servers
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        ["good"]
    );
    let messages: Vec<String> =
        errors.iter().map(ToString::to_string).collect();
    assert_eq!(messages.len(), 8, "{messages:#?}");
    assert!(messages.contains(&format!("server `sse`: {SSE_REFUSED}")));
}

/// A repository server waits for approval by its hash; changing the
/// entry asks again.
#[test]
fn repository_servers_wait_for_approval() {
    let (repo, _) = McpConfig::parse(
        r#"{"mcpServers": {"git": {"command": "uvx", "args": ["mcp-server-git"]}}}"#,
    );
    let mut settings = Settings::default();
    let sources = Sources::merge(None, &settings, Some(&repo));
    assert!(sources.servers.is_empty());
    assert_eq!(sources.pending.len(), 1);

    settings.approved.insert(sources.pending[0].hash.clone());
    let sources = Sources::merge(None, &settings, Some(&repo));
    assert_eq!(sources.servers.len(), 1);
    assert_eq!(sources.servers[0].0, Origin::Repo);

    let (changed, _) = McpConfig::parse(
        r#"{"mcpServers": {"git": {"command": "uvx", "args": ["evil"]}}}"#,
    );
    let sources = Sources::merge(None, &settings, Some(&changed));
    assert!(sources.servers.is_empty());
    assert_eq!(sources.pending.len(), 1);
}

/// The two files are read from the documented places, the settings'
/// servers go between them, and a missing file is no servers.
#[test]
fn files_are_read_from_their_places() {
    let user = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(
        user.path().join("mcp.json"),
        r#"{"mcpServers": {"a": {"command": "user"}, "b": {"command": "user"}}}"#,
    )
    .unwrap();
    std::fs::create_dir(repo.path().join(".tau")).unwrap();
    std::fs::write(
        repo.path().join(".tau/mcp.json"),
        r#"{"mcpServers": {"b": {"command": "repo"}}}"#,
    )
    .unwrap();
    let settings: Settings = serde_json::from_value(json!({
        "mcpServers": {"a": {"command": "settings"}}
    }))
    .unwrap();
    let sources =
        Sources::load(Some(user.path()), &settings, Some(repo.path()));
    assert!(sources.errors.is_empty(), "{:?}", sources.errors);
    assert_eq!(sources.servers.len(), 1);
    assert_eq!(sources.servers[0].0, Origin::Settings);
    assert_eq!(sources.pending.len(), 1);
    assert_eq!(sources.pending[0].server.name, "b");

    let empty = tempfile::tempdir().unwrap();
    let sources = Sources::load(Some(empty.path()), &Settings::default(), None);
    assert!(sources.servers.is_empty() && sources.errors.is_empty());
}
