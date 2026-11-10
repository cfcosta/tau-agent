//! A Git URL names a forge's repository exactly when it is one of the
//! forms Git and the forge take for one: GitHub's, as tau-github's
//! `LuauForge` reads them.

use hegel::{TestCase, generators as gs};
use tau_luau_plugins_host::slots::repository_of;

/// A URL, and the GitHub repository it names, when it names one.
#[derive(Debug, Clone, hegel::PrettyPrintable)]
struct Url {
    url: String,
    github: Option<(String, String)>,
}

/// One of `n` cases.
fn case(tc: &TestCase, n: u8) -> u8 {
    tc.draw(gs::sampled_from((0..n).collect::<Vec<_>>()))
}

/// A GitHub owner: letters, digits and inner hyphens.
#[hegel::composite]
fn owner(tc: &TestCase) -> String {
    tc.draw(
        gs::from_regex("[A-Za-z0-9]([A-Za-z0-9-]{0,6}[A-Za-z0-9])?")
            .fullmatch(true),
    )
}

/// A GitHub repository name, never `.`, `..` or ending in `.git`.
#[hegel::composite]
fn repo_name(tc: &TestCase) -> String {
    let name: String = tc.draw(
        gs::from_regex("[A-Za-z0-9_][A-Za-z0-9._-]{0,9}").fullmatch(true),
    );
    if name.ends_with(".git") {
        format!("{name}x")
    } else {
        name
    }
}

/// A URL on GitHub in one of the forms Git takes, or one that is not on
/// GitHub, with what it names.
#[hegel::composite]
fn url(tc: &TestCase) -> Url {
    let owner = tc.draw(owner());
    let name = tc.draw(repo_name());
    let suffix = if tc.draw(gs::booleans()) { ".git" } else { "" };
    let path = format!("{owner}/{name}{suffix}");
    let host = tc.draw(gs::sampled_from(vec![
        "github.com",
        "GitHub.com",
        "www.github.com",
    ]));
    let on_github = |url: String| Url {
        url,
        github: Some((owner.clone(), name.clone())),
    };
    let elsewhere = |url: String| Url { url, github: None };
    match case(tc, 12) {
        0 => {
            let slash = if tc.draw(gs::booleans()) { "/" } else { "" };
            on_github(format!("https://{host}/{path}{slash}"))
        }
        1 => on_github(format!("https://x-access-token:t0k@{host}/{path}")),
        2 => on_github(format!("http://{host}/{path}")),
        3 => on_github(format!("git://github.com/{path}")),
        4 => on_github(format!("git@github.com:{path}")),
        5 => on_github(format!("ssh://git@github.com/{path}")),
        6 => on_github(format!("ssh://git@ssh.github.com:443/{path}")),
        7 => on_github(format!("org-1@{host}:{path}")),
        8 => {
            let other = tc.draw(gs::sampled_from(vec![
                "gitlab.com",
                "github.com.evil.example",
                "notgithub.com",
                "github.co",
                "gist.github.com",
            ]));
            elsewhere(format!("https://{other}/{path}"))
        }
        9 => elsewhere(format!("https://github.com/{path}/extra")),
        10 => elsewhere(format!("https://github.com/{owner}")),
        _ => {
            let local = tc.draw(gs::sampled_from(vec![
                "/srv/github.com/",
                "file:///github.com/",
                "../",
            ]));
            elsewhere(format!("{local}{path}"))
        }
    }
}

/// An `origin` URL names a GitHub repository exactly when it is one of
/// the forms Git takes for one: HTTPS, HTTP, `git://`, SSH and
/// `user@host:owner/name`, on `github.com`, with or without `.git`.
#[hegel::test(test_cases = 500)]
fn a_url_names_a_github_repository_in_any_form(tc: TestCase) {
    let url = tc.draw(url());
    let expected = url
        .github
        .clone()
        .map(|(owner, name)| format!("{owner}/{name}"));
    assert_eq!(
        repository_of("github.com", &url.url),
        expected,
        "{}",
        url.url
    );
}
