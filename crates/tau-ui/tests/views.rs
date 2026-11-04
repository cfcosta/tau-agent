//! Plugins' views built for a phone (ADR 0046): what the computer builds
//! for a place, named as the phone names it, is what it draws there
//! itself, for every place the demo's runs and screens have.

use gpui::{Entity, TestAppContext, VisualTestContext};
use tau_ui::demo;
use tau_ui_plugin::{
    Point,
    PointCx,
    points::{self, AtApp, AtRepo, AtRun},
    wire::{Built, Contributed, Layout, View, ViewKey, Wired},
};
use tau_ui_remote::{
    Workspace,
    catalog::Catalog,
    plugins::registry,
    view::Item,
};

fn open(
    cx: &mut TestAppContext,
) -> (
    Entity<Workspace>,
    VisualTestContext,
    std::sync::Arc<demo::DemoHost>,
) {
    // The app's plugins, every host half included.
    tau::install();
    cx.update(tau_ui_remote::init);
    let window = cx.add_window(|window, cx| {
        let catalog = Catalog {
            repos: vec![tau_ui_remote::catalog::Repo {
                name: "tau-agent".into(),
                main: Some(demo::run_id()),
                ..Default::default()
            }],
            ..Default::default()
        };
        Workspace::new("tau", vec![demo::retry_after()], catalog, window, cx)
    });
    let workspace = window.root(cx).unwrap();
    let mut cx = VisualTestContext::from_window(*window, cx);
    workspace.update(&mut cx, |ws, cx| {
        ws.add_history(demo::history(), cx);
        ws.set_catalog(demo::catalog(), cx);
    });
    // The demo's host gives plugins their data.
    let host = cx.update(|_, cx| demo::start(&workspace, cx));
    cx.run_until_parked();
    (workspace, cx, host)
}

/// What `ws` draws at `point` with the context `at` gives each plugin,
/// as the built view says it.
fn drawn<'c, Cx: PointCx, Out: Wired>(
    ws: &Workspace,
    point: Point<Cx, Out>,
    at: impl Fn(&str) -> Option<&'c Cx>,
) -> Built {
    let outs = ws
        .contributions_named(point, at)
        .into_iter()
        .map(|(plugin, out)| Contributed {
            plugin: plugin.to_owned(),
            out: out.to_wire().unwrap(),
        })
        .collect();
    Built::Outs { outs }
}

/// `ws`'s view of the place `at` names, at `point`, built as a phone in
/// this window's layout would get it.
fn built<Cx: PointCx, Out>(
    ws: &Workspace,
    layout: Layout,
    point: Point<Cx, Out>,
    at: &Cx,
) -> Built {
    let at = at.wire().expect("the place crosses");
    ws.build_view(&ViewKey {
        view: View::Point {
            point: point.name.to_owned(),
            at,
        },
        layout,
    })
}

/// Every point of every place the demo shows comes back as drawn: a
/// run's own points, its anchors, rewrites, messages and cards, the
/// app's and each repository's; and each plugin's pages and settings
/// panes are built.
#[gpui::test]
fn what_the_computer_builds_is_what_it_draws(cx: &mut TestAppContext) {
    let (workspace, mut cx, _host) = open(cx);
    workspace.update(&mut cx, |ws, _| {
        let layout = Layout::of(ws.compact(), f32::from(ws.screen_width()));
        macro_rules! same {
            ($point:expr, $at:expr) => {{
                let at = $at;
                let want = drawn(ws, $point, |_| Some(&at));
                assert_eq!(
                    built(ws, layout, $point, &at),
                    want,
                    "{}",
                    $point.name
                );
                want
            }};
        }
        let mut drew = 0;
        let mut count = |built: Built| {
            if let Built::Outs { outs } = built {
                drew += outs.len();
            }
        };
        count(same!(points::SIDEBAR, AtApp));
        count(same!(points::SEARCH, AtApp));
        count(same!(points::MODELS, AtApp));
        count(same!(points::ACTIVITY, AtApp));
        count(same!(points::PICKER_AUTO, AtApp));
        for repo in ws.catalog().repos.clone() {
            let at = || AtRepo {
                repo: repo.name.clone(),
            };
            count(same!(points::SIDEBAR_REPO, at()));
            count(same!(points::REPO_MENU, at()));
            count(same!(points::REPO_ACTIONS, at()));
        }
        for run in ws.runs() {
            let at = || AtRun { run: run.info() };
            count(same!(points::STATUS, at()));
            count(same!(points::PLAN, at()));
            count(same!(points::CONTEXT_TRIGGER, at()));
            count(same!(points::RUN_ROW, at()));
            count(same!(points::ASKS, at()));
            count(same!(points::CONTEXT, at()));
            count(same!(points::RUN_BANNER, at()));
            count(same!(points::INSPECTOR, at()));
            count(same!(points::PLAN_STEPS, at()));
            count(same!(points::COMPOSER, at()));
            count(same!(points::COMPOSER_BAND, at()));
            count(same!(points::PANE, at()));
            for (index, item) in run.items.iter().enumerate() {
                match item {
                    Item::Anchor { key, .. } => {
                        count(same!(
                            points::TRANSCRIPT,
                            points::AtAnchor {
                                run: run.info(),
                                key: key.clone(),
                                index,
                            }
                        ));
                    }
                    Item::Rewrite {
                        key: Some(key),
                        tokens,
                        ..
                    } => {
                        count(same!(
                            points::REWRITE,
                            points::AtRewrite {
                                run: run.info(),
                                key: key.clone(),
                                tokens: *tokens,
                                index,
                            }
                        ));
                    }
                    Item::User(text) | Item::Tau(text) => {
                        count(same!(
                            points::USER_MESSAGE,
                            points::AtMessage {
                                run: run.info(),
                                text: text.clone(),
                                index,
                            }
                        ));
                    }
                    Item::Tool(card) => {
                        let contexts: Vec<(&str, points::AtCard)> = registry()
                            .plugins()
                            .map(|plugin| {
                                let at = tau_ui_remote::views::at_card(
                                    run,
                                    card,
                                    plugin.name(),
                                );
                                (plugin.name(), at)
                            })
                            .collect();
                        let at_card = |plugin: &str| {
                            contexts
                                .iter()
                                .find(|(name, _)| *name == plugin)
                                .map(|(_, at)| at)
                        };
                        let key = &contexts[0].1;
                        for (point, want) in [
                            (
                                points::CARD.name,
                                drawn(ws, points::CARD, at_card),
                            ),
                            (
                                points::CARD_BADGE.name,
                                drawn(ws, points::CARD_BADGE, at_card),
                            ),
                            (
                                points::CARD_BODY.name,
                                drawn(ws, points::CARD_BODY, at_card),
                            ),
                        ] {
                            let got = ws.build_view(&ViewKey {
                                view: View::Point {
                                    point: point.to_owned(),
                                    at: key.wire().unwrap(),
                                },
                                layout,
                            });
                            assert_eq!(
                                got, want,
                                "{point} at {}",
                                card.call_id
                            );
                            count(got);
                        }
                    }
                    _ => {}
                }
            }
        }
        assert!(drew > 20, "the demo draws plugins' views: {drew}");

        // Every plugin's pages and settings pane are built, and a place
        // the computer does not have is nothing.
        for plugin in registry().plugins() {
            for page in plugin.pages() {
                let built = ws.build_view(&ViewKey {
                    view: View::Page {
                        plugin: plugin.name().to_owned(),
                        page: page.name.to_owned(),
                        params: Default::default(),
                    },
                    layout,
                });
                assert!(
                    matches!(built, Built::Page { .. }),
                    "{}/{}",
                    plugin.name(),
                    page.name
                );
            }
            if plugin.has_settings() {
                let built = ws.build_view(&ViewKey {
                    view: View::Settings {
                        plugin: plugin.name().to_owned(),
                        scope: None,
                        entry: None,
                    },
                    layout,
                });
                assert!(
                    matches!(built, Built::Tree { .. }),
                    "{}",
                    plugin.name()
                );
            }
        }
        let gone = ws.build_view(&ViewKey {
            view: View::Point {
                point: points::STATUS.name.to_owned(),
                at: tau_ui_plugin::wire::AtKey::Run {
                    run: tau_agent::tool::RunId("no-such-run".into()),
                },
            },
            layout,
        });
        assert_eq!(gone, Built::Nothing);
    });
}

/// A phone's layout is the one its views are built in: pages that lay
/// themselves out for the phone come back for each layout as that layout
/// draws them.
#[gpui::test]
fn a_view_is_built_in_the_phone_s_layout(cx: &mut TestAppContext) {
    let (workspace, mut cx, _host) = open(cx);
    workspace.update(&mut cx, |ws, _| {
        let params: std::collections::BTreeMap<String, String> = [
            ("repo".to_owned(), "tau-agent".to_owned()),
            ("run".to_owned(), demo::run_id().0.to_string()),
        ]
        .into();
        let mut differ = Vec::new();
        for plugin in registry().plugins() {
            for page in plugin.pages() {
                let built = |compact: bool| {
                    ws.build_view(&ViewKey {
                        view: View::Page {
                            plugin: plugin.name().to_owned(),
                            page: page.name.to_owned(),
                            params: params.clone(),
                        },
                        layout: Layout::of(
                            compact,
                            if compact { 400. } else { 1440. },
                        ),
                    })
                };
                if built(true) != built(false) {
                    differ.push(format!("{}/{}", plugin.name(), page.name));
                }
            }
        }
        assert!(!differ.is_empty(), "some page is laid out for the phone");
        let Built::Commands { commands } = ws.build_view(&ViewKey {
            view: View::Commands {
                repo: Some("tau-agent".into()),
            },
            layout: Layout::of(true, 400.),
        }) else {
            panic!("no commands");
        };
        assert!(commands.iter().any(|(_, command)| command.name == "goal"));
    });
}

/// A run's plugin lines, as `ws` draws them.
fn statuses(
    ws: &Workspace,
) -> Vec<(&'static str, tau_ui_plugin::PluginStatus)> {
    let at = AtRun {
        run: ws.run(&demo::run_id()).unwrap().info(),
    };
    ws.contributions_named(points::STATUS, |_| Some(&at))
}

/// A phone in one process with the computer: it draws plugins' views
/// from what the computer builds, asked for once it has drawn without
/// them, and gets them again pushed after a change.
#[gpui::test]
fn a_phone_draws_what_the_computer_builds(cx: &mut TestAppContext) {
    use std::{cell::RefCell, rc::Rc};

    use gpui::{px, size};
    use tau_ui_remote::{
        route::Route,
        update::HostUpdate,
        workspace::WorkspaceEvent,
    };

    let (computer, mut ccx, _host) = open(cx);
    let window = cx.add_window(|window, cx| {
        Workspace::new("tau", Vec::new(), Catalog::default(), window, cx)
    });
    let phone = window.root(cx).unwrap();
    let mut pcx = VisualTestContext::from_window(*window, cx);
    pcx.simulate_resize(size(px(400.), px(844.)));
    let asked: Rc<RefCell<Vec<ViewKey>>> = Rc::default();
    let seen = asked.clone();
    pcx.update(|_, cx| {
        cx.subscribe(&phone, move |_, event: &WorkspaceEvent, _| {
            if let WorkspaceEvent::AskViews(keys) = event {
                seen.borrow_mut().extend(keys.iter().cloned());
            }
        })
        .detach()
    });
    let snapshot = computer.read_with(&ccx, |ws, _| ws.snapshot());
    phone.update(&mut pcx, |ws, cx| {
        ws.draw_remote_views();
        ws.apply(snapshot, cx);
        ws.navigate(Route::Run(demo::run_id()), cx);
    });
    pcx.run_until_parked();

    // It has drawn, and asked; nothing came yet, so it shows none of
    // the plugins' lines it would build itself.
    assert!(
        !asked.borrow().is_empty(),
        "the phone asks for what it draws"
    );
    assert!(phone.read_with(&pcx, |ws, _| statuses(ws)).is_empty());

    // The computer answers what it asks, until it asks for nothing new.
    let relay = |pcx: &mut VisualTestContext,
                     ccx: &mut VisualTestContext| {
        for _ in 0..8 {
            let keys = std::mem::take(&mut *asked.borrow_mut());
            if keys.is_empty() {
                return;
            }
            let answers = computer.update(ccx, |ws, _| ws.ask_views(keys));
            phone.update(pcx, |ws, cx| ws.apply(answers, cx));
            pcx.run_until_parked();
        }
        panic!("the phone keeps asking");
    };
    relay(&mut pcx, &mut ccx);
    let on_computer = computer.read_with(&ccx, |ws, _| statuses(ws));
    assert!(!on_computer.is_empty());
    assert_eq!(phone.read_with(&pcx, |ws, _| statuses(ws)), on_computer);

    // A goal set on the computer: what changed comes to the phone
    // pushed, and the phone draws the goal's banner from its tree.
    computer.update(&mut ccx, |ws, cx| {
        ws.apply(
            HostUpdate::PluginFold {
                run: demo::run_id(),
                plugin: tau_goal::NAME.into(),
                body: serde_json::to_value(tau_goal::Record::Set {
                    goal: "it ships".into(),
                    continuations: 3,
                    budget: 2.0,
                })
                .unwrap(),
            },
            cx,
        )
    });
    let pushed = computer.update(&mut ccx, |ws, _| ws.refresh_views());
    let Some(HostUpdate::Views { answers }) = pushed else {
        panic!("nothing changed for the phone");
    };
    assert!(!answers.is_empty());
    phone.update(&mut pcx, |ws, cx| {
        ws.apply(HostUpdate::Views { answers }, cx)
    });
    pcx.run_until_parked();
    relay(&mut pcx, &mut ccx);
    assert_eq!(
        phone.read_with(&pcx, |ws, _| statuses(ws)),
        computer.read_with(&ccx, |ws, _| statuses(ws))
    );
    // The goal's banner, its tree as the computer built it in the
    // phone's layout.
    let banners = |ws: &Workspace| {
        let at = AtRun {
            run: ws.run(&demo::run_id()).unwrap().info(),
        };
        let banners = ws.contributions_named(points::RUN_BANNER, |_| Some(&at));
        banners
            .into_iter()
            .map(|(plugin, tree)| (plugin, tree.to_wire()))
            .collect::<Vec<_>>()
    };
    let on_phone = phone.read_with(&pcx, |ws, _| banners(ws));
    assert!(on_phone.iter().any(|(plugin, _)| *plugin == tau_goal::NAME));
    let layout = phone.read_with(&pcx, |ws, _| ws.view_layout());
    let built = computer.read_with(&ccx, |ws, _| {
        ws.build_view(&ViewKey {
            view: View::Point {
                point: points::RUN_BANNER.name.to_owned(),
                at: tau_ui_plugin::wire::AtKey::Run {
                    run: demo::run_id(),
                },
            },
            layout,
        })
    });
    let Built::Outs { outs } = built else {
        panic!("no banners");
    };
    let built: Vec<_> = outs
        .into_iter()
        .map(|out| (out.plugin, Some(out.out)))
        .collect();
    let on_phone: Vec<_> = on_phone
        .into_iter()
        .map(|(plugin, tree)| (plugin.to_owned(), tree))
        .collect();
    assert_eq!(on_phone, built);
    // Built again with nothing new, nothing goes.
    assert!(
        computer
            .update(&mut ccx, |ws, _| ws.refresh_views())
            .is_none()
    );
}
