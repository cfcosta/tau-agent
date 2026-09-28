//! Opens the interface on a scripted session.
//!
//! `tau-ui` replays the demo run as it would stream; `--finished` opens it
//! already done. The layout follows the window's width: below 720 px it
//! is the phone layout, and `--phone` previews that layout in a 390×844
//! frame whatever the window's size. `--open <screen>` starts on a
//! screen: run, history, memory, plugins, constitution, compare, plan or
//! ledger.

use gpui::{
    App,
    AppContext,
    Application,
    Bounds,
    TitlebarOptions,
    WindowBounds,
    WindowOptions,
    px,
    size,
};
use tau_ui::{Workspace, WorkspaceEvent, assets::Assets, demo};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let finished = args.iter().any(|arg| arg == "--finished");
    let phone = args.iter().any(|arg| arg == "--phone");
    let open = args
        .iter()
        .position(|arg| arg == "--open")
        .and_then(|at| args.get(at + 1))
        .map(|screen| demo::route(screen));
    Application::new()
        .with_assets(Assets)
        .run(move |cx: &mut App| {
            tau_ui::init(cx);
            let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Tau".into()),
                    ..Default::default()
                }),
                window_min_size: Some(size(px(360.), px(560.))),
                app_id: Some("tau-ui".into()),
                ..Default::default()
            };
            let opened = cx.open_window(options, |window, cx| {
                cx.new(|cx| {
                    let mut runs = vec![demo::retry_after()];
                    runs.extend(demo::history());
                    let mut workspace = Workspace::new(
                        "tau-agent",
                        runs,
                        demo::catalog(),
                        window,
                        cx,
                    );
                    workspace.set_phone_preview(phone, cx);
                    if let Some(Some(route)) = open.clone() {
                        workspace.navigate(route, cx);
                    }
                    if finished {
                        for (_, update) in demo::script() {
                            workspace.update_run(&demo::run_id(), update, cx);
                        }
                    } else {
                        workspace.replay(demo::run_id(), demo::script(), cx);
                    }
                    workspace
                })
            });
            let workspace = match opened {
                Ok(window) => window.entity(cx),
                Err(error) => {
                    eprintln!("tau-ui: could not open a window: {error}");
                    cx.quit();
                    return;
                }
            };
            let Ok(workspace) = workspace else {
                cx.quit();
                return;
            };
            // No agent is wired in yet: show what the host would receive.
            cx.subscribe(&workspace, |_, event: &WorkspaceEvent, _| {
                eprintln!("tau-ui: {event:?}");
            })
            .detach();
            cx.activate(true);
        });
}
