//! A computer and a phone in one process, over the loopback: the phone
//! pairs by the computer's code, shows what the computer shows, and its
//! requests reach the computer's host (decision 0013).
#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{cell::RefCell, rc::Rc, sync::Arc, time::Duration};

use gpui::{Entity, TestAppContext, VisualTestContext};
use tau_phones::{Act, Phones};
use tau_ui::{demo, mirror};
use tau_ui_remote::{
    Workspace,
    WorkspaceEvent,
    catalog::Catalog,
    pairing::PairStep,
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
    let app = desktop_cx.update(|_, cx| {
        cx.subscribe(&desktop, move |_, event: &WorkspaceEvent, _| {
            seen.borrow_mut().push(event.clone())
        })
        .detach();
        mirror::app(&desktop, cx)
    });
    let phones = runtime.block_on(Phones::open(
        computer_dir.path().to_owned(),
        app,
        Arc::new(|| {}),
    ));
    phones.resume();
    // The server starts on the host's runtime, a moment after.
    until(&mut desktop_cx, "the server to listen", |_| {
        phones.listening().is_some() || phones.error().is_some()
    });
    assert!(phones.listening().is_some(), "{:?}", phones.error());
    runtime.block_on(phones.act(Act::ShowCode));
    let code = phones.code().unwrap().to_string();

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
    until(&mut desktop_cx, "the computer to list the phone", |_| {
        phones.paired().len() == 1
    });
    assert!(phones.code().is_none(), "a used code goes away");
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

    // A picture the phone asks for reaches the computer, and the answer
    // the phone, unnumbered: what the phone had of the feed is as before.
    let picture = tau_ui_remote::view::Picture {
        key: "cd".repeat(32),
        mime: "image/png".into(),
    };
    let asked_for = WorkspaceEvent::Picture {
        run: run.clone(),
        picture: picture.clone(),
    };
    phone.update(&mut phone_cx, |_, cx| cx.emit(asked_for.clone()));
    until(&mut desktop_cx, "the phone's picture request", |_| {
        asked.borrow().contains(&asked_for)
    });
    let png = {
        use base64::Engine as _;
        let mut bytes = Vec::new();
        image::RgbImage::new(2, 2)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        base64::engine::general_purpose::STANDARD.encode(bytes)
    };
    desktop.update(&mut desktop_cx, |ws, cx| {
        ws.apply(
            HostUpdate::Picture {
                picture: picture.clone(),
                data: Some(png),
                error: None,
            },
            cx,
        )
    });
    until(&mut phone_cx, "the picture", |cx| {
        phone.update(cx, |ws, _| ws.has_picture(&picture.key))
    });
    assert!(mirror::transient(&HostUpdate::Picture {
        picture,
        data: None,
        error: None,
    }));
    drop(runtime);
}

/// What a plugin that only the computer may reach keeps never goes to
/// a phone: tau-chatgpt's sign-in, in the catalog and in the views the
/// computer builds for a phone.
#[test]
fn a_computer_only_plugin_s_data_and_views_stay_on_the_computer() {
    use tau_ui_plugin::wire::{
        AtKey,
        Built,
        Contributed,
        Layout,
        View,
        ViewKey,
    };
    use tau_ui_remote::update::HostUpdate;
    tau_ui::hosted::install();
    let chatgpt = tau_ui_plugin::PluginValue::from_json(serde_json::json!({
        "sign_in": { "state": "waiting", "url": "https://auth.openai.com/x" },
    }));
    let mut catalog = tau_ui_remote::catalog::Catalog::default();
    catalog
        .plugin_data
        .insert(tau_chatgpt::NAME.into(), chatgpt);
    catalog.plugin_data.insert(
        tau_github::NAME.into(),
        tau_ui_plugin::PluginValue::from_json(serde_json::json!({})),
    );
    let sent = mirror::to_phones(&HostUpdate::catalog(catalog)).unwrap();
    let data = &sent["Catalog"]["plugin_data"];
    assert!(data.get(tau_chatgpt::NAME).is_none(), "{data}");
    assert!(data.get(tau_github::NAME).is_some(), "{data}");

    let layout = Layout::default();
    let page = ViewKey {
        view: View::Page {
            plugin: tau_chatgpt::NAME.into(),
            page: "any".into(),
            params: Default::default(),
        },
        layout,
    };
    let point = ViewKey {
        view: View::Point {
            point: tau_ui_plugin::points::MODELS.name.into(),
            at: AtKey::App,
        },
        layout,
    };
    let outs = |plugin: &str| Contributed {
        plugin: plugin.into(),
        out: serde_json::Value::Null,
    };
    let answers = HostUpdate::Views {
        answers: vec![
            (
                page,
                Built::Title {
                    title: "secret".into(),
                },
            ),
            (
                point,
                Built::Outs {
                    outs: vec![outs(tau_chatgpt::NAME), outs(tau_github::NAME)],
                },
            ),
        ],
    };
    let HostUpdate::Views { answers } = answers.for_phones() else {
        unreachable!()
    };
    assert_eq!(answers[0].1, Built::Nothing);
    assert_eq!(
        answers[1].1,
        Built::Outs {
            outs: vec![outs(tau_github::NAME)]
        }
    );
}
