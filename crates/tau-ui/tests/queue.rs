//! The landing queue through the real host and the workspace (ADR
//! 0024): a chat landed while main works lands after main's turn; a
//! queued chat that meets new conflicts waits for the person; tau's
//! resolving turn that leaves conflicts is held once, then marks main,
//! which refuses new chats until Resolve again clears it.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use gpui::{Entity, TestAppContext, VisualTestContext};
use serde_json::json;
use tau_agent::tool::RunId;
use tau_testing::{git::git, scripted::ScriptedModel};
use tau_ui::{
    accounts::Credentials,
    host::{Host, HostConfig},
};
use tau_ui_remote::{
    Workspace,
    catalog::Catalog,
    route::Route,
    view::Item,
    workspace::{LandingState, WorkspaceEvent},
};
use tau_vcs_host::{Identity, Project};

/// Runs GPUI until `done` holds, while runs work on the host's threads.
fn until(
    cx: &mut VisualTestContext,
    what: &str,
    mut done: impl FnMut(&mut VisualTestContext) -> bool,
) {
    for _ in 0..1000 {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}

/// What the conflicts-on-main hook heard: the repository and files.
type Told = Arc<Mutex<Vec<(String, Vec<String>)>>>;

struct Setup {
    workspace: Entity<Workspace>,
    cx: VisualTestContext,
    project: Project,
    main: RunId,
    told: Told,
    _dirs: [tempfile::TempDir; 3],
}

/// A host over `llm` on a repository whose README.md says `hello`.
fn setup(cx: &mut TestAppContext, llm: ScriptedModel) -> Setup {
    cx.executor().allow_parking();
    let src = tempfile::tempdir().unwrap();
    git(src.path(), &["init", "--quiet"]);
    std::fs::write(src.path().join("README.md"), "hello\n").unwrap();
    git(src.path(), &["add", "README.md"]);
    git(src.path(), &["commit", "--quiet", "-m", "first"]);
    let repos = tempfile::tempdir().unwrap();
    let project = tau_vcs_host::ProjectRepo::import(
        src.path().to_str().unwrap(),
        repos.path().join("p"),
        Identity::default(),
    )
    .map(Project::from)
    .unwrap();
    cx.update(tau_ui_remote::init);
    let window = cx.add_window(|window, cx| {
        Workspace::new("tau", Vec::new(), Catalog::default(), window, cx)
    });
    let workspace = window.root(cx).unwrap();
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let store = runtime.block_on(tau_store_sqlite::memory()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let config = HostConfig {
        account: tau_ai::chatgpt::AccountId::parse("test-account").unwrap(),
        credentials: Credentials::new(dir.path().join("config")),
        model: Some("gpt-5.5".into()),
        store: dir.path().join("unused.db"),
        repos: dir.path().join("repos"),
        settings: dir.path().join("models.json"),
        repo_list: dir.path().join("repos.json"),
        skills: std::env::temp_dir().join("tau-test-skills-none"),
    };
    let agent = tau_agent::agent::Agent::new(llm).name("coder");
    let told = Arc::new(Mutex::new(Vec::new()));
    let hook = {
        let told = told.clone();
        Arc::new(
            move |_: &RunId,
                  repo: &str,
                  files: &[String],
                  _: &mut gpui::App| {
                told.lock().unwrap().push((repo.to_owned(), files.to_vec()));
            },
        )
    };
    let (host, events) = Host::with_agent(runtime, agent, store, config);
    // Memory's pass after each run asks the model once more: these
    // scripts answer only the requests they write.
    let host = host.with_plugin_settings(
        tau_memory::NAME,
        serde_json::json!({ "after_each_run": false }),
    );
    let host = host
        .with_repo("hello", project.clone())
        .on_conflicts_on_main(hook);
    vcx.update(|_, cx| host.attach(&workspace, events, cx));
    // The host builds its catalog on its runtime, a moment after.
    until(&mut vcx, "the repository's main chat", |cx| {
        workspace.read_with(cx, |ws, _| !ws.catalog().repos.is_empty())
    });
    let main = workspace.read_with(&vcx, |ws, _| {
        ws.catalog().repos[0].main.clone().expect("a main chat")
    });
    Setup {
        workspace,
        cx: vcx,
        project,
        main,
        told,
        _dirs: [src, repos, dir],
    }
}

impl Setup {
    fn start(&mut self, prompt: &str) -> RunId {
        let before =
            self.workspace.read_with(&self.cx, |ws, _| ws.runs().len());
        self.workspace.update(&mut self.cx, |ws, cx| {
            // A chat forked from main, as "New run" asks for once main
            // was talked to; on an untouched main the composer goes to main.
            cx.emit(WorkspaceEvent::NewRun {
                prompt: prompt.to_owned(),
                model: ws.next_model().clone(),
                repo: ws.selected_repo().unwrap_or_default().to_owned(),
            });
        });
        let workspace = self.workspace.clone();
        until(&mut self.cx, "the run to start", |cx| {
            workspace.read_with(cx, |ws, _| ws.runs().len() > before)
        });
        self.workspace
            .read_with(&self.cx, |ws, _| ws.runs()[0].id.clone())
    }

    fn finished(&mut self, run: &RunId) {
        let workspace = self.workspace.clone();
        until(&mut self.cx, "the run to finish", |cx| {
            workspace.read_with(cx, |ws, _| {
                ws.run(run).is_some_and(|view| !view.status.is_live())
            })
        });
    }

    fn live(&mut self, run: &RunId) -> bool {
        self.workspace
            .read_with(&self.cx, |ws, _| ws.run(run).unwrap().status.is_live())
    }

    /// Writes `prompt` to the main chat.
    fn write_to_main(&mut self, prompt: &str) {
        let main = self.main.clone();
        self.workspace.update(&mut self.cx, |ws, cx| {
            ws.navigate(Route::Run(main), cx);
            ws.submit_prompt(prompt.to_owned(), cx);
        });
        let workspace = self.workspace.clone();
        let main = self.main.clone();
        let sent = prompt.to_owned();
        until(&mut self.cx, "main to take the message", |cx| {
            workspace.read_with(cx, |ws, _| {
                ws.run(&main).is_some_and(|view| {
                    view.items.iter().any(|item| {
                        matches!(item, Item::User(text) if *text == sent)
                    })
                })
            })
        });
    }

    /// Previews `run`'s landing, as the person opens its card.
    fn preview(&mut self, run: &RunId) -> tau_vcs_host::Landing {
        self.workspace
            .update(&mut self.cx, |ws, cx| ws.preview_landing(run, cx));
        let workspace = self.workspace.clone();
        until(&mut self.cx, "the preview", |cx| {
            workspace.read_with(cx, |ws, _| {
                matches!(ws.landing(run), Some(LandingState::Preview(_)))
            })
        });
        match self
            .workspace
            .read_with(&self.cx, |ws, _| ws.landing(run).cloned())
        {
            Some(LandingState::Preview(Ok(landing))) => landing,
            other => panic!("{other:?}"),
        }
    }

    fn land(&mut self, run: &RunId) {
        self.workspace
            .update(&mut self.cx, |ws, cx| ws.land(run, cx));
    }

    fn queued(&mut self) -> Vec<String> {
        let main = self.main.clone();
        self.workspace.read_with(&self.cx, |ws, _| {
            ws.run(&main)
                .unwrap()
                .landing_queue
                .iter()
                .map(|waiting| waiting.run.clone())
                .collect()
        })
    }

    fn is_closed(&mut self, run: &RunId) -> bool {
        self.workspace
            .read_with(&self.cx, |ws, _| ws.is_closed(run))
    }

    fn readme(&self) -> String {
        let trunk = self.project.blocking().trunk().unwrap();
        String::from_utf8(
            self.project
                .blocking()
                .file_at(&trunk, "README.md")
                .unwrap()
                .unwrap()
                .0,
        )
        .unwrap()
    }

    fn main_conflicts(
        &mut self,
    ) -> Option<tau_ui_remote::queue::MainConflicts> {
        let main = self.main.clone();
        self.workspace.read_with(&self.cx, |ws, _| {
            ws.run(&main).unwrap().main_conflicts.clone()
        })
    }
}

fn write(path: &str, text: &str) -> serde_json::Value {
    json!({ "path": path, "content": text })
}

fn commit(message: &str) -> serde_json::Value {
    json!({ "message": message })
}

/// A chat landed while main works joins the queue and lands once
/// main's turn ends, not before.
#[gpui::test]
fn a_chat_landed_while_main_works_lands_after_its_turn(
    cx: &mut TestAppContext,
) {
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("write", write("one.txt", "one\n")))
        .turn(|t| t.tool_call("vcs_commit", commit("docs: one")))
        .turn(|t| t.text("one is done"))
        // Main's turn, slow enough to land while it runs.
        .turn(|t| t.delay(Duration::from_secs(3)).text("thinking"));
    let mut s = setup(cx, llm.clone());
    let one = s.start("write one");
    s.finished(&one);
    s.write_to_main("think");
    let preview = s.preview(&one);
    assert_eq!(preview.changes.len(), 1);
    s.land(&one);
    until(&mut s.cx, "the chat to queue", |cx| {
        s.workspace.read_with(cx, |ws, _| ws.queued(&one).is_some())
    });
    assert_eq!(s.queued(), [one.0.to_string()]);
    assert!(s.live(&s.main.clone()), "main still works");
    assert!(!s.is_closed(&one), "it waits for main");
    let trunk = s.project.blocking().trunk().unwrap();
    assert!(
        s.project
            .blocking()
            .file_at(&trunk, "one.txt")
            .unwrap()
            .is_none()
    );

    let workspace = s.workspace.clone();
    until(&mut s.cx, "the chat to land", |cx| {
        workspace.read_with(cx, |ws, _| ws.is_closed(&one))
    });
    assert!(s.queued().is_empty());
    let trunk = s.project.blocking().trunk().unwrap();
    assert!(
        s.project
            .blocking()
            .file_at(&trunk, "one.txt")
            .unwrap()
            .is_some()
    );
    llm.assert_exhausted();
}

/// Two chats wait; the first lands, and the second, which meets a
/// conflict main's turn brought, waits for the person to confirm it.
/// Confirmed, it lands, and tau's turn resolves the conflict.
#[gpui::test]
fn a_queued_chat_with_new_conflicts_waits_for_confirmation(
    cx: &mut TestAppContext,
) {
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("write", write("a.txt", "a\n")))
        .turn(|t| t.tool_call("vcs_commit", commit("docs: a")))
        .turn(|t| t.text("a is done"))
        .turn(|t| t.tool_call("write", write("README.md", "b\n")))
        .turn(|t| t.tool_call("vcs_commit", commit("docs: b")))
        .turn(|t| t.text("b is done"))
        // Main changes what b changed, after both chats queued.
        .turn(|t| {
            t.delay(Duration::from_secs(4))
                .tool_call("write", write("README.md", "main\n"))
        })
        .turn(|t| t.tool_call("vcs_commit", commit("docs: main")))
        .turn(|t| t.text("main is done"))
        // tau's turn resolving b's landing.
        .turn(|t| t.tool_call("write", write("README.md", "main and b\n")))
        .turn(|t| t.tool_call("vcs_commit", commit("docs: keep both")))
        .turn(|t| t.text("resolved"));
    let mut s = setup(cx, llm.clone());
    let a = s.start("write a");
    s.finished(&a);
    let b = s.start("write b");
    s.finished(&b);
    s.write_to_main("change the readme");
    for run in [&a, &b] {
        assert!(s.preview(run).conflicts.is_empty());
        s.land(run);
    }
    until(&mut s.cx, "both chats to queue", |cx| {
        s.workspace.read_with(cx, |ws, _| {
            ws.queued(&a).is_some() && ws.queued(&b).is_some()
        })
    });
    assert_eq!(s.queued(), [a.0.to_string(), b.0.to_string()]);

    let workspace = s.workspace.clone();
    until(&mut s.cx, "a to land and b to wait", |cx| {
        workspace.read_with(cx, |ws, _| {
            ws.is_closed(&a)
                && ws
                    .queued(&b)
                    .is_some_and(|(_, waiting)| waiting.needs_confirmation())
        })
    });
    let (at, waiting) = s
        .workspace
        .read_with(&s.cx, |ws, _| ws.queued(&b).map(|(at, w)| (at, w.clone())))
        .unwrap();
    assert_eq!(at, 1);
    assert_eq!(waiting.conflicts, ["README.md"]);
    assert!(waiting.confirmed.is_empty());
    assert!(!s.is_closed(&b));
    assert_eq!(s.readme(), "main\n");

    // Confirmed, it lands with the conflict, and main resolves it.
    s.land(&b);
    let project = s.project.clone();
    until(&mut s.cx, "b to land and main to resolve", |cx| {
        workspace.read_with(cx, |ws, _| ws.is_closed(&b))
            && project
                .blocking()
                .file_at(&project.blocking().trunk().unwrap(), "README.md")
                .unwrap()
                .is_some_and(|(bytes, _)| bytes == b"main and b\n")
    });
    let main = s.main.clone();
    until(&mut s.cx, "main to finish", |cx| {
        workspace
            .read_with(cx, |ws, _| !ws.run(&main).unwrap().status.is_live())
    });
    s.cx.run_until_parked();
    assert!(s.queued().is_empty());
    assert_eq!(s.main_conflicts(), None);
    llm.assert_exhausted();
}

/// tau's resolving turn that leaves the conflict is held once with the
/// files named; still conflicted, main is marked and the hook hears it;
/// a new chat is refused; Resolve again clears it, and chats start
/// again.
#[gpui::test]
fn conflicts_left_on_main_hold_landings_and_new_chats(cx: &mut TestAppContext) {
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("write", write("README.md", "c\n")))
        .turn(|t| t.tool_call("vcs_commit", commit("docs: c")))
        .turn(|t| t.text("c is done"))
        // Main changes the same line, and commits.
        .turn(|t| t.tool_call("write", write("README.md", "m\n")))
        .turn(|t| t.tool_call("vcs_commit", commit("docs: m")))
        .turn(|t| t.text("m is done"))
        // tau's resolving turn gives up, twice: held once, then stops.
        .turn(|t| t.text("I cannot tell which to keep"))
        .turn(|t| t.text("still cannot"))
        // Resolve again.
        .turn(|t| t.tool_call("write", write("README.md", "c and m\n")))
        .turn(|t| t.tool_call("vcs_commit", commit("docs: keep both")))
        .turn(|t| t.text("resolved"))
        // A chat once main is clean.
        .turn(|t| t.text("hi"));
    let mut s = setup(cx, llm.clone());
    let c = s.start("write c");
    s.finished(&c);
    s.write_to_main("write m");
    let main = s.main.clone();
    s.finished(&main);
    assert_eq!(s.preview(&c).conflicts, ["README.md"]);
    s.land(&c);

    let workspace = s.workspace.clone();
    until(&mut s.cx, "main to be marked", |cx| {
        workspace.read_with(cx, |ws, _| {
            let view = ws.run(&main).unwrap();
            !view.status.is_live() && view.main_conflicts.is_some()
        })
    });
    let marked = s.main_conflicts().unwrap();
    assert_eq!(marked.files, ["README.md"]);
    assert_eq!(marked.from.as_deref(), Some(&*c.0));
    assert!(s.is_closed(&c), "c landed");
    assert_eq!(
        *s.told.lock().unwrap(),
        [("hello".to_owned(), vec!["README.md".to_owned()])]
    );
    // The stop was held once, naming the file.
    let held = llm.requests().iter().any(|request| {
        format!("{request:?}")
            .contains("Conflicts remain in README.md. Resolve them and commit.")
    });
    assert!(held, "the resolving turn was held");

    // A new chat would fork conflicted code.
    s.workspace.update(&mut s.cx, |ws, cx| {
        ws.navigate(Route::NewRun, cx);
        ws.submit_prompt("hi".to_owned(), cx);
    });
    until(&mut s.cx, "the refusal", |cx| {
        workspace.read_with(cx, |ws, _| ws.alert().is_some())
    });
    let (_, message) = s
        .workspace
        .read_with(&s.cx, |ws, _| {
            ws.alert().map(|(t, m)| (t.to_owned(), m.to_owned()))
        })
        .unwrap();
    assert_eq!(
        message,
        "main has conflicts in README.md; resolve them first"
    );
    s.workspace.update(&mut s.cx, |ws, cx| ws.dismiss_alert(cx));

    s.workspace
        .update(&mut s.cx, |ws, cx| ws.resolve_again(&main, cx));
    until(&mut s.cx, "the mark to clear", |cx| {
        workspace.read_with(cx, |ws, _| {
            let view = ws.run(&main).unwrap();
            !view.status.is_live() && view.main_conflicts.is_none()
        })
    });
    assert_eq!(s.readme(), "c and m\n");
    let hi = s.start("hi");
    s.finished(&hi);
    llm.assert_exhausted();
}
