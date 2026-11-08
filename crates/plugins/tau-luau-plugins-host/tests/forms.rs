//! A run's tool asks the person a form (ADR 0050, gap C): its call waits
//! while the plugin's view draws the form, one of its actions gives the
//! answers, from whichever device answers first, and the plugin's
//! `action` takes them in the run before the model reads what it says.
//! Answers that do not fit wait for better ones; a cancelled run ends
//! the wait.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use futures_util::StreamExt;
use serde_json::{Value, json};
use tau_agent::{agent::Agent, event::RunEvent};
use tau_luau_plugins::{ActionDone, NAME, Record};
use tau_luau_plugins_host::{
    agent::{Dispatch, LuauPlugins},
    registry::{ActionCall, Registry},
};
use tau_testing::{block_on_io, scripted::ScriptedModel};
use tau_vcs_host::{Identity, Project, ProjectRepo};

const QUIZ: &str = r#"
local tau = require("tau")
local ui = tau.ui
return tau.plugin {
  name = "quiz",
  tools = {
    quiz = {
      description = "Asks the person which.",
      model_only = true,
      call = function(args, ctx)
        ctx.state.call = ctx.run.call
        return tau.ask {
          questions = { {
            question = "Which?", header = "Pick", other = true,
            options = { { label = "A" }, { label = "B" } },
          } },
          action = "answered",
        }
      end,
    },
    bad = {
      description = "Asks nothing.",
      call = function(args, ctx)
        return tau.ask { questions = array({}) }
      end,
    },
  },
  actions = {
    pick = function(args, ctx)
      return tau.answer { call = ctx.state.call, answers = { { picked = { args.label } } } }
    end,
    decline = function(args, ctx)
      return tau.answer { call = ctx.state.call, declined = true }
    end,
    answered = function(args, ctx)
      ctx.state.reply = args
      if args.declined then return "declined" end
      return "picked " .. args.answers[1].picked[1] .. " for " .. args.call
    end,
  },
  view = function(state, ctx)
    if state.call and not state.reply then
      return { composer = ui.text("Which?") }
    end
    return {}
  end,
}
"#;

fn registry(home: &std::path::Path) -> Registry {
    let repo = ProjectRepo::init(
        home.join("v1"),
        Identity::default(),
        &[("quiz/plugin.luau".to_owned(), QUIZ.to_owned())],
    )
    .unwrap();
    let trunk = repo.trunk().unwrap();
    let registry = Registry::at(
        home.join("x"),
        home.join("allowed.json"),
        Identity::default(),
        || {},
    );
    block_on_io(registry.reload(&Project::from(repo), &trunk));
    let overview = block_on_io(registry.overview());
    assert!(
        matches!(
            overview.plugins.iter().find(|entry| entry.name == "quiz"),
            Some(entry) if matches!(entry.standing, tau_luau_plugins::Standing::Active)
        ),
        "{overview:?}"
    );
    registry
}

/// What a device did: an action of the plugin's, on the run's state as
/// that device has it.
struct Click {
    action: &'static str,
    args: Value,
}

/// What came of a run whose model calls `tool`, the person answering
/// with `clicks`, each from a device of its own, once the form waits:
/// what the model read, whether the call failed, each click's error, and
/// the plugin's state as the run left it.
struct Asked {
    result: String,
    failed: bool,
    errors: Vec<Option<String>>,
    state: Value,
    waited: bool,
}

fn ask(tool: &str, clicks: &[Click], cancel: bool) -> Asked {
    let home = tempfile::tempdir().unwrap();
    let registry = registry(home.path());
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call(tool, json!({})))
        .turn(|t| t.text("asked"));
    let store = block_on_io(tau_store_sqlite::memory()).unwrap();
    let plugins = LuauPlugins::new(
        block_on_io(registry.active()),
        json!({ "kind": "chat", "repo": "home", "model": "m" }),
        None,
    )
    .with_dispatch(Dispatch {
        allows: registry.allows(),
        forms: registry.forms(),
        registry: registry.clone(),
        chats: None,
    });
    let agent = Agent::new(llm.clone()).plugin(plugins);
    let forms = registry.forms();
    let (events, run, errors, waited) = block_on_io(async {
        let mut run = agent.start("which?", &store);
        let (id, control) = (run.id(), run.control());
        let answering = async {
            // Nothing to click: nothing is waited for.
            if clicks.is_empty() && !cancel {
                return (Vec::new(), !forms.waiting().is_empty());
            }
            let key = loop {
                if let Some(key) = forms.waiting().into_iter().next() {
                    break key;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            };
            assert_eq!(key.0, id.0.as_ref(), "the form is the run's");
            if cancel {
                control.cancel();
            }
            // What the plugin left while the call waits, as each device
            // has it.
            let state = stored(&store, &id.0).await;
            let mut errors = Vec::new();
            for click in clicks {
                let done: ActionDone = registry
                    .action(ActionCall {
                        plugin: "quiz",
                        action: click.action,
                        args: &click.args,
                        run: &id.0,
                        repo: None,
                        state: &state,
                        settings: None,
                        chats: None,
                        jev: None,
                        live: true,
                        keyed: false,
                    })
                    .await;
                assert!(!done.changed, "answering leaves the state be");
                errors.push(done.error.map(|(_, error)| error));
            }
            (errors, true)
        };
        let events = async {
            let events: Vec<RunEvent> = run.events().collect().await;
            events
        };
        let ((errors, waited), events) = tokio::join!(answering, events);
        (events, id, errors, waited)
    });
    let (result, failed) = events
        .iter()
        .find_map(|event| match event {
            RunEvent::ToolEnd {
                output, is_error, ..
            } => Some((output.text_content(), *is_error)),
            _ => None,
        })
        .unwrap_or_default();
    assert!(forms.waiting().is_empty(), "nothing waits once it ended");
    Asked {
        result,
        failed,
        errors,
        state: block_on_io(stored(&store, &run.0)),
        waited,
    }
}

/// The plugin's state as last stored with run `run`.
async fn stored(store: &tau_store::Store, run: &str) -> Value {
    let records = store.records(run, NAME).await.unwrap();
    records
        .iter()
        .rev()
        .filter_map(|body| serde_json::from_str::<Record>(body).ok())
        .find_map(|record| match record {
            Record::State { plugin, state } if plugin == "quiz" => Some(state),
            _ => None,
        })
        .unwrap_or_else(|| json!({}))
}

fn pick(label: &str) -> Click {
    Click {
        action: "pick",
        args: json!({ "label": label }),
    }
}

/// From two devices at once, the first answer takes the form and the
/// second finds it taken; the plugin's action takes the answers in the
/// run, and the model reads what it says.
#[test]
fn the_first_answer_takes_the_form_from_whichever_device() {
    let asked = ask("quiz", &[pick("B"), pick("A")], false);
    assert!(asked.waited);
    assert_eq!(asked.errors[0], None);
    assert!(
        asked.errors[1]
            .as_deref()
            .is_some_and(|error| error.contains("answered already")),
        "{:?}",
        asked.errors
    );
    assert!(!asked.failed);
    assert_eq!(asked.result, "picked B for call_0|fc_0");
    assert_eq!(
        asked.state["reply"],
        json!({ "call": "call_0|fc_0", "answers": [{ "picked": ["B"] }] })
    );
}

/// Answers that do not fit what was asked are the action's error, and
/// the form waits for ones that do; declining fits any form.
#[test]
fn answers_that_do_not_fit_leave_the_form_waiting() {
    let decline = Click {
        action: "decline",
        args: json!({}),
    };
    let asked = ask("quiz", &[pick("C"), decline], false);
    assert!(
        asked.errors[0]
            .as_deref()
            .is_some_and(|error| error.contains("offers no choice \"C\"")),
        "{:?}",
        asked.errors
    );
    assert_eq!(asked.errors[1], None);
    assert_eq!(asked.result, "declined");
    assert_eq!(asked.state["reply"]["declined"], true);
}

/// A run cancelled while the form waits ends the call with an error, and
/// an answer after it finds nothing to take.
#[test]
fn a_cancelled_run_stops_waiting_for_the_form() {
    let asked = ask("quiz", &[pick("A")], true);
    assert!(asked.failed);
    assert_eq!(
        asked.result,
        "The run was cancelled before the person answered."
    );
    assert!(asked.errors[0].is_some(), "{:?}", asked.errors);
    assert_eq!(asked.state.get("reply"), None);
}

/// A form with no questions is the call's error, and nothing waits.
#[test]
fn a_form_that_asks_nothing_is_refused() {
    let asked = ask("bad", &[], false);
    assert!(!asked.waited);
    assert!(asked.failed);
    assert!(
        asked.result.contains("1 to 4 questions"),
        "{}",
        asked.result
    );
}

/// A tool that waits for the person is the model's: no script calls it.
#[test]
fn a_tool_that_waits_for_the_person_is_the_models_alone() {
    let home = tempfile::tempdir().unwrap();
    let registry = registry(home.path());
    let plugins = LuauPlugins::new(
        block_on_io(registry.active()),
        json!({ "kind": "chat", "repo": "home", "model": "m" }),
        None,
    );
    use tau_agent::plugin::Plugin as _;
    let exposures: Vec<(String, tau_agent::tool::Exposure)> = plugins
        .tools()
        .iter()
        .map(|tool| (tool.name().to_owned(), tool.exposure()))
        .collect();
    assert_eq!(
        exposures,
        [
            ("bad".to_owned(), tau_agent::tool::Exposure::Direct),
            ("quiz".to_owned(), tau_agent::tool::Exposure::ModelOnly),
        ]
    );
}
