//! Signing in's laws (`docs/reference/mcp.md`, "Tests"): PKCE verifiers
//! and their challenges; the callback accepts exactly this attempt's
//! answer; the grants file keeps what it is given, owner-only, and never
//! shows a token; the callback address is loopback-only.

use std::{collections::BTreeMap, net::IpAddr, os::unix::fs::PermissionsExt};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hegel::{
    TestCase,
    generators::{self as gs, Generator},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use tau_mcp::{
    auth::{
        CallbackError,
        Grant,
        GrantKey,
        TokenStore,
        pkce_challenge,
        read_callback,
        valid_verifier,
    },
    config::callback_address,
};

/// RFC 7636's unreserved characters.
const UNRESERVED: &str =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";

/// Every verifier of 43 to 128 unreserved characters is valid, and its
/// challenge is 43 base64url characters that decode to the SHA-256 of
/// the verifier.
#[hegel::test(test_cases = 300)]
fn every_verifier_has_its_s256_challenge(tc: TestCase) {
    let verifier: String =
        tc.draw(gs::text().alphabet(UNRESERVED).min_size(43).max_size(128));
    assert!(valid_verifier(&verifier));
    let challenge = pkce_challenge(&verifier);
    assert_eq!(challenge.len(), 43);
    assert!(
        challenge
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    );
    let digest = URL_SAFE_NO_PAD.decode(&challenge).unwrap();
    assert_eq!(digest, Sha256::digest(verifier.as_bytes()).to_vec());
}

/// A verifier too short, too long, or with any other character is not
/// one.
#[hegel::test(test_cases = 300)]
fn anything_else_is_not_a_verifier(tc: TestCase) {
    let kind: u8 = tc.draw(gs::integers().max_value(2_u8));
    let verifier: String = match kind {
        0 => tc.draw(gs::text().alphabet(UNRESERVED).max_size(42)),
        1 => {
            tc.draw(gs::text().alphabet(UNRESERVED).min_size(129).max_size(200))
        }
        _ => {
            let good: String = tc.draw(
                gs::text().alphabet(UNRESERVED).min_size(42).max_size(127),
            );
            let bad: char =
                tc.draw(gs::characters().exclude_characters(UNRESERVED));
            let at = tc.draw(gs::integers().max_value(good.len()));
            let mut verifier = good;
            verifier.insert(at, bad);
            verifier
        }
    };
    assert!(!valid_verifier(&verifier), "{verifier:?}");
}

#[hegel::composite]
fn token(tc: &TestCase) -> String {
    tc.draw(gs::text().min_size(1).max_size(24))
}

fn target(path: &str, pairs: &[(&str, &str)]) -> String {
    let query: String = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish();
    format!("{path}?{query}")
}

/// The callback gives the code back exactly when the path is the
/// callback's and the state is this attempt's, once; any other state,
/// or none, or two, is a mismatch even when the server says why it
/// refused. With the right state, an error is the refusal.
#[hegel::test(test_cases = 500)]
fn the_callback_is_only_this_attempts(tc: TestCase) {
    let state = tc.draw(token());
    let code: String = tc.draw(gs::from_regex("c0de-[A-Za-z0-9 &=?%+]{1,16}"));
    let path = "/callback";
    let issuer: Option<String> =
        tc.draw(gs::optional(gs::from_regex("https://[a-z]{1,8}\\.dev")));
    let mut pairs = vec![("code", code.as_str()), ("state", state.as_str())];
    if let Some(issuer) = &issuer {
        pairs.push(("iss", issuer));
    }
    let shuffled: Vec<(&str, &str)> = tc.draw(gs::permutations(pairs.clone()));
    let got = read_callback(&target(path, &shuffled), path, &state).unwrap();
    assert_eq!(got.code, code);
    assert_eq!(got.issuer, issuer);
    // The code is not printed.
    assert!(!format!("{got:?}").contains(&format!("{code:?}")));

    let other = tc.draw(token());
    if other != state {
        let pairs = [("code", code.as_str()), ("state", other.as_str())];
        assert_eq!(
            read_callback(&target(path, &pairs), path, &state),
            Err(CallbackError::StateMismatch)
        );
        let pairs = [("error", "access_denied"), ("state", other.as_str())];
        assert_eq!(
            read_callback(&target(path, &pairs), path, &state),
            Err(CallbackError::StateMismatch)
        );
    }
    let missing = [("code", code.as_str())];
    assert_eq!(
        read_callback(&target(path, &missing), path, &state),
        Err(CallbackError::StateMismatch)
    );
    let twice = [
        ("code", code.as_str()),
        ("state", state.as_str()),
        ("state", state.as_str()),
    ];
    assert_eq!(
        read_callback(&target(path, &twice), path, &state),
        Err(CallbackError::StateMismatch)
    );
    let refused = [("state", state.as_str()), ("error", "access_denied")];
    assert!(matches!(
        read_callback(&target(path, &refused), path, &state),
        Err(CallbackError::Denied { error, .. }) if error == "access_denied"
    ));
    let no_code = [("state", state.as_str())];
    assert_eq!(
        read_callback(&target(path, &no_code), path, &state),
        Err(CallbackError::NoCode)
    );
    assert_eq!(
        read_callback(&target("/elsewhere", &pairs), path, &state),
        Err(CallbackError::WrongPath)
    );
}

/// What the grants file is asked to do, against a map of the grants.
#[derive(Debug, Clone)]
enum Step {
    Put(usize, String),
    SignOut(usize),
    Remove(usize),
}

/// A few keys: two servers, with and without a configured client.
fn keys() -> Vec<GrantKey> {
    let mut keys = Vec::new();
    for url in ["https://a.dev/mcp", "http://127.0.0.1:9/mcp"] {
        for client in [None, Some("pre".to_owned())] {
            keys.push(GrantKey {
                url: url.to_owned(),
                client,
            });
        }
    }
    keys
}

fn grant(key: &GrantKey, secret: &str) -> Grant {
    Grant {
        url: key.url.clone(),
        client: key.client.clone(),
        client_id: key.client.clone().unwrap_or_else(|| "issued".into()),
        client_secret: Some(format!("cs-{secret}")),
        redirect_uri: "http://127.0.0.1:4000/callback".into(),
        metadata: json!({ "authorization_endpoint": "x", "token_endpoint": "y" }),
        issuer: Some("https://auth.dev".into()),
        tokens: Some(json!({
            "access_token": format!("at-{secret}"),
            "refresh_token": format!("rt-{secret}"),
            "token_type": "Bearer",
            "expires_in": 60,
        })),
        received_at: Some(1_000),
        scopes: vec!["read".into()],
        account: None,
        signed_in: Some(format!("id-{secret}")),
        ..Grant::default()
    }
}

/// Whatever is put, signed out and removed, in whatever order, the file
/// holds what a map would, survives being read again, is the owner's
/// alone, and no grant's `Debug` shows a token or a secret. Signing out
/// keeps the client and forgets the sign-in.
#[hegel::test(test_cases = 80)]
fn the_grants_file_keeps_what_it_is_given(tc: TestCase) {
    let keys = keys();
    let steps: Vec<Step> = tc.draw(
        gs::vecs(hegel::one_of!(
            hegel::tuples!(
                gs::integers().max_value(3_usize),
                gs::from_regex("[a-z0-9]{1,6}")
            )
            .map(|(at, secret)| Step::Put(at, secret)),
            gs::integers().max_value(3_usize).map(Step::SignOut),
            gs::integers().max_value(3_usize).map(Step::Remove),
        ))
        .max_size(12)
        .print_as_debug(),
    );
    let dir = tempfile::tempdir().unwrap();
    let store = TokenStore::in_dir(dir.path());
    let mut model: BTreeMap<GrantKey, Grant> = BTreeMap::new();
    for step in steps {
        match step {
            Step::Put(at, secret) => {
                let grant = grant(&keys[at], &secret);
                store.put(grant.clone()).unwrap();
                model.insert(keys[at].clone(), grant);
            }
            Step::SignOut(at) => {
                let was = store.sign_out(&keys[at]).unwrap();
                let expected = model.get_mut(&keys[at]).map(|grant| {
                    let was = grant.is_signed_in();
                    grant.sign_out();
                    was
                });
                assert_eq!(was, expected.unwrap_or(false));
                assert_eq!(store.fingerprint(&keys[at]), None);
            }
            Step::Remove(at) => {
                store.update(&keys[at], |_| None).unwrap();
                model.remove(&keys[at]);
            }
        }
        // A new store reads the same file.
        let read = TokenStore::in_dir(dir.path());
        let got: BTreeMap<GrantKey, Grant> = read
            .grants()
            .unwrap()
            .into_iter()
            .map(|grant| (grant.key(), grant))
            .collect();
        assert_eq!(got, model);
        for (key, grant) in &model {
            assert_eq!(
                read.fingerprint(key),
                grant.signed_in.clone().filter(|_| grant.is_signed_in())
            );
            let shown = format!("{grant:?}");
            assert!(
                !shown.contains("at-")
                    && !shown.contains("rt-")
                    && !shown.contains("cs-")
            );
        }
        if store.path().exists() {
            let mode = std::fs::metadata(store.path())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}

/// A host for a callback URL, and whether it is loopback.
fn hosts() -> Vec<(&'static str, bool)> {
    vec![
        ("127.0.0.1", true),
        ("127.8.9.10", true),
        ("localhost", true),
        ("LocalHost", true),
        ("[::1]", true),
        ("0.0.0.0", false),
        ("10.0.0.1", false),
        ("128.0.0.1", false),
        ("[::2]", false),
        ("example.com", false),
        ("localhost.example.com", false),
        ("127.0.0.1.example.com", false),
    ]
}

/// A callback URL is accepted exactly when it is plain http on a
/// loopback host, with no query or fragment, and a port the URL or
/// `callbackPort` gives, the two agreeing. Then tau listens on loopback,
/// and the redirect URI it makes keeps the host, port and path.
#[hegel::test(test_cases = 500)]
fn the_callback_is_loopback_only(tc: TestCase) {
    let https: bool = tc.draw(gs::booleans());
    let (host, loopback) = tc.draw(gs::sampled_from(hosts()));
    let port: Option<u16> =
        tc.draw(gs::optional(gs::integers().min_value(1_u16)));
    let path: String = tc.draw(gs::from_regex("(/[a-z0-9]{0,6}){0,3}"));
    let query: bool = tc.draw(gs::booleans());
    let callback_port: Option<u16> = tc.draw(hegel::one_of!(
        gs::just(None),
        gs::integers().min_value(1_u16).map(Some),
        gs::just(port),
    ));
    let mut url = format!("{}://{host}", if https { "https" } else { "http" });
    if let Some(port) = port {
        url.push_str(&format!(":{port}"));
    }
    url.push_str(&path);
    if query {
        url.push_str("?x=1");
    }
    let agree = match (port, callback_port) {
        (Some(a), Some(b)) => a == b,
        (None, None) => false,
        _ => true,
    };
    let accepted = callback_address(Some(&url), callback_port);
    assert_eq!(
        accepted.is_ok(),
        !https && loopback && !query && agree,
        "{url} with {callback_port:?}: {accepted:?}"
    );
    if let Ok(address) = accepted {
        assert!(address.ip.is_loopback());
        let port = port.or(callback_port).unwrap();
        assert_eq!(address.port, port);
        let redirect = address.redirect_uri(port);
        let parsed = url::Url::parse(&redirect).unwrap();
        let expected = url::Url::parse(&url).unwrap();
        assert_eq!(parsed.host(), expected.host());
        assert_eq!(parsed.path(), expected.path());
        assert_eq!(parsed.port_or_known_default(), Some(port));
    }
}

/// Boundary inventory: None, 1, 80, and 65535 configured ports. The law is
/// loopback binding with the configured port (or zero), and a redirect to the
/// actual listener port. The oracle is the bind fields, a literal URI, and its
/// parsed components; these fixed rows need no shrinking.
#[test]
fn the_default_callback_binds_loopback_and_uses_the_listener_port() {
    let cases = [
        (None, 49_152),
        (Some(1), 1),
        (Some(80), 80),
        (Some(65535), 65535),
    ];

    for (configured_port, listener_port) in cases {
        let address = callback_address(None, configured_port).unwrap();
        assert_eq!(address.ip, IpAddr::from([127, 0, 0, 1]));
        assert_eq!(address.port, configured_port.unwrap_or(0));

        let redirect_uri = address.redirect_uri(listener_port);
        let expected_uri = format!("http://127.0.0.1:{listener_port}/callback");
        assert_eq!(redirect_uri, expected_uri);

        let parsed_uri = url::Url::parse(&redirect_uri).unwrap();
        assert_eq!(parsed_uri.scheme(), "http");
        assert_eq!(parsed_uri.host_str(), Some("127.0.0.1"));
        assert_eq!(parsed_uri.port_or_known_default(), Some(listener_port));
        assert_eq!(parsed_uri.path(), "/callback");
        assert_eq!(parsed_uri.query(), None);
        assert_eq!(parsed_uri.fragment(), None);
    }
}

/// The hosts' filter keeps rmcp's sign-in targets, and their modules,
/// at info and above, and lets everything else through at any level.
#[test]
fn the_secrets_filter_clamps_rmcps_sign_in() {
    use tracing::Level;
    use tracing_subscriber::filter::LevelFilter;
    let filter = tau_mcp::auth::secrets_filter();
    for target in tau_mcp::auth::SECRET_TARGETS {
        for level in [Level::TRACE, Level::DEBUG] {
            assert!(!filter.would_enable(target, &level), "{target} {level}");
            assert!(!filter.would_enable(&format!("{target}::inner"), &level));
        }
        assert!(filter.would_enable(target, &Level::INFO));
        assert!(filter.would_enable(target, &Level::WARN));
    }
    for target in [
        "tau_mcp::server",
        "rmcp::service",
        "rmcp::transport::worker",
    ] {
        assert!(filter.would_enable(target, &Level::TRACE), "{target}");
    }
    assert_eq!(
        tau_mcp::auth::LOG_DIRECTIVES.split(',').count(),
        tau_mcp::auth::SECRET_TARGETS.len()
    );
    let _ = LevelFilter::INFO;
}
