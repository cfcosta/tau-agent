//! heiko-sqlite's host half: it fills `heiko.store` as the host starts.

use std::{path::Path, sync::Arc};

use heiko_agent::plugin::Plugin;
use heiko_ui_plugin::{
    Group,
    HostCx,
    HostHalf,
    PluginInfo,
    RunCtx,
    slots::Fills,
    store::{OpenStore, Opened, STORE},
};

use crate::SqliteUi;

/// The host half for heiko's registry.
#[derive(Debug, Clone, Copy, Default)]
pub struct SqliteHost;

impl HostHalf for SqliteHost {
    type Plugin = SqliteUi;
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
            description: "Keeps every run, and each plugin's records, in \
                          SQLite in heiko's data directory"
                .into(),
            group: Group::App,
            ..Default::default()
        }
    }

    fn fill_early(&self, fills: &mut Fills) {
        fills.fill(&STORE, Arc::new(Sqlite) as Arc<dyn OpenStore>);
    }
}

/// Opens stores with heiko-store-sqlite.
struct Sqlite;

impl OpenStore for Sqlite {
    fn open<'a>(&'a self, path: &'a Path) -> Opened<'a> {
        Box::pin(async move { Ok(heiko_store_sqlite::open(path).await?) })
    }
}
