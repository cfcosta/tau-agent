//! Opens the interface.
//!
//! `tau` runs a real coding agent in the repositories cloned from
//! GitHub, on what fills `tau.provider`: type a task to start a run.
//! With no model to run on yet, it opens setup to sign in. With `--demo` it
//! replays the scripted session instead.
//!
//! - `--model <id>`: the model; gpt-6.1-sol by default.
//! - `--prompt <text>`: start a run with this task right away.
//! - `--demo`: the scripted session; `--finished` opens it done.
//! - `--open <screen>`: a screen of the demo, by its name in
//!   `tau_demo::SCREENS`: run, plugins, constitution, pr-opened,
//!   pair-scan, rules-broken, and so on.
//!   With a sign-in, only `page:<plugin>/<page>[?key=value&...]`, a
//!   plugin's page, such as `page:tau-mcp/servers?repo=` for the MCP
//!   servers outside a repository.
//! - `--phone`: the phone layout in a 390×844 frame.
//! - `--frame <w>x<h>`: lay out at exactly that size in the top-left
//!   corner, to compare with the designs.
//! - `--steps <n>`: stop the demo script after its first `n` updates.
//! - `--reduce-motion`: no looping motion, and transitions as plain
//!   fades. `TAU_REDUCE_MOTION=1` asks the same (`0` asks for all the
//!   motion), then `"reduce_motion": true` in `interface.json`, then
//!   the desktop's `enable-animations`.

use gpui::{
    App,
    AppContext,
    Bounds,
    TitlebarOptions,
    WindowBounds,
    WindowOptions,
    px,
    size,
};
use tau_demo as demo;
use tau_ui::{
    accounts::Credentials,
    host::{Host, HostConfig},
};
use tau_ui_remote::{
    Workspace,
    assets::Assets,
    catalog::Catalog,
    motion::MotionPreference,
    route::Route,
    setup::SetupStep,
    update::HostUpdate,
};

struct Args {
    demo: bool,
    finished: bool,
    phone: bool,
    open: Option<String>,
    prompt: Option<String>,
    frame: Option<(f32, f32)>,
    steps: Option<usize>,
    /// `None` takes gpt-6.1-sol.
    model: Option<String>,
    reduce_motion: bool,
}

/// `host`, with notices for what needs the person while tau is in the
/// background, given to what fills its `tau.notify` slot
/// ([`tau_ui::notify::follow`]), and when a turn leaves conflicts on a
/// main chat ([`tau_ui::notify::conflicts_on_main`]).
fn with_notices(
    host: Host,
    workspace: &gpui::Entity<Workspace>,
    window: gpui::AnyWindowHandle,
    cx: &mut gpui::App,
) -> Host {
    tau_ui::notify::follow(workspace, window, host.slots(), cx);
    host.on_conflicts_on_main(std::sync::Arc::new(
        tau_ui::notify::conflicts_on_main,
    ))
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
        model: value("--model"),
        reduce_motion: flag("--reduce-motion"),
    }
}

fn main() {
    // Every plugin's host half, the Luau ones included (ADR 0030).
    tau::install();
    let args = args();
    let credentials = Credentials::default_dir();
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let config = HostConfig {
        model: args.model.clone(),
        credentials: credentials.clone(),
        store: HostConfig::default_store(),
        repos: HostConfig::default_repos(),
        settings: HostConfig::default_settings(),
        repo_list: HostConfig::default_repo_list(),
        home: home.clone(),
        services: tau_ui::hosted::app_services(home.as_deref()),
    };
    // The host starts signed out or in: what fills `tau.provider` says
    // whether there is a model to run on, and setup opens when there is
    // none (ADR 0045).
    let host = (!args.demo).then(|| Host::new(config));

    gpui_platform::application().with_assets(Assets).run(
        move |cx: &mut App| {
            tau_ui_remote::init(cx);
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
            let name = "tau";
            let live = match &host {
                Some(Ok((host, _))) => Some(host.catalog_now()),
                Some(Err(_)) => Some(Catalog::default()),
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
            let Ok((workspace, window)) = opened.and_then(|window| {
                Ok((window.entity(cx)?, gpui::AnyWindowHandle::from(window)))
            }) else {
                eprintln!("tau: could not open a window");
                cx.quit();
                return;
            };
            workspace.update(cx, |ws, cx| {
                ws.use_interface_settings(
                    HostConfig::default_interface_settings(),
                    cx,
                )
            });
            follow_motion_preference(&workspace, args.reduce_motion, cx);
            match host {
                Some(Ok((host, events))) => {
                    let signed_in = host.signed_in();
                    with_notices(host, &workspace, window, cx)
                        .attach(&workspace, events, cx);
                    workspace.update(cx, |ws, cx| {
                        if signed_in {
                            ws.navigate(Route::NewRun, cx)
                        } else {
                            // No model yet: set one up.
                            ws.start_setup(SetupStep::Welcome, cx)
                        }
                    });
                    if let Some(prompt) = args.prompt.clone() {
                        workspace
                            .update(cx, |ws, cx| ws.submit_prompt(prompt, cx));
                    }
                    if let Some(route) =
                        args.open.as_deref().and_then(plugin_page)
                    {
                        workspace.update(cx, |ws, cx| ws.navigate(route, cx));
                    }
                }
                Some(Err(error)) => {
                    eprintln!("tau: cannot start agents: {error:#}");
                    workspace.update(cx, |ws, cx| {
                        ws.apply(
                            HostUpdate::alert(
                                "tau cannot start agents",
                                format!("{error:#}"),
                            ),
                            cx,
                        )
                    });
                }
                // The demo: answer the way a host would.
                None => {
                    let host = demo::start(&workspace, cx);
                    // After the host, so what the screen asks for on
                    // opening is answered.
                    if let Some(name) = args.open.as_deref() {
                        workspace.update(cx, |ws, cx| {
                            if !demo::open(name, ws, &host, cx) {
                                eprintln!("tau: the demo has no screen {name}");
                            }
                        });
                    }
                }
            }
            cx.activate(true);
        },
    );
}

/// The plugin page `--open page:<plugin>/<page>[?key=value&...]` names,
/// such as `page:tau-mcp/servers?repo=`.
fn plugin_page(open: &str) -> Option<Route> {
    let rest = open.strip_prefix("page:")?;
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (plugin, page) = path.split_once('/')?;
    let params = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (key.to_owned(), value.to_owned())
        })
        .collect();
    Some(Route::Plugin {
        plugin: plugin.to_owned(),
        page: page.to_owned(),
        params,
    })
}

/// Reduces motion when asked: at once from the flag, the environment or
/// the saved setting, and otherwise when the desktop answers whether it
/// animates.
fn follow_motion_preference(
    workspace: &gpui::Entity<Workspace>,
    flag: bool,
    cx: &mut App,
) {
    let mut preference = MotionPreference::from_startup(
        flag,
        &HostConfig::default_interface_settings(),
    );
    workspace
        .update(cx, |ws, cx| ws.set_reduce_motion(preference.reduce(), cx));
    if preference.stated().is_some() {
        return;
    }
    let workspace = workspace.downgrade();
    cx.spawn(async move |cx| {
        let asking = cx.background_spawn(desktop_animations());
        preference.desktop_animations = asking.await;
        let reduce = preference.reduce();
        let _ = workspace.update(cx, |ws, cx| ws.set_reduce_motion(reduce, cx));
    })
    .detach();
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

/// Whether the desktop animates, from GNOME's `enable-animations`
/// through the XDG settings portal; `None` without a portal or the key.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub async fn desktop_animations() -> Option<bool> {
    let settings = ashpd::desktop::settings::Settings::new().await.ok()?;
    settings
        .read::<bool>("org.gnome.desktop.interface", "enable-animations")
        .await
        .ok()
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
pub async fn desktop_animations() -> Option<bool> {
    None
}
