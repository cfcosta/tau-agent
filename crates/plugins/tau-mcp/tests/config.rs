//! The configuration's laws (`docs/reference/mcp.md`, "Tests"):
//! printing and parsing round-trips; merging is last-wins by name; names
//! that differ only in `-` and `_` clash; `${VAR}` expansion leaves text
//! without `${` unchanged; a tool's exposure follows its exact name, then
//! the first glob; a glob matches what its regular expression matches.

use std::collections::BTreeMap;

use hegel::{
    TestCase,
    generators::{self as gs, Generator},
};
use serde_json::{Value, json};
use tau_mcp::config::{
    ConfigError,
    Exposure,
    HttpConfig,
    McpConfig,
    OAuthConfig,
    Origin,
    SSE_REFUSED,
    ServerConfig,
    Settings,
    Sources,
    StdioConfig,
    Transport,
    callback_address,
    expand_vars,
    exposure_of,
    glob_matches,
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
        let mut http = HttpConfig {
            url: tc.draw(gs::from_regex("https?://[a-z0-9.:/${}_]{1,20}")),
            headers: tc.draw(pairs()),
            oauth: None,
        };
        // An `oauth` block only without an Authorization header.
        if http.uses_oauth() {
            http.oauth = tc.draw(gs::optional(oauth()).print_as_debug());
        }
        Transport::Http(http)
    }
}

/// A valid `oauth` block: a secret only with a client, a callback URL
/// on a loopback host whose port agrees with `callbackPort`.
#[hegel::composite]
fn oauth(tc: &TestCase) -> OAuthConfig {
    let text = |pattern: &'static str| gs::optional(gs::from_regex(pattern));
    let client_id: Option<String> = tc.draw(text("[a-z0-9-]{1,10}"));
    let client_secret = match client_id {
        Some(_) => tc.draw(text("[a-z0-9${}_]{1,10}")),
        None => None,
    };
    let port: u16 = tc.draw(gs::integers().min_value(1_u16));
    let callback_url: Option<String> = tc.draw(gs::optional(
        gs::sampled_from(vec!["127.0.0.1", "localhost", "[::1]"])
            .map(move |host| format!("http://{host}:{port}/cb")),
    ));
    let callback_port = match &callback_url {
        Some(_) => tc.draw(gs::optional(gs::just(port))),
        None => tc.draw(gs::optional(gs::integers().min_value(1_u16))),
    };
    OAuthConfig {
        client_id,
        client_secret,
        callback_port,
        callback_url,
        scope: tc.draw(text("[a-z:]{1,6}( [a-z:]{1,6}){0,2}")),
        client_name: tc.draw(text("[A-Za-z ]{1,10}")),
        auth_server_metadata_url: tc
            .draw(text("https://[a-z]{1,8}\\.dev/meta")),
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

/// A glob matches exactly the texts its regular expression does: `*` is
/// any run of characters, everything else is literal, and the match is
/// whole. Patterns and texts share a small alphabet so they often match,
/// and the oracle is `regex`, not the hand-written matcher.
#[hegel::test(test_cases = 500)]
fn a_glob_matches_what_its_regular_expression_matches(tc: TestCase) {
    let pattern: String =
        tc.draw(gs::from_regex("[ab_*]{0,6}").fullmatch(true));
    let text: String = tc.draw(gs::from_regex("[ab_]{0,8}").fullmatch(true));
    assert_eq!(
        glob_matches(&pattern, &text),
        glob_regex(&pattern).is_match(&text),
        "{pattern:?} against {text:?}"
    );
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

/// The ways an `oauth` block goes wrong, each with words only its own
/// error holds.
const OAUTH_DEFECTS: [&str; 8] = [
    "`Authorization`",
    "needs `oauth.clientId`",
    "loopback host",
    "must name its port",
    "1 to 65535",
    "`oauth` must be an object",
    "must be a string",
    "`oauth.authServerMetadataUrl` must be an http",
];

/// A valid `oauth` block, with one defect or none, is checked: with a
/// defect the entry is skipped and its one error is that defect's; with
/// none, the block reads back as it was written, empty strings and nulls
/// for the fields it leaves out changing nothing. A secret never shows
/// in `Debug`.
#[hegel::test(test_cases = 300)]
fn an_oauth_block_is_checked_one_defect_at_a_time(tc: TestCase) {
    let block = tc.draw(oauth().print_as_debug());
    let server = ServerConfig::new(
        "s",
        Transport::Http(HttpConfig {
            url: "https://mcp.dev/mcp".into(),
            headers: Vec::new(),
            oauth: Some(block.clone()),
        }),
    );
    let mut entry = McpConfig {
        servers: vec![server],
    }
    .to_value()["mcpServers"]["s"]
        .clone();
    let defect: Option<usize> = tc.draw(gs::optional(
        gs::integers::<usize>().max_value(OAUTH_DEFECTS.len() - 1),
    ));
    let oauth = entry["oauth"].as_object_mut().unwrap();
    match defect {
        Some(0) => {
            let name = tc.draw(gs::sampled_from(vec![
                "Authorization",
                "authorization",
                "AUTHORIZATION",
            ]));
            entry["headers"] = json!({ name: "Bearer x" });
        }
        Some(1) => {
            oauth.remove("clientId");
            oauth.insert("clientSecret".into(), json!("x"));
        }
        Some(2) => {
            let host = tc.draw(gs::sampled_from(vec![
                "10.0.0.1",
                "0.0.0.0",
                "example.com",
                "[::2]",
            ]));
            oauth.insert(
                "callbackUrl".into(),
                json!(format!("http://{host}:9/cb")),
            );
        }
        Some(3) => {
            oauth.insert("callbackUrl".into(), json!("http://127.0.0.1/cb"));
            oauth.remove("callbackPort");
        }
        Some(4) => {
            let port = tc.draw(gs::sampled_from(vec![
                json!(0),
                json!(65_536),
                json!(-1),
                json!(1.5),
                json!("8080"),
            ]));
            oauth.insert("callbackPort".into(), port);
        }
        Some(5) => {
            entry["oauth"] = tc.draw(gs::sampled_from(vec![
                json!("yes"),
                json!(1),
                json!([]),
            ]));
        }
        Some(6) => {
            let key = tc.draw(gs::sampled_from(vec![
                "clientId",
                "callbackUrl",
                "scope",
                "clientName",
                "authServerMetadataUrl",
            ]));
            oauth.insert(key.into(), json!(7));
        }
        Some(_) => {
            oauth.insert(
                "authServerMetadataUrl".into(),
                json!("ftp://auth.dev/meta"),
            );
        }
        None => {
            for key in [
                "clientId",
                "clientSecret",
                "callbackUrl",
                "scope",
                "clientName",
                "authServerMetadataUrl",
                "callbackPort",
            ] {
                if !oauth.contains_key(key) && tc.draw(gs::booleans()) {
                    let absent =
                        if key == "callbackPort" || tc.draw(gs::booleans()) {
                            Value::Null
                        } else {
                            json!("")
                        };
                    oauth.insert(key.into(), absent);
                }
            }
        }
    }
    let (config, errors) =
        McpConfig::from_value(&json!({ "mcpServers": { "s": entry } }));
    match defect {
        Some(index) => {
            assert!(config.servers.is_empty(), "{entry}");
            assert_eq!(errors.len(), 1, "{errors:?}");
            assert_eq!(errors[0].server.as_deref(), Some("s"));
            let message = &errors[0].message;
            assert!(message.contains(OAUTH_DEFECTS[index]), "{message}");
            for (other, words) in OAUTH_DEFECTS.iter().enumerate() {
                if other != index {
                    assert!(!message.contains(words), "{message}");
                }
            }
        }
        None => {
            assert!(errors.is_empty(), "{errors:?}");
            let Transport::Http(http) = &config.servers[0].transport else {
                panic!("an HTTP server");
            };
            assert_eq!(http.oauth.as_ref(), Some(&block), "{entry}");
        }
    }
    let secret = OAuthConfig {
        client_id: Some("c".into()),
        client_secret: Some("zqhush".into()),
        ..block
    };
    assert!(!format!("{secret:?}").contains("zqhush"));
}

/// A loopback host as a callback URL writes it, and as the redirect
/// URI must give it back.
#[hegel::composite]
fn loopback_host(tc: &TestCase) -> (String, String) {
    match tc.draw(gs::integers::<u8>().max_value(2)) {
        0 => {
            let ip = format!(
                "127.{}.{}.{}",
                tc.draw(gs::integers::<u8>()),
                tc.draw(gs::integers::<u8>()),
                tc.draw(gs::integers::<u8>())
            );
            (ip.clone(), ip)
        }
        1 => {
            let written = tc.draw(gs::sampled_from(vec![
                "localhost",
                "LOCALHOST",
                "LocalHost",
            ]));
            (written.to_owned(), "localhost".to_owned())
        }
        _ => ("[::1]".to_owned(), "[::1]".to_owned()),
    }
}

#[hegel::composite]
fn callback_path(tc: &TestCase) -> String {
    let segments =
        tc.draw(gs::vecs(gs::from_regex("[a-z0-9_-]{1,8}")).max_size(3));
    format!("/{}", segments.join("/"))
}

/// A callback URL on a loopback host with a path is taken as written,
/// its port from the URL or from `callbackPort`, which agree when both
/// are given; the redirect URI it registers is that URL with the port.
/// Port 80 is the case `Url` hides, so it is drawn on purpose.
#[hegel::test(test_cases = 300)]
fn a_loopback_callback_url_is_the_redirect_uri(tc: TestCase) {
    let (written, host) = tc.draw(loopback_host());
    let port = tc.draw(hegel::one_of!(
        gs::just(80_u16),
        gs::sampled_from(vec![1_u16, 65_535]),
        gs::integers::<u16>().min_value(1)
    ));
    let path = tc.draw(callback_path());
    // The port in the URL, in `callbackPort`, or in both.
    let (in_url, configured) = match tc.draw(gs::integers::<u8>().max_value(2))
    {
        0 => (true, None),
        1 => (false, Some(port)),
        _ => (true, Some(port)),
    };
    let url = if in_url {
        format!("http://{written}:{port}{path}")
    } else {
        format!("http://{written}{path}")
    };
    let address = callback_address(Some(&url), configured)
        .unwrap_or_else(|error| panic!("{url} with {configured:?}: {error}"));
    assert_eq!((address.port, &address.path), (port, &path), "{url}");
    assert_eq!(
        address.redirect_uri(port),
        format!("http://{host}:{port}{path}")
    );
    assert!(address.ip.is_loopback());
}

/// Everything else about a callback URL is refused: another scheme, a
/// user, a query or a fragment, any host that is not loopback, a port
/// the URL and `callbackPort` disagree on, and no port at all.
#[hegel::test(test_cases = 300)]
fn a_callback_url_that_is_not_plain_loopback_http_is_refused(tc: TestCase) {
    let (written, _) = tc.draw(loopback_host());
    let port = tc.draw(gs::integers::<u16>().min_value(1));
    let path = tc.draw(callback_path());
    let mut configured = Some(port);
    let url = match tc.draw(gs::integers::<u8>().max_value(7)) {
        0 => format!("https://{written}:{port}{path}"),
        1 => format!("http://user@{written}:{port}{path}"),
        2 => format!("http://user:pw@{written}:{port}{path}"),
        3 => format!("http://{written}:{port}{path}?x=1"),
        4 => format!("http://{written}:{port}{path}#frag"),
        5 => {
            // Any other port: the two disagree.
            configured = Some(port % u16::MAX + 1);
            format!("http://{written}:{port}{path}")
        }
        6 => {
            configured = None;
            format!("http://{written}{path}")
        }
        _ => {
            let host = tc.draw(hegel::one_of!(
                gs::sampled_from(vec![
                    "0.0.0.0".to_owned(),
                    "[::]".to_owned(),
                    "10.0.0.1".to_owned(),
                    "192.168.1.9".to_owned(),
                    "128.0.0.1".to_owned(),
                    "[::2]".to_owned(),
                    "localhost.example.com".to_owned(),
                    "127.0.0.1.example.com".to_owned(),
                ]),
                gs::domains().map(|domain| format!("x{domain}")),
            ));
            format!("http://{host}:{port}{path}")
        }
    };
    assert!(
        callback_address(Some(&url), configured).is_err(),
        "{url} with {configured:?}"
    );
}
