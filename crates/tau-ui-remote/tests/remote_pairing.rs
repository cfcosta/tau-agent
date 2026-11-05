//! A phone that pairs with another computer leaves behind what it asked
//! of the one before: a request made for one computer never reaches
//! another.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use gpui::{Entity, TestAppContext, VisualTestContext};
use tau_agent::tool::RunId;
use tau_remote::{Address, Server, ServerConfig, ServerEvent, ServerHandle};
use tau_ui_remote::{
    Workspace,
    WorkspaceEvent,
    catalog::Catalog,
    pairing::PairStep,
    remote::{self, Platform},
    route::Route,
};
use tokio::{runtime::Runtime, sync::mpsc::UnboundedReceiver};

struct Computer {
    server: ServerHandle,
    events: UnboundedReceiver<ServerEvent>,
    _dir: tempfile::TempDir,
}

impl Computer {
    fn start(runtime: &Runtime) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let (server, events) = runtime
            .block_on(Server::start(ServerConfig {
                listen: "127.0.0.1:0".parse().unwrap(),
                dir: dir.path().to_owned(),
                host: "desk".into(),
                clock: std::sync::Arc::new(tau_remote::SystemClock),
            }))
            .unwrap();
        Self {
            server,
            events,
            _dir: dir,
        }
    }

    fn code(&self) -> String {
        self.server
            .pairing_code(Address {
                host: "127.0.0.1".into(),
                port: self.server.local_addr().port(),
            })
            .to_string()
    }

    /// The next event, answering a phone that needs everything with
    /// nothing.
    fn event(&mut self, runtime: &Runtime) -> Option<ServerEvent> {
        let event = runtime.block_on(async {
            tokio::time::timeout(Duration::from_millis(100), self.events.recv())
                .await
                .ok()
                .flatten()
        })?;
        if let ServerEvent::NeedSnapshot { conn, .. } = &event {
            self.server.snapshot(*conn, Vec::new());
        }
        Some(event)
    }
}

fn wait_for(
    cx: &mut VisualTestContext,
    workspace: &Entity<Workspace>,
    route: Route,
) {
    for _ in 0..500 {
        cx.run_until_parked();
        if workspace.read_with(cx, |ws, _| ws.route() == &route) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the phone never showed {route:?}");
}

#[gpui::test]
fn a_request_for_one_computer_never_reaches_the_next(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let mut first = Computer::start(&runtime);
    let mut second = Computer::start(&runtime);
    let phone_dir = tempfile::tempdir().unwrap();
    let code = Arc::new(Mutex::new(first.code()));

    cx.update(tau_ui_remote::init);
    let window = cx.add_window(|window, cx| {
        Workspace::new("tau", Vec::new(), Catalog::default(), window, cx)
    });
    let workspace = window.root(cx).unwrap();
    let mut phone = VisualTestContext::from_window(window.into(), cx);
    let scanned = code.clone();
    let platform = Platform {
        dir: phone_dir.path().to_owned(),
        scan: Arc::new(move || Ok(Some(scanned.lock().unwrap().clone()))),
        name: "Test phone".into(),
    };
    phone.update(|_, cx| remote::connect(platform, &workspace, cx));
    workspace.update(&mut phone, |ws, cx| ws.scan_pairing_code(cx));
    while first.event(&runtime).is_some() {}
    wait_for(&mut phone, &workspace, Route::Pair(PairStep::Paired));

    // The first computer goes; the person asks for something meanwhile,
    // then pairs with the second.
    first.server.stop();
    std::thread::sleep(Duration::from_millis(100));
    phone.run_until_parked();
    let asked = WorkspaceEvent::Cancel {
        run: RunId("for-the-first".into()),
    };
    workspace.update(&mut phone, |_, cx| cx.emit(asked.clone()));
    *code.lock().unwrap() = second.code();
    workspace.update(&mut phone, |ws, cx| ws.scan_pairing_code(cx));
    wait_for(&mut phone, &workspace, Route::Pair(PairStep::Paired));
    for _ in 0..10 {
        phone.run_until_parked();
        while let Some(event) = second.event(&runtime) {
            if let ServerEvent::Up { body, .. } = event {
                assert!(
                    !body.to_string().contains("for-the-first"),
                    "the second computer got what was asked of the first: \
                     {body}"
                );
            }
        }
    }
    drop(runtime);
}
