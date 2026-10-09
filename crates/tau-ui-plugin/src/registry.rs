//! The plugins an interface has, with their types erased: what `tau-ui`
//! holds, and the only way a plugin reaches a run.

use std::{
    any::Any,
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use gpui::{AnyElement, AnyEntity, App, AppContext as _};
use serde_json::Value;
use tau_agent::plugin::Plugin;

use crate::{
    Fold,
    HostHalf,
    PluginHost,
    PluginInfo,
    PluginUi,
    PluginValue,
    RecordOf,
    SettingsOf,
    UiPlugin,
    host::{HostCx, RepoCtx, RunCtx},
    manifest::Manifest,
    run::RunCx,
    view::{Handle, RunInfo, ViewCx},
};

/// What the interface hands a plugin's UI as it draws: the plugin's
/// values.
pub struct Env<'a> {
    /// The plugin's window state, made by [`ErasedPlugin::new_ui`].
    pub ui: AnyEntity,
    /// The plugin's state in the run drawn, when there is one.
    pub state: Option<&'a PluginValue>,
    pub data: &'a PluginValue,
    pub settings: &'a PluginValue,
    /// Its data for each repository, by name.
    pub repos: &'a BTreeMap<String, &'a PluginValue>,
    pub run: Option<&'a RunInfo>,
    pub params: &'a BTreeMap<String, String>,
    pub compact: bool,
    pub jev: bool,
    /// The window's width, in pixels.
    pub width: f32,
    pub handle: Handle,
    /// Every run, with the plugin's state in it.
    pub runs: &'a dyn Fn() -> Vec<(RunInfo, PluginValue)>,
    /// A run's tool calls.
    pub cards: &'a dyn Fn(&tau_agent::tool::RunId) -> Vec<crate::CardInfo>,
    pub cx: &'a mut App,
}

/// A page, by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageInfo {
    pub name: &'static str,
}

/// What the composer lists commands from: the plugin's data and, when
/// the composer is in a repository, that repository's name and the
/// plugin's data for it.
#[derive(Debug, Clone, Copy)]
pub struct CommandsAt<'a> {
    pub data: &'a PluginValue,
    pub repo: Option<(&'a str, &'a PluginValue)>,
}

/// A slash command, by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandInfo {
    pub name: String,
    pub hint: String,
    pub args: String,
    pub icon: tau_ui_kit::assets::Icon,
    /// Whether it has a popover while it is written.
    pub popover: bool,
}

/// A host half's state, made once per host by [`ErasedPlugin::host`].
pub type HostState = Box<dyn Any + Send + Sync>;

/// What a host half's hook gives back: a future the host awaits on its
/// runtime (ADR 0028).
pub type HostFuture<'a, T> =
    std::pin::Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// [`UiPlugin`] and its [`HostHalf`], with their types erased.
pub trait ErasedPlugin: Send + Sync {
    fn name(&self) -> &'static str;

    /// Itself, to take back its typed form ([`Registry::host`]).
    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync>;

    fn host<'a>(
        &'a self,
        cx: &'a HostCx,
    ) -> HostFuture<'a, anyhow::Result<HostState>>;
    fn default_settings(&self) -> PluginValue;
    fn agent_plugins<'a>(
        &'a self,
        host: &'a HostState,
        run: &'a RunCtx,
        settings: &'a PluginValue,
    ) -> HostFuture<'a, anyhow::Result<Vec<Box<dyn Plugin>>>>;
    fn starting<'a>(
        &'a self,
        host: &'a HostState,
        run: &'a RunCtx,
        settings: &'a PluginValue,
    ) -> HostFuture<'a, Vec<Value>>;
    fn catalog<'a>(
        &'a self,
        host: &'a HostState,
        cx: &'a HostCx,
        settings: &'a PluginValue,
    ) -> HostFuture<'a, PluginInfo>;
    fn data<'a>(
        &'a self,
        host: &'a HostState,
        cx: &'a HostCx,
    ) -> HostFuture<'a, PluginValue>;
    fn launcher<'a>(
        &'a self,
        host: &'a HostState,
        repo: &'a RepoCtx,
        settings: &'a PluginValue,
    ) -> HostFuture<'a, Option<std::sync::Arc<dyn tau_agent::launch::Launcher>>>;
    fn repo_data<'a>(
        &'a self,
        host: &'a HostState,
        repo: &'a RepoCtx,
        cx: &'a HostCx,
    ) -> HostFuture<'a, PluginValue>;
    fn lands_itself<'a>(
        &'a self,
        host: &'a HostState,
        repo: &'a RepoCtx,
        workspace: &'a std::path::Path,
    ) -> HostFuture<'a, bool>;
    fn act<'a>(
        &'a self,
        host: &'a HostState,
        action: Value,
        cx: &'a HostCx,
    ) -> HostFuture<'a, anyhow::Result<Option<Value>>>;

    /// Folds `body`, a record the plugin published (or the details of a
    /// rewrite, [`crate::REWRITE`]), into its state in a run.
    fn apply(&self, state: &mut PluginValue, body: &Value, run: &mut dyn RunCx);

    fn new_ui(&self, handle: Handle, cx: &mut App) -> AnyEntity;
    fn reply(&self, ui: AnyEntity, reply: Value, cx: &mut App);
    fn declared(&self) -> Vec<&'static str>;
    fn contributes_to(&self) -> Vec<&'static str>;
    /// What the plugin contributes at `point`, given its context, with
    /// each contribution's order.
    fn contribute(
        &self,
        point: &str,
        point_cx: &dyn Any,
        env: Env<'_>,
    ) -> Vec<(i32, Box<dyn Any>)>;
    /// Whether it draws a settings pane (`Manifest::settings`).
    fn has_settings(&self) -> bool;
    /// Its settings pane, with `env.settings` the value for the scope in
    /// `env.params`.
    fn draw_settings(&self, env: Env<'_>) -> Option<AnyElement>;
    fn pages(&self) -> Vec<PageInfo>;
    fn page_title(&self, page: &str, env: Env<'_>) -> Option<String>;
    fn draw_page(&self, page: &str, env: Env<'_>) -> Option<AnyElement>;
    /// Its commands: the manifest's, then those it lists from `at`.
    fn commands(&self, at: CommandsAt<'_>) -> Vec<CommandInfo>;
    /// Runs the command `name` in `repo`; false when the plugin has none.
    fn run_command(
        &self,
        name: &str,
        args: &str,
        repo: Option<&str>,
        env: Env<'_>,
    ) -> bool;
    /// The popover of command `name` while it is written.
    fn command_popover(
        &self,
        name: &str,
        args: &str,
        env: Env<'_>,
    ) -> Option<AnyElement>;
    /// What a run on `prompt` is about, when the plugin reads it as its
    /// own: `/goal the tests pass` is about the tests passing.
    fn read_prompt(&self, prompt: &str) -> Option<String>;
    fn rewrites_keep_transcript(&self) -> bool;
}

/// The host half of a plugin whose own is not here: an interface's,
/// which runs no agents. Asked for anything only a host does, it fails.
pub struct NoHost<P>(std::marker::PhantomData<fn() -> P>);

impl<P> Default for NoHost<P> {
    fn default() -> Self {
        Self(std::marker::PhantomData)
    }
}

impl<P: UiPlugin> HostHalf for NoHost<P> {
    type Plugin = P;
    type Host = Missing;

    async fn agent_plugins(
        &self,
        _: &Missing,
        _: &RunCtx,
        _: &SettingsOf<Self>,
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        unreachable!("a plugin without its host half has no host state")
    }

    async fn catalog(
        &self,
        _: &Missing,
        _: &HostCx,
        _: &SettingsOf<Self>,
    ) -> PluginInfo {
        unreachable!("a plugin without its host half has no host state")
    }
}

/// The host state of a plugin without its host half, which cannot be
/// made: a host that reaches such a plugin fails as it starts, naming it.
pub struct Missing;

impl PluginHost for Missing {
    async fn new(_: &HostCx) -> anyhow::Result<Self> {
        anyhow::bail!("its host half is not in this registry (Registry::host)")
    }
}

struct Typed<P: UiPlugin, H: HostHalf<Plugin = P>> {
    plugin: P,
    half: H,
    manifest: Manifest<P>,
}

impl<P: UiPlugin, H: HostHalf<Plugin = P>> Typed<P, H> {
    fn host_of<'h>(&self, host: &'h HostState) -> &'h H::Host {
        host.downcast_ref()
            .expect("a host state is made by its own plugin")
    }

    /// Runs `f` with a [`ViewCx`] made from `env`.
    fn with_view<R>(
        &self,
        env: Env<'_>,
        f: impl for<'b> FnOnce(&mut ViewCx<'b, P>) -> R,
    ) -> Option<R> {
        let ui = env.ui.downcast::<P::Ui>().ok()?;
        let mut view = ViewCx::new(
            &self.plugin,
            ui,
            env.state.map(PluginValue::get::<P::State>),
            env.data.get::<P::Data>(),
            env.settings.get::<P::Settings>(),
            env.repos,
            env.run,
            env.params,
            env.compact,
            env.jev,
            env.width,
            env.handle,
            env.runs,
            env.cards,
            env.cx,
        );
        Some(f(&mut view))
    }
}

impl<P: UiPlugin, H: HostHalf<Plugin = P>> ErasedPlugin for Typed<P, H> {
    fn name(&self) -> &'static str {
        self.plugin.name()
    }

    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self
    }

    fn host<'a>(
        &'a self,
        cx: &'a HostCx,
    ) -> HostFuture<'a, anyhow::Result<HostState>> {
        Box::pin(async move {
            let host =
                <H::Host as PluginHost>::new(cx).await.map_err(|error| {
                    error.context(format!("plugin {}", self.plugin.name()))
                })?;
            Ok(Box::new(host) as HostState)
        })
    }

    fn default_settings(&self) -> PluginValue {
        PluginValue::typed(P::Settings::default())
    }

    fn agent_plugins<'a>(
        &'a self,
        host: &'a HostState,
        run: &'a RunCtx,
        settings: &'a PluginValue,
    ) -> HostFuture<'a, anyhow::Result<Vec<Box<dyn Plugin>>>> {
        Box::pin(self.half.agent_plugins(
            self.host_of(host),
            run,
            settings.get(),
        ))
    }

    fn starting<'a>(
        &'a self,
        host: &'a HostState,
        run: &'a RunCtx,
        settings: &'a PluginValue,
    ) -> HostFuture<'a, Vec<Value>> {
        Box::pin(async move {
            let records: Vec<RecordOf<P>> = self
                .half
                .starting(self.host_of(host), run, settings.get())
                .await;
            records
                .iter()
                .map(|record| {
                    serde_json::to_value(record)
                        .expect("a plugin's record serializes")
                })
                .collect()
        })
    }

    fn catalog<'a>(
        &'a self,
        host: &'a HostState,
        cx: &'a HostCx,
        settings: &'a PluginValue,
    ) -> HostFuture<'a, PluginInfo> {
        Box::pin(async move {
            PluginInfo {
                name: self.plugin.name().to_owned(),
                settings: self.manifest.settings.is_some(),
                ..self
                    .half
                    .catalog(self.host_of(host), cx, settings.get())
                    .await
            }
        })
    }

    fn data<'a>(
        &'a self,
        host: &'a HostState,
        cx: &'a HostCx,
    ) -> HostFuture<'a, PluginValue> {
        Box::pin(async move {
            PluginValue::typed(self.half.data(self.host_of(host), cx).await)
        })
    }

    fn launcher<'a>(
        &'a self,
        host: &'a HostState,
        repo: &'a RepoCtx,
        settings: &'a PluginValue,
    ) -> HostFuture<'a, Option<std::sync::Arc<dyn tau_agent::launch::Launcher>>>
    {
        Box::pin(self.half.launcher(self.host_of(host), repo, settings.get()))
    }

    fn repo_data<'a>(
        &'a self,
        host: &'a HostState,
        repo: &'a RepoCtx,
        cx: &'a HostCx,
    ) -> HostFuture<'a, PluginValue> {
        Box::pin(async move {
            PluginValue::typed(
                self.half.repo_data(self.host_of(host), repo, cx).await,
            )
        })
    }

    fn lands_itself<'a>(
        &'a self,
        host: &'a HostState,
        repo: &'a RepoCtx,
        workspace: &'a std::path::Path,
    ) -> HostFuture<'a, bool> {
        Box::pin(self.half.lands_itself(self.host_of(host), repo, workspace))
    }

    fn act<'a>(
        &'a self,
        host: &'a HostState,
        action: Value,
        cx: &'a HostCx,
    ) -> HostFuture<'a, anyhow::Result<Option<Value>>> {
        Box::pin(self.half.act(self.host_of(host), action, cx))
    }

    fn apply(
        &self,
        state: &mut PluginValue,
        body: &Value,
        run: &mut dyn RunCx,
    ) {
        let state = state.get_mut::<P::State>();
        if let Some(details) = body.get(crate::REWRITE)
            && body.get("kind").is_none()
        {
            state.rewritten(details.clone(), run);
            return;
        }
        if let Some(record) =
            tau_agent::plugin::read_record(self.plugin.name(), body)
        {
            state.apply(record, run);
        }
    }

    fn new_ui(&self, handle: Handle, cx: &mut App) -> AnyEntity {
        cx.new(|cx| <P::Ui as PluginUi>::new(handle, cx)).into_any()
    }

    fn reply(&self, ui: AnyEntity, reply: Value, cx: &mut App) {
        if let Ok(ui) = ui.downcast::<P::Ui>() {
            ui.update(cx, |ui, cx| {
                self.plugin.reply(ui, reply, cx);
                cx.notify();
            });
        }
    }

    fn declared(&self) -> Vec<&'static str> {
        self.manifest.points.clone()
    }

    fn contributes_to(&self) -> Vec<&'static str> {
        let mut points: Vec<&'static str> = self
            .manifest
            .contributions
            .iter()
            .map(|contribution| contribution.point)
            .collect();
        points.dedup();
        points
    }

    fn contribute(
        &self,
        point: &str,
        point_cx: &dyn Any,
        env: Env<'_>,
    ) -> Vec<(i32, Box<dyn Any>)> {
        let mine: Vec<_> = self
            .manifest
            .contributions
            .iter()
            .filter(|contribution| contribution.point == point)
            .collect();
        if mine.is_empty() {
            return Vec::new();
        }
        self.with_view(env, |view| {
            mine.iter()
                .filter_map(|contribution| {
                    (contribution.contribute)(point_cx, view)
                        .map(|out| (contribution.order, out))
                })
                .collect()
        })
        .unwrap_or_default()
    }

    fn has_settings(&self) -> bool {
        self.manifest.settings.is_some()
    }

    fn draw_settings(&self, env: Env<'_>) -> Option<AnyElement> {
        let draw = self.manifest.settings.as_ref()?;
        self.with_view(env, |view| draw(view))
    }

    fn pages(&self) -> Vec<PageInfo> {
        self.manifest
            .pages
            .iter()
            .map(|page| PageInfo { name: page.name })
            .collect()
    }

    fn page_title(&self, page: &str, env: Env<'_>) -> Option<String> {
        let page = self.manifest.pages.iter().find(|p| p.name == page)?;
        self.with_view(env, |view| page.title_of(view))
    }

    fn draw_page(&self, page: &str, env: Env<'_>) -> Option<AnyElement> {
        let page = self.manifest.pages.iter().find(|p| p.name == page)?;
        self.with_view(env, |view| page.draw(view))
    }

    fn commands(&self, at: CommandsAt<'_>) -> Vec<CommandInfo> {
        let own = self.manifest.commands.iter().map(|command| CommandInfo {
            name: command.name.to_owned(),
            hint: command.hint.to_owned(),
            args: command.args.to_owned(),
            icon: command.icon,
            popover: command.has_popover(),
        });
        let listed = self.manifest.listed.as_ref().map(|listed| {
            let repo = at.repo.map(|(_, value)| value.get::<P::RepoData>());
            (listed.list)(at.data.get::<P::Data>(), repo)
        });
        let mut commands: Vec<CommandInfo> = own.collect();
        for command in listed.into_iter().flatten() {
            if commands.iter().any(|known| known.name == command.name) {
                continue;
            }
            commands.push(CommandInfo {
                name: command.name,
                hint: command.hint,
                args: command.args,
                icon: command.icon,
                popover: false,
            });
        }
        commands
    }

    fn run_command(
        &self,
        name: &str,
        args: &str,
        repo: Option<&str>,
        env: Env<'_>,
    ) -> bool {
        if let Some(command) =
            self.manifest.commands.iter().find(|c| c.name == name)
        {
            return self
                .with_view(env, |view| command.run(args, view))
                .is_some();
        }
        let Some(listed) = &self.manifest.listed else {
            return false;
        };
        self.with_view(env, |view| (listed.run)(name, args, repo, view))
            .is_some()
    }

    fn command_popover(
        &self,
        name: &str,
        args: &str,
        env: Env<'_>,
    ) -> Option<AnyElement> {
        let command = self.manifest.commands.iter().find(|c| c.name == name)?;
        self.with_view(env, |view| command.draw_popover(args, view))
            .flatten()
    }

    fn read_prompt(&self, prompt: &str) -> Option<String> {
        self.plugin.read_prompt(prompt)
    }

    fn rewrites_keep_transcript(&self) -> bool {
        self.plugin.rewrites_keep_transcript()
    }
}

/// The plugins an interface has, in the order they were added: the
/// order the host adds them to a run's agent, and the order their
/// contributions sit in among equals.
#[derive(Default)]
pub struct Registry {
    plugins: Vec<Arc<dyn ErasedPlugin>>,
    /// Points contributed to that nobody declares, logged once each.
    dropped: Mutex<BTreeSet<String>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `plugin`, with its UI and without its host half: what an
    /// interface has. A host gives it its half with [`Self::host`].
    pub fn with<P: UiPlugin>(mut self, plugin: P) -> Self {
        let manifest = plugin.manifest();
        self.plugins.push(Arc::new(Typed {
            plugin,
            half: NoHost::<P>::default(),
            manifest,
        }));
        self
    }

    /// Gives `half` to the plugin it is the host half of, in its place.
    ///
    /// # Panics
    ///
    /// When that plugin is not here yet, or already has a host half: the
    /// list of plugins and the host halves given them are both tau's own.
    pub fn host<H: HostHalf>(mut self, half: H) -> Self {
        let slot = self.plugins.iter().position(|plugin| {
            Arc::clone(plugin)
                .into_any()
                .is::<Typed<H::Plugin, NoHost<H::Plugin>>>()
        });
        let Some(slot) = slot else {
            panic!(
                "no plugin without a host half for {}",
                std::any::type_name::<H>()
            )
        };
        let erased = Arc::clone(&self.plugins[slot]).into_any();
        // Two references: the slot's and `erased`; the slot's goes now.
        self.plugins.remove(slot);
        let typed = erased
            .downcast::<Typed<H::Plugin, NoHost<H::Plugin>>>()
            .ok()
            .and_then(|typed| Arc::try_unwrap(typed).ok())
            .expect("the registry holds the only reference while it is built");
        self.plugins.insert(
            slot,
            Arc::new(Typed {
                plugin: typed.plugin,
                half,
                manifest: typed.manifest,
            }),
        );
        self
    }

    pub fn plugins(&self) -> impl Iterator<Item = &Arc<dyn ErasedPlugin>> {
        self.plugins.iter()
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn ErasedPlugin>> {
        self.plugins.iter().find(|plugin| plugin.name() == name)
    }

    /// Every point declared: `declared` (the interface's own) and the
    /// plugins'.
    pub fn points(&self, declared: &[&'static str]) -> BTreeSet<&'static str> {
        declared
            .iter()
            .copied()
            .chain(self.plugins.iter().flat_map(|plugin| plugin.declared()))
            .collect()
    }

    /// Contributions to points nobody declares, as `plugin → point`. The
    /// interface drops them; [`Self::log_dropped`] says so once.
    pub fn undeclared(
        &self,
        declared: &[&'static str],
    ) -> Vec<(String, String)> {
        let points = self.points(declared);
        self.plugins
            .iter()
            .flat_map(|plugin| {
                plugin
                    .contributes_to()
                    .into_iter()
                    .filter(|point| !points.contains(point))
                    .map(|point| (plugin.name().to_owned(), point.to_owned()))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Logs each contribution to an undeclared point, once.
    pub fn log_dropped(&self, declared: &[&'static str]) {
        let mut logged = self.dropped.lock().expect("not poisoned");
        for (plugin, point) in self.undeclared(declared) {
            if logged.insert(format!("{plugin} {point}")) {
                eprintln!(
                    "tau-ui-plugin: {plugin} contributes to {point}, which \
                     nobody declares; it is dropped"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};
    use serde_json::json;

    use super::*;
    use crate::{
        Manifest,
        Point,
        points::{AtApp, AtRun, STATUS},
        view::{NavEntry, PluginStatus},
    };

    /// Counts what it is published, and anchors each at the transcript.
    struct Counter;

    /// What the counter folds: the numbers published to it.
    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    struct Counted(Vec<u32>);

    #[derive(Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    enum Count {
        N { n: u32 },
    }

    impl Fold for Counted {
        type Record = Count;

        fn apply(&mut self, record: Count, run: &mut dyn RunCx) {
            let Count::N { n } = record;
            self.0.push(n);
            run.transcript(&format!("n-{n}"));
        }
    }

    const ELSEWHERE: Point<AtApp, NavEntry> =
        Point::new("nobody.declares.this");
    const MINE: Point<AtApp, NavEntry> = Point::new("counter.mine");

    impl UiPlugin for Counter {
        type State = Counted;
        type Data = ();
        type RepoData = ();
        type Settings = ();
        type Ui = ();

        fn name(&self) -> &'static str {
            "counter"
        }

        fn manifest(&self) -> Manifest<Self> {
            Manifest::new()
                .point(MINE)
                .contribute(STATUS, |cx: &AtRun, view| {
                    Some(PluginStatus {
                        name: cx.run.title.clone(),
                        state: format!("{:?}", view.state),
                        tone: Default::default(),
                    })
                })
                .contribute(ELSEWHERE, |_, _| None)
                .contribute(MINE, |_, _| None)
        }
    }

    /// Folding through the registry is folding the typed state: every
    /// record lands in order, with its anchor. A body the plugin cannot
    /// read changes nothing.
    #[hegel::test(test_cases = 100)]
    fn the_erased_fold_is_the_typed_fold(tc: hegel::TestCase) {
        use hegel::generators as gs;
        let bodies: Vec<Option<u32>> =
            tc.draw(gs::vecs(gs::optional(gs::integers::<u32>())).max_size(8));
        let registry = Registry::new().with(Counter);
        let plugin = registry.get("counter").unwrap();
        let mut state = PluginValue::default();
        let mut anchors = crate::testing::FakeRun::default();
        for body in &bodies {
            let body = match body {
                Some(n) => json!({ "kind": "n", "n": n }),
                None => json!({ "kind": "other" }),
            };
            plugin.apply(&mut state, &body, &mut anchors);
        }
        let kept: Vec<u32> = bodies.iter().flatten().copied().collect();
        // A record the plugin cannot read is skipped.
        assert_eq!(state.get::<Counted>().0, kept);
        let keys: Vec<String> = kept.iter().map(|n| format!("n-{n}")).collect();
        assert_eq!(anchors.anchors, keys);
    }

    /// Counts on the host too: its host state is made from the host's
    /// context.
    struct CounterHost;

    impl HostHalf for CounterHost {
        type Plugin = Counter;
        type Host = ();

        async fn agent_plugins(
            &self,
            _: &(),
            _: &RunCtx,
            _: &(),
        ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
            Ok(Vec::new())
        }

        async fn catalog(&self, _: &(), _: &HostCx, _: &()) -> PluginInfo {
            PluginInfo::default()
        }
    }

    /// Another plugin, so the registry has an order to keep.
    struct Other;

    impl UiPlugin for Other {
        type State = ();
        type Data = ();
        type RepoData = ();
        type Settings = ();
        type Ui = ();

        fn name(&self) -> &'static str {
            "other"
        }

        fn manifest(&self) -> Manifest<Self> {
            Manifest::new()
        }
    }

    /// A host half takes its plugin's place: the order stays, the plugin
    /// makes its host state, and one still without its half fails to,
    /// naming itself.
    #[test]
    #[allow(
        clippy::disallowed_methods,
        reason = "a test is a synchronous entry point (ADR 0028)"
    )]
    fn a_host_half_goes_to_its_plugin_in_place() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let store = runtime.block_on(tau_store_sqlite::memory()).unwrap();
        let cx = HostCx::new(
            store,
            runtime.handle().clone(),
            crate::Services::default(),
            std::path::PathBuf::from("/data/tau"),
            Vec::new(),
            Arc::new(|_| {}),
        );
        let registry =
            Registry::new().with(Other).with(Counter).host(CounterHost);
        let names: Vec<&str> =
            registry.plugins().map(|plugin| plugin.name()).collect();
        assert_eq!(names, ["other", "counter"]);
        let counter = registry.get("counter").unwrap();
        assert!(runtime.block_on(counter.host(&cx)).is_ok());
        let other = registry.get("other").unwrap();
        let error = runtime.block_on(other.host(&cx)).err().unwrap();
        assert!(format!("{error:#}").contains("plugin other"), "{error:#}");
    }

    /// A host half for a plugin the registry does not list is a mistake
    /// in tau's own list.
    #[test]
    #[should_panic(expected = "no plugin without a host half")]
    fn a_host_half_without_its_plugin_panics() {
        let _ = Registry::new().with(Other).host(CounterHost);
    }

    /// A contribution to a point nobody declares is reported; one to the
    /// interface's own points, or to a point a plugin declares, is not.
    #[test]
    fn contributions_to_undeclared_points_are_found() {
        let registry = Registry::new().with(Counter);
        assert_eq!(
            registry.undeclared(&crate::points::ALL),
            [("counter".to_owned(), ELSEWHERE.name.to_owned())]
        );
        assert!(registry.points(&crate::points::ALL).contains(MINE.name));
    }
}
