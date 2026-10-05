//! A live probe of prompt caching on `wss://api.openai.com/v1/responses`
//! with a ChatGPT plan, run by hand: which ways of sending a
//! conversation read the cache (`docs/reference/openai-websocket.md`,
//! "Prompt cache"). It measured what connection affinity is built on
//! (ADR 0022).
//!
//! ```sh
//! cargo run -p tau-ai --example cache_probe -- --cases     [--only NAME]
//! cargo run -p tau-ai --example cache_probe -- --variants SECONDS [--no-key]
//! cargo run -p tau-ai --example cache_probe -- --fork SECONDS
//! cargo run -p tau-ai --example cache_probe -- --shared
//! cargo run -p tau-ai --example cache_probe -- --handoff
//! ```
//!
//! - `--cases`: a chain of three requests on one connection (new, delta,
//!   full resend), then the whole conversation resent on a new
//!   connection; with and without `prompt_cache_key`, and with one tool
//!   fewer on the resend. `--only` runs one case by name.
//! - `--variants N`: a chain, then on the same connection one tool
//!   fewer, a longer instructions tail and another effort; then after
//!   `N` seconds a new connection, and back on the first.
//! - `--fork N`: a parent chain under key P; a fork that starts under P
//!   on a new connection after `N` seconds and moves to its own key F.
//! - `--shared`: a parent and a fork interleaved on one connection, each
//!   continuing its own chain.
//! - `--handoff`: a connection writes a prefix; new connections read it
//!   at once with the same key, another key or none, then after the
//!   first closes.
//!
//! Every command takes `--model M` (default `gpt-5.5`), `--account ID`
//! (default: the active account) and `--store DIR` (default:
//! `$XDG_CONFIG_HOME/tau/chatgpt`). Every case starts its instructions
//! with a fresh nonce, so none reads what an earlier one, or an earlier
//! run, cached. Requests are about 4k tokens. It prints each response's
//! input and cached tokens, and never a token of the sign-in.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{collections::HashMap, time::Duration};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tau_ai::{
    chatgpt::{AccountId, ChatGpt, ChatGptConnector, Store},
    ws::io::connection::Connector,
};
use tokio_tungstenite::tungstenite::Message;

type Error = Box<dyn std::error::Error>;
type Socket =
    tokio_tungstenite::WebSocketStream<<ChatGptConnector as Connector>::Stream>;

struct Args {
    flags: HashMap<String, Option<String>>,
}

impl Args {
    fn parse() -> Self {
        let rest: Vec<String> = std::env::args().skip(1).collect();
        let mut flags = HashMap::new();
        let mut at = 0;
        while at < rest.len() {
            let name = rest[at].trim_start_matches("--").to_owned();
            let value = rest
                .get(at + 1)
                .filter(|next| !next.starts_with("--"))
                .cloned();
            at += if value.is_some() { 2 } else { 1 };
            flags.insert(name, value);
        }
        Self { flags }
    }

    fn value(&self, name: &str) -> Option<&str> {
        self.flags.get(name).and_then(|value| value.as_deref())
    }

    fn has(&self, name: &str) -> bool {
        self.flags.contains_key(name)
    }

    fn seconds(&self, name: &str) -> Result<u64, Error> {
        Ok(self.value(name).ok_or("a number of seconds")?.parse()?)
    }
}

/// What one response reported.
#[derive(Debug, Default, Clone)]
struct Turn {
    id: String,
    input: u64,
    cached: u64,
    output_items: Vec<Value>,
}

/// Opens connections and builds requests for one case.
struct Probe {
    connector: ChatGptConnector,
    model: String,
    /// Starts the instructions, so no case reads another's cache.
    nonce: String,
}

impl Probe {
    async fn connect(&self) -> Result<Socket, Error> {
        let stream = self.connector.connect().await?;
        let (socket, _) =
            tokio_tungstenite::client_async(self.connector.request(), stream)
                .await?;
        Ok(socket)
    }

    /// A request for `input`, with every tool or one fewer, under `key`.
    fn body(
        &self,
        input: Vec<Value>,
        all_tools: bool,
        key: Option<&str>,
    ) -> Value {
        let mut body = json!({
            "model": self.model,
            "store": false,
            "instructions": instructions(&self.nonce, 2400),
            "tools": tools(all_tools),
            "reasoning": {"effort": "low", "summary": "auto"},
            "include": ["reasoning.encrypted_content"],
            "input": input,
        });
        if let Some(key) = key {
            body["prompt_cache_key"] = key.into();
        }
        body
    }

    /// A request continuing `previous` with `input`.
    fn delta(
        &self,
        previous: &Turn,
        input: Vec<Value>,
        key: Option<&str>,
    ) -> Value {
        let mut body = self.body(input, true, key);
        body["previous_response_id"] = previous.id.clone().into();
        body
    }
}

/// Instructions of roughly `words` words, distinct per `nonce`.
fn instructions(nonce: &str, words: usize) -> String {
    let mut text = format!(
        "Probe {nonce}. You are a terse assistant. Answer in at most five words.\n"
    );
    for n in 0..words / 12 {
        text.push_str(&format!(
            "Rule {n}: keep answers short, plain and literal; never add item {n} twice.\n"
        ));
    }
    text
}

fn tool(name: &str) -> Value {
    json!({
        "type": "function",
        "name": name,
        "description": format!(
            "The {name} tool. Takes a path and returns its text; used for \
             reading files in the repository, listing directories and similar."
        ),
        "parameters": {
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"],
            "additionalProperties": false,
        },
        "strict": true,
    })
}

/// The tools of a run; without `delegate` unless `all`.
fn tools(all: bool) -> Vec<Value> {
    let mut list: Vec<Value> =
        ["read", "ls", "grep", "find", "edit", "write", "bash"]
            .iter()
            .map(|name| tool(name))
            .collect();
    if all {
        list.push(tool("delegate"));
    }
    list
}

fn user(text: &str) -> Value {
    json!({"role": "user", "content": [{"type": "input_text", "text": text}]})
}

/// Sends `body` and waits for its response.
async fn send(socket: &mut Socket, mut body: Value) -> Result<Turn, Error> {
    body["type"] = "response.create".into();
    socket.send(Message::text(body.to_string())).await?;
    loop {
        let message =
            tokio::time::timeout(Duration::from_secs(120), socket.next())
                .await?
                .ok_or("the connection closed")??;
        let Message::Text(text) = message else {
            continue;
        };
        let frame: Value = serde_json::from_str(&text)?;
        match frame["type"].as_str() {
            Some("response.completed") => {
                let response = &frame["response"];
                let usage = &response["usage"];
                return Ok(Turn {
                    id: response["id"].as_str().unwrap_or_default().into(),
                    input: usage["input_tokens"].as_u64().unwrap_or(0),
                    cached: usage["input_tokens_details"]["cached_tokens"]
                        .as_u64()
                        .unwrap_or(0),
                    output_items: response["output"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default(),
                });
            }
            Some("response.failed" | "error") => {
                return Err(format!("failed: {text}").into());
            }
            _ => {}
        }
    }
}

fn line(label: &str, turn: &Turn) {
    let percent = (turn.cached * 100).checked_div(turn.input).unwrap_or(0);
    println!(
        "  {label:<48} input {:>6}  cached {:>6}  ({percent}%)",
        turn.input, turn.cached
    );
}

async fn pause(seconds: u64) {
    tokio::time::sleep(Duration::from_secs(seconds)).await;
}

/// Three requests on one connection, each continuing the last, then the
/// whole conversation resent on a new connection, as a lost
/// continuation, a resumed chat or a fork would.
async fn chain_then_resend(
    probe: &Probe,
    key: Option<&str>,
    resend_all_tools: bool,
) -> Result<(), Error> {
    let mut a = probe.connect().await?;
    let mut input = vec![user("Say: one.")];
    let first = send(&mut a, probe.body(input.clone(), true, key)).await?;
    line("1 first request (new chain)", &first);
    input.extend(first.output_items.clone());
    pause(3).await;
    let ask = user("Say: two.");
    let second =
        send(&mut a, probe.delta(&first, vec![ask.clone()], key)).await?;
    line("2 delta on the same connection", &second);
    input.push(ask);
    input.extend(second.output_items.clone());
    pause(3).await;
    input.push(user("Say: three."));
    let full = send(&mut a, probe.body(input.clone(), true, key)).await?;
    line("3 full resend, same connection", &full);
    input.extend(full.output_items.clone());
    let _ = a.close(None).await;
    pause(3).await;
    let mut b = probe.connect().await?;
    input.push(user("Say: four."));
    let label = if resend_all_tools {
        "4 full resend, NEW connection"
    } else {
        "4 full resend, new connection, one tool fewer"
    };
    let resent = send(&mut b, probe.body(input, resend_all_tools, key)).await?;
    line(label, &resent);
    let _ = b.close(None).await;
    Ok(())
}

/// One chain on a connection, then variants on the same connection, and
/// a new connection after `delay` seconds.
async fn variants(
    probe: &Probe,
    key: Option<&str>,
    delay: u64,
) -> Result<(), Error> {
    let mut a = probe.connect().await?;
    let mut input = vec![user("Say: one.")];
    let first = send(&mut a, probe.body(input.clone(), true, key)).await?;
    line("1 first request", &first);
    input.extend(first.output_items.clone());
    pause(3).await;
    input.push(user("Say: two."));
    let same = send(&mut a, probe.body(input.clone(), true, key)).await?;
    line("2 full resend, same conn", &same);
    let fewer = send(&mut a, probe.body(input.clone(), false, key)).await?;
    line("3 same conn, one tool fewer", &fewer);
    let mut longer = probe.body(input.clone(), true, key);
    let text = longer["instructions"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
        + "\nRepository notes: be brief.";
    longer["instructions"] = text.into();
    let tail = send(&mut a, longer).await?;
    line("4 same conn, instructions tail changed", &tail);
    let mut effort = probe.body(input.clone(), true, key);
    effort["reasoning"]["effort"] = "medium".into();
    let effort = send(&mut a, effort).await?;
    line("5 same conn, effort low->medium", &effort);
    println!("  (waiting {delay}s, then a new connection)");
    pause(delay).await;
    let mut b = probe.connect().await?;
    let later = send(&mut b, probe.body(input.clone(), true, key)).await?;
    line("6 new conn after delay", &later);
    let again = send(&mut b, probe.body(input.clone(), true, key)).await?;
    line("7 same new conn, resend again", &again);
    pause(3).await;
    let back = send(&mut a, probe.body(input, true, key)).await?;
    line("8 back on the FIRST conn, after the delay", &back);
    Ok(())
}

/// A parent chain under key `p`; a fork that starts under `p` on a new
/// connection after `wait` seconds, then moves to its own key `f`.
async fn fork_keys(
    probe: &Probe,
    p: &str,
    f: &str,
    wait: u64,
) -> Result<(), Error> {
    let mut a = probe.connect().await?;
    let mut input = vec![user("Say: one.")];
    let first = send(&mut a, probe.body(input.clone(), true, Some(p))).await?;
    line("A1 parent, key P", &first);
    input.extend(first.output_items.clone());
    input.push(user("Say: two."));
    let second = send(&mut a, probe.body(input.clone(), true, Some(p))).await?;
    line("A2 parent, key P", &second);
    input.extend(second.output_items.clone());
    let _ = a.close(None).await;
    println!("  (waiting {wait}s)");
    pause(wait).await;
    let mut b = probe.connect().await?;
    input.push(user("Fork: say three."));
    let fork = send(&mut b, probe.body(input.clone(), true, Some(p))).await?;
    line("B1 fork's first request, new conn, key P", &fork);
    input.extend(fork.output_items.clone());
    let ask = user("Say: four.");
    match send(&mut b, probe.delta(&fork, vec![ask.clone()], Some(f))).await {
        Ok(turn) => {
            line("B2 delta, same conn, key changed to F", &turn);
            input.push(ask);
            input.extend(turn.output_items.clone());
        }
        Err(error) => {
            println!("  B2 delta with a changed key refused: {error}");
            input.push(ask);
        }
    }
    input.push(user("Say: five."));
    let full = send(&mut b, probe.body(input.clone(), true, Some(f))).await?;
    line("B3 full resend, same conn, key F", &full);
    input.extend(full.output_items.clone());
    let _ = b.close(None).await;
    println!("  (waiting {wait}s)");
    pause(wait).await;
    let mut c = probe.connect().await?;
    input.push(user("Say: six."));
    let later = send(&mut c, probe.body(input.clone(), true, Some(f))).await?;
    line("C1 fork resumed, new conn, key F", &later);
    let mut d = probe.connect().await?;
    let parent = send(&mut d, probe.body(input, true, Some(p))).await?;
    line("D1 same input, new conn, key P", &parent);
    Ok(())
}

/// A parent and a fork interleaved on one connection, each continuing
/// its own chain with `previous_response_id`.
async fn shared(probe: &Probe) -> Result<(), Error> {
    let mut a = probe.connect().await?;
    let mut parent = vec![user("Say: one.")];
    let p1 = send(&mut a, probe.body(parent.clone(), true, Some("P"))).await?;
    line("P1 parent", &p1);
    parent.extend(p1.output_items.clone());
    let mut fork = parent;
    fork.push(user("Fork: say two."));
    let f1 = send(&mut a, probe.body(fork, true, Some("F"))).await?;
    line("F1 fork's first request, parent's conn, key F", &f1);
    match send(
        &mut a,
        probe.delta(&p1, vec![user("Parent: say three.")], Some("P")),
    )
    .await
    {
        Ok(turn) => line("P2 parent delta from P1 (after F1)", &turn),
        Err(error) => println!("  P2 parent delta refused: {error}"),
    }
    match send(
        &mut a,
        probe.delta(&f1, vec![user("Fork: say four.")], Some("F")),
    )
    .await
    {
        Ok(turn) => line("F2 fork delta from F1 (after P2)", &turn),
        Err(error) => println!("  F2 fork delta refused: {error}"),
    }
    Ok(())
}

/// A connection writes a prefix; new connections read it at once with
/// the same key, another key or none, while it stays open; then one
/// reads it after it closed.
async fn handoff(probe: &Probe, key: &str) -> Result<(), Error> {
    let mut a = probe.connect().await?;
    let mut input = vec![user("Say: one.")];
    let first =
        send(&mut a, probe.body(input.clone(), true, Some(key))).await?;
    line("source, first request", &first);
    input.extend(first.output_items.clone());
    input.push(user("Say: two."));
    let warm = send(&mut a, probe.body(input.clone(), true, Some(key))).await?;
    line("source, second request", &warm);
    input.extend(warm.output_items.clone());
    input.push(user("Say: three."));
    let other = format!("{key}-other");
    for (label, key) in [
        ("new conn now, same key", Some(key)),
        ("new conn now, other key", Some(other.as_str())),
        ("new conn now, no key", None),
    ] {
        let mut b = probe.connect().await?;
        let turn = send(&mut b, probe.body(input.clone(), true, key)).await?;
        line(label, &turn);
        let _ = b.close(None).await;
    }
    let turn = send(&mut a, probe.body(input.clone(), true, Some(key))).await?;
    line("source again, same key", &turn);
    let _ = a.close(None).await;
    pause(2).await;
    let mut c = probe.connect().await?;
    let turn = send(&mut c, probe.body(input, true, Some(key))).await?;
    line("after the source closed, same key", &turn);
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Error> {
    let args = Args::parse();
    let store = match args.value("store") {
        Some(dir) => Store::open(dir)?,
        None => Store::open_default()?,
    };
    let chatgpt = ChatGpt::new(store);
    let account = match args.value("account") {
        Some(id) => AccountId::parse(id).ok_or("not an account id")?,
        None => chatgpt
            .active()?
            .ok_or("no active account: sign in with chatgpt_probe first")?,
    };
    let model = args.value("model").unwrap_or("gpt-5.5").to_owned();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis();
    let probe = |case: &str| Probe {
        connector: ChatGptConnector::new(chatgpt.clone(), account.clone()),
        model: model.clone(),
        nonce: format!("{case}-{stamp}"),
    };
    if args.has("handoff") {
        println!("== handoff ({model})");
        return handoff(&probe("handoff"), &format!("k-{stamp}")).await;
    }
    if args.has("shared") {
        println!("== shared connection ({model})");
        return shared(&probe("shared")).await;
    }
    if args.has("fork") {
        println!("== fork keys ({model})");
        let (p, f) = (format!("parent-{stamp}"), format!("fork-{stamp}"));
        return fork_keys(&probe("fork"), &p, &f, args.seconds("fork")?).await;
    }
    if args.has("variants") {
        let key = format!("probe-key-{stamp}");
        let key = (!args.has("no-key")).then_some(key);
        let set = if key.is_some() { "set" } else { "unset" };
        println!("== variants ({model}), key {set}");
        return variants(
            &probe("variants"),
            key.as_deref(),
            args.seconds("variants")?,
        )
        .await;
    }
    if !args.has("cases") {
        println!(
            "pick one: --cases, --variants N, --fork N, --shared, --handoff"
        );
        return Ok(());
    }
    let cases: [(&str, bool, bool); 4] = [
        ("no-key", false, true),
        ("key", true, true),
        ("no-key-tools-differ", false, false),
        ("key-tools-differ", true, false),
    ];
    for (name, keyed, all_tools) in cases {
        if args.value("only").is_some_and(|only| only != name) {
            continue;
        }
        println!("== {name} ({model})");
        let key = keyed.then(|| format!("probe-key-{stamp}"));
        if let Err(error) =
            chain_then_resend(&probe(name), key.as_deref(), all_tools).await
        {
            println!("  error: {error}");
        }
    }
    Ok(())
}
