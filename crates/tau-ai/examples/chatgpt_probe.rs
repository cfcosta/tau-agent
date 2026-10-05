//! A live probe of Sign in with ChatGPT and plan usage on
//! `api.openai.com` (`tau_ai::chatgpt`), run by hand to learn what the
//! route accepts before tau's transport moves to it.
//!
//! ```sh
//! cargo run -p tau-ai --example chatgpt_probe -- sign-in [--new] [--consent] [--port N]
//! cargo run -p tau-ai --example chatgpt_probe -- accounts
//! cargo run -p tau-ai --example chatgpt_probe -- models
//! cargo run -p tau-ai --example chatgpt_probe -- http        [--model M]
//! cargo run -p tau-ai --example chatgpt_probe -- http-tools  [--model M]
//! cargo run -p tau-ai --example chatgpt_probe -- ws          [--model M]
//! cargo run -p tau-ai --example chatgpt_probe -- ws-tools    [--model M]
//! cargo run -p tau-ai --example chatgpt_probe -- refresh
//! cargo run -p tau-ai --example chatgpt_probe -- sign-out
//! ```
//!
//! Every command takes `--account ID` (default: the active account) and
//! `--store DIR` (default: `$XDG_CONFIG_HOME/tau/chatgpt`). Without
//! `--model`, the first listed model is used. It prints HTTP statuses,
//! request ids, error bodies and stream events verbatim, and never a
//! token.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{collections::HashMap, time::Duration};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tau_ai::{
    chatgpt::{
        AccountId,
        AccountStatus,
        ApiError,
        ChatGpt,
        ChatGptConnector,
        Loopback,
        Revocation,
        Store,
    },
    http::{self, Request, Tls},
    time::rfc3339,
    ws::io::connection::Connector,
};
use tokio_tungstenite::tungstenite::{self, Message};

type Error = Box<dyn std::error::Error>;

const PROMPT: &str = "Say exactly: Hello, world!";
const FOLLOW_UP: &str = "Now say exactly: Goodbye, world!";
const TOOL_PROMPT: &str =
    "What is the weather in Paris? Use the get_weather tool.";

struct Args {
    command: String,
    flags: HashMap<String, Option<String>>,
}

impl Args {
    fn parse() -> Self {
        let mut args = std::env::args().skip(1);
        let command = args.next().unwrap_or_else(|| "help".into());
        let mut flags = HashMap::new();
        let rest: Vec<String> = args.collect();
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
        Self { command, flags }
    }

    fn value(&self, name: &str) -> Option<&str> {
        self.flags.get(name).and_then(|value| value.as_deref())
    }

    fn has(&self, name: &str) -> bool {
        self.flags.contains_key(name)
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Error> {
    let args = Args::parse();
    let store = match args.value("store") {
        Some(dir) => Store::open(dir)?,
        None => Store::open_default()?,
    };
    println!("store: {}", store.dir().display());
    let chatgpt = ChatGpt::new(store);
    match args.command.as_str() {
        "sign-in" => sign_in(&chatgpt, &args).await,
        "accounts" => accounts(&chatgpt),
        "models" => models(&chatgpt, &account(&chatgpt, &args)?).await,
        "http" => http_probe(&chatgpt, &args, false).await,
        "http-tools" => http_probe(&chatgpt, &args, true).await,
        "ws" => ws_probe(&chatgpt, &args, false).await,
        "ws-tools" => ws_probe(&chatgpt, &args, true).await,
        "refresh" => refresh(&chatgpt, &account(&chatgpt, &args)?).await,
        "sign-out" => sign_out(&chatgpt, &account(&chatgpt, &args)?).await,
        _ => {
            println!(
                "commands: sign-in, accounts, models, http, http-tools, ws, \
                 ws-tools, refresh, sign-out"
            );
            Ok(())
        }
    }
}

fn account(chatgpt: &ChatGpt, args: &Args) -> Result<AccountId, Error> {
    if let Some(id) = args.value("account") {
        return AccountId::parse(id).ok_or_else(|| "not an account id".into());
    }
    chatgpt
        .active()?
        .ok_or_else(|| "no active account: run sign-in first".into())
}

async fn sign_in(chatgpt: &ChatGpt, args: &Args) -> Result<(), Error> {
    let returning = if args.has("new") {
        None
    } else if let Some(id) = args.value("account") {
        Some(AccountId::parse(id).ok_or("not an account id")?)
    } else {
        chatgpt.active()?
    };
    // The listener starts before the browser opens.
    let loopback = match args.value("port") {
        Some(port) => Loopback::bind_port(port.parse()?).await?,
        None => Loopback::bind().await?,
    };
    let attempt = chatgpt.start_sign_in(
        returning.as_ref(),
        loopback.redirect_uri(),
        args.has("consent"),
    )?;
    match &returning {
        Some(id) => println!("signing in again as {id}"),
        None => println!("registering a new sign-in"),
    }
    println!("host id: {}", chatgpt.store().host_id()?);
    println!("callback: {}", loopback.redirect_uri());
    println!(
        "\nOpen this page to sign in (it may carry an id_token_hint: do not \
         share it):\n\n{}\n",
        attempt.url()
    );
    open_browser(attempt.url());
    println!(
        "Waiting for the browser. Or paste the redirect URL here and press \
         Enter."
    );
    let (paste_tx, paste_rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        // Blank lines are skipped; at end of input (no terminal) only the
        // browser can finish.
        let mut line = String::new();
        while matches!(std::io::stdin().read_line(&mut line), Ok(n) if n > 0) {
            if !line.trim().is_empty() {
                let _ = paste_tx.send(line);
                return;
            }
            line.clear();
        }
    });
    let callback = tokio::select! {
        callback = loopback.wait(&attempt) => callback,
        Ok(pasted) = paste_rx => attempt.callback(&pasted),
    };
    let signed_in = match callback {
        Ok(callback) => chatgpt.finish_sign_in(&attempt, &callback).await,
        Err(error) => Err(error),
    };
    match signed_in {
        Ok(signed_in) => {
            println!("signed in: {}", signed_in.account);
            println!("  label: {}", signed_in.label);
            println!("  email: {:?}", signed_in.email);
            println!("  plan usage: {:?}", signed_in.plan_usage);
            let saved = chatgpt.store().load(&signed_in.account)?;
            println!("  client id: {}", saved.client_id);
            println!("  scopes: {}", saved.scopes.join(" "));
            Ok(())
        }
        Err(error) => {
            println!("sign-in failed: {error}");
            println!("  recovery: {:?}", error.recovery());
            Err(error.into())
        }
    }
}

fn open_browser(url: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

fn accounts(chatgpt: &ChatGpt) -> Result<(), Error> {
    let active = chatgpt.active()?;
    println!("host id: {}", chatgpt.store().host_id()?);
    for saved in chatgpt.store().accounts()? {
        let id = saved.id();
        let mark = if Some(&id) == active.as_ref() {
            "*"
        } else {
            " "
        };
        let status = match saved.status() {
            AccountStatus::SignedIn(usage) => {
                format!("signed in, plan usage {usage:?}")
            }
            AccountStatus::SignedOut => "signed out".into(),
        };
        println!("{mark} {id}  {}  ({status})", saved.label);
        println!("    client id: {}", saved.client_id);
        if let Some(at) = saved.expires_at {
            println!("    access token expires: {}", rfc3339(at));
        }
        println!("    scopes: {}", saved.scopes.join(" "));
    }
    Ok(())
}

async fn models(chatgpt: &ChatGpt, account: &AccountId) -> Result<(), Error> {
    match chatgpt.models(account).await {
        Ok(models) => {
            for model in models {
                println!("{}\t{}", model.slug, model.display_name);
            }
            Ok(())
        }
        Err(error) => report(error),
    }
}

fn report(error: tau_ai::chatgpt::ChatGptError) -> Result<(), Error> {
    if let tau_ai::chatgpt::ChatGptError::Api(api) = &error {
        print_api_error(api);
    }
    println!("error: {error}");
    println!("recovery: {:?}", error.recovery());
    Err(error.into())
}

fn print_api_error(api: &ApiError) {
    println!("HTTP {}", api.status);
    println!("request id: {}", api.request_id.as_deref().unwrap_or("-"));
    println!("body: {}", api.body);
    println!("recovery: {:?}", api.recovery());
}

async fn model(
    chatgpt: &ChatGpt,
    account: &AccountId,
    args: &Args,
) -> Result<String, Error> {
    if let Some(model) = args.value("model") {
        return Ok(model.to_owned());
    }
    let models = chatgpt.models(account).await?;
    let first = models.first().ok_or("the account lists no models")?;
    println!("model: {} ({})", first.slug, first.display_name);
    Ok(first.slug.clone())
}

fn user(text: &str) -> Value {
    json!({"role": "user", "content": text})
}

fn weather_tool() -> Value {
    json!({
        "type": "function",
        "name": "get_weather",
        "description": "The current weather in a city.",
        "parameters": {
            "type": "object",
            "properties": {"city": {"type": "string"}},
            "required": ["city"],
            "additionalProperties": false,
        },
    })
}

/// The tool variants: plain top-level, in a namespace, and supplied
/// through an `additional_tools` input item.
fn tool_variants(model: &str) -> Vec<(&'static str, Value)> {
    let base = |tools: Value, input: Value| {
        let mut body = json!({"model": model, "store": false, "input": input});
        if tools.as_array().is_some_and(|tools| !tools.is_empty()) {
            body["tools"] = tools;
        }
        body
    };
    vec![
        (
            "top-level function tool",
            base(json!([weather_tool()]), json!([user(TOOL_PROMPT)])),
        ),
        (
            "function tool in a namespace",
            base(
                json!([{
                    "type": "namespace",
                    "name": "weather",
                    "description": "Weather lookups.",
                    "tools": [weather_tool()],
                }]),
                json!([user(TOOL_PROMPT)]),
            ),
        ),
        (
            "function tool in additional_tools",
            base(
                json!([]),
                json!([
                    {
                        "type": "additional_tools",
                        "role": "developer",
                        "tools": [weather_tool()],
                    },
                    user(TOOL_PROMPT),
                ]),
            ),
        ),
    ]
}

fn is_terminal(kind: &str) -> bool {
    matches!(
        kind,
        "response.completed"
            | "response.failed"
            | "response.incomplete"
            | "error"
    )
}

/// What a stream ended with, for the summary line.
#[derive(Debug, Default)]
struct Outcome {
    terminal: Option<String>,
    response_id: Option<String>,
    error: Option<Value>,
    function_calls: Vec<Value>,
}

impl Outcome {
    fn see(&mut self, event: &Value) {
        let kind = event["type"].as_str().unwrap_or_default();
        if kind == "response.output_item.done"
            && event["item"]["type"] == "function_call"
        {
            self.function_calls.push(event["item"].clone());
        }
        if is_terminal(kind) {
            self.terminal = Some(kind.to_owned());
            self.response_id =
                event["response"]["id"].as_str().map(str::to_owned);
            let error = if kind == "error" {
                event.get("error").cloned().or_else(|| Some(event.clone()))
            } else {
                event["response"]
                    .get("error")
                    .filter(|e| !e.is_null())
                    .cloned()
            };
            self.error = error;
        }
    }

    fn summary(&self) -> String {
        let mut text = format!(
            "ended with {}",
            self.terminal.as_deref().unwrap_or("no terminal event")
        );
        if let Some(error) = &self.error {
            text.push_str(&format!("; error {error}"));
        }
        for call in &self.function_calls {
            text.push_str(&format!(
                "; function_call {} namespace={} arguments={}",
                call["name"], call["namespace"], call["arguments"]
            ));
        }
        text
    }
}

async fn http_probe(
    chatgpt: &ChatGpt,
    args: &Args,
    tools: bool,
) -> Result<(), Error> {
    let account = account(chatgpt, args)?;
    let model = model(chatgpt, &account, args).await?;
    let requests = if tools {
        tool_variants(&model)
    } else {
        vec![(
            "plain text",
            json!({"model": model, "store": false, "input": [user(PROMPT)]}),
        )]
    };
    let mut summaries = Vec::new();
    for (name, mut body) in requests {
        body["stream"] = true.into();
        println!("\n=== HTTP: {name}");
        println!("request: {body}");
        let outcome = stream_http(chatgpt, &account, &body).await?;
        summaries.push(format!("{name}: {}", outcome.summary()));
    }
    println!("\n=== summary");
    for line in summaries {
        println!("{line}");
    }
    Ok(())
}

async fn stream_http(
    chatgpt: &ChatGpt,
    account: &AccountId,
    body: &Value,
) -> Result<Outcome, Error> {
    let token = chatgpt.inference_token(account).await?;
    let url = chatgpt.config().api_base.join("responses")?;
    let request = Request::post_json(url, body)
        .header("Accept", "text/event-stream")
        .bearer(&token);
    let mut response = http::open(&Tls, &request).await?;
    println!("HTTP {}", response.status);
    println!("request id: {}", response.request_id().unwrap_or("-"));
    println!(
        "content-type: {}",
        response.header("content-type").unwrap_or("-")
    );
    let mut outcome = Outcome::default();
    if !response.is_success() {
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            body.extend_from_slice(&chunk);
        }
        let error = ApiError::new(
            response.status,
            response.request_id().map(str::to_owned),
            &body,
        );
        print_api_error(&error);
        outcome.terminal = Some(format!("HTTP {}", response.status));
        outcome.error = Some(
            serde_json::from_slice(&body).unwrap_or(Value::String(error.body)),
        );
        return Ok(outcome);
    }
    let mut buffer = String::new();
    while let Some(chunk) = response.chunk().await? {
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(end) = buffer.find("\n\n") {
            let event: String = buffer.drain(..end + 2).collect();
            for line in event.lines() {
                println!("{line}");
                if let Some(data) = line.strip_prefix("data:")
                    && let Ok(value) =
                        serde_json::from_str::<Value>(data.trim())
                {
                    outcome.see(&value);
                }
            }
            if outcome.terminal.is_some() {
                return Ok(outcome);
            }
        }
    }
    if !buffer.trim().is_empty() {
        println!("{buffer}");
    }
    println!("(the stream ended without a terminal event)");
    Ok(outcome)
}

async fn ws_probe(
    chatgpt: &ChatGpt,
    args: &Args,
    tools: bool,
) -> Result<(), Error> {
    let account = account(chatgpt, args)?;
    let model = model(chatgpt, &account, args).await?;
    let connector = ChatGptConnector::new(chatgpt.clone(), account);
    println!(
        "\n=== WebSocket: connecting to {}",
        chatgpt.config().websocket_url
    );
    let stream = connector.connect().await?;
    let (mut socket, handshake) = match tokio_tungstenite::client_async(
        connector.request(),
        stream,
    )
    .await
    {
        Ok(done) => done,
        Err(tungstenite::Error::Http(response)) => {
            println!("upgrade refused: HTTP {}", response.status());
            let id = response
                .headers()
                .get("x-request-id")
                .and_then(|v| v.to_str().ok());
            println!("request id: {}", id.unwrap_or("-"));
            let body = response.body().as_deref().unwrap_or_default();
            let error = ApiError::new(
                response.status().as_u16(),
                id.map(str::to_owned),
                body,
            );
            print_api_error(&error);
            return Err("the WebSocket upgrade was refused".into());
        }
        Err(error) => return Err(error.into()),
    };
    println!("upgrade: HTTP {}", handshake.status());
    let id = handshake
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok());
    println!("request id: {}", id.unwrap_or("-"));

    let mut summaries = Vec::new();
    if tools {
        for (name, body) in tool_variants(&model) {
            println!("\n=== WebSocket: {name}");
            let outcome = ws_turn(&mut socket, body).await?;
            summaries.push(format!("{name}: {}", outcome.summary()));
        }
    } else {
        println!("\n=== WebSocket: first request");
        let body =
            json!({"model": model, "store": false, "input": [user(PROMPT)]});
        let first = ws_turn(&mut socket, body).await?;
        summaries.push(format!("first: {}", first.summary()));
        match &first.response_id {
            Some(previous) => {
                println!(
                    "\n=== WebSocket: continuing from {previous} on the same connection"
                );
                let body = json!({
                    "model": model,
                    "store": false,
                    "previous_response_id": previous,
                    "input": [user(FOLLOW_UP)],
                });
                let second = ws_turn(&mut socket, body).await?;
                summaries.push(format!("continuation: {}", second.summary()));
            }
            None => {
                summaries.push("continuation: skipped, no response id".into())
            }
        }
    }
    let _ = socket.close(None).await;
    println!("\n=== summary");
    for line in summaries {
        println!("{line}");
    }
    Ok(())
}

async fn ws_turn<S>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    mut body: Value,
) -> Result<Outcome, Error>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    body["type"] = "response.create".into();
    println!("send: {body}");
    socket.send(Message::text(body.to_string())).await?;
    let mut outcome = Outcome::default();
    loop {
        let next =
            tokio::time::timeout(Duration::from_secs(120), socket.next()).await;
        let message = match next {
            Err(_) => {
                println!("(no frame for 120 s)");
                return Ok(outcome);
            }
            Ok(None) => {
                println!("(the connection closed)");
                return Ok(outcome);
            }
            Ok(Some(message)) => message?,
        };
        match message {
            Message::Text(text) => {
                println!("{text}");
                if let Ok(value) = serde_json::from_str::<Value>(&text) {
                    outcome.see(&value);
                }
                if outcome.terminal.is_some() {
                    return Ok(outcome);
                }
            }
            Message::Close(frame) => {
                println!("(closed by the server: {frame:?})");
                return Ok(outcome);
            }
            _ => {}
        }
    }
}

async fn refresh(chatgpt: &ChatGpt, account: &AccountId) -> Result<(), Error> {
    let before = chatgpt.store().load(account)?;
    match chatgpt.refresh(account).await {
        Ok(after) => {
            println!("refreshed {account}");
            println!(
                "  access token expires: {}",
                after.expires_at.map(rfc3339).unwrap_or_default()
            );
            println!(
                "  refresh token rotated: {}",
                after.refresh_token != before.refresh_token
            );
            println!("  scopes: {}", after.scopes.join(" "));
            println!("  earliest_refresh_at: {:?}", after.earliest_refresh_at);
            Ok(())
        }
        Err(error) => report(error),
    }
}

async fn sign_out(chatgpt: &ChatGpt, account: &AccountId) -> Result<(), Error> {
    match chatgpt.sign_out(account).await? {
        Revocation::Confirmed => {
            println!("signed out; OpenAI confirmed the revocation")
        }
        Revocation::NothingToRevoke => {
            println!("signed out; there was no refresh token")
        }
        Revocation::Unconfirmed { reason } => println!(
            "signed out locally, but OpenAI did not confirm the revocation \
             ({reason}); disconnect tau in ChatGPT Settings to be sure"
        ),
    }
    Ok(())
}
