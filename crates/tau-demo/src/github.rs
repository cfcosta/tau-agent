//! The demo's GitHub sign-in (tau-github, a Luau plugin): what the real
//! plugin draws of a sign-in standing as a screen or the scripted flow
//! has it, its store and secret store filled as the plugin would have
//! them. The demo reaches no GitHub; the plugin's `app` asks none.

use serde_json::json;
use tau_luau_plugins_host::{
    agent::Active,
    files::Files,
    runtime::{self, Context, StoreRef},
    store::{Fuse, Kv},
};

/// tau-github's name.
pub const NAME: &str = tau_luau_plugins::names::GITHUB;

/// The step of first-run setup it draws, as tau-luau-plugins names it.
pub const STEP: &str = "tau-github/sign-in";

/// The demo's device code, and where to enter it.
pub const DEVICE_CODE: &str = "WDJB-MJHT";
const DEVICE_URL: &str = "github.com/login/device";
const EXPIRES: &str = "15 minutes";

/// The demo's GitHub account.
pub const USER: &str = "cfcosta";

/// How the sign-in stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignIn {
    SignedOut,
    /// The device code shown, the person yet to approve it.
    Waiting,
    SignedIn,
}

/// The sign-in, and whether the token form shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct State {
    pub sign_in: SignIn,
    pub token_form: bool,
}

impl State {
    pub fn of(sign_in: SignIn) -> Self {
        Self {
            sign_in,
            token_form: false,
        }
    }
}

/// What the scripted flow answers: what tau-github's page asked of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    SignIn,
    /// "I have approved it", or a token typed in: signed in.
    Check,
    SignOut,
    TokenForm(bool),
}

/// What `action`, sent to `plugin`, asks of tau-github, if it is its.
pub fn act(plugin: &str, action: &serde_json::Value) -> Option<Act> {
    if plugin != tau_luau_plugins::NAME {
        return None;
    }
    match serde_json::from_value(action.clone()).ok()? {
        tau_luau_plugins::Act::AppAction {
            plugin,
            action,
            args,
        } if plugin == NAME => match action.as_str() {
            "sign_in" => Some(Act::SignIn),
            "check" => Some(Act::Check),
            "sign_out" => Some(Act::SignOut),
            "token_form" => Some(Act::TokenForm(args["open"] == json!(true))),
            _ => None,
        },
        tau_luau_plugins::Act::SetSecret { plugin, name, .. }
            if plugin == NAME && name == "github" =>
        {
            Some(Act::Check)
        }
        _ => None,
    }
}

/// tau-github, loaded as tau loads it.
pub struct GitHub {
    active: Active,
}

impl GitHub {
    pub async fn load() -> anyhow::Result<Self> {
        let files = tau_luau_plugins_host::built_in::files(NAME)
            .map_err(anyhow::Error::msg)?;
        let loaded = runtime::load(NAME, files)
            .await
            .map_err(anyhow::Error::msg)?;
        let active = Active {
            settings: loaded.declaration.default_settings(),
            loaded,
        };
        Ok(Self { active })
    }

    /// Its entry among the Luau plugins tau comes with: as it declares
    /// itself, active, keeping `app` of the app.
    pub fn entry(
        &self,
        app: tau_luau_plugins::AppView,
    ) -> tau_luau_plugins::Entry {
        tau_luau_plugins::Entry {
            name: NAME.into(),
            description: String::new(),
            version: String::new(),
            standing: tau_luau_plugins::Standing::Active,
            declaration: Some(self.active.loaded.declaration.clone()),
            tests: Vec::new(),
            keeps_earlier: false,
            built_in: true,
            app: Some(app),
        }
    }

    /// What it keeps of the app with its sign-in as `state`: its store
    /// and its `github` secret as signing in leaves them, the setup step
    /// and the Models screen's section drawn.
    pub async fn app_view(
        &self,
        state: State,
    ) -> anyhow::Result<tau_luau_plugins::AppView> {
        let dir = tempfile::tempdir()?;
        let secrets = dir.path().join("secrets.json");
        let kv = std::sync::Arc::new(
            Kv::memory(Fuse::never())
                .await
                .map_err(anyhow::Error::msg)?,
        );
        let mut session = kv.session().await;
        session.put("token_form", json!(state.token_form));
        match state.sign_in {
            SignIn::SignedOut => {}
            SignIn::Waiting => session.put(
                "sign_in",
                json!({
                    "state": "waiting",
                    "code": {
                        "code": DEVICE_CODE,
                        "url": DEVICE_URL,
                        "expires": EXPIRES,
                    },
                }),
            ),
            SignIn::SignedIn => {
                session.put("account", json!({ "user": USER }));
                tau_tools_host::secrets::SecretStore::new(&secrets)
                    .set("github", "demo")
                    .await?;
            }
        }
        session.commit().await.map_err(anyhow::Error::msg)?;
        let context = Context {
            run: json!({ "kind": "app" }),
            now: json!({}),
            settings: self.active.settings.clone(),
            state: json!({}),
            secrets: Some(secrets),
            store: Some(StoreRef(kv)),
            job: None,
        };
        let files =
            Files::new(&self.active.loaded.declaration.uses.files, None, None);
        let view = tau_luau_plugins_host::registry::app_view(
            &self.active,
            files,
            &context,
        )
        .await;
        match &view.error {
            Some(error) => anyhow::bail!("tau-github: {error}"),
            None => Ok(view),
        }
    }
}

/// `overview` with tau-github's entry as `entry`, in place of any it had.
pub fn put_entry(
    overview: &mut tau_luau_plugins::Overview,
    entry: tau_luau_plugins::Entry,
) {
    overview.plugins.retain(|plugin| plugin.name != NAME);
    overview.plugins.push(entry);
}
