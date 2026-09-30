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

fn client_id() -> impl PrintableGenerator<String> {
    gs::from_regex("oaiapp_[A-Za-z0-9]{1,24}").fullmatch(true)
}

fn some_text() -> impl PrintableGenerator<String> {
    gs::text().min_size(1).max_size(40)
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

#[hegel::test(test_cases = 300)]
fn documented_codes_decide_the_recovery_whatever_the_status(tc: TestCase) {
    let at = tc.draw(gs::integers::<usize>().max_value(CODES.len() - 1));
    let (code, recovery) = CODES[at];
    let status = tc.draw(gs::integers::<u16>().min_value(400).max_value(599));
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
    assert_eq!(Recovery::of_code(code), Some(recovery));
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

#[hegel::test(test_cases = 300)]
fn bodies_without_a_code_fall_back_to_the_status(tc: TestCase) {
    let status = tc.draw(gs::integers::<u16>().min_value(400).max_value(599));
    let body = tc.draw(gs::sampled_from(vec![
        String::new(),
        "<html>bad gateway</html>".to_owned(),
        json!({"error": {"message": "no code"}}).to_string(),
        json!({"error": {"code": "some_future_code"}}).to_string(),
    ]));
    let error = ApiError::new(status, None, body.as_bytes());
    assert_eq!(error.recovery(), Recovery::of_status(status));
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
