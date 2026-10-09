//! Properties of the Sign in with ChatGPT pieces that need no server:
//! the authorization URL, callback checks, error classification and the
//! credential record (`tau_ai::chatgpt`).

use std::collections::HashMap;

use hegel::{
    TestCase,
    generators::{self as gs, Generator as _, PrintableGenerator},
};
use serde_json::json;
use tau_ai::chatgpt::{
    AUTHORIZE_URL,
    AccountId,
    AccountStatus,
    ApiError,
    AuthorizeParams,
    Callback,
    ChatGptError,
    Credentials,
    DYNAMIC_CLIENT_ID,
    HostId,
    RESOURCE,
    Recovery,
    RedirectUri,
    Registration,
    SCOPES,
    query_pairs,
    read_callback,
};
use url::Url;

/// Documented plan-usage codes and what they ask for.
const CODES: [(&str, Recovery); 9] = [
    (
        "subscription_sharing_user_not_eligible",
        Recovery::Restricted,
    ),
    (
        "subscription_sharing_usage_limit_exceeded",
        Recovery::UsageLimit,
    ),
    (
        "subscription_sharing_usage_unavailable",
        Recovery::RetryLater,
    ),
    (
        "subscription_sharing_unsupported_capability",
        Recovery::FixRequest,
    ),
    (
        "subscription_sharing_route_not_supported",
        Recovery::FixRequest,
    ),
    ("subscription_sharing_invalid_user", Recovery::SignInAgain),
    ("chatpass_v2_scope_not_authorized", Recovery::Restricted),
    (
        "chatpass_v2_invalid_authorization_context",
        Recovery::Restricted,
    ),
    (
        "subscription_sharing_user_unavailable",
        Recovery::RetryLater,
    ),
];

fn expected_status_fallback(status: u16) -> Recovery {
    match status {
        401 => Recovery::SignInAgain,
        403 => Recovery::Restricted,
        408 | 409 | 429 | 500..=599 => Recovery::RetryLater,
        _ => Recovery::FixRequest,
    }
}

fn client_id() -> impl PrintableGenerator<String> {
    gs::from_regex("oaiapp_[A-Za-z0-9]{1,24}").fullmatch(true)
}

fn some_text() -> impl PrintableGenerator<String> {
    gs::text().min_size(1).max_size(40)
}

#[hegel::composite]
fn unrecognized_error_body(tc: &TestCase) -> String {
    match tc.draw(gs::integers::<u8>().min_value(0).max_value(3)) {
        0 => {
            let suffix = tc.draw(
                gs::text()
                    .alphabet("abcdefghijklmnopqrstuvwxyz0123456789 ")
                    .max_size(50),
            );
            format!("not-json:{suffix}")
        }
        1 => {
            let message = tc.draw(
                gs::text()
                    .alphabet("abcdefghijklmnopqrstuvwxyz0123456789 ")
                    .max_size(20),
            );
            json!({"error": {"message": message}}).to_string()
        }
        2 => {
            let suffix = tc.draw(
                gs::text()
                    .alphabet("abcdefghijklmnopqrstuvwxyz0123456789_")
                    .max_size(24),
            );
            let code = format!("future_{suffix}");
            json!({"error": {"code": code}}).to_string()
        }
        _ => {
            let wrong_type = tc
                .draw(gs::sampled_from(vec!["null", "true", "7", "[]", "{}"]));
            format!(r#"{{"error":{{"code":{wrong_type}}}}}"#)
        }
    }
}

fn map(url: &Url) -> HashMap<String, String> {
    let pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
    let map: HashMap<String, String> = pairs.iter().cloned().collect();
    assert_eq!(map.len(), pairs.len(), "no parameter twice");
    map
}

#[hegel::composite]
fn authorize_params(tc: &TestCase) -> AuthorizeParams {
    let client_id = tc.draw(gs::optional(client_id()));
    let returning = client_id.is_some();
    AuthorizeParams {
        client_id,
        host_id: HostId::new_uuid(),
        redirect_uri: RedirectUri {
            port: tc.draw(gs::integers::<u16>().min_value(1)),
        },
        state: tc.draw(some_text()),
        nonce: tc.draw(some_text()),
        code_challenge: tc
            .draw(gs::from_regex("[A-Za-z0-9_-]{43}").fullmatch(true)),
        id_token_hint: if returning {
            tc.draw(gs::optional(some_text()))
        } else {
            None
        },
        login_hint: if returning {
            tc.draw(gs::optional(some_text()))
        } else {
            None
        },
        ask_consent: tc.draw(gs::booleans()),
    }
}

#[hegel::test(test_cases = 300)]
fn the_authorize_url_carries_every_parameter_back(tc: TestCase) {
    let params = tc.draw(authorize_params().print_as_debug());
    let base = Url::parse(AUTHORIZE_URL).unwrap();
    let url = params.url(&base);
    assert_eq!(url.origin(), base.origin());
    assert_eq!(url.path(), "/api/accounts/authorize");
    let got = map(&url);
    let get = |name: &str| got.get(name).map(String::as_str);
    assert_eq!(get("response_type"), Some("code"));
    assert_eq!(
        get("client_id"),
        Some(params.client_id.as_deref().unwrap_or(DYNAMIC_CLIENT_ID))
    );
    // The name hint only on a first registration.
    assert_eq!(
        get("agent_name_hint"),
        params.client_id.is_none().then_some("tau")
    );
    assert_eq!(get("ext_agent_host_id"), Some(params.host_id.as_str()));
    assert_eq!(
        get("redirect_uri"),
        Some(
            format!(
                "http://127.0.0.1:{}/auth/callback",
                params.redirect_uri.port
            )
            .as_str()
        )
    );
    assert_eq!(get("scope"), Some(SCOPES));
    assert_eq!(get("resource"), Some(RESOURCE));
    assert_eq!(get("state"), Some(params.state.as_str()));
    assert_eq!(get("nonce"), Some(params.nonce.as_str()));
    assert_eq!(get("code_challenge"), Some(params.code_challenge.as_str()));
    assert_eq!(get("code_challenge_method"), Some("S256"));
    assert_eq!(get("id_token_hint"), params.id_token_hint.as_deref());
    assert_eq!(get("login_hint"), params.login_hint.as_deref());
    assert_eq!(get("prompt"), params.ask_consent.then_some("consent"));
}

/// A redirect URL carrying `pairs`, as OpenAI would build it.
fn redirect(port: u16, pairs: &[(&str, &str)]) -> String {
    let mut url = Url::parse(&RedirectUri { port }.to_string()).unwrap();
    url.query_pairs_mut().extend_pairs(pairs);
    url.into()
}

fn returning(client_id: &str) -> Registration {
    Registration::Returning {
        account: AccountId::new(client_id, "sub"),
        client_id: client_id.to_owned(),
        subject: "sub".into(),
    }
}

#[hegel::test(test_cases = 300)]
fn a_callback_reads_back_what_was_sent(tc: TestCase) {
    let code = tc.draw(some_text());
    let state = tc.draw(some_text());
    let issued = tc.draw(client_id());
    let scope = tc.draw(gs::optional(some_text()));
    let port = tc.draw(gs::integers::<u16>().min_value(1));
    let mut pairs = vec![("code", code.as_str()), ("state", state.as_str())];
    if let Some(scope) = &scope {
        pairs.push(("scope", scope));
    }
    let with_client = {
        let mut pairs = pairs.clone();
        pairs.push(("client_id", &issued));
        pairs
    };
    let expected = Callback {
        code: code.clone(),
        client_id: issued.clone(),
        scope: scope.clone(),
    };
    let check = |returned: &str, registration: &Registration| {
        read_callback(&query_pairs(returned), &state, registration)
    };

    // A new registration needs the issued client id.
    let url = redirect(port, &with_client);
    assert!(check(&url, &Registration::New).is_ok_and(|got| got == expected));
    // Pasted as a query string, it reads the same.
    let query = Url::parse(&url).unwrap().query().unwrap().to_owned();
    assert!(check(&query, &Registration::New).is_ok_and(|got| got == expected));
    assert!(matches!(
        check(&redirect(port, &pairs), &Registration::New),
        Err(ChatGptError::RegistrationIncomplete)
    ));
    // A returning sign-in may omit it, but never change it.
    assert!(
        check(&redirect(port, &pairs), &returning(&issued))
            .is_ok_and(|got| got == expected)
    );
    assert!(check(&url, &returning(&issued)).is_ok_and(|got| got == expected));
    assert!(matches!(
        check(&url, &returning(&format!("{issued}x"))),
        Err(ChatGptError::ClientMismatch { .. })
    ));
}

#[hegel::test(test_cases = 300)]
fn any_other_state_is_refused_first(tc: TestCase) {
    let state = tc.draw(some_text());
    let other = tc.draw(some_text());
    tc.assume(state != other);
    let error = tc.draw(gs::optional(gs::sampled_from(vec![
        "access_denied".to_owned(),
        "server_error".to_owned(),
    ])));
    let mut pairs = vec![
        ("state", other.as_str()),
        ("code", "c"),
        ("client_id", "oaiapp_1"),
    ];
    if let Some(error) = &error {
        pairs.push(("error", error));
    }
    let returned = redirect(1455, &pairs);
    assert!(matches!(
        read_callback(&query_pairs(&returned), &state, &Registration::New),
        Err(ChatGptError::StateMismatch)
    ));
    // With the right state, a declined consent stops the attempt.
    let declined =
        redirect(1455, &[("state", &state), ("error", "access_denied")]);
    assert!(matches!(
        read_callback(&query_pairs(&declined), &state, &Registration::New),
        Err(ChatGptError::ConsentDeclined)
    ));
}

/// Property inventory: the literal `CODES` table is the independent recovery
/// oracle. Drawn codes keep their exact spelling in the JSON body; status and
/// table index shrink within 0..=700 and the finite documented-code list.
#[hegel::test(test_cases = 300)]
fn documented_codes_decide_the_recovery_whatever_the_status(tc: TestCase) {
    let at = tc.draw(gs::integers::<usize>().max_value(CODES.len() - 1));
    let (code, recovery) = CODES[at];
    let status = tc.draw(gs::integers::<u16>().min_value(0).max_value(700));
    let param = tc.draw(gs::optional(some_text()));
    let body = json!({"error": {
        "code": code,
        "message": "m",
        "param": param,
        "type": "invalid_request_error",
    }});
    let error = ApiError::new(
        status,
        Some("req_1".into()),
        body.to_string().as_bytes(),
    );
    assert_eq!(error.recovery(), recovery);
    assert_eq!(error.code(), Some(code));
    assert_eq!(error.body, body.to_string(), "kept verbatim");
}

#[hegel::test(test_cases = 300)]
fn admission_details_are_classified_by_status(tc: TestCase) {
    let detail = tc.draw(gs::text().max_size(60));
    let status = tc.draw(gs::sampled_from(vec![401_u16, 403, 503]));
    let body = json!({ "detail": detail }).to_string();
    let error = ApiError::new(status, None, body.as_bytes());
    let expected = match status {
        401 => Recovery::SignInAgain,
        403 => Recovery::Restricted,
        _ => Recovery::RetryLater,
    };
    assert_eq!(error.recovery(), expected);
    assert_eq!(error.code(), None);
}

/// Property inventory: bodies without a documented code use the literal
/// status table below. Status shrinks within 0..=700; body generation selects
/// malformed, missing, unknown, or wrong-type codes, with ASCII bodies capped
/// at 60 bytes so shrinking leaves small, readable counterexamples.
#[hegel::test(test_cases = 300)]
fn bodies_without_a_code_fall_back_to_the_status(tc: TestCase) {
    let status = tc.draw(gs::integers::<u16>().min_value(0).max_value(700));
    let body = tc.draw(unrecognized_error_body());
    assert!(body.len() <= 60, "generated body exceeds its byte bound");
    let error = ApiError::new(status, None, body.as_bytes());
    assert_eq!(error.recovery(), expected_status_fallback(status));
    assert_eq!(error.body, body, "kept verbatim");
}

#[test]
fn status_boundaries_apply_to_every_unrecognized_body_shape() {
    const STATUSES: [u16; 10] =
        [400, 401, 403, 408, 409, 429, 499, 500, 599, 600];
    const BODIES: [(&str, &str); 11] = [
        ("empty malformed body", ""),
        ("malformed text", "not-json"),
        ("malformed html", "<html>bad gateway</html>"),
        ("missing code", r#"{"error":{}}"#),
        ("missing code with message", r#"{"error":{"message":"m"}}"#),
        (
            "unknown string code",
            r#"{"error":{"code":"future_missing"}}"#,
        ),
        ("null code", r#"{"error":{"code":null}}"#),
        ("boolean code", r#"{"error":{"code":true}}"#),
        ("number code", r#"{"error":{"code":7}}"#),
        ("array code", r#"{"error":{"code":[]}}"#),
        ("object code", r#"{"error":{"code":{}}}"#),
    ];

    for status in STATUSES {
        for (shape, body) in BODIES {
            let error = ApiError::new(status, None, body.as_bytes());
            assert_eq!(
                error.recovery(),
                expected_status_fallback(status),
                "status {status}, body shape {shape}"
            );
            assert_eq!(error.body.as_str(), body, "status {status}, {shape}");
        }
    }
}

#[hegel::composite]
fn credentials(tc: &TestCase) -> Credentials {
    let token = || gs::from_regex("tok_[A-Za-z0-9]{8,24}").fullmatch(true);
    Credentials {
        label: tc.draw(some_text()),
        email: tc.draw(gs::optional(some_text())),
        issuer: "https://auth.openai.com".into(),
        subject: tc.draw(some_text()),
        client_id: tc.draw(client_id()),
        ext_agent_host_id: HostId::new_uuid(),
        id_token: tc.draw(gs::optional(token())),
        access_token: tc.draw(gs::optional(token())),
        refresh_token: tc.draw(gs::optional(token())),
        token_type: Some("Bearer".into()),
        expires_in: tc.draw(gs::optional(gs::integers::<u64>())),
        expires_at: tc.draw(gs::optional(gs::integers::<u64>())),
        earliest_refresh_at: None,
        scopes: tc.draw(gs::vecs(some_text()).max_size(6)),
        saved_at: "2026-09-29T00:00:00Z".into(),
    }
}

#[hegel::test(test_cases = 200)]
fn a_record_round_trips_and_never_prints_tokens(tc: TestCase) {
    let record = tc.draw(credentials().print_as_debug());
    let text = serde_json::to_string(&record).unwrap();
    let back: Credentials = serde_json::from_str(&text).unwrap();
    assert_eq!(back, record);
    let debug = format!("{record:?}");
    for token in [
        &record.id_token,
        &record.access_token,
        &record.refresh_token,
    ]
    .into_iter()
    .flatten()
    {
        assert!(!debug.contains(token.as_str()));
    }
    assert_eq!(
        back.id(),
        AccountId::new(&record.client_id, &record.subject)
    );
}

/// The retry policy follows the documented recovery of every plan-usage
/// code, whatever `type` and HTTP status come with it: only "retry
/// later" retries, so a usage limit never loops.
#[hegel::test(test_cases = 200)]
fn the_retry_policy_follows_the_documented_recovery(tc: TestCase) {
    use tau_ai::retry::{Class, Failure, classify};
    let (code, recovery) =
        CODES[tc.draw(gs::integers::<usize>().max_value(CODES.len() - 1))];
    let kind = tc.draw(gs::optional(gs::sampled_from(vec![
        "invalid_request_error",
        "server_error",
        "api_error",
    ])));
    let status = tc.draw(gs::optional(gs::integers::<u16>()));
    let class = classify(&Failure::Api {
        code: Some(code),
        kind,
        status,
    });
    assert_eq!(class, recovery.class(), "{code}");
    assert_eq!(
        class == Class::Retryable,
        recovery == Recovery::RetryLater,
        "{code}"
    );
}

/// Unix seconds a clock could show; `now + 5 minutes` cannot overflow.
fn clock() -> impl PrintableGenerator<u64> {
    gs::integers::<u64>().max_value(1 << 62)
}

/// The restatement `needs_refresh` is held to: no usable access token,
/// or an expired one, always refreshes; inside the five-minute margin
/// it waits for `earliest_refresh_at`; before the margin it never does.
fn should_refresh(record: &Credentials, now: u64) -> bool {
    let (Some(_), Some(expires)) = (&record.access_token, record.expires_at)
    else {
        return true;
    };
    if now >= expires {
        return true;
    }
    let near = expires - now <= 300;
    near && record
        .earliest_refresh()
        .is_none_or(|earliest| now >= earliest)
}

#[hegel::test(test_cases = 500)]
fn a_refresh_is_due_by_expiry_margin_and_earliest_time(tc: TestCase) {
    let mut record = tc.draw(credentials().print_as_debug());
    let now = tc.draw(clock());
    // Draw the expiry near `now` often: the margin is where the logic is.
    record.expires_at = tc.draw(gs::optional(hegel::one_of!(
        clock(),
        gs::integers::<i64>()
            .min_value(-400)
            .max_value(400)
            .map(move |delta| now.saturating_add_signed(delta)),
    )));
    record.earliest_refresh_at = tc.draw(gs::optional(hegel::one_of!(
        clock().map(|n| json!(n)),
        clock().map(|n| json!(n.to_string())),
        gs::just(json!("soon")),
        gs::just(json!(-5)),
        gs::just(json!(1.5)),
        gs::just(json!(null)),
    )));
    assert_eq!(
        record.needs_refresh(now),
        should_refresh(&record, now),
        "{record:?} {:?} at {now}",
        record.earliest_refresh_at
    );
}

/// Once a refresh is due it stays due as time passes: the client never
/// refreshes, then stops wanting to before the token is replaced.
#[hegel::test(test_cases = 500)]
fn a_due_refresh_stays_due_as_time_passes(tc: TestCase) {
    let mut record = tc.draw(credentials().print_as_debug());
    let now = tc.draw(clock());
    record.expires_at =
        Some(now.saturating_add(tc.draw(gs::integers::<u64>().max_value(900))));
    record.earliest_refresh_at =
        tc.draw(gs::optional(clock().map(|n| json!(n))));
    let later =
        now.saturating_add(tc.draw(gs::integers::<u64>().max_value(2000)));
    if record.needs_refresh(now) {
        assert!(record.needs_refresh(later), "{record:?} {now} {later}");
    }
}

/// `earliest_refresh_at` counts only as a non-negative integer, or a
/// string holding one.
#[hegel::test(test_cases = 300)]
fn the_earliest_refresh_reads_integers_and_integer_strings(tc: TestCase) {
    let mut record = tc.draw(credentials().print_as_debug());
    let n = tc.draw(gs::integers::<u64>());
    let blanks = tc.draw(gs::text().alphabet(" \t").max_size(2));
    for (given, want) in [
        (json!(n), Some(n)),
        (json!(format!("{blanks}{n}{blanks}")), Some(n)),
        (json!(format!("-{}", n.max(1))), None),
        (json!(n as f64 + 0.5), None),
        (json!("never"), None),
        (json!(null), None),
        (json!([n]), None),
    ] {
        record.earliest_refresh_at = Some(given.clone());
        assert_eq!(record.earliest_refresh(), want, "{given}");
    }
}

/// Signing out forgets every token and leaves the registration; a dead
/// session keeps only the ID token. Either way the account reads as
/// signed out, and a record with a token reads as signed in with
/// whatever plan usage its scopes grant.
#[hegel::test(test_cases = 300)]
fn clearing_tokens_signs_the_account_out_and_keeps_its_registration(
    tc: TestCase,
) {
    let record = tc.draw(credentials().print_as_debug());
    let expected = if record.has_tokens() {
        AccountStatus::SignedIn(record.plan_usage())
    } else {
        AccountStatus::SignedOut
    };
    assert_eq!(record.status(), expected);
    assert_eq!(
        record.has_tokens(),
        record.access_token.is_some() || record.refresh_token.is_some()
    );

    let mut session = record.clone();
    session.clear_session();
    assert_eq!(session.status(), AccountStatus::SignedOut);
    assert_eq!(session.id_token, record.id_token);
    assert!(session.needs_refresh(tc.draw(clock())));

    let mut signed_out = record.clone();
    signed_out.clear_tokens();
    assert_eq!(signed_out.status(), AccountStatus::SignedOut);
    assert_eq!(signed_out.id_token, None);
    assert_eq!(
        (
            &signed_out.label,
            &signed_out.client_id,
            &signed_out.subject
        ),
        (&record.label, &record.client_id, &record.subject)
    );
    assert_eq!(signed_out.id(), record.id());
    assert_eq!(signed_out.plan_usage(), record.plan_usage());
}

/// An account id is the client id made file-safe, a dash and twelve
/// hex digits of the subject's hash: a name any file may carry, which
/// parses back to itself, and which keeps two subjects of one client
/// apart.
#[hegel::test(test_cases = 300)]
fn an_account_id_is_a_safe_file_name_that_tells_subjects_apart(tc: TestCase) {
    let client = tc.draw(gs::text().max_size(100));
    let (a, b) = (
        tc.draw(gs::text().max_size(30)),
        tc.draw(gs::text().max_size(30)),
    );
    let id = AccountId::new(&client, &a);
    let (prefix, hash) = id.as_str().rsplit_once('-').unwrap();
    let safe: String = client
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '-'
            }
        })
        .take(64)
        .collect();
    assert_eq!(prefix, safe);
    assert!(hash.len() == 12 && hash.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(AccountId::parse(id.as_str()), Some(id.clone()));
    assert_eq!(id, AccountId::new(&client, &a));
    assert_eq!(a == b, id == AccountId::new(&client, &b));
}

/// A typed id names a file only if it is made of file-safe characters;
/// the surrounding whitespace the user typed is dropped.
#[hegel::test(test_cases = 300)]
fn a_typed_account_id_is_trimmed_and_must_be_file_safe(tc: TestCase) {
    let text = tc.draw(gs::text().max_size(20));
    let want = text.trim();
    let safe = !want.is_empty()
        && want
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    assert_eq!(
        AccountId::parse(&text).map(|id| id.to_string()),
        safe.then(|| want.to_owned())
    );
    let word = tc.draw(gs::from_regex("[A-Za-z0-9_-]{1,20}").fullmatch(true));
    let padded = format!("  {word}\n");
    assert_eq!(AccountId::parse(&padded).unwrap().as_str(), word);
}

/// A made host id is a version 4 `urn:uuid:` that `parse` takes back;
/// `parse` takes the three documented forms with something after the
/// prefix, and nothing else.
#[hegel::test(test_cases = 200)]
fn host_ids_are_uuid_v4_urns_and_parse_only_the_documented_forms(tc: TestCase) {
    let made = HostId::new_uuid();
    let uuid = made.as_str().strip_prefix("urn:uuid:").unwrap();
    let groups: Vec<&str> = uuid.split('-').collect();
    assert_eq!(
        groups.iter().map(|g| g.len()).collect::<Vec<_>>(),
        [8, 4, 4, 4, 12]
    );
    assert!(uuid.bytes().all(|b| b == b'-' || b.is_ascii_hexdigit()));
    assert!(groups[2].starts_with('4'));
    assert!(groups[3].starts_with(['8', '9', 'a', 'b']));
    assert_eq!(HostId::parse(made.as_str()), Some(made));

    let rest = tc.draw(gs::text().max_size(10));
    for prefix in [
        "urn:uuid:",
        "urn:ietf:params:oauth:jwk-thumbprint:",
        "did:key:",
    ] {
        let text = format!("{prefix}{rest}");
        assert_eq!(HostId::parse(&text).is_some(), !rest.is_empty(), "{text}");
    }
    let other = format!("x{rest}");
    assert!(HostId::parse(&other).is_none());
}
