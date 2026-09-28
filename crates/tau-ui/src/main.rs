//! Opens the interface.
//!
//! With a ChatGPT sign-in (`cargo run -p tau-ai --example codex_login`)
//! or `OPENAI_API_KEY`, `tau-ui` runs a real coding agent in the current
//! directory: type a task to start a run. Without either, or with
//! `--demo`, it replays the scripted session instead.
//!
//! - `--model <id>`: the model, `gpt-5.5` by default.
//! - `--root <dir>`: where the coding tools work.
//! - `--prompt <text>`: start a run with this task right away.
//! - `--demo`: the scripted session; `--finished` opens it done.
//! - `--open <screen>`: run, history, memory, plugins, constitution,
//!   compare, plan or ledger (demo screens).
//! - `--phone`: the phone layout in a 390×844 frame.

use std::path::PathBuf;

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
use tau_ui::{
    Workspace,
    WorkspaceEvent,
    assets::Assets,
    demo,
    host::{Access, Host, HostConfig},
};

struct Args {
    demo: bool,
    finished: bool,
    phone: bool,
    open: Option<String>,
    prompt: Option<String>,
    model: String,
    root: PathBuf,
}

fn args() -> Args {
    let args: Vec<String> = std::env::args().collect();
    let flag = |name: &str| args.iter().any(|arg| arg == name);
    let value = |name: &str| {
        args.iter()
            .position(|arg| arg == name)
            .and_then(|at| args.get(at + 1))
            .cloned()
    };
    Args {
        demo: flag("--demo"),
        finished: flag("--finished"),
        phone: flag("--phone"),
        open: value("--open"),
        prompt: value("--prompt"),
        model: value("--model").unwrap_or_else(|| "gpt-5.5".into()),
        root: value("--root")
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from(".")),
    }
}

fn main() {
    let args = args();
    let access = if args.demo { None } else { Access::detect() };
    let host = access.and_then(|access| {
        let config = HostConfig {
            access,
            model: args.model.clone(),
            root: args.root.clone(),
            store: HostConfig::default_store(),
        };
        match Host::new(config) {
            Ok(host) => Some(host),
            Err(error) => {
                eprintln!(
                    "tau-ui: cannot start agents ({error}); showing the demo"
                );
                None
            }
        }
    });
    if host.is_none() && !args.demo {
        eprintln!(
            "tau-ui: no model configured. Sign in with \
             `cargo run -p tau-ai --example codex_login` or set \
             OPENAI_API_KEY. Showing the demo."
        );
    }

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
            let name = args.root.file_name().map_or("tau".into(), |name| {
                name.to_string_lossy().into_owned()
            });
            let live = host.as_ref().map(|(host, _)| host.catalog());
            let opened = cx.open_window(options, |window, cx| {
                cx.new(|cx| match live {
                    Some(catalog) => {
                        let mut workspace = Workspace::new(
                            name,
                            Vec::new(),
                            catalog,
                            window,
                            cx,
                        );
                        workspace.set_phone_preview(args.phone, cx);
                        workspace
                    }
                    None => demo_workspace(&args, window, cx),
                })
            });
            let Ok(workspace) = opened.and_then(|window| window.entity(cx))
            else {
                eprintln!("tau-ui: could not open a window");
                cx.quit();
                return;
            };
            match host {
                Some((host, events)) => {
                    workspace.update(cx, |ws, cx| {
                        ws.navigate(tau_ui::route::Route::NewRun, cx)
                    });
                    let prompt = args.prompt.clone();
                    host.attach(&workspace, events, cx);
                    if let Some(prompt) = prompt {
                        workspace
                            .update(cx, |ws, cx| ws.submit_prompt(prompt, cx));
                    }
                }
                None => {
                    // No agent: show what a host would receive.
                    cx.subscribe(&workspace, |_, event: &WorkspaceEvent, _| {
                        eprintln!("tau-ui: {event:?}");
                    })
                    .detach();
                }
            }
            cx.activate(true);
        });
}

fn demo_workspace(
    args: &Args,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<Workspace>,
) -> Workspace {
    let mut runs = vec![demo::retry_after()];
    runs.extend(demo::history());
    let mut workspace =
        Workspace::new("tau-agent", runs, demo::catalog(), window, cx);
    workspace.set_phone_preview(args.phone, cx);
    if let Some(route) = args.open.as_deref().and_then(demo::route) {
        workspace.navigate(route, cx);
    }
    if args.finished {
        for (_, update) in demo::script() {
            workspace.update_run(&demo::run_id(), update, cx);
        }
    } else {
        workspace.replay(demo::run_id(), demo::script(), cx);
    }
    workspace
}
