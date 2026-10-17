//! Signing in to GitHub and listing repositories, against a fake GitHub
//! that answers the way the real one does.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

mod support;

use std::sync::{
    Arc,
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

use hegel::{TestCase, generators as gs};
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
    // It says how to fix it: turn the flow on, or use a token.
    assert!(error.contains("turn on Device Flow"), "{error}");
    assert!(error.contains("personal access token"), "{error}");
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
        "GET /user/installations?per_page=100&page=1" => {
            (200, json!({ "installations": [{ "id": 7 }, { "id": 9 }] }))
        }
        "GET /user/installations/7/repositories?per_page=100&page=1" => (
            200,
            json!({ "repositories": [
                { "full_name": "cfcosta/tau-agent", "description": "agents",
                  "default_branch": "main",
                  "pushed_at": "2026-10-04T21:13:07Z" },
                { "full_name": "cfcosta/docbert", "description": null,
                  "default_branch": "trunk",
                  "pushed_at": "2026-09-30T08:00:00Z" },
                { "full_name": "cfcosta/old", "default_branch": "main",
                  "pushed_at": "2024-01-02T03:04:05Z" }
            ]}),
        ),
        "GET /user/installations/9/repositories?per_page=100&page=1" => (
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
    // The most recently pushed first, each once.
    assert_eq!(
        names,
        [
            ("cfcosta/tau-agent", "main"),
            ("cfcosta/docbert", "trunk"),
            ("cfcosta/old", "main"),
        ]
    );
    assert_eq!(repos[0].description, "agents");
}

/// A full page means there may be more: every page is read, until one
/// comes back short, for installations, their repositories and a
/// personal token's repositories alike.
#[test]
fn every_page_of_repositories_is_read() {
    let full: Vec<_> = (0..100)
        .map(|n| json!({ "full_name": format!("cfcosta/old-{n:03}") }))
        .collect();
    let (base, seen) = fake(move |line| match line {
        "GET /user/installations?per_page=100&page=1" => {
            (200, json!({ "installations": [{ "id": 7 }] }))
        }
        "GET /user/installations/7/repositories?per_page=100&page=1" => {
            (200, json!({ "repositories": full.clone() }))
        }
        "GET /user/installations/7/repositories?per_page=100&page=2" => (
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
    assert_eq!(repos.len(), 101);
    assert!(repos.iter().any(|repo| repo.name == "cfcosta/tau-agent"));
    // A short page ends the list: no third page is asked for.
    assert!(seen.lock().unwrap().iter().all(|r| !r.contains("page=3")));

    let (base, _) = fake(|line| match line {
        "GET /user/installations?per_page=100&page=1" => (403, json!({})),
        "GET /user/repos?sort=pushed&per_page=100&page=1" => {
            (
                200,
                json!((0..100)
                .map(|n| json!({ "full_name": format!("octocat/r{n:03}") }))
                .collect::<Vec<_>>()),
            )
        }
        "GET /user/repos?sort=pushed&per_page=100&page=2" => (200, json!([])),
        _ => (404, json!({})),
    });
    let repos = runtime()
        .block_on(Api::at(&base, &base).repos("github_pat_x"))
        .unwrap();
    // A personal token cannot list installations: its own repositories.
    assert_eq!(repos.len(), 100);
    assert_eq!(repos[0].name, "octocat/r000");
}

/// A page that fails is an error, not a shorter list, and a token
/// GitHub refuses is an error that says so.
#[test]
fn a_failed_page_is_an_error() {
    let (base, _) = fake(|line| match line {
        "GET /user/installations?per_page=100&page=1" => {
            (200, json!({ "installations": [{ "id": 7 }] }))
        }
        _ => (502, json!({})),
    });
    let error = runtime()
        .block_on(Api::at(&base, &base).repos("ghu_token"))
        .unwrap_err();
    assert!(error.contains("502"), "{error}");
    // A token GitHub refuses says so.
    let (base, _) = fake(|_| (401, json!({ "message": "Bad credentials" })));
    let error = runtime()
        .block_on(Api::at(&base, &base).user("bad"))
        .unwrap_err();
    assert!(error.contains("did not accept"), "{error}");
}

/// Whether tau's app can push to a repository is GitHub's
/// `permissions.push` for it; one the token does not reach is one it
/// cannot push to, and a refused token is an error.
#[test]
fn can_push_is_what_github_says_of_the_repository() {
    let (base, seen) = fake(|line| match line {
        "GET /repos/octocat/pushes" => (
            200,
            json!({ "full_name": "octocat/pushes",
                    "permissions": { "pull": true, "push": true } }),
        ),
        "GET /repos/octocat/reads" => (
            200,
            json!({ "full_name": "octocat/reads",
                    "permissions": { "pull": true, "push": false } }),
        ),
        "GET /repos/octocat/bare" => (200, json!({ "full_name": "x" })),
        "GET /repos/octocat/hidden" => (404, json!({ "message": "Not Found" })),
        "GET /repos/octocat/refused" => (401, json!({})),
        _ => (502, json!({})),
    });
    let api = Api::at(&base, &base);
    let can = |name: &str| runtime().block_on(api.can_push("ghu_t", name));
    assert_eq!(can("octocat/pushes"), Ok(true));
    assert_eq!(can("octocat/reads"), Ok(false));
    assert_eq!(can("octocat/bare"), Ok(false));
    assert_eq!(can("octocat/hidden"), Ok(false));
    assert!(
        can("octocat/refused")
            .unwrap_err()
            .contains("did not accept")
    );
    assert!(can("octocat/down").unwrap_err().contains("502"));
    let requests = seen.lock().unwrap();
    assert!(requests.iter().all(|r| r.contains("Bearer ghu_t")));
}

/// How an `origin` URL is written.
#[derive(Debug, Clone, hegel::PrettyPrintable)]
enum Form {
    Https,
    Http,
    Git,
    /// `git@github.com:owner/name`.
    Scp,
    /// `ssh://git@github.com/owner/name`, with a port now and then.
    Ssh {
        port: bool,
    },
}

#[derive(Debug, Clone, hegel::PrettyPrintable)]
struct Written {
    owner: String,
    name: String,
    form: Form,
    host: &'static str,
    dot_git: bool,
    slash: bool,
}

#[hegel::composite]
fn written(tc: &TestCase) -> Written {
    let owner = tc
        .draw(gs::from_regex("[A-Za-z0-9][A-Za-z0-9-]{0,38}").fullmatch(true));
    let name = tc.draw(gs::from_regex("[A-Za-z0-9_.-]{1,20}").fullmatch(true));
    // GitHub's names: not `.` or `..`, and `.git` is not part of one.
    let name = match name.as_str() {
        "." | ".." => format!("{name}x"),
        _ if name.ends_with(".git") => format!("{name}x"),
        _ => name,
    };
    let form = match tc.draw(gs::integers::<u8>().max_value(4)) {
        0 => Form::Https,
        1 => Form::Http,
        2 => Form::Git,
        3 => Form::Scp,
        _ => Form::Ssh {
            port: tc.draw(gs::booleans()),
        },
    };
    Written {
        owner,
        name,
        form,
        host: tc.draw(gs::sampled_from(vec![
            "github.com",
            "www.github.com",
            "ssh.github.com",
            "GitHub.com",
            "gitlab.com",
        ])),
        dot_git: tc.draw(gs::booleans()),
        slash: tc.draw(gs::booleans()),
    }
}

impl Written {
    fn url(&self) -> String {
        let Self {
            owner, name, host, ..
        } = self;
        let path = format!(
            "{owner}/{name}{}{}",
            if self.dot_git { ".git" } else { "" },
            if self.slash { "/" } else { "" }
        );
        match self.form {
            Form::Https => format!("https://{host}/{path}"),
            Form::Http => format!("http://{host}/{path}"),
            Form::Git => format!("git://{host}/{path}"),
            Form::Scp => format!("git@{host}:{path}"),
            Form::Ssh { port: false } => format!("ssh://git@{host}/{path}"),
            Form::Ssh { port: true } => format!("ssh://git@{host}:443/{path}"),
        }
    }
}

/// An `origin` written any way Git takes for a GitHub repository maps to
/// that repository's HTTPS URL, which names it again; the GitHub
/// repository is the one inspecting a folder finds
/// (`tau_vcs_host::local::github`); one on another host maps to none.
#[hegel::test(test_cases = 300)]
fn an_origin_on_github_maps_to_its_https_url(tc: TestCase) {
    let written = tc.draw(written());
    let url = written.url();
    tc.note(&url);
    let api = Api::default();
    let mapped = api.https_of(&url);
    let found = tau_vcs_host::local::github(&url)
        .map(|found| format!("{}/{}", found.owner, found.name));
    assert_eq!(mapped.as_ref().map(|(name, _)| name), found.as_ref());
    if written.host == "gitlab.com" {
        assert_eq!(mapped, None);
        return;
    }
    let full_name = format!("{}/{}", written.owner, written.name);
    let (name, https) = mapped.expect("a GitHub origin maps");
    assert_eq!(name, full_name);
    assert_eq!(https, format!("https://github.com/{full_name}.git"));
    // The HTTPS URL names the same repository.
    assert_eq!(api.https_of(&https).map(|(name, _)| name), Some(full_name));
}
