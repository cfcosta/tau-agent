//! Signing in, as tau-github's UI asks ([`Act`]): a device code polled
//! until the person approves, or a personal access token; the sign-in
//! saved in tau's config directory, and forgotten on sign-out. How it
//! stands is the plugin's data ([`SignIn`]); each change asks the
//! interface to draw it again.

use std::sync::{
    Arc,
    Mutex,
    atomic::{AtomicU64, Ordering},
};

use serde_json::Value;
use tau_github::{Act, DeviceCode, SignIn, Signed};
use tau_ui_plugin::HostCx;
use tokio::sync::Notify;

use crate::{DeviceStart, GitHub, Token};

/// A sign-in going on, or the last one that failed.
#[derive(Default)]
pub(crate) struct Flow {
    /// How it stands while it goes on or failed; `None` reads the saved
    /// sign-in.
    progress: Mutex<Option<SignIn>>,
    /// Wakes the device sign-in's poll when the person says they
    /// approved.
    wake: Mutex<Option<Arc<Notify>>>,
    /// Counts sign-ins started: a device sign-in a later one replaced
    /// says nothing once it ends.
    attempt: AtomicU64,
}

impl GitHub {
    /// How the sign-in stands: one going on or failed, else the saved
    /// one.
    pub async fn sign_in_state(&self) -> SignIn {
        let progress = self.flow.progress.lock().expect("not poisoned").clone();
        if let Some(progress) = progress {
            return progress;
        }
        match Token::load(self.dir()).await {
            Some(token) => SignIn::SignedIn { user: token.user },
            None => SignIn::SignedOut,
        }
    }

    /// Carries out what the UI asked. A saved sign-in answers
    /// [`Signed`].
    pub async fn act(
        &self,
        act: Act,
        cx: &HostCx,
    ) -> anyhow::Result<Option<Value>> {
        match act {
            Act::SignIn => {
                self.start_device(cx);
                Ok(None)
            }
            Act::Check => {
                if let Some(wake) =
                    self.flow.wake.lock().expect("not poisoned").as_ref()
                {
                    wake.notify_one();
                }
                Ok(None)
            }
            Act::Token { token } => {
                let attempt = self.next_attempt();
                self.show(attempt, Some(SignIn::Checking), cx);
                let checked = self.api().user(&token).await.map(|user| Token {
                    token,
                    user,
                    expires_at: None,
                });
                Ok(self.finish(attempt, checked, cx).await)
            }
            Act::SignOut => {
                let attempt = self.next_attempt();
                let forgotten = Token::forget(self.dir()).await;
                let progress = forgotten.err().map(|error| SignIn::Failed {
                    error: error.to_string(),
                });
                self.show(attempt, progress, cx);
                Ok(None)
            }
        }
    }

    /// Starts a sign-in, which stops any going on.
    fn next_attempt(&self) -> u64 {
        *self.flow.wake.lock().expect("not poisoned") = None;
        self.flow.attempt.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Says how sign-in `attempt` stands, unless a later one started.
    fn show(&self, attempt: u64, progress: Option<SignIn>, cx: &HostCx) {
        if self.flow.attempt.load(Ordering::SeqCst) != attempt {
            return;
        }
        *self.flow.progress.lock().expect("not poisoned") = progress;
        cx.refresh();
    }

    /// Asks GitHub for a device code, shows it, and polls in the
    /// background until the person approves, the code expires or a
    /// later sign-in starts.
    fn start_device(&self, cx: &HostCx) {
        let attempt = self.next_attempt();
        let wake = Arc::new(Notify::new());
        *self.flow.wake.lock().expect("not poisoned") = Some(wake.clone());
        self.show(attempt, Some(SignIn::Starting), cx);
        let (github, runtime, cx) =
            (self.clone(), cx.runtime.clone(), cx.clone());
        runtime.spawn(async move {
            let show = |start: &DeviceStart| {
                let code = DeviceCode {
                    code: start.user_code.clone(),
                    url: start.address().to_owned(),
                    expires: format!("{} minutes", start.expires_in / 60),
                };
                github.show(attempt, Some(SignIn::Waiting { code }), &cx);
            };
            let signed = github.api().sign_in(show, &wake).await;
            github.finish(attempt, signed, &cx).await;
        });
    }

    /// Once sign-in `attempt` is over: saves it, and answers who signed
    /// in; or says what went wrong.
    async fn finish(
        &self,
        attempt: u64,
        signed: Result<Token, String>,
        cx: &HostCx,
    ) -> Option<Value> {
        if self.flow.attempt.load(Ordering::SeqCst) != attempt {
            return None;
        }
        let saved = match signed {
            Ok(token) => token
                .save(self.dir())
                .await
                .map(|()| token)
                .map_err(|error| error.to_string()),
            Err(error) => Err(error),
        };
        match saved {
            Ok(token) => {
                self.show(attempt, None, cx);
                serde_json::to_value(Signed { user: token.user }).ok()
            }
            Err(error) => {
                self.show(attempt, Some(SignIn::Failed { error }), cx);
                None
            }
        }
    }
}
