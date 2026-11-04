//! GitHub's web and API: the device sign-in, the account, the
//! repositories the app reaches, whether it can push to one, pull
//! requests, and their checks.

use std::time::Duration;

use serde_json::{Value, json};
use tau_ui_plugin::forge::{Checks, ForgeRepo, Opened};
use tokio::sync::Notify;

use crate::token::{Token, now_ms};

/// The GitHub App tau signs in through.
pub const CLIENT_ID: &str = "Iv23lisaZLq1FOECQUNe";
/// Items per page of a GitHub list: its maximum.
pub const PER_PAGE: usize = 100;

/// Where GitHub's web and API are: a host service
/// (`tau_ui_plugin::Services`) that points tau-github at a fake GitHub,
/// as in tests. Without it, GitHub's own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    pub web: String,
    pub api: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            web: "https://github.com".into(),
            api: "https://api.github.com".into(),
        }
    }
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

impl DeviceStart {
    /// Where to enter the code, without its scheme:
    /// `github.com/login/device`.
    pub fn address(&self) -> &str {
        self.verification_uri
            .trim_start_matches("https://")
            .trim_start_matches("http://")
    }
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
        Self::of(&Endpoints::default())
    }
}

impl Api {
    /// GitHub at `endpoints`.
    pub fn of(endpoints: &Endpoints) -> Self {
        Self::at(&endpoints.web, &endpoints.api)
    }

    pub fn at(web: &str, api: &str) -> Self {
        let http = reqwest::Client::builder()
            .tls_backend_preconfigured(tau_ai::http::tls_config())
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

    /// The GitHub repository a Git URL names, as `owner/name`, and its
    /// HTTPS URL, which tau pushes to with the sign-in's token: an SSH
    /// URL (`git@github.com:owner/name.git`, `ssh://…`) maps to it too.
    /// None when the URL is not on GitHub ([`crate::url::repository_of`]).
    pub fn https_of(&self, url: &str) -> Option<(String, String)> {
        let full_name = crate::url::repository_of(url)?.full_name();
        let url = self.clone_url(&full_name);
        Some((full_name, url))
    }

    /// Whether tau's GitHub App can push to `full_name` (`owner/name`)
    /// with `token`: GitHub's `permissions.push` for the repository. A
    /// repository the token does not reach (404, 403) is one it cannot
    /// push to.
    pub async fn can_push(
        &self,
        token: &str,
        full_name: &str,
    ) -> Result<bool, String> {
        let (status, value) =
            self.get(token, &format!("/repos/{full_name}")).await?;
        match status {
            200..300 => {
                Ok(value["permissions"]["push"].as_bool() == Some(true))
            }
            403 | 404 => Ok(false),
            401 => Err("GitHub did not accept the token.".into()),
            _ => Err(format!("GitHub answered {status} for {full_name}")),
        }
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

    /// Signs in with a device code: shows the code, then polls until
    /// the user approves (or `wake` says they did), and returns the
    /// token with its account.
    pub async fn sign_in(
        &self,
        show: impl Fn(&DeviceStart),
        wake: &Notify,
    ) -> Result<Token, String> {
        let start = self.start_device().await?;
        show(&start);
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

    /// Every item of a paged list: `path` page by page, [`PER_PAGE`] at
    /// a time, until a page comes back short. `key` names the array in
    /// each page, or `None` when the page is the array. `Err` carries
    /// the status of a page that failed.
    async fn all_pages(
        &self,
        token: &str,
        path: &str,
        key: Option<&str>,
    ) -> Result<Result<Vec<Value>, u16>, String> {
        let join = if path.contains('?') { '&' } else { '?' };
        let mut items = Vec::new();
        for page in 1.. {
            let (status, value) = self
                .get(
                    token,
                    &format!("{path}{join}per_page={PER_PAGE}&page={page}"),
                )
                .await?;
            if status != 200 {
                return Ok(Err(status));
            }
            let list = match key {
                Some(key) => value.get(key),
                None => Some(&value),
            }
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
            let short = list.len() < PER_PAGE;
            items.extend(list);
            if short {
                break;
            }
        }
        Ok(Ok(items))
    }

    /// The repositories the token reaches: those the app is installed
    /// on, or, for a personal token, the user's. Every page of each.
    pub async fn repos(&self, token: &str) -> Result<Vec<ForgeRepo>, String> {
        let mut found = Vec::new();
        match self
            .all_pages(token, "/user/installations", Some("installations"))
            .await?
        {
            Ok(installations) => {
                let ids: Vec<u64> = installations
                    .iter()
                    .filter_map(|installation| installation.get("id")?.as_u64())
                    .collect();
                for id in ids {
                    let repositories = self
                        .all_pages(
                            token,
                            &format!("/user/installations/{id}/repositories"),
                            Some("repositories"),
                        )
                        .await?
                        .map_err(|status| {
                            format!(
                                "GitHub answered {status} for the \
                                 repositories of installation {id}"
                            )
                        })?;
                    found.extend(repositories.iter().filter_map(forge_repo));
                }
            }
            // Personal tokens cannot list app installations.
            Err(_) => {
                let repositories = self
                    .all_pages(token, "/user/repos?sort=pushed", None)
                    .await?
                    .map_err(|status| {
                        format!(
                            "GitHub answered {status} for your repositories"
                        )
                    })?;
                found.extend(repositories.iter().filter_map(forge_repo));
            }
        }
        // One of each, the most recently pushed first.
        found.sort_by(|a, b| a.name.cmp(&b.name));
        found.dedup_by(|a, b| a.name == b.name);
        found.sort_by(|a, b| {
            b.pushed_at
                .cmp(&a.pushed_at)
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(found)
    }

    /// Sends `body` as JSON to the API with `method`, and returns the
    /// status and the answer.
    async fn send(
        &self,
        method: reqwest::Method,
        token: &str,
        path: &str,
        body: &Value,
    ) -> Result<(u16, Value), String> {
        let response = self
            .http
            .request(method, format!("{}{path}", self.api))
            .header("Accept", "application/vnd.github+json")
            .header("Authorization", format!("Bearer {token}"))
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("Content-Type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(|error| format!("Cannot reach GitHub: {error}"))?;
        let status = response.status().as_u16();
        let bytes = response
            .bytes()
            .await
            .map_err(|error| format!("Cannot read GitHub's answer: {error}"))?;
        Ok((
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        ))
    }

    /// `send`, expecting a success, which it returns the answer of;
    /// `what` says what failed.
    async fn expect(
        &self,
        method: reqwest::Method,
        token: &str,
        path: &str,
        body: &Value,
        what: &str,
    ) -> Result<Value, String> {
        let (status, value) = self.send(method, token, path, body).await?;
        if (200..300).contains(&status) {
            return Ok(value);
        }
        let message = value["message"].as_str().unwrap_or("no reason given");
        Err(match status {
            403 | 404 => format!(
                "GitHub refused to {what} ({status}: {message}). tau's app \
                 needs contents and pull requests write access to the \
                 repository."
            ),
            _ => format!("GitHub could not {what} ({status}: {message})"),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn open_pull(
        &self,
        token: &str,
        repo: &str,
        title: &str,
        body: &str,
        head: &str,
        base: &str,
        draft: bool,
    ) -> Result<Opened, String> {
        let value = self
            .expect(
                reqwest::Method::POST,
                token,
                &format!("/repos/{repo}/pulls"),
                &json!({
                    "title": title, "body": body, "head": head,
                    "base": base, "draft": draft,
                }),
                "open the pull request",
            )
            .await?;
        Ok(Opened {
            number: value["number"].as_u64().unwrap_or_default(),
            url: value["html_url"].as_str().unwrap_or_default().to_owned(),
        })
    }

    pub async fn request_reviewers(
        &self,
        token: &str,
        repo: &str,
        number: u64,
        reviewers: &[String],
    ) -> Result<(), String> {
        if reviewers.is_empty() {
            return Ok(());
        }
        self.expect(
            reqwest::Method::POST,
            token,
            &format!("/repos/{repo}/pulls/{number}/requested_reviewers"),
            &json!({ "reviewers": reviewers }),
            "ask for reviews",
        )
        .await
        .map(drop)
    }

    /// How the check runs on `commit` stand.
    pub async fn checks(
        &self,
        token: &str,
        repo: &str,
        commit: &str,
    ) -> Result<Checks, String> {
        let (status, value) = self
            .get(token, &format!("/repos/{repo}/commits/{commit}/check-runs"))
            .await?;
        if status != 200 {
            return Err(format!("GitHub answered {status} for the checks"));
        }
        let runs = value["check_runs"].as_array().cloned().unwrap_or_default();
        if runs.is_empty() {
            return Ok(Checks::None);
        }
        let failed = runs.iter().any(|run| {
            matches!(
                run["conclusion"].as_str(),
                Some("failure" | "timed_out" | "cancelled" | "action_required")
            )
        });
        let done = runs.iter().all(|run| run["status"] == "completed");
        Ok(if failed {
            Checks::Failed
        } else if done {
            Checks::Passed
        } else {
            Checks::Running
        })
    }
}

/// A repository as GitHub lists it.
fn forge_repo(repo: &Value) -> Option<ForgeRepo> {
    Some(ForgeRepo {
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
        pushed_at: repo
            .get("pushed_at")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        remote: repo
            .get("clone_url")
            .and_then(Value::as_str)
            .map(str::to_owned),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every value encodes to unreserved characters and `%XX` escapes
    /// only, escaping exactly the bytes that are not unreserved, and
    /// decodes back to itself.
    #[hegel::test(test_cases = 300)]
    fn a_form_value_decodes_to_itself(tc: hegel::TestCase) {
        use hegel::generators as gs;
        // Any text, and text of the characters a grant type and ids are
        // made of, unreserved and not.
        let value: String = tc.draw(hegel::one_of!(
            gs::text().max_size(40),
            gs::text().alphabet("aZ09-_.~:/ %+&=é").max_size(40),
        ));
        let unreserved =
            |byte: u8| byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte);
        let encoded = encode(&value);
        let mut bytes = Vec::new();
        let mut rest = encoded.as_bytes();
        while let Some((&byte, tail)) = rest.split_first() {
            if byte == b'%' {
                let hex = std::str::from_utf8(&tail[..2]).unwrap();
                assert_eq!(hex, hex.to_uppercase(), "{encoded}");
                let byte = u8::from_str_radix(hex, 16).unwrap();
                assert!(!unreserved(byte), "needless escape in {encoded}");
                bytes.push(byte);
                rest = &tail[2..];
            } else {
                assert!(unreserved(byte), "{encoded}");
                bytes.push(byte);
                rest = tail;
            }
        }
        assert_eq!(String::from_utf8(bytes).unwrap(), value);
    }
}
