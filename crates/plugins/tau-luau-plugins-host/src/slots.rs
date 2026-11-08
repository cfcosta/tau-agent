//! The slots a Luau plugin fills (ADR 0045). tau-luau-plugins fills each
//! once, as tau starts, with a stand-in that asks the active plugin that
//! fills it on every call: the person's plugins change while tau runs. A
//! slot no active plugin fills answers as an empty filler would, and a
//! hook that fails or runs out of time answers nothing.

use serde_json::json;
use tau_ui_plugin::{
    compaction::Compaction,
    notify::{Notice, Notify},
    title::Title,
};
use tokio_util::sync::CancellationToken;

use crate::{
    registry::Registry,
    runtime::{Context, Hook},
};

/// What a hook outside any run has as its `ctx`: no run, no state.
pub(crate) fn context(
    registry: &Registry,
    active: &crate::agent::Active,
) -> Context {
    Context {
        run: json!({}),
        now: crate::agent::now(),
        settings: active.settings.clone(),
        state: json!({}),
        secrets: registry.secrets_of(&active.loaded.declaration.name),
        store: None,
    }
}

/// What a hook outside any run reaches: `infer`, through the person's
/// model, asking `model` with it.
pub(crate) struct ModelReach {
    model: Option<tau_ui_plugin::PluginModel>,
    /// The model `uses.model` names, if it names one.
    named: Option<String>,
    /// What `infer` asks when neither the call nor `uses.model` names a
    /// model: the run's, for `title`.
    fallback: Option<String>,
}

impl ModelReach {
    /// What `active`'s hooks outside a run reach in `registry`: the model
    /// `uses.model` names, else `fallback`.
    pub(crate) fn of(
        registry: &Registry,
        active: &crate::agent::Active,
        fallback: Option<&str>,
    ) -> std::sync::Arc<Self> {
        let declaration = &active.loaded.declaration;
        std::sync::Arc::new(Self {
            model: registry.model_of(&declaration.name),
            named: declaration.uses.model.clone(),
            fallback: fallback.map(str::to_owned),
        })
    }
}

#[async_trait::async_trait]
impl crate::runtime::Reach for ModelReach {
    fn tools(&self) -> Vec<tau_codemode_host::ToolEntry> {
        vec![tau_codemode_host::ToolEntry {
            name: "infer".into(),
            description: "Asks the person's model one task.".into(),
            input_schema:
                tau_codemode_host::inference::InferRequest::parameters(),
            output_schema: None,
            namespace: None,
            sequential: false,
        }]
    }

    async fn call(
        &self,
        call: tau_codemode_host::ToolCall,
    ) -> Result<tau_codemode_host::ToolReply, String> {
        if call.name != "infer" {
            return Err(format!(
                "`{}` cannot be called outside a run",
                call.name
            ));
        }
        // As `infer` answers in a run: `{ ok, value }`, or `{ ok, error }`.
        let reply = match self.infer(call.args).await {
            Ok(value) => json!({ "ok": true, "value": value }),
            Err(error) => json!({ "ok": false, "error": error }),
        };
        Ok(tau_codemode_host::ToolReply::success(reply))
    }
}

impl ModelReach {
    async fn infer(
        &self,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        use tau_ai::message::{Message, UserContent, UserMessage};
        let request = tau_codemode_host::inference::InferRequest::parse(args)?;
        let model = self
            .model
            .as_ref()
            .ok_or("there is no model to ask: the person is signed out")?;
        let named = request.model.as_deref().or(self.named.as_deref());
        let name = model_named(named, self.fallback.as_deref())
            .ok_or("name the model `infer` asks in `uses.model`")?;
        let effort = effort_for(&name, request.effort);
        let mut settings = tau_ui_plugin::AskSettings::new(name, effort);
        if let Some(format) = request.text_format() {
            settings = settings.text_format(format);
        }
        let input = [Message::User(UserMessage {
            content: UserContent::Text(request.input_text()),
            timestamp: 0,
        })];
        let answer = model
            .ask(&settings, &input, &[], &CancellationToken::new())
            .await
            .map_err(|error| error.to_string())?;
        if let Some(error) = &answer.error_message {
            return Err(error.clone());
        }
        request.decode_answer(&answer.text())
    }
}

/// The model `uses.model` names: a model by its id; a family by its name
/// (`"luna"`), the plan's newest of it, else `fallback`; any other name
/// as written. With no name, `fallback`.
pub(crate) fn model_named(
    named: Option<&str>,
    fallback: Option<&str>,
) -> Option<String> {
    use tau_ai::model::{family_version, find, models, plan_models};
    let Some(named) = named else {
        return fallback.map(str::to_owned);
    };
    if find(named).is_some() {
        return Some(named.to_owned());
    }
    let of_family = |id: &str| {
        family_version(id).is_some_and(|(family, _)| family == named)
    };
    if let Some(newest) =
        plan_models().into_iter().find(|model| of_family(&model.id))
    {
        return Some(newest.id.clone());
    }
    if models().iter().any(|model| of_family(&model.id)) {
        return fallback.map(str::to_owned);
    }
    Some(named.to_owned())
}

/// `effort`, when `model` takes it: a model the table knows to take
/// none of it is asked with its own.
pub(crate) fn effort_for(
    model: &str,
    effort: Option<tau_ai::responses::request::ReasoningEffort>,
) -> Option<tau_ai::responses::request::ReasoningEffort> {
    let effort = effort?;
    match tau_ai::model::find(model) {
        Some(known) if !known.efforts.contains(&effort) => None,
        _ => Some(effort),
    }
}

/// `tau.title` from the active plugin that fills it.
pub struct LuauTitle(pub Registry);

impl Title for LuauTitle {
    fn write<'a>(
        &'a self,
        task: &'a str,
        run_model: &'a str,
    ) -> tau_ui_plugin::title::Written<'a> {
        Box::pin(async move {
            let Some(active) =
                self.0.filler(tau_ui_plugin::title::TITLE.name()).await
            else {
                return Ok(None);
            };
            let outcome = active
                .loaded
                .call(
                    &Hook::Title,
                    json!({ "task": task, "model": run_model }),
                    &context(&self.0, &active),
                    ModelReach::of(&self.0, &active, Some(run_model)),
                    CancellationToken::new(),
                )
                .await;
            if let Some(error) = outcome.error {
                anyhow::bail!(
                    "{}'s title: {error}",
                    active.loaded.declaration.name
                );
            }
            Ok(outcome
                .value
                .as_str()
                .map(str::trim)
                .filter(|title| !title.is_empty())
                .map(|title| {
                    title
                        .chars()
                        .take(tau_ui_plugin::title::MAX_CHARS)
                        .collect()
                }))
        })
    }
}

/// `tau.notify` from the active plugin that fills it: each notice goes
/// to its `notify`, on the host's runtime. Opening one is the person's
/// own, wherever the plugin sent it.
pub struct LuauNotify(pub Registry);

impl Notify for LuauNotify {
    fn show(&self, notice: Notice, _opened: Box<dyn FnOnce() + Send>) {
        let Some(runtime) = self.0.runtime() else {
            return;
        };
        let registry = self.0.clone();
        runtime.spawn(async move {
            let Some(active) =
                registry.filler(tau_ui_plugin::notify::NOTIFY.name()).await
            else {
                return;
            };
            let outcome = active
                .loaded
                .call(
                    &Hook::Notify,
                    json!({ "title": notice.title, "body": notice.body }),
                    &context(&registry, &active),
                    ModelReach::of(&registry, &active, None),
                    CancellationToken::new(),
                )
                .await;
            if let Some(error) = outcome.error {
                eprintln!(
                    "tau-luau-plugins: {}'s notify: {error}",
                    active.loaded.declaration.name
                );
            }
        });
    }
}

/// `tau.compaction` from the active plugin that fills it: the Luau
/// plugins' own rewrites, and the plugins it names.
pub struct LuauCompaction(pub Registry);

impl Compaction for LuauCompaction {
    fn rewriters(&self) -> Vec<String> {
        let mut rewriters = vec![tau_luau_plugins::NAME.to_owned()];
        let filler = self
            .0
            .filler_now(tau_ui_plugin::compaction::COMPACTION.name());
        if let Some(fill) =
            filler.and_then(|active| active.loaded.declaration.compaction)
        {
            rewriters.extend(fill.rewriters);
        }
        rewriters
    }
}

/// `tau.forge` from the active plugin that fills it. What the forge is
/// (its name, its clones' directory, the host repositories clone from)
/// and the remote's token come from its `forge` table and its own secret
/// store, which the plugin never reads; what it asks the forge goes to
/// its hooks, which reach it over HTTP.
pub struct LuauForge(pub Registry);

/// The forge's words as a `&'static str`, as the slot gives them: each
/// once, for the few forges a person writes.
fn interned(text: &str) -> &'static str {
    static SEEN: std::sync::Mutex<Vec<&'static str>> =
        std::sync::Mutex::new(Vec::new());
    let mut seen = SEEN.lock().expect("not poisoned");
    if let Some(known) = seen.iter().find(|known| **known == text) {
        return known;
    }
    let leaked: &'static str = Box::leak(text.to_owned().into_boxed_str());
    seen.push(leaked);
    leaked
}

impl LuauForge {
    fn active(
        &self,
    ) -> Option<(crate::agent::Active, tau_luau_plugins::ForgeFill)> {
        let active = self.0.filler_now(tau_ui_plugin::forge::FORGE.name())?;
        let fill = active.loaded.declaration.forge.clone()?;
        Some((active, fill))
    }

    async fn filling(
        &self,
    ) -> anyhow::Result<(crate::agent::Active, tau_luau_plugins::ForgeFill)>
    {
        let active = self
            .0
            .filler(tau_ui_plugin::forge::FORGE.name())
            .await
            .ok_or_else(|| anyhow::anyhow!("no Luau plugin fills tau.forge"))?;
        let fill = active
            .loaded
            .declaration
            .forge
            .clone()
            .ok_or_else(|| anyhow::anyhow!("its `forge` table is gone"))?;
        Ok((active, fill))
    }

    /// The sign-in's token: the forge's `token` secret, when it is set.
    async fn token(
        &self,
        active: &crate::agent::Active,
        fill: &tau_luau_plugins::ForgeFill,
    ) -> Option<String> {
        let path = self.0.secrets_of(&active.loaded.declaration.name)?;
        let secrets = tau_tools_host::secrets::SecretStore::new(path)
            .read()
            .await
            .ok()?;
        secrets
            .get(&fill.token)
            .filter(|token| !token.is_empty())
            .cloned()
    }

    /// Calls the forge's hook `hook` with `input`; one it lacks is an
    /// error that names it.
    async fn ask(
        &self,
        hook: &str,
        input: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let (active, _) = self.filling().await?;
        let hook = Hook::Forge(hook.to_owned());
        anyhow::ensure!(
            hook.declared_in(&active.loaded.declaration),
            "{}'s forge has no `{}`",
            active.loaded.declaration.name,
            match &hook {
                Hook::Forge(name) => name.as_str(),
                _ => "",
            }
        );
        let outcome = active
            .loaded
            .call(
                &hook,
                input,
                &context(&self.0, &active),
                ModelReach::of(&self.0, &active, None),
                CancellationToken::new(),
            )
            .await;
        match outcome.error {
            Some(error) => {
                anyhow::bail!("{}: {error}", active.loaded.declaration.name)
            }
            None => Ok(outcome.value),
        }
    }
}

impl tau_ui_plugin::forge::Forge for LuauForge {
    fn name(&self) -> &str {
        self.active()
            .map_or("A Luau forge", |(_, fill)| interned(&fill.name))
    }

    fn id(&self) -> &str {
        self.active().map_or("luau", |(_, fill)| interned(&fill.id))
    }

    fn clone_url(&self, full_name: &str) -> String {
        let host = self.active().map(|(_, fill)| fill.host).unwrap_or_default();
        format!("https://{host}/{full_name}.git")
    }

    fn repository_of(&self, url: &str) -> Option<String> {
        let (_, fill) = self.active()?;
        repository_of(&fill.host, url)
    }

    fn signed_in(
        &self,
    ) -> tau_ui_plugin::registry::HostFuture<'_, Option<String>> {
        Box::pin(async move {
            let (active, fill) = self.filling().await.ok()?;
            self.token(&active, &fill).await?;
            if !fill.hooks.iter().any(|hook| hook == "account") {
                return Some(fill.name.clone());
            }
            self.ask("account", json!({}))
                .await
                .ok()
                .and_then(|account| account.as_str().map(str::to_owned))
        })
    }

    fn remote<'a>(
        &'a self,
        full_name: &'a str,
    ) -> tau_ui_plugin::registry::HostFuture<
        'a,
        anyhow::Result<tau_ui_plugin::vcs::Remote>,
    > {
        Box::pin(async move {
            let (active, fill) = self.filling().await?;
            let token = self.token(&active, &fill).await.ok_or_else(|| {
                anyhow::anyhow!(
                    "Sign in to {}: set its `{}` secret on {}'s settings",
                    fill.name,
                    fill.token,
                    active.loaded.declaration.name
                )
            })?;
            Ok(tau_ui_plugin::vcs::Remote {
                url: format!("https://{}/{full_name}.git", fill.host),
                token: Some(token),
            })
        })
    }

    fn can_push<'a>(
        &'a self,
        full_name: &'a str,
    ) -> tau_ui_plugin::registry::HostFuture<'a, anyhow::Result<bool>> {
        Box::pin(async move {
            let answer =
                self.ask("can_push", json!({ "repo": full_name })).await?;
            Ok(answer.as_bool().unwrap_or(false))
        })
    }

    fn repos(
        &self,
    ) -> tau_ui_plugin::registry::HostFuture<
        '_,
        anyhow::Result<Vec<tau_ui_plugin::forge::ForgeRepo>>,
    > {
        Box::pin(async move {
            let answer = self.ask("repos", json!({})).await?;
            let repos = answer
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|repo| {
                    let text = |key: &str| {
                        repo[key].as_str().unwrap_or_default().to_owned()
                    };
                    Some(tau_ui_plugin::forge::ForgeRepo {
                        name: repo["name"].as_str()?.to_owned(),
                        description: text("description"),
                        branch: repo["branch"]
                            .as_str()
                            .unwrap_or("main")
                            .to_owned(),
                        pushed_at: text("pushed_at"),
                        remote: repo["remote"].as_str().map(str::to_owned),
                    })
                })
                .collect();
            Ok(repos)
        })
    }

    fn open_pull(
        &self,
        pull: tau_ui_plugin::forge::OpenPull,
    ) -> tau_ui_plugin::registry::HostFuture<
        '_,
        anyhow::Result<tau_ui_plugin::forge::Opened>,
    > {
        Box::pin(async move {
            let answer = self
                .ask(
                    "open_pull",
                    json!({
                        "repo": pull.repo, "title": pull.title, "body": pull.body,
                        "head": pull.head, "base": pull.base, "draft": pull.draft,
                        "reviewers": pull.reviewers,
                    }),
                )
                .await?;
            let number = answer["number"].as_u64().ok_or_else(|| {
                anyhow::anyhow!("`open_pull` answered no `number`")
            })?;
            Ok(tau_ui_plugin::forge::Opened {
                number,
                url: answer["url"].as_str().unwrap_or_default().to_owned(),
            })
        })
    }

    fn checks<'a>(
        &'a self,
        full_name: &'a str,
        head: &'a str,
    ) -> tau_ui_plugin::registry::HostFuture<
        'a,
        anyhow::Result<tau_ui_plugin::forge::Checks>,
    > {
        use tau_ui_plugin::forge::Checks;
        Box::pin(async move {
            let answer = self
                .ask("checks", json!({ "repo": full_name, "head": head }))
                .await?;
            Ok(match answer.as_str() {
                Some("running") => Checks::Running,
                Some("passed") => Checks::Passed,
                Some("failed") => Checks::Failed,
                Some("none") => Checks::None,
                _ => anyhow::bail!(
                    "`checks` answers \"running\", \"passed\", \"failed\" or \"none\""
                ),
            })
        })
    }
}

/// The full name of the repository the Git URL `url` names on `host`,
/// in the forms Git takes: `https://host/owner/name(.git)`,
/// `ssh://git@host/owner/name(.git)` and `git@host:owner/name(.git)`.
pub fn repository_of(host: &str, url: &str) -> Option<String> {
    let url = url.trim();
    let path = ["https://", "http://", "ssh://git@", "git://"]
        .iter()
        .find_map(|scheme| {
            url.strip_prefix(scheme)?
                .strip_prefix(host)?
                .strip_prefix('/')
        })
        .or_else(|| {
            url.strip_prefix("git@")?
                .strip_prefix(host)?
                .strip_prefix(':')
        })?;
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.split('/');
    let (owner, name) = (parts.next()?, parts.next()?);
    (parts.next().is_none() && !owner.is_empty() && !name.is_empty())
        .then(|| format!("{owner}/{name}"))
}
