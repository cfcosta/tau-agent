//! A computer and a phone in one process, over the loopback: the phone
//! pairs by the computer's code, shows what the computer shows, and its
//! requests reach the computer's host (decision 0013).

use std::{cell::RefCell, rc::Rc, sync::Arc, time::Duration};

use gpui::{Entity, TestAppContext, VisualTestContext};
use tau_ui::{demo, phone_server};
use tau_ui_remote::{
    Workspace,
    WorkspaceEvent,
    catalog::Catalog,
    pairing::PairStep,
    phones::PhonesRequest,
    remote::{self, Platform},
    route::Route,
    update::HostUpdate,
};

fn window(
    cx: &mut TestAppContext,
    runs: Vec<tau_ui_remote::view::RunView>,
) -> (Entity<Workspace>, VisualTestContext) {
    cx.update(tau_ui_remote::init);
    let window = cx.add_window(|window, cx| {
        Workspace::new("tau", runs, Catalog::default(), window, cx)
    });
    let workspace = window.root(cx).unwrap();
    (workspace, VisualTestContext::from_window(window.into(), cx))
}

/// Runs GPUI until `done` holds, while the network works on its own
/// threads.
fn until(
    cx: &mut VisualTestContext,
    what: &str,
    mut done: impl FnMut(&mut VisualTestContext) -> bool,
) {
    for _ in 0..200 {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("timed out waiting for {what}");
}

#[gpui::test]
fn a_phone_pairs_and_steers_the_computer(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let computer_dir = tempfile::tempdir().unwrap();
    let phone_dir = tempfile::tempdir().unwrap();
    // Phones allowed, on the loopback, on any free port.
    std::fs::write(
        computer_dir.path().join("settings.json"),
        r#"{"allow": true, "listen": "127.0.0.1", "port": 0}"#,
    )
    .unwrap();

    let (desktop, mut desktop_cx) = window(cx, demo::history());
    let asked = Rc::new(RefCell::new(Vec::new()));
    let seen = asked.clone();
    desktop_cx.update(|_, cx| {
        phone_server::serve(
            runtime.handle().clone(),
            computer_dir.path().to_owned(),
            &desktop,
            cx,
        );
        cx.subscribe(&desktop, move |_, event: &WorkspaceEvent, _| {
            seen.borrow_mut().push(event.clone())
        })
        .detach();
    });
    desktop.update(&mut desktop_cx, |ws, cx| {
        assert!(ws.phones().listening.is_some(), "{:?}", ws.phones().error);
        ws.ask_phones(PhonesRequest::ShowCode, cx);
    });
    desktop_cx.run_until_parked();
    let code = desktop.update(&mut desktop_cx, |ws, _| {
        ws.phones().code.as_ref().unwrap().code.to_string()
    });

    // The phone's camera reads the code on the computer's screen.
    let (phone, mut phone_cx) = window(cx, Vec::new());
    let platform = Platform {
        dir: phone_dir.path().to_owned(),
        scan: Arc::new(move || Ok(Some(code.clone()))),
        name: "Test phone".into(),
    };
    phone_cx.update(|_, cx| remote::connect(platform, &phone, cx));
    phone.update(&mut phone_cx, |ws, cx| {
        assert_eq!(ws.route(), &Route::Pair(PairStep::Welcome));
        ws.scan_pairing_code(cx);
    });
    until(&mut phone_cx, "the phone to pair", |cx| {
        phone.update(cx, |ws, _| ws.route() == &Route::Pair(PairStep::Paired))
    });
    // It now shows what the computer shows.
    let runs = desktop.update(&mut desktop_cx, |ws, _| ws.runs().to_vec());
    until(&mut phone_cx, "the snapshot", |cx| {
        phone.update(cx, |ws, _| ws.runs() == runs.as_slice())
    });
    desktop.update(&mut desktop_cx, |ws, _| {
        assert_eq!(ws.phones().paired.len(), 1);
        assert!(ws.phones().code.is_none(), "a used code goes away");
    });
    phone.update(&mut phone_cx, |ws, cx| ws.open_tau(cx));

    // What the phone asks for reaches the computer's host.
    let run = runs[0].id.clone();
    phone.update(&mut phone_cx, |_, cx| {
        cx.emit(WorkspaceEvent::Cancel { run: run.clone() })
    });
    until(&mut desktop_cx, "the phone's request", |_| {
        asked
            .borrow()
            .contains(&WorkspaceEvent::Cancel { run: run.clone() })
    });

    // And what the computer's host applies reaches the phone.
    desktop.update(&mut desktop_cx, |ws, cx| {
        ws.apply(HostUpdate::alert("Could not cancel", "it had ended"), cx)
    });
    until(&mut phone_cx, "the host's update", |cx| {
        phone.update(cx, |ws, _| {
            ws.alert() == Some(("Could not cancel", "it had ended"))
        })
    });
    drop(runtime);
}
