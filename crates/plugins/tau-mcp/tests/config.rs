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

/// An `oauth` block is checked: never with an Authorization header, a
/// secret only with a client, a loopback callback, a port that fits;
/// empty strings and nulls are left out.
#[test]
fn oauth_blocks_are_checked() {
    let error = |entry: serde_json::Value| {
        let (_, errors) =
            McpConfig::from_value(&json!({ "mcpServers": { "s": entry } }));
        errors.first().map(|error| error.message.clone())
    };
    let url = "https://mcp.dev/mcp";
    assert!(
        error(json!({ "url": url, "headers": { "authorization": "Bearer x" }, "oauth": {} }))
            .unwrap()
            .contains("Authorization")
    );
    assert!(
        error(json!({ "url": url, "oauth": { "clientSecret": "x" } }))
            .unwrap()
            .contains("clientId")
    );
    assert!(
        error(json!({ "url": url, "oauth": { "callbackUrl": "http://10.0.0.1:9/cb" } }))
            .unwrap()
            .contains("loopback")
    );
    assert!(
        error(json!({ "url": url, "oauth": { "callbackUrl": "http://127.0.0.1/cb" } }))
            .unwrap()
            .contains("port")
    );
    assert!(
        error(json!({ "url": url, "oauth": { "callbackPort": 70000 } }))
            .unwrap()
            .contains("port")
    );
    assert!(error(json!({ "url": url, "oauth": "yes" })).is_some());
    let (config, errors) = McpConfig::from_value(
        &json!({ "mcpServers": { "s": {
        "url": url,
        "oauth": { "clientId": "", "scope": null, "callbackUrl": "http://localhost:8/cb" }
    } } }),
    );
    assert!(errors.is_empty(), "{errors:?}");
    let Transport::Http(http) = &config.servers[0].transport else {
        panic!("an HTTP server");
    };
    assert_eq!(
        http.oauth,
        Some(OAuthConfig {
            callback_url: Some("http://localhost:8/cb".into()),
            ..OAuthConfig::default()
        })
    );
    // A secret never shows in Debug.
    let secret = OAuthConfig {
        client_id: Some("c".into()),
        client_secret: Some("hush".into()),
        ..OAuthConfig::default()
    };
    assert!(!format!("{secret:?}").contains("hush"));
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

/// A callback URL on a loopback host with a port and a path is taken
/// as written, and the redirect URI it registers is that URL. Port 80
/// is the case `Url` hides, so it is drawn on purpose.
#[hegel::test(test_cases = 300)]
fn a_loopback_callback_url_is_the_redirect_uri(tc: TestCase) {
    let (written, host) = tc.draw(loopback_host());
    let port = tc.draw(hegel::one_of!(
        gs::just(80_u16),
        gs::integers::<u16>().min_value(1)
    ));
    let path = tc.draw(callback_path());
    let agreeing = tc.draw(gs::optional(gs::just(port)));
    let url = format!("http://{written}:{port}{path}");
    let address = callback_address(Some(&url), agreeing)
        .unwrap_or_else(|error| panic!("{url}: {error}"));
    assert_eq!((address.port, &address.path), (port, &path), "{url}");
    assert_eq!(
        address.redirect_uri(port),
        format!("http://{host}:{port}{path}")
    );
    assert!(address.ip.is_loopback());
}

/// The port may come from `callbackPort` alone; without either, the
/// URL is refused; with both they must agree.
#[hegel::test(test_cases = 300)]
fn the_callback_port_comes_from_the_url_or_the_setting_and_they_agree(
    tc: TestCase,
) {
    let (written, _) = tc.draw(loopback_host());
    let (url_port, setting) = (
        tc.draw(gs::integers::<u16>().min_value(1)),
        tc.draw(gs::integers::<u16>()),
    );
    let bare = format!("http://{written}/cb");
    assert_eq!(
        callback_address(Some(&bare), Some(setting)).unwrap().port,
        setting
    );
    assert!(callback_address(Some(&bare), None).is_err());
    let with_port = format!("http://{written}:{url_port}/cb");
    let both = callback_address(Some(&with_port), Some(setting));
    assert_eq!(
        both.is_ok(),
        url_port == setting,
        "{with_port} and {setting}"
    );
}

/// Everything else about a callback URL is refused: another scheme, a
/// user, a query or a fragment, and any host that is not loopback.
#[hegel::test(test_cases = 300)]
fn a_callback_url_that_is_not_plain_loopback_http_is_refused(tc: TestCase) {
    let (written, _) = tc.draw(loopback_host());
    let port = tc.draw(gs::integers::<u16>().min_value(1));
    let path = tc.draw(callback_path());
    let url = match tc.draw(gs::integers::<u8>().max_value(5)) {
        0 => format!("https://{written}:{port}{path}"),
        1 => format!("http://user@{written}:{port}{path}"),
        2 => format!("http://user:pw@{written}:{port}{path}"),
        3 => format!("http://{written}:{port}{path}?x=1"),
        4 => format!("http://{written}:{port}{path}#frag"),
        _ => {
            let host = tc.draw(hegel::one_of!(
                gs::sampled_from(vec![
                    "0.0.0.0".to_owned(),
                    "[::]".to_owned(),
                    "10.0.0.1".to_owned(),
                    "192.168.1.9".to_owned(),
                    "128.0.0.1".to_owned(),
                    "[::2]".to_owned(),
                ]),
                gs::domains().map(|domain| format!("x{domain}")),
            ));
            format!("http://{host}:{port}{path}")
        }
    };
    assert!(callback_address(Some(&url), Some(port)).is_err(), "{url}");
}

/// With no URL the callback is `127.0.0.1` on the asked port, or any
/// free one, at the default path.
#[hegel::test(test_cases = 100)]
fn the_default_callback_is_127_0_0_1_at_the_default_path(tc: TestCase) {
    let setting = tc.draw(gs::optional(gs::integers::<u16>()));
    let address = callback_address(None, setting).unwrap();
    assert_eq!(address.port, setting.unwrap_or(0));
    assert_eq!(address.redirect_uri(1234), "http://127.0.0.1:1234/callback");
}
