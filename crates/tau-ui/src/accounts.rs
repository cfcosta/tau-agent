//! How tau reaches a model: a ChatGPT sign-in or an OpenAI API key,
//! where each is kept, and signing in and out. Only what the user set up
//! in tau counts; the environment is never read, so tau does not run on
//! whatever key a shell exports.

use std::{
    io,
    path::{Path, PathBuf},
    rc::Rc,
};

use gpui::{App, Entity};
use tau_ai::codex::{
    BrowserLogin,
    CodexCredentials,
    DeviceLogin,
    ORIGINATOR,
    oauth,
};
use tokio::sync::mpsc;

use crate::{
    models::AccessKind,
    setup::{DeviceCode, ModelAccess, SetupUpdate},
    workspace::{Workspace, WorkspaceEvent},
};

/// How the host reaches a model.
#[derive(Clone, PartialEq, Eq)]
pub enum Access {
    /// A ChatGPT sign-in, from this credentials file.
    Codex(PathBuf),
    /// An OpenAI API key.
    ApiKey(String),
}

impl std::fmt::Debug for Access {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Codex(path) => f.debug_tuple("Codex").field(path).finish(),
            Self::ApiKey(_) => f.write_str("ApiKey(..)"),
        }
    }
}

impl Access {
    pub fn kind(&self) -> AccessKind {
        match self {
            Self::Codex(_) => AccessKind::ChatGpt,
            Self::ApiKey(_) => AccessKind::ApiKey,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Codex(_) => "ChatGPT (Codex)",
            Self::ApiKey(_) => "OpenAI API key",
        }
    }

    /// How the model reads in onboarding: `gpt-5.5 · Codex`.
    pub fn short_label(&self, model: &str) -> String {
        match self {
            Self::Codex(_) => format!("{model} · Codex"),
            Self::ApiKey(_) => format!("{model} · API key"),
        }
    }
}

/// Where tau keeps what the user signed in with, each readable only by
/// them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    /// Usually `$XDG_CONFIG_HOME/tau`.
    pub dir: PathBuf,
}

impl Credentials {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// Beside the ChatGPT sign-in, in tau's config directory.
    pub fn default_dir() -> Self {
        let dir = CodexCredentials::default_path()
            .and_then(|path| path.parent().map(Path::to_owned))
            .unwrap_or_else(|| PathBuf::from("."));
        Self::new(dir)
    }

    pub fn codex(&self) -> PathBuf {
        self.dir.join("codex.json")
    }

    pub fn api_key(&self) -> PathBuf {
        self.dir.join("openai-key")
    }

    pub fn github(&self) -> PathBuf {
        self.dir.join("github.json")
    }

    /// Where the TypeSafe key for Jev is kept.
    pub fn jev(&self) -> PathBuf {
        self.dir.join("typesafe-key")
    }

    /// The saved TypeSafe key, which tau-constitution checks with.
    pub fn jev_key(&self) -> Option<String> {
        std::fs::read_to_string(self.jev())
            .ok()
            .map(|key| key.trim().to_owned())
            .filter(|key| !key.is_empty())
    }

    /// Saves the TypeSafe key, or forgets it with `None`.
    pub fn set_jev_key(&self, key: Option<&str>) -> io::Result<()> {
        match key.map(str::trim).filter(|key| !key.is_empty()) {
            Some(key) => write_private(&self.jev(), key.as_bytes()),
            None => match std::fs::remove_file(self.jev()) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => {
                    Err(error)
                }
                _ => Ok(()),
            },
        }
    }

    /// What runs use: the ChatGPT sign-in if saved, else the saved API
    /// key.
    pub fn access(&self) -> Option<Access> {
        if self.has(AccessKind::ChatGpt) {
            return Some(Access::Codex(self.codex()));
        }
        self.saved_key().map(Access::ApiKey)
    }

    /// Whether that kind of access is saved.
    pub fn has(&self, kind: AccessKind) -> bool {
        match kind {
            AccessKind::ChatGpt => self.codex().is_file(),
            AccessKind::ApiKey => self.saved_key().is_some(),
        }
    }

    fn saved_key(&self) -> Option<String> {
        std::fs::read_to_string(self.api_key())
            .ok()
            .map(|key| key.trim().to_owned())
            .filter(|key| !key.is_empty())
    }

    pub fn save_api_key(&self, key: &str) -> io::Result<()> {
        write_private(&self.api_key(), key.trim().as_bytes())
    }

    pub fn save_codex(
        &self,
        credentials: &CodexCredentials,
    ) -> Result<PathBuf, String> {
        let path = self.codex();
        credentials
            .save(&path)
            .map(|()| path)
            .map_err(|error| error.to_string())
    }

    /// Forgets that kind of access. Forgetting what is not saved is
    /// fine.
    pub fn forget(&self, kind: AccessKind) -> io::Result<()> {
        let path = match kind {
            AccessKind::ChatGpt => self.codex(),
            AccessKind::ApiKey => self.api_key(),
        };
        match std::fs::remove_file(path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }
}

/// Writes `bytes` to `path`, readable only by the user.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(bytes)
}

/// Called with the new access once a sign-in or key is saved.
pub type Connected = Rc<dyn Fn(Access, &mut App)>;

/// Carries out a model sign-in the workspace asked for: ChatGPT in the
/// browser or with a device code, or an API key. Reports progress to
/// the workspace's setup, and calls `connected` once the access is
/// saved. Returns whether `event` was one of these.
pub fn handle_sign_in(
    event: &WorkspaceEvent,
    workspace: &Entity<Workspace>,
    credentials: &Credentials,
    model: &str,
    connected: &Connected,
    cx: &mut App,
) -> bool {
    let connect = {
        let (connected, model) = (connected.clone(), model.to_owned());
        move |access: Access, workspace: &Entity<Workspace>, cx: &mut App| {
            let label = access.short_label(&model);
            connected(access, cx);
            workspace.update(cx, |ws, cx| {
                ws.update_setup(
                    SetupUpdate::Model(ModelAccess::Connected { label }),
                    cx,
                )
            });
        }
    };
    match event {
        WorkspaceEvent::CodexSignIn { device } => {
            let browser = (!device).then(|| BrowserLogin::start(ORIGINATOR));
            if let Some(login) = &browser {
                cx.open_url(&login.url);
                let pending = ModelAccess::SigningIn {
                    url: Some(login.url.clone()),
                    device: None,
                };
                workspace.update(cx, |ws, cx| {
                    ws.update_setup(SetupUpdate::Model(pending), cx)
                });
            }
            let (progress, mut updates) = mpsc::unbounded_channel();
            std::thread::spawn(move || {
                let done = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| error.to_string())
                    .and_then(|runtime| {
                        runtime.block_on(sign_in(browser, &progress))
                    });
                let _ = progress.send(SignIn::Done(done));
            });
            let workspace = workspace.downgrade();
            let credentials = credentials.clone();
            cx.spawn(async move |cx| {
                while let Some(update) = updates.recv().await {
                    let Some(workspace) = workspace.upgrade() else {
                        return;
                    };
                    let applied = cx.update(|cx| match update {
                        SignIn::Code(code) => {
                            let pending = ModelAccess::SigningIn {
                                url: None,
                                device: Some(code),
                            };
                            workspace.update(cx, |ws, cx| {
                                ws.update_setup(SetupUpdate::Model(pending), cx)
                            });
                        }
                        SignIn::Done(done) => {
                            match done.and_then(|c| credentials.save_codex(&c))
                            {
                                Ok(path) => {
                                    connect(Access::Codex(path), &workspace, cx)
                                }
                                Err(error) => fail(&workspace, error, cx),
                            }
                        }
                    });
                    if applied.is_err() {
                        return;
                    }
                }
            })
            .detach();
            true
        }
        WorkspaceEvent::ApiKey { key } => {
            match credentials.save_api_key(key) {
                Ok(()) => connect(
                    Access::ApiKey(key.trim().to_owned()),
                    workspace,
                    cx,
                ),
                Err(error) => fail(workspace, error.to_string(), cx),
            }
            true
        }
        _ => false,
    }
}

fn fail(workspace: &Entity<Workspace>, error: String, cx: &mut App) {
    workspace.update(cx, |ws, cx| {
        ws.update_setup(SetupUpdate::Model(ModelAccess::Failed(error)), cx)
    });
}

/// How a sign-in running on its own thread is going.
enum SignIn {
    Code(DeviceCode),
    Done(Result<CodexCredentials, String>),
}

/// Signs in to ChatGPT: waits for the browser, or asks for a device code
/// and reports it before waiting.
async fn sign_in(
    browser: Option<BrowserLogin>,
    progress: &mpsc::UnboundedSender<SignIn>,
) -> Result<CodexCredentials, String> {
    let credentials = match browser {
        Some(login) => login.wait().await,
        None => {
            let login =
                DeviceLogin::start().await.map_err(|e| e.to_string())?;
            let url = oauth::DEVICE_VERIFICATION_URL;
            let _ = progress.send(SignIn::Code(DeviceCode {
                code: login.user_code.clone(),
                url: url.trim_start_matches("https://").to_owned(),
                expires: "15 minutes".into(),
            }));
            login.wait().await
        }
    };
    credentials.map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_comes_only_from_what_was_saved() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = Credentials::new(dir.path());
        assert_eq!(credentials.access(), None);
        std::fs::write(credentials.api_key(), "  \n").unwrap();
        assert_eq!(credentials.access(), None, "a blank key is no key");
        credentials.save_api_key("sk-saved\n").unwrap();
        assert_eq!(
            credentials.access(),
            Some(Access::ApiKey("sk-saved".into()))
        );
        std::fs::write(credentials.codex(), "{}").unwrap();
        assert_eq!(
            credentials.access(),
            Some(Access::Codex(credentials.codex()))
        );
        // Signing out of ChatGPT falls back to the key, then to nothing.
        credentials.forget(AccessKind::ChatGpt).unwrap();
        assert_eq!(
            credentials.access().map(|a| a.kind()),
            Some(AccessKind::ApiKey)
        );
        credentials.forget(AccessKind::ApiKey).unwrap();
        credentials.forget(AccessKind::ApiKey).unwrap();
        assert_eq!(credentials.access(), None);
    }

    #[cfg(unix)]
    #[test]
    fn saved_keys_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let credentials = Credentials::new(dir.path().join("tau"));
        credentials.save_api_key("sk-x").unwrap();
        let mode = std::fs::metadata(credentials.api_key())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
