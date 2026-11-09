//! tau-ask, Luau now (ADR 0050). In a host: a run waits on its
//! questions, answered from two devices, the first answers taking them
//! and the model reading them. In the workspace: a question a live run
//! waits on takes the composer's place and the keys, and what the person
//! picks, writes and notes goes to the call as its answers; here the
//! test runs the plugin's actions as the host would.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    cell::RefCell,
    path::PathBuf,
    rc::Rc,
    sync::{Arc, Once},
    time::Duration,
};

use gpui::{Entity, TestAppContext, VisualTestContext};
use serde_json::{Value, json};
use tau_agent::{event::RunEvent, tool::RunId};
use tau_demo as demo;
use tau_luau_plugins::{Act, ActionDone, Record};
use tau_luau_plugins_host::runtime::{Context, Hook, Loaded, NoReach, load};
use tau_testing::{block_on_io, git::git, scripted::ScriptedModel};
use tau_ui::{
    accounts::Credentials,
    host::{Host, HostConfig},
};
use tau_ui_remote::{
    Workspace,
    WorkspaceEvent,
    catalog::Catalog,
    models::ModelChoice,
    route::Route,
    update::HostUpdate,
    view::RunView,
};
use tau_vcs_host::{Identity, Project};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_util::sync::CancellationToken;

const REPO: &str = "repo";
const WAIT: Duration = Duration::from_secs(60);

fn questions() -> Value {
    json!({ "questions": [
        {
            "question": "How should the ask tool wait for your answer?",
            "header": "Waiting",
            "options": [
                { "label": "Hold the call open (Recommended)", "description": "Block until the answer comes.", "preview": "select! { a = rx => a }" },
                { "label": "End the turn", "description": "Read the answer as the next message.", "preview": "stop_run()" }
            ]
        },
        {
            "question": "Where else should a waiting question show?",
            "header": "Surfaces",
            "multi_select": true,
            "options": [
                { "label": "Run list badge", "description": "A dot on the run." },
                { "label": "Notification", "description": "When the window is away." },
                { "label": "Parent run", "description": "For child runs." }
            ]
        }
    ]})
}

// In a host.

/// The app's plugins, the Luau ones among them: a process has one
/// registry.
fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| tau_ui_remote::plugins::install(tau::plugins()));
}

fn own_dir() -> PathBuf {
    tempfile::tempdir().unwrap().keep()
}

fn project() -> Project {
    let checkout = own_dir();
    git(&checkout, &["init", "--quiet"]);
    git(
        &checkout,
        &["commit", "--quiet", "--allow-empty", "-m", "first"],
    );
    tau_vcs_host::ProjectRepo::import(
        checkout.to_str().unwrap(),
        own_dir().join("p"),
        Identity::default(),
    )
    .map(Project::from)
    .unwrap()
}

fn host_on(llm: &ScriptedModel) -> (Host, UnboundedReceiver<RunEvent>) {
    install();
    let dir = own_dir();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let store = runtime.block_on(tau_store_sqlite::memory()).unwrap();
    let config = HostConfig {
        credentials: Credentials::new(dir.join("config")),
        model: Some("gpt-6-luna".into()),
        store: dir.join("runs.db"),
        repos: dir.join("repos"),
        settings: dir.join("models.json"),
        repo_list: dir.join("repos.json"),
        home: None,
        services: Default::default(),
    };
    let agent = tau_agent::agent::Agent::new(llm.clone()).name("coder");
    let (host, events) = Host::with_agent(runtime, agent, store, config);
    let host = host
        .with_plugin_settings(
            tau_memory::NAME,
            json!({ "after_each_run": false }),
        )
        .with_repository(
            REPO,
            Arc::new(tau_vcs_host::JjRepository::new(
                project(),
                Identity::default(),
            )),
        );
    (host, events)
}

/// tau-ask's state in `view`, as tau-luau-plugins' fold keeps it.
fn ask_of(view: &RunView) -> Value {
    view.plugin_states
        .get(tau_luau_plugins::NAME)
        .map(|state| {
            state.json()["plugins"][tau_luau_plugins::names::ASK]["state"]
                .clone()
        })
        .unwrap_or_default()
}

/// Folds `events` into `view` until `done` holds of it, or the run ends.
fn fold_until(
    view: &mut RunView,
    events: &mut UnboundedReceiver<RunEvent>,
    done: impl Fn(&RunView) -> bool,
) {
    let deadline = std::time::Instant::now() + WAIT;
    while std::time::Instant::now() < deadline {
        match events.try_recv() {
            Ok(event) => {
                let end = matches!(event, RunEvent::RunEnd { .. });
                view.apply(&event);
                if done(view) || end {
                    return;
                }
            }
            Err(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    panic!("not within {WAIT:?}");
}

/// A device's click or key: `action` with `args` on `run`'s tau-ask
/// state as that device has it, what it left stored as the interface
/// does. The action's error, if it had one.
fn act(
    host: &Host,
    run: &RunId,
    action: &str,
    args: Value,
    state: &Value,
) -> (Option<String>, Value) {
    let act = Act::Action {
        plugin: tau_luau_plugins::names::ASK.into(),
        action: action.into(),
        args,
        run: run.0.to_string(),
        repo: Some(REPO.into()),
        state: state.clone(),
        settings: None,
        live: true,
        jev: false,
    };
    let done = host
        .block_on(host.plugin_act(
            tau_luau_plugins::NAME,
            serde_json::to_value(act).unwrap(),
        ))
        .unwrap()
        .expect("what the action left");
    let done: ActionDone = serde_json::from_value(done).unwrap();
    for record in done.records() {
        host.block_on(host.store_plugin_record(
            run,
            tau_luau_plugins::NAME,
            &serde_json::to_value(record).unwrap(),
        ))
        .unwrap();
    }
    (done.error.map(|(_, error)| error), done.state)
}

/// A run waits on its questions while two devices work the panel: the
/// phone picks the first answer, the computer, from what the phone left,
/// the checklist's and sends; the phone's Send, from its own state,
/// finds the call answered. The model reads the computer's answers, and
/// the run's state keeps them.
#[test]
fn a_waiting_run_is_answered_from_whichever_device_first() {
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("ask", questions()))
        .turn(|t| t.text("Holding the call open."));
    let (host, mut events) = host_on(&llm);
    let mut view = host
        .block_on(host.start("ask me", &ModelChoice::default(), REPO))
        .unwrap();
    let run = view.id.clone();
    let waits = |view: &RunView| {
        let state = ask_of(view);
        state["calls"][0].is_object() && state["calls"][0]["reply"].is_null()
    };
    fold_until(&mut view, &mut events, waits);
    assert!(waits(&view), "the call waits on its questions");
    assert_eq!(
        ask_of(&view)["calls"][0]["ask"]["questions"][0]["header"],
        "Waiting"
    );

    let first = ask_of(&view);
    let (error, phone) = act(&host, &run, "key", json!({ "key": "2" }), &first);
    assert_eq!(error, None);
    let (_, computer) = act(&host, &run, "key", json!({ "key": "1" }), &phone);
    let (_, computer) =
        act(&host, &run, "key", json!({ "key": "right" }), &computer);
    let (error, _) =
        act(&host, &run, "key", json!({ "key": "enter" }), &computer);
    assert_eq!(error, None, "the computer's answers take the call");
    let (late, _) = act(&host, &run, "decline", json!({}), &phone);
    assert!(
        late.as_deref()
            .is_some_and(|why| why.contains("answered already")),
        "{late:?}"
    );

    fold_until(&mut view, &mut events, |_| false);
    let requests = llm.requests();
    let sent = serde_json::to_string(&requests[1].transcript).unwrap();
    assert!(
        sent.contains(
            r#"\"Where else should a waiting question show?\" = \"Run list badge\""#
        ),
        "{sent}"
    );
    assert!(sent.contains(r#"= \"End the turn\""#), "{sent}");
    llm.assert_exhausted();
    let history = host.block_on(host.history()).unwrap();
    let stored = history.iter().find(|stored| stored.id == run).unwrap();
    assert_eq!(
        ask_of(stored)["calls"][0]["reply"]["answers"][1]["picked"],
        json!(["Run list badge"])
    );
}

/// A key from a device that had not yet seen the answers, stored after the
/// run took them, changes nothing: the first call keeps its reply, the run
/// goes on to its second question, and that one takes its answers too.
#[test]
fn a_late_key_does_not_take_back_the_answers() {
    let single = json!({ "questions": [{
        "question": "Which lane?",
        "header": "Lane",
        "options": [
            { "label": "Left", "description": "l" },
            { "label": "Right", "description": "r" }
        ]
    }]});
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("ask", single.clone()))
        .turn(|t| t.tool_call("ask", single.clone()))
        .turn(|t| t.text("Both answered."));
    let (host, mut events) = host_on(&llm);
    let mut view = host
        .block_on(host.start("ask me", &ModelChoice::default(), REPO))
        .unwrap();
    let run = view.id.clone();
    fold_until(&mut view, &mut events, |view| {
        ask_of(view)["calls"][0].is_object()
    });
    let stored = |host: &Host| {
        host.block_on(host.history())
            .unwrap()
            .iter()
            .find(|stored| stored.id == run)
            .map(ask_of)
            .unwrap()
    };
    let behind = ask_of(&view);
    let (error, _) = act(&host, &run, "key", json!({ "key": "1" }), &behind);
    assert_eq!(error, None);
    let picked = stored(&host);
    let (error, _) =
        act(&host, &run, "key", json!({ "key": "enter" }), &picked);
    assert_eq!(error, None, "the answers take the first call");
    fold_until(&mut view, &mut events, |view| {
        ask_of(view)["calls"][1].is_object()
    });

    // The other device still has the state from before the answers.
    let (late, left) = act(&host, &run, "key", json!({ "key": "2" }), &behind);
    assert_eq!(late, None);
    assert_ne!(left, behind, "the key changed what that device had");

    let now = stored(&host);
    assert_eq!(
        now["calls"][0]["reply"]["answers"][0]["picked"],
        json!(["Left"])
    );
    assert!(now["calls"][1].is_object(), "{now}");

    let (error, picked) = act(&host, &run, "key", json!({ "key": "2" }), &now);
    assert_eq!(error, None);
    let (error, _) =
        act(&host, &run, "key", json!({ "key": "enter" }), &picked);
    assert_eq!(error, None, "the second call still takes its answers");
    fold_until(&mut view, &mut events, |_| false);
    llm.assert_exhausted();
    let last = stored(&host);
    assert_eq!(
        last["calls"][0]["reply"]["answers"][0]["picked"],
        json!(["Left"])
    );
    assert_eq!(
        last["calls"][1]["reply"]["answers"][0]["picked"],
        json!(["Right"])
    );
}

/// tau-ask's `ask` holds a turn open for the person; an agent's runs,
/// which no one watches, leave it out (ADR 0037): they ask through
/// tau-agents', whose answer comes back as an event.
#[test]
fn an_agents_runs_have_no_ask_and_the_persons_do() {
    let llm = ScriptedModel::new()
        .turn(|t| t.text("done"))
        .turn(|t| t.text("done"));
    let (host, mut events) = host_on(&llm);
    let repo = host
        .block_on(host.make_repo(tau_agents::KIND, "triage"))
        .unwrap();
    let main = repo.main.clone().expect("a main chat");
    host.block_on(host.resume(&main, "hello", &ModelChoice::default()))
        .unwrap();
    fold_until(
        &mut RunView::new(main.clone(), "", "", ""),
        &mut events,
        |_| false,
    );
    let mut view = host
        .block_on(host.start("hello", &ModelChoice::default(), REPO))
        .unwrap();
    fold_until(&mut view, &mut events, |_| false);
    let requests = llm.requests();
    assert_eq!(requests.len(), 2);
    // tau-ask's `ask`, which an agent's own `ask` is not.
    let asks = |request: &tau_testing::scripted::Request| {
        request.settings.tools.iter().any(|tool| {
            tool.name == "ask"
                && tool.description.starts_with("Ask the person one to four")
        })
    };
    assert!(!asks(&requests[0]), "an agent's run");
    assert!(asks(&requests[1]), "the person's run");
}

// In the workspace.

type Events = Rc<RefCell<Vec<WorkspaceEvent>>>;

/// tau-ask's Luau, as tau loads it.
fn plugin() -> Loaded {
    let files =
        tau_luau_plugins_host::built_in::files(tau_luau_plugins::names::ASK)
            .unwrap();
    block_on_io(load(tau_luau_plugins::names::ASK, files)).unwrap()
}

/// `hook` with `input` on `state`, in `run`: its value and the state it
/// leaves.
fn call(
    loaded: &Loaded,
    hook: Hook,
    input: Value,
    run: &RunId,
    state: Value,
) -> (Value, Value) {
    let context = Context {
        run: json!({ "id": run.0.as_ref(), "kind": "main", "call": "call_ask", "live": true }),
        now: json!({}),
        settings: json!({}),
        state,
        secrets: None,
        store: None,
        job: None,
    };
    let outcome = block_on_io(loaded.call(
        &hook,
        input,
        &context,
        Arc::new(NoReach),
        CancellationToken::new(),
    ));
    assert_eq!(outcome.error, None);
    (outcome.value, outcome.state)
}

/// The state and the view drawn from it, folded into `run` as the host
/// publishes them.
fn publish(
    ws: &mut Workspace,
    loaded: &Loaded,
    run: &RunId,
    state: Value,
    cx: &mut gpui::Context<Workspace>,
) {
    let (view, _) = call(loaded, Hook::View, Value::Null, run, state.clone());
    for record in [
        Record::State {
            plugin: tau_luau_plugins::names::ASK.into(),
            state,
        },
        Record::View {
            plugin: tau_luau_plugins::names::ASK.into(),
            view,
        },
    ] {
        ws.apply(
            HostUpdate::PluginFold {
                run: run.clone(),
                plugin: tau_luau_plugins::NAME.into(),
                body: serde_json::to_value(record).unwrap(),
            },
            cx,
        );
    }
}

/// A host for the workspace's tau-ask: the calls that ask, the actions
/// its panel sends, and the answers they give.
struct Serving {
    loaded: Loaded,
    events: Events,
    /// The answers given, by run: `tau_answer` as the actions gave it.
    sent: Vec<(String, Value)>,
}

impl Serving {
    /// `run`'s call asks `asked`.
    fn ask(
        &self,
        workspace: &Entity<Workspace>,
        cx: &mut VisualTestContext,
        run: &RunId,
        asked: Value,
    ) {
        let (_, state) = call(
            &self.loaded,
            Hook::Tool("ask".into()),
            asked,
            run,
            json!({}),
        );
        workspace
            .update(cx, |ws, cx| publish(ws, &self.loaded, run, state, cx));
        cx.run_until_parked();
    }

    /// Carries out the actions the panel sent, as the host does, until
    /// none is left: what changed is published, and answers are kept.
    fn serve(
        &mut self,
        workspace: &Entity<Workspace>,
        cx: &mut VisualTestContext,
    ) {
        loop {
            cx.run_until_parked();
            let asked: Vec<WorkspaceEvent> =
                self.events.borrow_mut().drain(..).collect();
            let acts: Vec<Act> = asked
                .into_iter()
                .filter_map(|event| match event {
                    WorkspaceEvent::PluginAct { plugin, action, .. }
                        if plugin == tau_luau_plugins::NAME =>
                    {
                        serde_json::from_value(action).ok()
                    }
                    _ => None,
                })
                .collect();
            if acts.is_empty() {
                return;
            }
            for act in acts {
                let Act::Action {
                    plugin,
                    action,
                    args,
                    run,
                    state,
                    ..
                } = act
                else {
                    continue;
                };
                assert_eq!(plugin, tau_luau_plugins::names::ASK);
                let run = RunId(run.as_str().into());
                let (value, after) = call(
                    &self.loaded,
                    Hook::Action(action),
                    args,
                    &run,
                    state.clone(),
                );
                if let Some(answer) = value.get("tau_answer") {
                    assert_eq!(after, state, "sending changes nothing");
                    self.sent.push((run.0.to_string(), answer.clone()));
                    // The run takes the answers: the call has its reply.
                    let reply = if answer["declined"] == true {
                        json!({ "call": answer["call"], "declined": true })
                    } else {
                        json!({ "call": answer["call"], "answers": answer["answers"] })
                    };
                    let (_, answered) = call(
                        &self.loaded,
                        Hook::Action("answered".into()),
                        reply,
                        &run,
                        after,
                    );
                    workspace.update(cx, |ws, cx| {
                        publish(ws, &self.loaded, &run, answered, cx)
                    });
                } else if after != state {
                    workspace.update(cx, |ws, cx| {
                        publish(ws, &self.loaded, &run, after, cx)
                    });
                }
            }
        }
    }
}

fn open(
    cx: &mut TestAppContext,
    runs: Vec<RunView>,
) -> (Entity<Workspace>, VisualTestContext, Serving) {
    tau_ui::hosted::install();
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
        Workspace::new("tau", runs, catalog, window, cx)
    });
    let workspace = window.root(cx).unwrap();
    let events = Events::default();
    let seen = events.clone();
    cx.update(|cx| {
        cx.subscribe(&workspace, move |_, event: &WorkspaceEvent, _| {
            seen.borrow_mut().push(event.clone())
        })
        .detach()
    });
    (
        workspace,
        VisualTestContext::from_window(window.into(), cx),
        Serving {
            loaded: plugin(),
            events,
            sent: Vec::new(),
        },
    )
}

/// Keystrokes, each carried out by the plugin before the next.
fn keys(
    serving: &mut Serving,
    workspace: &Entity<Workspace>,
    cx: &mut VisualTestContext,
    keys: &str,
) {
    for key in keys.split(' ') {
        cx.simulate_keystrokes(key);
        serving.serve(workspace, cx);
    }
}

#[gpui::test]
fn a_waiting_question_takes_the_composers_place_and_its_keys(
    cx: &mut TestAppContext,
) {
    let (workspace, mut cx, mut serving) = open(cx, vec![demo::retry_after()]);
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(demo::run_id()), cx);
        assert!(ws.run(&demo::run_id()).unwrap().status.is_live());
    });
    serving.ask(&workspace, &mut cx, &demo::run_id(), questions());

    // The panel took the keys as it appeared. `2` picks the second
    // choice and goes on; on the checklist `1` and `3` toggle, and `n`
    // opens the note, which takes text and closes on Enter.
    keys(&mut serving, &workspace, &mut cx, "2 1 3 n");
    cx.simulate_input("only for long waits");
    keys(&mut serving, &workspace, &mut cx, "enter");
    // Back on the panel: Right is the review, and Enter sends.
    keys(&mut serving, &workspace, &mut cx, "right enter");

    assert_eq!(serving.sent.len(), 1, "{:?}", serving.sent);
    let (run, answer) = &serving.sent[0];
    assert_eq!(*run, demo::run_id().0.to_string());
    assert_eq!(answer["call"], "call_ask");
    assert_eq!(
        answer["answers"],
        json!([
            { "picked": ["End the turn"] },
            { "picked": ["Run list badge", "Parent run"], "note": "only for long waits" },
        ])
    );

    // Answered, the composer is back with the keys: what the person
    // types goes into it.
    cx.simulate_input("thanks");
    cx.run_until_parked();
    workspace.update(&mut cx, |ws, cx| {
        assert_eq!(ws.composer_text(cx), "thanks");
    });
}

/// A question that comes while the person writes a message does not
/// take their keys: what they type goes nowhere near an answer. Its
/// Cancel run stops the run.
#[gpui::test]
fn the_panel_does_not_take_the_keys_from_a_message_being_written(
    cx: &mut TestAppContext,
) {
    let (workspace, mut cx, mut serving) = open(cx, vec![demo::retry_after()]);
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(demo::run_id()), cx)
    });
    cx.run_until_parked();
    cx.simulate_input("now also");
    cx.run_until_parked();
    workspace.update(&mut cx, |ws, cx| {
        assert_eq!(ws.composer_text(cx), "now also");
    });
    serving.ask(&workspace, &mut cx, &demo::run_id(), questions());
    keys(&mut serving, &workspace, &mut cx, "2 right enter");
    assert!(serving.sent.is_empty(), "{:?}", serving.sent);
    // The panel's Cancel run is tau's own action, on the run.
    workspace.update(&mut cx, |ws, cx| {
        let origin = tau_ui_remote::plugins::Origin {
            run: Some(demo::run_id()),
            params: Default::default(),
        };
        ws.plugin_action(
            tau_luau_plugins::NAME,
            &format!("luau/{}/tau.cancel", tau_luau_plugins::names::ASK),
            &json!({}),
            &origin,
            cx,
        );
    });
    cx.run_until_parked();
    assert!(serving.events.borrow().iter().any(|event| matches!(
        event,
        WorkspaceEvent::Cancel { run } if *run == demo::run_id()
    )));
}

#[gpui::test]
fn the_persons_own_answer_counts(cx: &mut TestAppContext) {
    let (workspace, mut cx, mut serving) = open(cx, vec![demo::retry_after()]);
    workspace.update(&mut cx, |ws, cx| {
        ws.navigate(Route::Run(demo::run_id()), cx)
    });
    serving.ask(&workspace, &mut cx, &demo::run_id(), questions());
    // `3` is the person's own answer on a two-choice question: its field
    // takes the text, and Enter goes on.
    keys(&mut serving, &workspace, &mut cx, "3");
    cx.simulate_input("hold, but time out");
    keys(&mut serving, &workspace, &mut cx, "enter 2 right enter");
    assert_eq!(serving.sent.len(), 1, "{:?}", serving.sent);
    let answers = &serving.sent[0].1["answers"];
    assert_eq!(answers[0]["other"], "hold, but time out");
    assert_eq!(answers[0]["picked"], json!([]));
    assert_eq!(answers[1]["picked"], json!(["Notification"]));
}

/// Away to a chat with no question and back: the panel has the keys again.
#[gpui::test]
fn a_waiting_question_takes_the_keys_again_on_coming_back(
    cx: &mut TestAppContext,
) {
    let second = RunId("second".into());
    let mut other = demo::retry_after();
    other.id = second.clone();
    other.title = "second".into();
    let (workspace, mut cx, mut serving) =
        open(cx, vec![demo::retry_after(), other]);
    let go = |run: &RunId, cx: &mut VisualTestContext| {
        workspace.update(cx, |ws, cx| ws.navigate(Route::Run(run.clone()), cx));
        cx.run_until_parked();
    };
    go(&demo::run_id(), &mut cx);
    serving.ask(&workspace, &mut cx, &demo::run_id(), questions());
    go(&second, &mut cx);
    go(&demo::run_id(), &mut cx);
    keys(&mut serving, &workspace, &mut cx, "2");
    keys(&mut serving, &workspace, &mut cx, "1 right enter");
    assert_eq!(serving.sent.len(), 1, "{:?}", serving.sent);
    assert_eq!(
        serving.sent[0].1["answers"][0]["picked"],
        json!(["End the turn"])
    );
}

/// Two runs waiting at once, on calls with the same id: each keeps its
/// own answers as the person goes between them, and each reply goes to
/// its own run.
#[gpui::test]
fn two_waiting_runs_keep_their_own_answers(cx: &mut TestAppContext) {
    let second = RunId("second".into());
    let mut other = demo::retry_after();
    other.id = second.clone();
    other.title = "second".into();
    let (workspace, mut cx, mut serving) =
        open(cx, vec![demo::retry_after(), other]);
    let three = json!({ "questions": [{
        "question": "Which lane?",
        "header": "Lane",
        "options": [
            { "label": "Left", "description": "l" },
            { "label": "Middle", "description": "m" },
            { "label": "Right", "description": "r" }
        ]
    }]});
    serving.ask(&workspace, &mut cx, &demo::run_id(), questions());
    serving.ask(&workspace, &mut cx, &second, three);
    let go = |run: &RunId, cx: &mut VisualTestContext| {
        workspace.update(cx, |ws, cx| ws.navigate(Route::Run(run.clone()), cx));
        cx.run_until_parked();
    };
    go(&demo::run_id(), &mut cx);
    // The first run's first question, answered; then the second run's.
    keys(&mut serving, &workspace, &mut cx, "2");
    go(&second, &mut cx);
    keys(&mut serving, &workspace, &mut cx, "3");
    // Back to the first: it is where it was left, on its checklist.
    go(&demo::run_id(), &mut cx);
    keys(&mut serving, &workspace, &mut cx, "1 right enter");
    // The first answered, the composer has the keys again, until the
    // second run's panel is back in its place: it is on its review, and
    // Enter sends.
    go(&second, &mut cx);
    keys(&mut serving, &workspace, &mut cx, "enter");
    let send = |cx: &mut VisualTestContext| {
        workspace.update(cx, |ws, cx| {
            let origin = tau_ui_remote::plugins::Origin {
                run: Some(second.clone()),
                params: Default::default(),
            };
            ws.plugin_action(
                tau_luau_plugins::NAME,
                &format!("luau/{}/send", tau_luau_plugins::names::ASK),
                &json!({}),
                &origin,
                cx,
            );
        });
    };
    assert_eq!(serving.sent.len(), 2, "{:?}", serving.sent);
    // A second Send, once answered, sends nothing more.
    send(&mut cx);
    serving.serve(&workspace, &mut cx);

    assert_eq!(serving.sent.len(), 2, "{:?}", serving.sent);
    let of = |run: &str| {
        serving
            .sent
            .iter()
            .find(|(sent, _)| sent == run)
            .map(|(_, answer)| answer["answers"].clone())
            .unwrap()
    };
    assert_eq!(
        of(demo::run_id().0.as_ref()),
        json!([{ "picked": ["End the turn"] }, { "picked": ["Run list badge"] }])
    );
    assert_eq!(of("second"), json!([{ "picked": ["Right"] }]));
}
