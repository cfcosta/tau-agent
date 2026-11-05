//! The chain of compactions, as a plugin (ADR 0045): it fills
//! `tau.compaction` with fast pruning, the tree and the summary, which
//! rewrite in that turn, as each declares.

use std::sync::Arc;

use tau_agent::plugin::Plugin;
use tau_ui_plugin::{
    Declaration,
    Group,
    HostCx,
    HostHalf,
    Manifest,
    PluginInfo,
    RunCtx,
    UiPlugin,
    compaction::{COMPACTION, Compaction},
    slots::Fills,
};

pub const NAME: &str = "tau-compaction-chain";

/// The rewriters, in their turn.
pub const CHAIN: [&str; 3] = [
    tau_fast_compaction::NAME,
    tau_tree_compaction::NAME,
    tau_compaction::NAME,
];

/// tau-compaction-chain with its UI: its row on the Plugins screen.
#[derive(Debug, Clone, Copy, Default)]
pub struct ChainUi;

impl UiPlugin for ChainUi {
    type State = ();
    type Data = ();
    type RepoData = ();
    type Settings = ();

    fn name(&self) -> &'static str {
        NAME
    }

    fn declaration(&self) -> Declaration {
        Declaration::new().fills(&[COMPACTION.name()])
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
    }
}

/// The host half for tau's registry.
#[derive(Debug, Clone, Copy, Default)]
pub struct ChainHost;

impl HostHalf for ChainHost {
    type Plugin = ChainUi;
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
        PluginInfo {
            description: "Compacts with fast pruning, then the tree, then \
                          the summary, each declining when it would save too \
                          little"
                .into(),
            group: Group::Context,
            ..Default::default()
        }
    }

    fn fill(&self, _: &(), fills: &mut Fills) {
        fills.fill(&COMPACTION, Arc::new(Chain) as Arc<dyn Compaction>);
    }
}

/// The three in turn.
struct Chain;

impl Compaction for Chain {
    fn rewriters(&self) -> Vec<String> {
        CHAIN.iter().map(|name| (*name).to_owned()).collect()
    }
}
