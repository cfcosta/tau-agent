//! Fakes for testing a plugin's UI half (feature `testing`).

use std::collections::BTreeMap;

use serde_json::Value;

use crate::{
    CardInfo,
    CardMark,
    Dropped,
    OutputCut,
    PluginValue,
    Registry,
    RepoCtx,
    RunCtx,
    RunCx,
    RunKind,
    Services,
    UiPlugin,
};

/// A run a fold reaches: the cards and turn it shows, and everything
/// the fold did to it. A call-targeted change lands when a card has
/// that call id.
#[derive(Debug, Clone, Default)]
pub struct FakeRun {
    pub cards: Vec<CardInfo>,
    pub last_text: Option<String>,
    pub turn: u32,
    /// The anchors placed in the transcript, in order.
    pub anchors: Vec<String>,
    /// `(call_id, key)` of the anchors attached to cards, in order.
    pub attached: Vec<(String, String)>,
    pub marks: BTreeMap<String, CardMark>,
    pub dropped: BTreeMap<String, Dropped>,
    pub cut: BTreeMap<String, OutputCut>,
    /// The keys rewrites were named by, in order.
    pub rewrites: Vec<String>,
}

impl FakeRun {
    /// A run showing `cards`.
    pub fn with_cards(cards: Vec<CardInfo>) -> Self {
        Self {
            cards,
            ..Self::default()
        }
    }

    fn has(&self, call_id: &str) -> bool {
        self.cards.iter().any(|card| card.call_id == call_id)
    }
}

impl RunCx for FakeRun {
    fn transcript(&mut self, key: &str) {
        self.anchors.push(key.to_owned());
    }

    fn attach(&mut self, call_id: &str, key: &str) -> bool {
        self.attached.push((call_id.to_owned(), key.to_owned()));
        self.has(call_id)
    }

    fn mark(&mut self, call_id: &str, mark: CardMark) -> bool {
        self.marks.insert(call_id.to_owned(), mark);
        self.has(call_id)
    }

    fn dropped(&mut self, call_id: &str, dropped: Dropped) -> bool {
        self.dropped.insert(call_id.to_owned(), dropped);
        self.has(call_id)
    }

    fn cut(&mut self, call_id: &str, cut: OutputCut) -> bool {
        self.cut.insert(call_id.to_owned(), cut);
        self.has(call_id)
    }

    fn rewrite(&mut self, key: &str) {
        self.rewrites.push(key.to_owned());
    }

    fn cards(&self) -> Vec<CardInfo> {
        self.cards.clone()
    }

    fn last_text(&self) -> Option<String> {
        self.last_text.clone()
    }

    fn turn(&self) -> u32 {
        self.turn
    }
}

/// A run of `kind` in the repository `repo` at `/tmp/repo`, on
/// `gpt-5.5`, with no services.
pub fn run_ctx(kind: RunKind) -> RunCtx {
    RunCtx {
        kind,
        repo: RepoCtx {
            name: "repo".into(),
            checkout: "/tmp/repo".into(),
            dir: "/tmp/tau/repo".into(),
        },
        model: "gpt-5.5".into(),
        effort: None,
        services: Services::default(),
    }
}

/// Folds `body` into `state` as the interface does: through `plugin`'s
/// registry entry, which reads it as one of the plugin's records, or as
/// a rewrite's details, and skips anything else.
pub fn fold<P: UiPlugin>(
    plugin: P,
    state: &mut P::State,
    body: &Value,
    run: &mut dyn RunCx,
) {
    let name = plugin.name();
    let registry = Registry::new().with(plugin);
    let mut value = PluginValue::typed(state.clone());
    registry
        .get(name)
        .expect("the plugin was just added")
        .apply(&mut value, body, run);
    *state = value.get::<P::State>().clone();
}

/// Each repository's data as the interface holds it, for a
/// [`crate::ViewCx`] made by hand: build it once, then pass
/// [`RepoValues::refs`].
pub struct RepoValues(BTreeMap<String, PluginValue>);

impl RepoValues {
    pub fn new<T>(repos: &BTreeMap<String, T>) -> Self
    where
        T: serde::Serialize + Clone + Send + Sync + 'static,
    {
        Self(
            repos
                .iter()
                .map(|(name, data)| {
                    (name.clone(), PluginValue::typed(data.clone()))
                })
                .collect(),
        )
    }

    pub fn refs(&self) -> BTreeMap<String, &PluginValue> {
        self.0
            .iter()
            .map(|(name, value)| (name.clone(), value))
            .collect()
    }
}
