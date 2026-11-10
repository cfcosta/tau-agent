//! tau-github against a fake GitHub (ADR 0050, gaps E and H): what Luau's
//! fake HTTP cannot say, the host's requests and its secret store, the
//! token kept from GitHub's answer without the plugin seeing it.

mod fake;
mod real;

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tau_luau_plugins_host::runtime::{
    Context,
    Files,
    Hook,
    Loaded,
    NoReach,
    PLUGIN,
    StoreRef,
    load,
};
use tau_testing::block_on_io;
use tokio_util::sync::CancellationToken;

/// tau-github, its endpoints the fake GitHub at `base`, with a secret
/// store and its store of the app of its own.
pub struct Luau {
    pub loaded: Loaded,
    pub secrets: PathBuf,
    pub store: StoreRef,
    pub now: std::cell::Cell<i64>,
    _dir: tempfile::TempDir,
}

/// Copies `from` into `to`, and all under it.
fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            copy(&path, &to.join(path.file_name().unwrap()));
        } else {
            std::fs::copy(&path, to.join(path.file_name().unwrap())).unwrap();
        }
    }
}

impl Luau {
    pub fn at(base: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("tau-github");
        copy(
            &tau_luau_plugins::packages::find()
                .unwrap()
                .join(tau_luau_plugins::names::GITHUB),
            &package,
        );
        let host = base
            .trim_start_matches("http://")
            .split(':')
            .next()
            .unwrap()
            .to_owned();
        std::fs::write(
            package.join("src/endpoints.luau"),
            format!(
                "return {{ web = {base:?}, api = {base:?}, hosts = {{ {host:?} }}, \
                 client_id = \"Iv23lisaZLq1FOECQUNe\", app_slug = \"ascend-repository-cfcosta\" }}\n"
            ),
        )
        .unwrap();
        let files = Files::read(&package, PLUGIN).unwrap();
        let loaded = block_on_io(load("tau-github", files)).unwrap();
        let store = block_on_io(async {
            StoreRef(std::sync::Arc::new(
                tau_luau_plugins_host::store::Kv::memory(
                    tau_luau_plugins_host::store::Fuse::never(),
                )
                .await
                .unwrap(),
            ))
        });
        Self {
            loaded,
            secrets: dir.path().join("secrets.json"),
            store,
            now: std::cell::Cell::new(1_794_302_400),
            _dir: dir,
        }
    }

    /// The secret store's `github`, if set.
    pub fn kept(&self) -> Option<String> {
        block_on_io(
            tau_tools_host::secrets::SecretStore::new(&self.secrets).read(),
        )
        .ok()?
        .get("github")
        .cloned()
    }

    /// What `hook` answers with `input`, or why it failed.
    pub fn call(&self, hook: Hook, input: Value) -> Result<Value, String> {
        let context = Context {
            run: json!({ "kind": "app" }),
            now: json!({ "unix": self.now.get() }),
            settings: json!({}),
            state: json!({}),
            secrets: Some(self.secrets.clone()),
            store: Some(self.store.clone()),
            job: None,
        };
        let outcome = block_on_io(self.loaded.call(
            &hook,
            input,
            &context,
            std::sync::Arc::new(NoReach),
            CancellationToken::new(),
        ));
        match outcome.error {
            Some(error) => Err(error),
            None => Ok(outcome.value),
        }
    }
}
