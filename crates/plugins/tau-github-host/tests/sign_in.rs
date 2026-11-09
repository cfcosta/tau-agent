//! What tau-github's UI asks of its host half, against a fake GitHub: a
//! token is checked and saved, answered with who signed in, and the
//! plugin's data says so; a refused one says why; signing out forgets
//! it. Each change asks the interface to draw again.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use serde_json::json;
use support::fake;
use tau_github::{Act, GitHubData, SignIn, Signed};
use tau_github_host::{Endpoints, GitHub, GitHubHost, Token};
use tau_ui_plugin::{HostCx, HostHalf, Push, Services};

use crate::support;

#[test]
fn a_token_signs_in_and_signing_out_forgets_it() {
    let (base, _) = fake(|line| match line {
        "GET /user" => (200, json!({ "login": "octocat" })),
        _ => (404, json!({})),
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let github = GitHub::new(
        dir.path(),
        &Endpoints {
            web: base.clone(),
            api: base,
        },
    );
    let refreshed = Arc::new(AtomicUsize::new(0));
    let counted = refreshed.clone();
    let store = runtime.block_on(tau_store_sqlite::memory()).unwrap();
    let cx = HostCx::new(
        store,
        runtime.handle().clone(),
        Services::default(),
        dir.path().to_owned(),
        Vec::new(),
        Arc::new(move |push: Push| {
            if matches!(push, Push::Catalog) {
                counted.fetch_add(1, Ordering::SeqCst);
            }
        }),
    );
    let data = |github: &GitHub| -> GitHubData {
        runtime.block_on(GitHubHost.data(github, &cx))
    };
    assert_eq!(data(&github).sign_in, SignIn::SignedOut);

    let act = |act: Act| {
        let action = serde_json::to_value(act).unwrap();
        runtime
            .block_on(GitHubHost.act(&github, action, &cx))
            .unwrap()
    };
    let reply = act(Act::Token {
        token: "github_pat_x".into(),
    });
    assert_eq!(
        reply,
        Some(
            serde_json::to_value(Signed {
                user: "octocat".into()
            })
            .unwrap()
        )
    );
    assert_eq!(
        data(&github).sign_in,
        SignIn::SignedIn {
            user: "octocat".into()
        }
    );
    let saved = runtime.block_on(Token::load(dir.path())).unwrap();
    assert_eq!(
        (saved.token.as_str(), saved.expires_at),
        ("github_pat_x", None)
    );
    assert!(
        refreshed.load(Ordering::SeqCst) >= 2,
        "checking, then signed in"
    );

    assert_eq!(act(Act::SignOut), None);
    assert_eq!(data(&github).sign_in, SignIn::SignedOut);
    assert!(!Token::file(dir.path()).exists());
}

#[test]
fn a_refused_token_says_why() {
    let (base, _) = fake(|line| match line {
        "GET /user" => (401, json!({ "message": "Bad credentials" })),
        _ => (404, json!({})),
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let github = GitHub::new(
        dir.path(),
        &Endpoints {
            web: base.clone(),
            api: base,
        },
    );
    let store = runtime.block_on(tau_store_sqlite::memory()).unwrap();
    let cx = HostCx::new(
        store,
        runtime.handle().clone(),
        Services::default(),
        dir.path().to_owned(),
        Vec::new(),
        Arc::new(|_| {}),
    );
    let action = serde_json::to_value(Act::Token {
        token: "nope".into(),
    })
    .unwrap();
    let reply = runtime
        .block_on(GitHubHost.act(&github, action, &cx))
        .unwrap();
    assert_eq!(reply, None);
    let data = runtime.block_on(GitHubHost.data(&github, &cx));
    assert_eq!(
        data.sign_in,
        SignIn::Failed {
            error: "GitHub did not accept the token.".into()
        }
    );
    assert!(runtime.block_on(Token::load(dir.path())).is_none());
}
