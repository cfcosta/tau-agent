//! Opens the interface.
//!
//! With a ChatGPT sign-in or an OpenAI API key, `tau-ui` runs a real
//! coding agent in the current directory: type a task to start a run.
//! Without either it opens onboarding to set one up. With `--demo` it
//! replays the scripted session instead.
//!
//! - `--model <id>`: the model, `gpt-5.5` by default.
//! - `--root <dir>`: where the coding tools work.
//! - `--prompt <text>`: start a run with this task right away.
//! - `--demo`: the scripted session; `--finished` opens it done.
//! - `--open <screen>`: run, history, memory, plugins, constitution,
//!   compare, plan or ledger; onboarding's welcome, github, token,
//!   model, repos or ready; pr and pr-opened; alert, a sample dialog;
//!   models, picker, model-info or fork-picker (demo screens).
//! - `--phone`: the phone layout in a 390×844 frame.
//! - `--frame <w>x<h>`: lay out at exactly that size in the top-left
//!   corner, to compare with the designs.
//! - `--steps <n>`: stop the demo script after its first `n` updates.

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
    accounts::Credentials,
    assets::Assets,
    catalog::Catalog,
    demo,
    host::{self, Host, HostConfig},
    route::Route,
    setup::SetupStep,
};

struct Args {
    demo: bool,
    finished: bool,
    phone: bool,
    open: Option<String>,
    prompt: Option<String>,
    frame: Option<(f32, f32)>,
    steps: Option<usize>,
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
        frame: value("--frame").and_then(|frame| {
            let (w, h) = frame.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        }),
        steps: value("--steps").and_then(|steps| steps.parse().ok()),
        model: value("--model").unwrap_or_else(|| "gpt-5.5".into()),
        root: value("--root")
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from(".")),
    }
}

fn main() {
    let args = args();
    let credentials = Credentials::default_dir();
    let access = if args.demo {
        None
    } else {
        credentials.access()
    };
    let (model, root) = (args.model.clone(), args.root.clone());
    let saved = credentials.clone();
    let config = move |access| HostConfig {
        access,
        credentials: saved.clone(),
        model: model.clone(),
        root: root.clone(),
        store: HostConfig::default_store(),
        repos: HostConfig::default_repos(),
        settings: HostConfig::default_settings(),
        repo_list: HostConfig::default_repo_list(),
    };
    let host = access.and_then(|access| match Host::new(config(access)) {
        Ok(host) => Some(host),
        Err(error) => {
            eprintln!("tau-ui: cannot start agents: {error}");
            None
        }
    });
    let onboarding = host.is_none() && !args.demo;

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
            let live = match &host {
                Some((host, _)) => Some(host.catalog()),
                None if onboarding => Some(Catalog::default()),
                None => None,
            };
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
                        workspace.set_frame(args.frame, cx);
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
                    workspace
                        .update(cx, |ws, cx| ws.navigate(Route::NewRun, cx));
                    host.attach(&workspace, events, cx);
                    if let Some(prompt) = args.prompt.clone() {
                        workspace
                            .update(cx, |ws, cx| ws.submit_prompt(prompt, cx));
                    }
                }
                // No model yet: set one up, then start the host.
                None if onboarding => {
                    workspace.update(cx, |ws, cx| {
                        ws.start_setup(SetupStep::Model, cx)
                    });
                    let entity = workspace.clone();
                    host::onboard(
                        &workspace,
                        args.model.clone(),
                        credentials.clone(),
                        cx,
                        move |access, cx| match Host::new(config(access)) {
                            Ok((host, events)) => {
                                let catalog = host.catalog();
                                entity.update(cx, |ws, cx| {
                                    ws.set_catalog(catalog, cx)
                                });
                                host.attach(&entity, events, cx);
                            }
                            Err(error) => {
                                eprintln!(
                                    "tau-ui: cannot start agents: {error}"
                                )
                            }
                        },
                    );
                }
                // The demo: answer the way a host would.
                None => {
                    demo::respond(&workspace, cx);
                    // After the responder, so what the screen asks for
                    // on opening is answered.
                    workspace
                        .update(cx, |ws, cx| open_demo_screen(ws, &args, cx));
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
    workspace.set_frame(args.frame, cx);
    if let Some(steps) = args.steps {
        for (_, update) in demo::script().into_iter().take(steps) {
            workspace.update_run(&demo::run_id(), update, cx);
        }
    } else if args.finished {
        for (_, update) in demo::script() {
            workspace.update_run(&demo::run_id(), update, cx);
        }
    } else {
        workspace.replay(demo::run_id(), demo::script(), cx);
    }
    workspace
}

/// The screen `--open` names, with the demo data it needs.
fn open_demo_screen(
    workspace: &mut Workspace,
    args: &Args,
    cx: &mut gpui::Context<Workspace>,
) {
    // The model picker's states, for looking at them.
    match args.open.as_deref() {
        Some("picker") => {
            workspace.navigate(Route::NewRun, cx);
            workspace.show_picker(tau_ui::workspace::PickerTarget::Next, cx);
            return;
        }
        Some("model-info") => {
            workspace.navigate(Route::Run(demo::run_id()), cx);
            workspace.show_model_info(cx);
            return;
        }
        Some("fork-picker") => {
            workspace.navigate(Route::Run(demo::run_id()), cx);
            workspace.start_fork_at(&demo::run_id(), 2, cx);
            workspace.show_picker(tau_ui::workspace::PickerTarget::Fork, cx);
            return;
        }
        _ => {}
    }
    // The repository tree's states.
    match args.open.as_deref() {
        Some("add-repo") => {
            workspace.ask_for_repo(cx);
            return;
        }
        Some("repo-menu") => {
            workspace.toggle_repo_open("docbert", cx);
            workspace.toggle_repo_menu("homelab.nix", cx);
            return;
        }
        Some("repo-open") => {
            workspace.toggle_repo_open("docbert", cx);
            return;
        }
        _ => {}
    }
    if args.open.as_deref() == Some("alert") {
        workspace.show_alert(
            "Could not fork the run",
            "Forking needs a project: /home/you/notes could not be copied: \
             it is not a Git repository.",
            cx,
        );
        return;
    }
    match args.open.as_deref().and_then(demo::route) {
        Some(Route::Setup(step)) => {
            workspace.set_setup(demo::setup(step), cx);
            workspace.navigate(Route::Setup(step), cx);
        }
        Some(Route::PullRequest(run)) => {
            let mut pr = demo::pull_request();
            if args.open.as_deref() == Some("pr-opened") {
                pr.state = demo::opened();
            }
            workspace.set_pull_request(&run, pr, cx);
            workspace.open_pull_request(&run, cx);
        }
        Some(route) => workspace.navigate(route, cx),
        None => {}
    }
}
