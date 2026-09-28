//! GitHub through tau's GitHub App: signing in with a device code (or a
//! personal access token), the account, and the repositories the app can
//! reach. The token stays in tau's config directory, readable only by
//! the user.
//!
//! A device sign-in needs no secret, so none ships with tau. Tokens from
//! the app expire after eight hours, and renewing one needs the app's
//! secret, so an expired token counts as signed out: sign in again.

use std::{
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};

use gpui::{App, Entity};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{Notify, oneshot};
use tokio_rustls::rustls::{ClientConfig, RootCertStore, crypto::ring};

use crate::{
    accounts::{Credentials, write_private},
    setup::{DeviceCode, GitHub, RepoChoice, SetupUpdate},
    workspace::{Workspace, WorkspaceEvent},
};

/// The GitHub App tau signs in through.
pub const CLIENT_ID: &str = "Iv23lisaZLq1FOECQUNe";
pub const APP_SLUG: &str = "ascend-repository-cfcosta";

/// Where to give the app access to more repositories.
pub fn install_url() -> String {
    format!("https://github.com/apps/{APP_SLUG}/installations/new")
}

/// A saved sign-in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Token {
    pub token: String,
    /// The login it belongs to.
    pub user: String,
    /// When it stops working, in Unix milliseconds; `None` for tokens
    /// that do not expire.
    #[serde(default)]
    pub expires_at: Option<u64>,
}

impl Token {
    pub fn is_expired(&self, now_ms: u64) -> bool {
        self.expires_at.is_some_and(|at| now_ms >= at)
    }

    /// The saved sign-in, unless it expired.
    pub fn load(credentials: &Credentials) -> Option<Self> {
        let text = std::fs::read_to_string(credentials.github()).ok()?;
        let token: Self = serde_json::from_str(&text).ok()?;
        (!token.is_expired(now_ms())).then_some(token)
    }

    pub fn save(&self, credentials: &Credentials) -> std::io::Result<()> {
        let text = serde_json::to_vec_pretty(self)?;
        write_private(&credentials.github(), &text)
    }

    pub fn forget(credentials: &Credentials) -> std::io::Result<()> {
        match std::fs::remove_file(credentials.github()) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                Err(error)
            }
            _ => Ok(()),
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

/// A device sign-in GitHub started: the code to show, and the one to
/// poll with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceStart {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    /// Seconds to wait between polls.
    pub interval: u64,
}

/// What a poll of a device sign-in found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Poll {
    /// The user has not approved yet.
    Pending,
    /// Polling too often: wait five more seconds each time.
    SlowDown,
    Done {
        token: String,
        /// Seconds the token lasts, if it expires.
        expires_in: Option<u64>,
    },
    Failed(String),
}

/// GitHub's web and API, at their usual addresses or at a fake server's.
#[derive(Clone)]
pub struct Api {
    http: reqwest::Client,
    web: String,
    api: String,
}

impl Default for Api {
    fn default() -> Self {
        Self::at("https://github.com", "https://api.github.com")
    }
}

impl Api {
    pub fn at(web: &str, api: &str) -> Self {
        // rustls with ring and Mozilla's roots, as for OpenAI: no
        // system certificate store.
        let roots = RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        let tls = ClientConfig::builder_with_provider(Arc::new(
            ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("ring supports the default protocol versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
        let http = reqwest::Client::builder()
            .tls_backend_preconfigured(tls)
            .user_agent("tau")
            .timeout(Duration::from_secs(30))
            .build()
            .expect("a static client configuration builds");
        Self {
            http,
            web: web.trim_end_matches('/').to_owned(),
            api: api.trim_end_matches('/').to_owned(),
        }
    }

    /// Where a repository clones from.
    pub fn clone_url(&self, full_name: &str) -> String {
        format!("{}/{full_name}.git", self.web)
    }

    async fn post_form(
        &self,
        path: &str,
        fields: &[(&str, &str)],
    ) -> Result<Value, String> {
        let body = fields
            .iter()
            .map(|(name, value)| format!("{name}={}", encode(value)))
            .collect::<Vec<_>>()
            .join("&");
        let response = self
            .http
            .post(format!("{}{path}", self.web))
            .header("Accept", "application/json")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|error| format!("Cannot reach GitHub: {error}"))?;
        let bytes = response
            .bytes()
            .await
            .map_err(|error| format!("Cannot read GitHub's answer: {error}"))?;
        serde_json::from_slice(&bytes)
            .map_err(|error| format!("GitHub answered oddly: {error}"))
    }

    async fn get(
        &self,
        token: &str,
        path: &str,
    ) -> Result<(u16, Value), String> {
        let response = self
            .http
            .get(format!("{}{path}", self.api))
            .header("Accept", "application/vnd.github+json")
            .header("Authorization", format!("Bearer {token}"))
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .map_err(|error| format!("Cannot reach GitHub: {error}"))?;
        let status = response.status().as_u16();
        let bytes = response
            .bytes()
            .await
            .map_err(|error| format!("Cannot read GitHub's answer: {error}"))?;
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Ok((status, value))
    }

    pub async fn start_device(&self) -> Result<DeviceStart, String> {
        let value = self
            .post_form("/login/device/code", &[("client_id", CLIENT_ID)])
            .await?;
        if let Some(error) = value.get("error").and_then(Value::as_str) {
            return Err(explain(error, &value));
        }
        let text = |name: &str| {
            value
                .get(name)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| format!("GitHub sent no `{name}`"))
        };
        let number = |name: &str, default| {
            value.get(name).and_then(Value::as_u64).unwrap_or(default)
        };
        Ok(DeviceStart {
            device_code: text("device_code")?,
            user_code: text("user_code")?,
            verification_uri: text("verification_uri")?,
            expires_in: number("expires_in", 900),
            interval: number("interval", 5),
        })
    }

    pub async fn poll(&self, device_code: &str) -> Result<Poll, String> {
        let value = self
            .post_form(
                "/login/oauth/access_token",
                &[
                    ("client_id", CLIENT_ID),
                    ("device_code", device_code),
                    (
                        "grant_type",
                        "urn:ietf:params:oauth:grant-type:device_code",
                    ),
                ],
            )
            .await?;
        Ok(match value.get("error").and_then(Value::as_str) {
            Some("authorization_pending") => Poll::Pending,
            Some("slow_down") => Poll::SlowDown,
            Some(error) => Poll::Failed(explain(error, &value)),
            None => match value.get("access_token").and_then(Value::as_str) {
                Some(token) => Poll::Done {
                    token: token.to_owned(),
                    expires_in: value.get("expires_in").and_then(Value::as_u64),
                },
                None => Poll::Failed("GitHub sent no token".into()),
            },
        })
    }

    /// The login a token belongs to.
    pub async fn user(&self, token: &str) -> Result<String, String> {
        let (status, value) = self.get(token, "/user").await?;
        if status == 401 {
            return Err("GitHub did not accept the token.".into());
        }
        value
            .get("login")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("GitHub answered {status} for the account"))
    }

    /// Signs in with a device code: reports the code, then polls until
    /// the user approves (or `wake` says they did), and returns the
    /// token with its account.
    pub async fn sign_in(
        &self,
        show: impl Fn(DeviceCode),
        wake: &Notify,
    ) -> Result<Token, String> {
        let start = self.start_device().await?;
        show(DeviceCode {
            code: start.user_code.clone(),
            url: start
                .verification_uri
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .to_owned(),
            expires: format!("{} minutes", start.expires_in / 60),
        });
        let deadline = now_ms() + start.expires_in * 1000;
        let mut interval = Duration::from_secs(start.interval.max(1));
        loop {
            // The user saying they approved checks right away.
            let _ = tokio::time::timeout(interval, wake.notified()).await;
            if now_ms() >= deadline {
                return Err("The code expired. Ask for a new one.".into());
            }
            match self.poll(&start.device_code).await? {
                Poll::Pending => {}
                Poll::SlowDown => interval += Duration::from_secs(5),
                Poll::Failed(error) => return Err(error),
                Poll::Done { token, expires_in } => {
                    let user = self.user(&token).await?;
                    return Ok(Token {
                        token,
                        user,
                        expires_at: expires_in.map(|s| now_ms() + s * 1000),
                    });
                }
            }
        }
    }

    /// The repositories the token reaches: those the app is installed
    /// on, or, for a personal token, the user's.
    pub async fn repos(&self, token: &str) -> Result<Vec<RepoChoice>, String> {
        let (status, installations) =
            self.get(token, "/user/installations?per_page=100").await?;
        let mut found = Vec::new();
        if status == 200 {
            let ids = installations
                .get("installations")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|installation| installation.get("id")?.as_u64());
            for id in ids.collect::<Vec<_>>() {
                let (_, page) = self
                    .get(
                        token,
                        &format!(
                            "/user/installations/{id}/repositories?per_page=100"
                        ),
                    )
                    .await?;
                found.extend(
                    page.get("repositories")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(repo_choice),
                );
            }
        } else {
            let (status, page) = self
                .get(token, "/user/repos?per_page=100&sort=pushed")
                .await?;
            if status != 200 {
                return Err(format!(
                    "GitHub answered {status} for your repositories"
                ));
            }
            found.extend(
                page.as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(repo_choice),
            );
        }
        found.sort_by(|a, b| a.name.cmp(&b.name));
        found.dedup_by(|a, b| a.name == b.name);
        Ok(found)
    }
}

fn repo_choice(repo: &Value) -> Option<RepoChoice> {
    Some(RepoChoice {
        name: repo.get("full_name")?.as_str()?.to_owned(),
        description: repo
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        branch: repo
            .get("default_branch")
            .and_then(Value::as_str)
            .unwrap_or("main")
            .to_owned(),
        selected: false,
    })
}

/// A GitHub OAuth error, in words.
fn explain(error: &str, value: &Value) -> String {
    match error {
        "device_flow_disabled" => "tau's GitHub App does not allow signing \
            in with a code yet: turn on Device Flow in the app's settings, \
            or use a personal access token."
            .into(),
        "expired_token" => "The code expired. Ask for a new one.".into(),
        "access_denied" => "The sign-in was cancelled on GitHub.".into(),
        _ => value
            .get("error_description")
            .and_then(Value::as_str)
            .map_or_else(|| format!("GitHub said {error}"), str::to_owned),
    }
}

/// `application/x-www-form-urlencoded` for one value.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'~' => (byte as char).to_string(),
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// Runs `future` on a thread of its own, for callers without a tokio
/// runtime (onboarding has none yet).
pub(crate) fn background<T: Send + 'static>(
    future: impl Future<Output = T> + Send + 'static,
) -> oneshot::Receiver<T> {
    let (done, receiver) = oneshot::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a current-thread runtime builds");
        let _ = done.send(runtime.block_on(future));
    });
    receiver
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
                    api.sign_in(|code| drop(codes.send(code)), &wake).await
                }
            });
            let workspace_for_codes = workspace.downgrade();
            cx.spawn(async move |cx| {
                while let Some(code) = shown.recv().await {
                    let _ = workspace_for_codes.update(cx, |ws, cx| {
                        ws.update_setup(
                            SetupUpdate::GitHub(GitHub::Waiting(code)),
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
            let update = match Token::forget(credentials) {
                Ok(()) => GitHub::SignedOut,
                Err(error) => GitHub::Failed(error.to_string()),
            };
            workspace.update(cx, |ws, cx| {
                ws.update_setup(SetupUpdate::GitHub(update), cx);
                ws.update_setup(SetupUpdate::Repos(Vec::new()), cx);
            });
            true
        }
        _ => false,
    }
}

/// Once a sign-in is over: save it, say who is signed in, and list the
/// repositories; or say what went wrong.
fn finish(
    done: oneshot::Receiver<Result<Token, String>>,
    workspace: &Entity<Workspace>,
    credentials: &Credentials,
    api: &Api,
    cx: &mut App,
) {
    let (workspace, credentials, api) =
        (workspace.downgrade(), credentials.clone(), api.clone());
    cx.spawn(async move |cx| {
        let result = done.await.unwrap_or_else(|_| Err("stopped".into()));
        let result = result.and_then(|token| {
            token
                .save(&credentials)
                .map(|()| token)
                .map_err(|error| error.to_string())
        });
        let Ok(token) = result else {
            let error = result.err().unwrap_or_default();
            let _ = workspace.update(cx, |ws, cx| {
                ws.update_setup(SetupUpdate::GitHub(GitHub::Failed(error)), cx)
            });
            return;
        };
        let _ = workspace.update(cx, |ws, cx| {
            ws.update_setup(
                SetupUpdate::GitHub(GitHub::SignedIn {
                    user: token.user.clone(),
                }),
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
    let Some(token) = Token::load(credentials) else {
        return;
    };
    workspace.update(cx, |ws, cx| {
        ws.update_setup(
            SetupUpdate::GitHub(GitHub::SignedIn {
                user: token.user.clone(),
            }),
            cx,
        )
    });
    let (workspace, api) = (workspace.downgrade(), api.clone());
    cx.spawn(async move |cx| list_repos(&api, &token, workspace, cx).await)
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
        Ok(repos) => ws.update_setup(SetupUpdate::Repos(repos), cx),
        Err(error) => {
            eprintln!("tau-ui: cannot list GitHub repositories: {error}")
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_values_are_encoded() {
        assert_eq!(
            encode("urn:ietf:params:oauth:grant-type:device_code"),
            "urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code"
        );
        assert_eq!(encode("Iv23-a_b.c~"), "Iv23-a_b.c~");
    }

    #[test]
    fn expired_sign_ins_are_not_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = Credentials::new(dir.path());
        let token = Token {
            token: "ghu_x".into(),
            user: "octocat".into(),
            expires_at: Some(now_ms() + 60_000),
        };
        token.save(&credentials).unwrap();
        assert_eq!(Token::load(&credentials), Some(token.clone()));
        Token {
            expires_at: Some(1),
            ..token
        }
        .save(&credentials)
        .unwrap();
        assert_eq!(Token::load(&credentials), None);
        Token::forget(&credentials).unwrap();
        Token::forget(&credentials).unwrap();
        assert!(!credentials.github().exists());
    }

    #[test]
    fn a_disabled_device_flow_says_how_to_fix_it() {
        let text = explain("device_flow_disabled", &Value::Null);
        assert!(text.contains("Device Flow"), "{text}");
        assert!(text.contains("personal access token"), "{text}");
    }
}
