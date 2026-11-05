//! The phone's remote lives as long as its window: once the window is
//! gone, as when the app closes, it reaches its computer no more.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{sync::Arc, time::Duration};

use gpui::{TestAppContext, VisualTestContext};
use tau_remote::{Address, Server, ServerConfig, ServerEvent};
use tau_ui_remote::{
    Workspace,
    catalog::Catalog,
    pairing::PairStep,
    remote::{self, Platform},
    route::Route,
};

#[gpui::test]
fn a_closed_window_leaves_the_computer(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let computer_dir = tempfile::tempdir().unwrap();
    let phone_dir = tempfile::tempdir().unwrap();
    let (server, mut events) = runtime
        .block_on(Server::start(ServerConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            dir: computer_dir.path().to_owned(),
            host: "desk".into(),
            clock: std::sync::Arc::new(tau_remote::SystemClock),
        }))
        .unwrap();
    let code = server
        .pairing_code(Address {
            host: "127.0.0.1".into(),
            port: server.local_addr().port(),
        })
        .to_string();

    cx.update(tau_ui_remote::init);
    let window = cx.add_window(|window, cx| {
        Workspace::new("tau", Vec::new(), Catalog::default(), window, cx)
    });
    let workspace = window.root(cx).unwrap();
    let mut phone_cx = VisualTestContext::from_window(window.into(), cx);
    let platform = Platform {
        dir: phone_dir.path().to_owned(),
        scan: Arc::new(move || Ok(Some(code.clone()))),
        name: "Test phone".into(),
    };
    phone_cx.update(|_, cx| remote::connect(platform, &workspace, cx));
    workspace.update(&mut phone_cx, |ws, cx| ws.scan_pairing_code(cx));
    let next = |events: &mut tokio::sync::mpsc::UnboundedReceiver<_>| {
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), events.recv())
                .await
                .expect("an event in time")
                .expect("the server runs")
        })
    };
    loop {
        if let ServerEvent::NeedSnapshot { conn, .. } = next(&mut events) {
            server.snapshot(conn, Vec::new());
            break;
        }
    }
    for _ in 0..200 {
        phone_cx.run_until_parked();
        if workspace.read_with(&phone_cx, |ws, _| {
            ws.route() == &Route::Pair(PairStep::Paired)
        }) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    // The app closes: its window, and with it the phone's connection.
    phone_cx.update(|window, _| window.remove_window());
    drop(workspace);
    cx.update(|_| {});
    loop {
        if let ServerEvent::Left { .. } = next(&mut events) {
            break;
        }
    }
    drop(runtime);
}
