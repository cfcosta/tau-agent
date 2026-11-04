//! Signing in to GitHub from the setup and Models screens: the device
//! code (or a personal access token) through tau-github-host's
//! [`Api`], the sign-in kept in tau's config directory, and the
//! repositories the app reaches for setup's list.

use std::{
    future::Future,
    sync::{Arc, Mutex},
};

use gpui::{App, Entity};
pub use tau_github_host::{Api, Token};
use tau_ui_plugin::forge::ForgeRepo;
use tokio::sync::Notify;

use crate::{
    accounts::Credentials,
    setup::{DeviceCode, GitHub, RepoChoice, SetupUpdate},
    update::HostUpdate,
    workspace::{Workspace, WorkspaceEvent},
};

/// Runs `future` on the interface's runtime, for callers without a host
/// (onboarding has none yet).
pub(crate) fn background<T: Send + 'static>(
    future: impl Future<Output = T> + Send + 'static,
) -> tokio::task::JoinHandle<T> {
    crate::interface_runtime::spawn(future)
}

/// Wakes the device sign-in going on, when the user says they approved.
static WAKE: Mutex<Option<Arc<Notify>>> = Mutex::new(None);

/// Carries out the GitHub sign-ins the workspace asks for. Returns
/// whether `event` was one of them.
pub fn handle(
    event: &WorkspaceEvent,
    workspace: &Entity<Workspace>,
    credentials: &Credentials,
    api: &Api,
    cx: &mut App,
) -> bool {
    match event {
        WorkspaceEvent::GitHubSignIn => {
            let wake = Arc::new(Notify::new());
            *WAKE.lock().expect("not poisoned") = Some(wake.clone());
            let (codes, mut shown) = tokio::sync::mpsc::unbounded_channel();
            let done = background({
                let api = api.clone();
                async move {
                    let show = |start: &tau_github_host::DeviceStart| {
                        drop(codes.send(DeviceCode {
                            code: start.user_code.clone(),
                            url: start.address().to_owned(),
                            expires: format!(
                                "{} minutes",
                                start.expires_in / 60
                            ),
                        }))
                    };
                    api.sign_in(show, &wake).await
                }
            });
            let workspace_for_codes = workspace.downgrade();
            cx.spawn(async move |cx| {
                while let Some(code) = shown.recv().await {
                    let _ = workspace_for_codes.update(cx, |ws, cx| {
                        ws.apply(
                            HostUpdate::Setup(SetupUpdate::GitHub(
                                GitHub::Waiting(code),
                            )),
                            cx,
                        )
                    });
                }
            })
            .detach();
            finish(done, workspace, credentials, api, cx);
            true
        }
        WorkspaceEvent::GitHubCheck => {
            if let Some(wake) = WAKE.lock().expect("not poisoned").as_ref() {
                wake.notify_one();
            }
            true
        }
        WorkspaceEvent::GitHubToken { token } => {
            let token = token.clone();
            let done = background({
                let api = api.clone();
                async move {
                    let user = api.user(&token).await?;
                    Ok(Token {
                        token,
                        user,
                        expires_at: None,
                    })
                }
            });
            finish(done, workspace, credentials, api, cx);
            true
        }
        WorkspaceEvent::GitHubSignOut => {
            let forgotten = background({
                let dir = credentials.dir.clone();
                async move { Token::forget(&dir).await }
            });
            let workspace = workspace.downgrade();
            cx.spawn(async move |cx| {
                let update = match forgotten.await {
                    Ok(Ok(())) => GitHub::SignedOut,
                    Ok(Err(error)) => GitHub::Failed(error.to_string()),
                    Err(error) => GitHub::Failed(error.to_string()),
                };
                let _ = workspace.update(cx, |ws, cx| {
                    ws.apply(
                        HostUpdate::Setup(SetupUpdate::GitHub(update)),
                        cx,
                    );
                    ws.apply(
                        HostUpdate::Setup(SetupUpdate::Repos(Vec::new())),
                        cx,
                    );
                });
            })
            .detach();
            true
        }
        _ => false,
    }
}

/// Once a sign-in is over: save it, say who is signed in, and list the
/// repositories; or say what went wrong.
fn finish(
    done: tokio::task::JoinHandle<Result<Token, String>>,
    workspace: &Entity<Workspace>,
    credentials: &Credentials,
    api: &Api,
    cx: &mut App,
) {
    let (workspace, dir, api) =
        (workspace.downgrade(), credentials.dir.clone(), api.clone());
    // The sign-in, then its save, on the interface's runtime.
    let saved = background(async move {
        let token = done.await.unwrap_or_else(|_| Err("stopped".into()))?;
        token
            .save(&dir)
            .await
            .map(|()| token)
            .map_err(|error| error.to_string())
    });
    cx.spawn(async move |cx| {
        let result = saved.await.unwrap_or_else(|_| Err("stopped".into()));
        let Ok(token) = result else {
            let error = result.err().unwrap_or_default();
            let _ = workspace.update(cx, |ws, cx| {
                ws.apply(
                    HostUpdate::Setup(SetupUpdate::GitHub(GitHub::Failed(
                        error,
                    ))),
                    cx,
                )
            });
            return;
        };
        let _ = workspace.update(cx, |ws, cx| {
            ws.apply(
                HostUpdate::Setup(SetupUpdate::GitHub(GitHub::SignedIn {
                    user: token.user.clone(),
                })),
                cx,
            )
        });
        list_repos(&api, &token, workspace, cx).await;
    })
    .detach();
}

/// Says who is signed in, if anyone, and lists their repositories: for
/// starting with a saved sign-in.
pub fn restore(
    workspace: &Entity<Workspace>,
    credentials: &Credentials,
    api: &Api,
    cx: &mut App,
) {
    let saved = background({
        let dir = credentials.dir.clone();
        async move { Token::load(&dir).await }
    });
    let (workspace, api) = (workspace.downgrade(), api.clone());
    cx.spawn(async move |cx| {
        let Ok(Some(token)) = saved.await else {
            return;
        };
        let _ = workspace.update(cx, |ws, cx| {
            ws.apply(
                HostUpdate::Setup(SetupUpdate::GitHub(GitHub::SignedIn {
                    user: token.user.clone(),
                })),
                cx,
            )
        });
        list_repos(&api, &token, workspace, cx).await
    })
    .detach();
}

async fn list_repos(
    api: &Api,
    token: &Token,
    workspace: gpui::WeakEntity<Workspace>,
    cx: &mut gpui::AsyncApp,
) {
    let repos = background({
        let (api, token) = (api.clone(), token.token.clone());
        async move { api.repos(&token).await }
    })
    .await
    .unwrap_or_else(|_| Err("stopped".into()));
    let _ = workspace.update(cx, |ws, cx| match repos {
        Ok(repos) => {
            let repos = repos.into_iter().map(repo_choice).collect();
            ws.apply(HostUpdate::Setup(SetupUpdate::Repos(repos)), cx)
        }
        Err(error) => {
            eprintln!("tau-ui: cannot list GitHub repositories: {error}")
        }
    });
}

/// A repository on setup's list, not picked yet.
fn repo_choice(repo: ForgeRepo) -> RepoChoice {
    RepoChoice {
        name: repo.name,
        description: repo.description,
        branch: repo.branch,
        selected: false,
        pushed_at: repo.pushed_at,
        remote: repo.remote,
    }
}
