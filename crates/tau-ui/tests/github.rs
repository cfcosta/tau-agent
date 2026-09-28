//! Signing in to GitHub and listing repositories, against a fake GitHub
//! that answers the way the real one does.

mod support;

use std::sync::{
    Arc,
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

use serde_json::json;
use support::fake;
use tau_ui::github::{Api, Poll};
use tokio::sync::Notify;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn a_device_sign_in_polls_until_approved() {
    let polls = Arc::new(AtomicUsize::new(0));
    let counted = polls.clone();
    let (base, seen) = fake(move |line| match line {
        "POST /login/device/code" => (
            200,
            json!({
                "device_code": "dev-1", "user_code": "ABCD-1234",
                "verification_uri": "https://github.com/login/device",
                "expires_in": 900, "interval": 1
            }),
        ),
        "POST /login/oauth/access_token" => {
            if counted.fetch_add(1, Ordering::SeqCst) == 0 {
                (200, json!({ "error": "authorization_pending" }))
            } else {
                (
                    200,
                    json!({ "access_token": "ghu_token", "expires_in": 28800 }),
                )
            }
        }
        "GET /user" => (200, json!({ "login": "octocat" })),
        _ => (404, json!({})),
    });
    let api = Api::at(&base, &base);
    let shown = Mutex::new(None);
    let wake = Notify::new();
    // "I have approved it" checks right away, twice.
    wake.notify_one();
    let token = runtime()
        .block_on(
            api.sign_in(|code| *shown.lock().unwrap() = Some(code), &wake),
        )
        .unwrap();
    let code = shown.into_inner().unwrap().unwrap();
    assert_eq!(code.code, "ABCD-1234");
    assert_eq!(code.url, "github.com/login/device");
    assert_eq!(token.token, "ghu_token");
    assert_eq!(token.user, "octocat");
    assert!(token.expires_at.is_some());
    assert_eq!(polls.load(Ordering::SeqCst), 2);
    let requests = seen.lock().unwrap();
    // The app's id, and the device code, go in form bodies; no secret.
    assert!(requests[0].contains("client_id=Iv23"), "{}", requests[0]);
    assert!(requests[1].contains("device_code=dev-1"), "{}", requests[1]);
    assert!(requests.iter().all(|r| !r.contains("client_secret")));
    assert!(
        requests.iter().any(|r| r.contains("Bearer ghu_token")),
        "the account is read with the new token"
    );
}

#[test]
fn a_disabled_device_flow_is_explained() {
    let (base, _) = fake(|_| {
        (
            200,
            json!({
                "error": "device_flow_disabled",
                "error_description": "Device Flow must be explicitly enabled"
            }),
        )
    });
    let api = Api::at(&base, &base);
    let error = runtime().block_on(api.start_device()).unwrap_err();
    assert!(error.contains("turn on Device Flow"), "{error}");
    let (base, _) = fake(|_| (200, json!({ "error": "access_denied" })));
    let poll = runtime().block_on(Api::at(&base, &base).poll("d")).unwrap();
    assert_eq!(
        poll,
        Poll::Failed("The sign-in was cancelled on GitHub.".into())
    );
}

#[test]
fn repositories_come_from_the_apps_installations() {
    let (base, _) = fake(|line| match line {
        "GET /user/installations?per_page=100" => {
            (200, json!({ "installations": [{ "id": 7 }, { "id": 9 }] }))
        }
        "GET /user/installations/7/repositories?per_page=100" => (
            200,
            json!({ "repositories": [
                { "full_name": "cfcosta/tau-agent", "description": "agents",
                  "default_branch": "main" },
                { "full_name": "cfcosta/docbert", "description": null,
                  "default_branch": "trunk" }
            ]}),
        ),
        "GET /user/installations/9/repositories?per_page=100" => (
            200,
            json!({ "repositories": [
                { "full_name": "cfcosta/tau-agent", "default_branch": "main" }
            ]}),
        ),
        _ => (404, json!({})),
    });
    let repos = runtime()
        .block_on(Api::at(&base, &base).repos("ghu_token"))
        .unwrap();
    let names: Vec<(&str, &str)> = repos
        .iter()
        .map(|repo| (repo.name.as_str(), repo.branch.as_str()))
        .collect();
    assert_eq!(
        names,
        [("cfcosta/docbert", "trunk"), ("cfcosta/tau-agent", "main")]
    );
    assert_eq!(repos[1].description, "agents");
}

#[test]
fn a_personal_token_lists_the_users_repositories() {
    let (base, _) = fake(|line| match line {
        // Personal tokens cannot list app installations.
        "GET /user/installations?per_page=100" => (403, json!({})),
        "GET /user/repos?per_page=100&sort=pushed" => (
            200,
            json!([{ "full_name": "octocat/hello", "default_branch": "main" }]),
        ),
        _ => (404, json!({})),
    });
    let repos = runtime()
        .block_on(Api::at(&base, &base).repos("github_pat_x"))
        .unwrap();
    assert_eq!(repos.len(), 1);
    assert_eq!(repos[0].name, "octocat/hello");
    let (base, _) = fake(|_| (401, json!({ "message": "Bad credentials" })));
    let error = runtime()
        .block_on(Api::at(&base, &base).user("bad"))
        .unwrap_err();
    assert!(error.contains("did not accept"), "{error}");
}
