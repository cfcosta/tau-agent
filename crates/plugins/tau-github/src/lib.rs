//! GitHub as where tau's repositories are hosted (ADR 0045): its host
//! half, tau-github-host, fills the `tau.forge` slot and signs in. Its
//! UI draws the sign-in: a step of first-run setup before the
//! repositories ([`step`]), and a section of the Models screen with who
//! is signed in ([`section`]). Both are view trees; their buttons carry
//! a [`Do`], carried out in [`UiPlugin::on_action`], and what reaches
//! GitHub goes to the host half as an [`Act`].

pub mod sign_in;

use serde::{Deserialize, Serialize};
use serde_json::Value;
pub use sign_in::{section, step};
use tau_ui_plugin::{Handle, Manifest, UiPlugin, ViewCx, points};

/// The plugin's name, for both its halves.
pub const NAME: &str = "tau-github";

/// The setup step's name among the plugin's.
pub const STEP: &str = "sign-in";

/// The action of the sign-in's buttons, whose arguments are a [`Do`].
pub const SIGN_IN: &str = "sign-in";

/// The action of the token input's Enter, with `{ "text": token }`.
pub const TOKEN: &str = "token";

/// The UI state key that says the token form shows in place of the
/// device code.
pub const TOKEN_FORM: &str = "token-form";

/// The GitHub App tau signs in through, by its name in GitHub's URLs.
pub const APP_SLUG: &str = "ascend-repository-cfcosta";

/// Where to give the app access to more repositories.
pub fn install_url() -> String {
    format!("https://github.com/apps/{APP_SLUG}/installations/new")
}

/// A code to enter on GitHub, for a device sign-in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceCode {
    pub code: String,
    /// Where to enter it, without the scheme: `github.com/login/device`.
    pub url: String,
    /// How long it stays valid, in words.
    pub expires: String,
}

/// How the sign-in stands, as the host half says.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SignIn {
    /// No host has said: the plugin draws nothing.
    #[default]
    Unknown,
    SignedOut,
    /// Asking GitHub for a device code.
    Starting,
    /// Waiting for the person to enter `code` and approve.
    Waiting {
        code: DeviceCode,
    },
    /// A token is being checked.
    Checking,
    SignedIn {
        user: String,
    },
    Failed {
        error: String,
    },
}

/// What the plugin knows across repositories, from its host half.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GitHubData {
    pub sign_in: SignIn,
}

/// What the UI asks of the host half.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "act", rename_all = "snake_case")]
pub enum Act {
    /// Starts a device sign-in: the host half asks for a code, shows it
    /// in its data, and polls until the person approves.
    SignIn,
    /// The person says they approved: poll now rather than at the next
    /// interval.
    Check,
    /// Signs in with a fine-grained personal access token.
    Token { token: String },
    /// Forgets the sign-in.
    SignOut,
}

/// What the host half answers once a sign-in is saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signed {
    pub user: String,
}

/// What a button of the sign-in asks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum Do {
    /// Asks the host half for `act`.
    Act { act: Act },
    /// Copies the code and opens the page to enter it on.
    OpenDevice { code: DeviceCode },
    /// Shows the token form, or the device code again.
    TokenForm { open: bool },
    /// Opens the page that gives the app more repositories.
    InstallApp,
    /// Ends the setup step.
    Continue,
}

impl Do {
    pub fn args(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

/// tau-github with its UI.
#[derive(Debug, Clone, Copy, Default)]
pub struct GitHubUi;

impl UiPlugin for GitHubUi {
    type State = ();
    type Data = GitHubData;
    type RepoData = ();
    type Settings = ();

    fn name(&self) -> &'static str {
        NAME
    }

    /// A saved sign-in closes the token form and ends the setup step,
    /// when setup is on it.
    fn reply(&self, reply: Value, handle: &Handle) {
        if serde_json::from_value::<Signed>(reply).is_ok() {
            handle.set_ui(TOKEN_FORM, None);
            handle.setup_done();
        }
    }

    /// Carries out a sign-in button's [`Do`], or the token input's Enter.
    fn on_action(
        &self,
        action: &str,
        args: &Value,
        view: &mut ViewCx<'_, Self>,
    ) {
        let handle = &view.handle;
        if action == TOKEN {
            let token = args.get("text").and_then(Value::as_str);
            if let Some(token) = token.map(str::trim).filter(|t| !t.is_empty())
            {
                handle.act(Act::Token {
                    token: token.to_owned(),
                });
            }
            return;
        }
        if action != SIGN_IN {
            return;
        }
        let Ok(what) = serde_json::from_value::<Do>(args.clone()) else {
            return;
        };
        match what {
            Do::Act { act } => {
                if act == Act::SignIn {
                    handle.set_ui(TOKEN_FORM, None);
                }
                handle.act(act)
            }
            Do::OpenDevice { code } => {
                handle.copy(code.code);
                handle.open_url(format!("https://{}", code.url));
            }
            Do::TokenForm { open } => {
                handle.set_ui(TOKEN_FORM, open.then_some(Value::Bool(true)))
            }
            Do::InstallApp => handle.open_url(install_url()),
            Do::Continue => handle.setup_done(),
        }
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .contribute(points::SETUP, step)
            .contribute(points::MODELS, section)
    }
}
