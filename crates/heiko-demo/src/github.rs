//! The demo's GitHub sign-in (heiko-github, a Luau plugin): what the real
//! plugin draws of a sign-in standing as a screen or the scripted flow
//! has it, its store and secret store filled as the plugin would have
//! them. The demo reaches no GitHub; the plugin's `app` asks none.

use serde_json::json;
use heiko_luau_plugins_host::{
    agent::Active,
    files::Files,
    runtime::{self, Context, StoreRef},
    store::{Fuse, Kv},
};

/// heiko-github's name.
pub const NAME: &str = heiko_luau_plugins::names::GITHUB;

/// The step of first-run setup it draws, as heiko-luau-plugins names it.
pub const STEP: &str = "heiko-github/sign-in";

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

/// What the scripted flow answers: what heiko-github's page asked of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    SignIn,
    /// "I have approved it", or a token typed in: signed in.
    Check,
    SignOut,
    TokenForm(bool),
}

/// What `action`, sent to `plugin`, asks of heiko-github, if it is its.
pub fn act(plugin: &str, action: &serde_json::Value) -> Option<Act> {
    if plugin != heiko_luau_plugins::NAME {
        return None;
    }
    match serde_json::from_value(action.clone()).ok()? {
        heiko_luau_plugins::Act::AppAction {
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
        heiko_luau_plugins::Act::SetSecret { plugin, name, .. }
            if plugin == NAME && name == "github" =>
        {
            Some(Act::Check)
        }
        _ => None,
    }
}

/// heiko-github, loaded as heiko loads it.
pub struct GitHub {
    active: Active,
}

impl GitHub {
    pub async fn load() -> anyhow::Result<Self> {
        let files = heiko_luau_plugins_host::built_in::files(NAME)
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

    /// Its entry among the Luau plugins heiko comes with: as it declares
    /// itself, active, keeping `app` of the app.
    pub fn entry(
        &self,
        app: heiko_luau_plugins::AppView,
    ) -> heiko_luau_plugins::Entry {
        heiko_luau_plugins::Entry {
            name: NAME.into(),
            description: String::new(),
            version: String::new(),
            standing: heiko_luau_plugins::Standing::Active,
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
    ) -> anyhow::Result<heiko_luau_plugins::AppView> {
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
                heiko_tools_host::secrets::SecretStore::new(&secrets)
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
        let view = heiko_luau_plugins_host::registry::app_view(
            &self.active,
            files,
            &context,
        )
        .await;
        match &view.error {
            Some(error) => anyhow::bail!("heiko-github: {error}"),
            None => Ok(view),
        }
    }
}

/// `overview` with heiko-github's entry as `entry`, in place of any it had.
pub fn put_entry(
    overview: &mut heiko_luau_plugins::Overview,
    entry: heiko_luau_plugins::Entry,
) {
    overview.plugins.retain(|plugin| plugin.name != NAME);
    overview.plugins.push(entry);
}
