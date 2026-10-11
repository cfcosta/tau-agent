//! The MCP resources a host half reads and watches outside any run
//! (ADR 0037): each server goes to the first source that has it, a
//! watch hears what its source says until the source lets it go, and a
//! server no source has says so.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use serde_json::{Value, json};
use tau_ui_plugin::{
    OutsideResources,
    ResourceSource,
    ResourceWatch,
    WatchError,
    registry::HostFuture,
};
use tokio::sync::mpsc;

/// Has the server `server`: reads answer where they were asked, and
/// watches hear what goes through its senders.
struct Server {
    server: &'static str,
    senders: Mutex<Vec<mpsc::UnboundedSender<()>>>,
}

impl Server {
    fn new(server: &'static str) -> Arc<Self> {
        Arc::new(Self {
            server,
            senders: Mutex::default(),
        })
    }

    /// Says every watch's resource changed.
    fn change(&self) {
        for sender in self.senders.lock().unwrap().iter() {
            let _ = sender.send(());
        }
    }

    /// Lets every watch go, as a connection that ended does.
    fn end(&self) {
        self.senders.lock().unwrap().clear();
    }
}

impl ResourceSource for Server {
    fn read<'a>(
        &'a self,
        checkout: &'a Path,
        server: &'a str,
        uri: &'a str,
    ) -> Option<HostFuture<'a, Result<Value, String>>> {
        (server == self.server).then(|| {
            let answer = json!({
                "server": server,
                "checkout": checkout.display().to_string(),
                "uri": uri,
            });
            Box::pin(async move { Ok(answer) }) as HostFuture<'a, _>
        })
    }

    fn watch<'a>(
        &'a self,
        _checkout: &'a Path,
        server: &'a str,
        _uri: &'a str,
    ) -> Option<HostFuture<'a, Result<ResourceWatch, WatchError>>> {
        (server == self.server).then(|| {
            let (sender, updates) = mpsc::unbounded_channel();
            self.senders.lock().unwrap().push(sender);
            Box::pin(async move { Ok(ResourceWatch::new(updates, ())) })
                as HostFuture<'a, _>
        })
    }
}

fn block_on<T>(future: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(future)
}

#[test]
fn a_read_goes_to_the_source_that_has_its_server() {
    let resources = OutsideResources::default();
    resources.offer(Server::new("ci"));
    // A clone is the same set of sources.
    resources.clone().offer(Server::new("github"));
    let answer = block_on(resources.read(
        Path::new("/agents/a"),
        "github",
        "repo://tau",
    ))
    .unwrap();
    assert_eq!(answer["server"], "github");
    assert_eq!(answer["checkout"], "/agents/a");
    assert_eq!(answer["uri"], "repo://tau");
}

#[test]
fn a_watch_hears_its_source_until_it_lets_go() {
    let resources = OutsideResources::default();
    let ci = Server::new("ci");
    resources.offer(ci.clone());
    block_on(async {
        let mut watch = resources
            .watch(Path::new("/a"), "ci", "ci://main")
            .await
            .unwrap();
        ci.change();
        assert!(watch.changed().await);
        ci.end();
        assert!(!watch.changed().await);
    });
}

#[test]
fn a_server_no_source_has_says_so() {
    let resources = OutsideResources::default();
    resources.offer(Server::new("ci"));
    let error =
        block_on(resources.read(Path::new("/a"), "jira", "x")).unwrap_err();
    assert!(error.contains("`jira`"), "{error}");
    let error = block_on(resources.watch(Path::new("/a"), "jira", "x"))
        .expect_err("no source has jira");
    assert!(
        matches!(&error, WatchError::Failed(why) if why.contains("`jira`")),
        "{error:?}"
    );
}
