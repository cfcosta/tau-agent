//! Replays the latest stored runs through tau-reasoning with the real
//! Jev, and prints what it would pick next to what the runs went out at.
//! Needs `TYPESAFE_API_KEY` and the network:
//!
//! ```sh
//! cargo run -p tau-built-in-tests --example replay -- ~/.local/share/tau/runs.db 20
//! ```
//!
//! tau-reasoning is Luau (ADR 0050): the replay loads the plugin as tau
//! does and calls its hooks as a run would, `start` as a message comes
//! in and `before_request` before each request, each run on its own
//! model. It reports decisions, not savings: what a request would have
//! cost at another effort is unknown.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]
#![allow(dead_code, reason = "tests/replay.rs uses the replay alone")]

use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use serde_json::{Value, json};
use tau_ai::message::{InputBlock, Message, UserContent};
use tau_codemode_host::{ToolCall, ToolEntry, ToolReply};
use tau_jev::Jev;
use tau_luau_plugins_host::runtime::{Context, Hook, Loaded, Reach, load};
use tokio_util::sync::CancellationToken;

/// One entry of a stored run's timeline, as the replay reads it.
#[derive(Debug, Clone)]
pub enum Entry {
    Message(Message),
    /// What the run kept of tau-reasoning: the effort a request went out
    /// at, when it says.
    Ran(Option<String>),
}

/// What the plugin decided before one stored request.
#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    /// The request's place in the run, from 1.
    pub request: usize,
    /// `user_turn` or `tool_step`.
    pub step: &'static str,
    /// The effort the stored request went out at, as the run's records
    /// say.
    pub recorded: Option<String>,
    /// What Jev answered, when the plugin asked: its choice, or why it
    /// failed; `None` when a lease held.
    pub asked: Option<Result<Value, String>>,
    /// The effort the plugin sends the request at.
    pub runs_at: Option<String>,
}

/// The plugin, as tau loads it.
pub async fn plugin() -> Loaded {
    let files = tau_luau_plugins_host::built_in::files(
        tau_luau_plugins::names::REASONING,
    )
    .unwrap();
    load(tau_luau_plugins::names::REASONING, files)
        .await
        .expect("it loads")
}

/// What the hooks reach in the replay: Jev, and the conversation so far.
struct Replaying {
    jev: Arc<dyn Jev>,
    transcript: Value,
}

#[async_trait]
impl Reach for Replaying {
    fn tools(&self) -> Vec<ToolEntry> {
        vec![ToolEntry {
            name: "transcript".into(),
            description: "The run's conversation.".into(),
            input_schema: json!({ "type": "object" }),
            output_schema: None,
            namespace: None,
            sequential: false,
        }]
    }

    async fn call(&self, call: ToolCall) -> Result<ToolReply, String> {
        match call.name.as_str() {
            "transcript" => Ok(ToolReply::success(self.transcript.clone())),
            other => Err(format!("`{other}` is not in the replay")),
        }
    }

    fn jev(&self) -> Option<Arc<dyn Jev>> {
        Some(self.jev.clone())
    }
}

/// Replays one run's `timeline` on `model`, with `settings` as the
/// plugin's (`redecide`, `threshold`).
pub async fn replay(
    plugin: &Loaded,
    jev: Arc<dyn Jev>,
    model: &str,
    settings: Value,
    timeline: &[Entry],
) -> Vec<Decision> {
    let efforts: Vec<&str> = tau_ai::model::efforts(model)
        .into_iter()
        .map(|effort| effort.as_str())
        .collect();
    let mut decisions = Vec::new();
    if efforts.is_empty() {
        return decisions;
    }
    let mut transcript: Vec<Message> = Vec::new();
    let mut state = json!({});
    let mut recorded: Option<String> = None;
    let mut task = String::new();
    let mut starts = false;
    let mut turn = 0;
    for entry in timeline {
        let message = match entry {
            Entry::Ran(ran) => {
                recorded = ran.clone();
                continue;
            }
            Entry::Message(message) => message,
        };
        match message {
            Message::User(user) if answered(&transcript) => {
                task = user_text(&user.content);
                starts = true;
            }
            Message::Assistant(_) => {
                let start = std::mem::take(&mut starts);
                turn += 1;
                let choices = count(&state["choices"]);
                let notes = count(&state["notes"]);
                let reach = Arc::new(Replaying {
                    jev: jev.clone(),
                    transcript: tau_luau_plugins_host::rewrite::transcript(
                        &transcript,
                    ),
                });
                let context = |state: &Value| Context {
                    run: json!({ "id": "replay", "kind": "main", "model": model }),
                    now: json!({}),
                    settings: settings.clone(),
                    state: state.clone(),
                    secrets: None,
                    store: None,
                    job: None,
                };
                let call = |hook: Hook, input: Value, state: Value| {
                    let reach = reach.clone();
                    let context = context(&state);
                    async move {
                        plugin
                            .call(
                                &hook,
                                input,
                                &context,
                                reach,
                                CancellationToken::new(),
                            )
                            .await
                    }
                };
                if start {
                    let prefix = tau_agent::context::estimate_context_tokens(
                        &transcript,
                    );
                    let input = json!({
                        "input": task, "instructions": "", "model": model,
                        "efforts": efforts, "prefix_tokens": prefix,
                    });
                    state = call(Hook::Start, input, state).await.state;
                }
                let replies = transcript
                    .iter()
                    .filter(|message| matches!(message, Message::Assistant(_)))
                    .count();
                let mut input = json!({
                    "model": model, "turn": turn,
                    "messages": transcript.len(), "replies": replies,
                });
                // Left out at the model's default, as tau leaves it.
                if let Some(effort) = state["effort_now"].as_str() {
                    input["effort"] = json!(effort);
                }
                state = call(Hook::BeforeRequest, input, state).await.state;
                let asked = if count(&state["choices"]) > choices {
                    let choice =
                        state["choices"].as_array().and_then(|all| all.last());
                    Some(Ok(choice.cloned().unwrap_or(Value::Null)))
                } else if count(&state["notes"]) > notes {
                    Some(Err("Jev could not score".to_owned()))
                } else {
                    None
                };
                decisions.push(Decision {
                    request: decisions.len() + 1,
                    step: if start { "user_turn" } else { "tool_step" },
                    recorded: recorded.clone(),
                    asked,
                    runs_at: state["effort_now"].as_str().map(str::to_owned),
                });
            }
            _ => {}
        }
        transcript.push(message.clone());
    }
    decisions
}

fn count(value: &Value) -> usize {
    value
        .as_array()
        .map(Vec::len)
        .or_else(|| value.as_object().map(serde_json::Map::len))
        .unwrap_or(0)
}

/// Whether the transcript is empty or ends with a final answer, so a
/// user message starts a new message rather than steering the run.
fn answered(transcript: &[Message]) -> bool {
    match transcript.last() {
        None => true,
        Some(Message::Assistant(reply)) => reply.tool_calls().next().is_none(),
        Some(_) => false,
    }
}

/// The user's words: the last text block, after any context put before
/// them.
fn user_text(content: &UserContent) -> String {
    match content {
        UserContent::Text(text) => text.clone(),
        UserContent::Blocks(blocks) => blocks
            .iter()
            .rev()
            .find_map(|block| match block {
                InputBlock::Text(text) => Some(text.text.clone()),
                InputBlock::Image(_) => None,
            })
            .unwrap_or_default(),
    }
}

/// The effort a stored record says a request went out at: a choice of
/// the Rust plugin's, or the Luau plugin's state after one.
fn ran_at(plugin: &str, body: &str) -> Option<Option<String>> {
    let body: Value = serde_json::from_str(body).ok()?;
    let choice = match plugin {
        tau_luau_plugins::names::REASONING if body["kind"] == "choice" => body,
        "tau-luau-plugins"
            if body["kind"] == "state"
                && body["plugin"] == tau_luau_plugins::names::REASONING =>
        {
            body["state"]["choices"].as_array()?.last()?.clone()
        }
        _ => return None,
    };
    Some(match choice["runs_at"].as_str() {
        Some(effort) => Some(effort.to_owned()),
        None => (choice["verdict"] == "chose")
            .then(|| choice["effort"].as_str().map(str::to_owned))
            .flatten(),
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().ok_or("usage: replay <runs.db> [runs]")?;
    let limit: u32 = args.next().map_or(Ok(20), |n| n.parse())?;
    let jev: Arc<dyn Jev> = Arc::new(tau_jev::TypeSafe::from_env()?);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let plugin = plugin().await;
        let store = tau_store_sqlite::open(&path).await?;
        let settings = json!({ "redecide": false, "threshold": 0.7 });
        let (mut requests, mut asked, mut changed, mut cost) = (0, 0, 0, 0.0);
        // Stored effort → replayed effort, per request.
        let mut moves: BTreeMap<(String, String), usize> = BTreeMap::new();
        for run in store.recent_runs(limit).await? {
            let timeline: Vec<Entry> = store
                .timeline(&run.id)
                .await?
                .into_iter()
                .filter_map(|entry| match entry {
                    tau_store::Entry::Message { body, .. } => {
                        serde_json::from_str(&body).ok().map(Entry::Message)
                    }
                    tau_store::Entry::Plugin { plugin, body } => {
                        ran_at(&plugin, &body).map(Entry::Ran)
                    }
                    _ => None,
                })
                .collect();
            let decisions = replay(
                &plugin,
                jev.clone(),
                &run.model,
                settings.clone(),
                &timeline,
            )
            .await;
            println!(
                "run {} ({}, {} requests)",
                run.id,
                run.model,
                decisions.len()
            );
            let name = |effort: &Option<String>| {
                effort.clone().unwrap_or_else(|| "default".into())
            };
            for decision in &decisions {
                let how = match &decision.asked {
                    None => "lease held".to_owned(),
                    Some(Err(error)) => format!("failed: {error}"),
                    Some(Ok(choice)) => {
                        cost += choice["cost"].as_f64().unwrap_or(0.0);
                        format!(
                            "{} {} at {:.2}, lease {}",
                            choice["verdict"].as_str().unwrap_or_default(),
                            choice["effort"].as_str().unwrap_or_default(),
                            choice["confidence"].as_f64().unwrap_or_default(),
                            choice["lease"].as_str().unwrap_or("none"),
                        )
                    }
                };
                println!(
                    "  #{:<3} {:<9} stored {:<8} → {:<8} ({how})",
                    decision.request,
                    decision.step,
                    name(&decision.recorded),
                    name(&decision.runs_at),
                );
                requests += 1;
                asked += usize::from(decision.asked.is_some());
                changed += usize::from(decision.recorded != decision.runs_at);
                *moves
                    .entry((name(&decision.recorded), name(&decision.runs_at)))
                    .or_default() += 1;
            }
        }
        println!(
            "\n{requests} requests; Jev asked {asked} times (${cost:.4}); \
             {changed} would go out at another effort"
        );
        for ((from, to), count) in moves {
            println!("  {from:<8} → {to:<8} {count}");
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    })
}
