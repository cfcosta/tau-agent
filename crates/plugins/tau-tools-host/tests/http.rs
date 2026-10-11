//! `http` and tau's secret store (ADR 0037), against a server on the
//! loopback: secrets go in as a request leaves and never come back.

#![cfg(unix)]

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, OnceLock},
};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_agent::{
    agent::Agent,
    error::ToolError,
    plugin::Plugin,
    tool::{AgentTool, ToolCtx, ToolOutput},
};
use tau_artifacts::{Bytes, Quotas};
use tau_testing::{block_on_io as block_on, scripted::ScriptedModel};
use tau_tools_host::{
    http::{self, INLINE_BODY_BYTES, Request},
    path::Root,
    plugin::CodingTools,
    secrets::{Scrub, SecretStore, valid_name},
};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio_util::sync::CancellationToken;

const TOKEN: &str = "s3cr3t-token-value";

/// What the server saw of each request: its head and body.
type Seen = Arc<Mutex<Vec<(String, String)>>>;

/// A server on the loopback that answers `/echo` with what it got,
/// `/big` with a long body holding [`TOKEN`], and `/moved` with a
/// redirect.
async fn serve() -> (String, Seen) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let seen: Seen = Arc::default();
    let kept = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let seen = kept.clone();
            tokio::spawn(async move {
                let mut bytes = Vec::new();
                let mut buffer = [0u8; 4096];
                let (head, body) = loop {
                    let read = socket.read(&mut buffer).await.unwrap();
                    bytes.extend_from_slice(&buffer[..read]);
                    let text = String::from_utf8_lossy(&bytes).into_owned();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let head = text[..end].to_owned();
                        let length = head
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|n| n.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if bytes.len() >= end + 4 + length || read == 0 {
                            break (head, text[end + 4..].to_owned());
                        }
                    }
                    if read == 0 {
                        break (text, String::new());
                    }
                };
                seen.lock().unwrap().push((head.clone(), body.clone()));
                let path = head.split(' ').nth(1).unwrap_or("/").to_owned();
                let authorization = head
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("authorization")
                            .then(|| value.trim().to_owned())
                    })
                    .unwrap_or_default();
                let (status, kind, extra, answer) = match path.as_str() {
                    "/echo" => (
                        "200 OK",
                        "application/json",
                        format!("x-echo: {authorization}\r\n"),
                        json!({ "authorization": authorization, "body": body })
                            .to_string(),
                    ),
                    "/big" => {
                        let mut text = String::from("FIRST\n");
                        while text.len() < 3 * INLINE_BODY_BYTES {
                            text.push_str(&format!("row {TOKEN}\n"));
                        }
                        text.push_str("LAST\n");
                        ("200 OK", "text/plain", String::new(), text)
                    }
                    "/moved" => (
                        "302 Found",
                        "text/plain",
                        "location: http://127.0.0.1:1/elsewhere\r\n".into(),
                        String::new(),
                    ),
                    _ => (
                        "404 Not Found",
                        "text/plain",
                        String::new(),
                        String::new(),
                    ),
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: {kind}\r\n{extra}content-length: {}\r\nconnection: close\r\n\r\n{answer}",
                    answer.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    (address, seen)
}

/// A secret store in a fresh directory, holding `github` = [`TOKEN`].
async fn store(dir: &std::path::Path) -> SecretStore {
    let store = SecretStore::in_dir(&dir.join("config"));
    store.set("github", TOKEN).await.unwrap();
    store
}

fn schema() -> &'static Value {
    static SCHEMA: OnceLock<Value> = OnceLock::new();
    SCHEMA.get_or_init(|| json!({"type": "object"}))
}

/// The coding tools with `http`, and a tool that calls `http` the way a
/// script does, reads any artifact back, and keeps what it got.
#[derive(Clone)]
struct Fixture {
    coding: CodingTools,
    captured: Arc<Mutex<Vec<Value>>>,
}

impl Plugin for Fixture {
    fn name(&self) -> &str {
        tau_tools::ui::NAME
    }

    fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        let mut tools = self.coding.tools();
        tools.push(Arc::new(Capture(self.captured.clone())));
        tools
    }
}

struct Capture(Arc<Mutex<Vec<Value>>>);

#[async_trait]
impl AgentTool for Capture {
    fn name(&self) -> &str {
        "capture_http"
    }

    fn description(&self) -> &str {
        "Calls http as a script would."
    }

    fn parameters(&self) -> &Value {
        schema()
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let mut captured = json!({});
        match ctx.call("http", args).await {
            Ok(output) => {
                captured["result"] = output.structured.unwrap();
                captured["display"] = json!(output.content);
            }
            Err(error) => captured["error"] = json!(error.to_string()),
        }
        if let Some(id) = captured["result"]["artifact"]["id"].as_str() {
            let mut offset = 0_u64;
            let mut bytes = Vec::new();
            loop {
                let page = ctx
                    .call(
                        "artifact_read",
                        json!({"id": id, "offset": offset, "limit": 65536, "encoding": "base64"}),
                    )
                    .await?
                    .structured
                    .unwrap();
                bytes.extend(
                    STANDARD.decode(page["data"].as_str().unwrap()).unwrap(),
                );
                match page["next_offset"].as_u64() {
                    Some(next) => offset = next,
                    None => break,
                }
            }
            captured["artifact_text"] =
                json!(String::from_utf8(bytes).unwrap());
        }
        self.0.lock().unwrap().push(captured);
        Ok(ToolOutput::text("captured"))
    }
}

/// Runs one `http` call as a script would, and what it got.
async fn call(
    dir: &std::path::Path,
    store: SecretStore,
    artifacts: bool,
    args: Value,
) -> Value {
    let mut coding = CodingTools::new(Root::new(dir)).with_http(store);
    if artifacts {
        coding = coding.with_artifacts(
            Bytes::new(dir.join("artifacts"), Quotas::default()).unwrap(),
        );
    }
    let fixture = Fixture {
        coding,
        captured: Arc::default(),
    };
    let model = ScriptedModel::new()
        .turn(move |turn| turn.tool_call("capture_http", args))
        .turn(|turn| turn.text("done"));
    let runs = tau_store_sqlite::memory().await.unwrap();
    Agent::new(model)
        .plugin(fixture.clone())
        .run("http", &runs)
        .await
        .unwrap();
    fixture.captured.lock().unwrap().last().unwrap().clone()
}

#[test]
fn a_secret_goes_in_as_the_request_leaves_and_never_comes_back() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let (address, seen) = serve().await;
        let got = call(
            dir.path(),
            store(dir.path()).await,
            false,
            json!({
                "method": "post",
                "url": format!("{address}/echo"),
                "headers": {
                    "authorization": {"$secret": "github", "prefix": "Bearer "},
                    "x-plain": "plain"
                },
                "body": {"token": {"$secret": "github"}, "n": 1},
            }),
        )
        .await;
        // The server got the value, in the header and in the body.
        let (head, body) = seen
            .lock()
            .unwrap()
            .last()
            .cloned()
            .unwrap_or_else(|| panic!("{got}"));
        assert!(head.contains(&format!("Bearer {TOKEN}")), "{head}");
        assert!(
            head.to_ascii_lowercase()
                .contains("content-type: application/json")
        );
        let body: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body, json!({"token": TOKEN, "n": 1}));
        // Nothing the script gets holds it.
        let result = &got["result"];
        assert_eq!(result["status"], 200);
        assert!(!got.to_string().contains(TOKEN), "{got}");
        assert_eq!(result["headers"]["x-echo"], "Bearer [secret:github]");
        assert_eq!(result["json"]["authorization"], "Bearer [secret:github]");
        assert_eq!(result["artifact"], Value::Null);
        assert_eq!(result["truncated"], false);
    });
}

#[test]
fn a_missing_secret_is_named_and_nothing_is_sent() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let (address, seen) = serve().await;
        let got = call(
            dir.path(),
            store(dir.path()).await,
            false,
            json!({
                "url": format!("{address}/echo"),
                "headers": {"authorization": {"$secret": "linear"}},
            }),
        )
        .await;
        let error = got["error"].as_str().unwrap();
        assert!(error.contains("`linear`"), "{error}");
        assert!(seen.lock().unwrap().is_empty());
    });
}

#[test]
fn a_long_body_is_an_artifact_scrubbed_like_the_rest() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let (address, _) = serve().await;
        let got = call(
            dir.path(),
            store(dir.path()).await,
            true,
            json!({"url": format!("{address}/big")}),
        )
        .await;
        let result = &got["result"];
        assert_eq!(result["body"], Value::Null);
        assert!(
            result["artifact"]["size_bytes"].as_u64().unwrap()
                > INLINE_BODY_BYTES as u64
        );
        let text = got["artifact_text"].as_str().unwrap();
        assert!(text.starts_with("FIRST\n") && text.ends_with("LAST\n"));
        assert!(text.contains("row [secret:github]"));
        assert!(!text.contains(TOKEN));
    });
}

#[test]
fn without_artifact_storage_a_long_body_is_cut() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let (address, _) = serve().await;
        let got = call(
            dir.path(),
            store(dir.path()).await,
            false,
            json!({"url": format!("{address}/big")}),
        )
        .await;
        let result = &got["result"];
        assert_eq!(result["truncated"], true);
        assert_eq!(result["body"].as_str().unwrap().len(), INLINE_BODY_BYTES);
        assert!(!got.to_string().contains(TOKEN));
    });
}

#[test]
fn a_redirect_is_answered_not_followed() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let (address, seen) = serve().await;
        let got = call(
            dir.path(),
            store(dir.path()).await,
            false,
            json!({
                "url": format!("{address}/moved"),
                "headers": {"authorization": {"$secret": "github"}},
            }),
        )
        .await;
        assert_eq!(got["result"]["status"], 302);
        assert_eq!(seen.lock().unwrap().len(), 1);
    });
}

#[test]
fn fetch_is_the_same_request_for_the_host() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let (address, _) = serve().await;
        let store = store(dir.path()).await;
        let fetched = http::fetch(
            &http::client(),
            &store,
            &json!({
                "url": format!("{address}/echo"),
                "headers": {"authorization": {"$secret": "github", "prefix": "token "}},
            }),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(fetched.status, 200);
        assert_eq!(
            fetched.json.unwrap()["authorization"],
            "token [secret:github]"
        );
        assert!(!fetched.body.contains(TOKEN));
    });
}

#[test]
fn a_request_says_what_is_wrong_with_it() {
    let wrong = |spec: Value| Request::parse(&spec).unwrap_err();
    assert!(
        wrong(json!({"url": "ftp://example.com"})).contains("http or https")
    );
    assert!(
        wrong(json!({"url": "https://example.com", "timeout": 61}))
            .contains("at most 60")
    );
    assert!(
        wrong(json!({"url": "https://example.com", "method": "TRACE"}))
            .contains("TRACE")
    );
    assert!(
        wrong(json!({"url": "https://example.com", "headers": {"a": 1}}))
            .contains("`a`")
    );
    assert!(
        wrong(json!({"url": "https://example.com", "cookies": {}}))
            .contains("cookies")
    );
    assert!(wrong(json!({})).contains("url"));
    let fine =
        Request::parse(&json!({"url": "https://example.com", "timeout": 2.5}))
            .unwrap();
    assert_eq!(fine.method, reqwest::Method::GET);
    assert_eq!(fine.timeout.as_millis(), 2500);
}

#[test]
fn the_names_a_request_uses_are_its_markers() {
    let spec = json!({
        "url": "https://example.com",
        "headers": {"a": {"$secret": "one"}, "b": {"$secret": "two", "prefix": "x"}},
        "body": {"nested": [{"$secret": "one"}, {"$secret": "three"}]},
    });
    assert_eq!(http::secret_names(&spec), ["one", "two", "three"]);
}

#[test]
fn the_store_is_owner_only_and_replaced_whole() {
    block_on(async {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let store = SecretStore::in_dir(&dir.path().join("config"));
        assert!(store.read().await.unwrap().is_empty());
        store.set("github", TOKEN).await.unwrap();
        store.set("linear", "another-value").await.unwrap();
        let mode = std::fs::metadata(store.path())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let config = std::fs::metadata(dir.path().join("config")).unwrap();
        assert_eq!(config.permissions().mode() & 0o777, 0o700);
        assert_eq!(store.names().await.unwrap(), ["github", "linear"]);
        assert!(store.remove("github").await.unwrap());
        assert!(!store.remove("github").await.unwrap());
        assert_eq!(store.names().await.unwrap(), ["linear"]);
        // Only the file is left: no temporary file beside it.
        let left: Vec<_> = std::fs::read_dir(dir.path().join("config"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left, ["secrets.json"]);
        assert!(store.set("not a name", "x").await.is_err());
    });
}

#[test]
fn secret_names_are_short_and_plain() {
    assert!(valid_name("github"));
    assert!(valid_name("ci.token_2-b"));
    assert!(!valid_name(""));
    assert!(!valid_name("with space"));
    assert!(!valid_name(&"x".repeat(65)));
}

/// Property inventory: whatever text a value of at least four bytes is
/// put into, at any places, scrubbed text holds no copy of it and keeps
/// the text around it. The oracle is the text built without the value.
/// Generator plan: draw the value from printable ASCII and the text as
/// pieces joined by it; shrinking shortens both without rejection.
#[hegel::test]
fn scrubbed_text_never_holds_a_secret(tc: TestCase) {
    let value: String = tc.draw(
        gs::text()
            .alphabet("abcxyz019-_.:[]")
            .min_size(4)
            .max_size(24),
    );
    let pieces: Vec<String> = tc.draw(
        gs::vecs(gs::text().alphabet("abc xyz-:[]").max_size(12))
            .min_size(1)
            .max_size(6),
    );
    // The mark itself must not hold the value.
    tc.assume(!"[secret:key]".contains(&value));
    // A piece that holds the value, or makes it with its neighbours,
    // would hide what the oracle expects; keep pieces without it.
    tc.assume(!pieces.iter().any(|piece| piece.contains(&value)));
    let text = pieces.join(&value);
    // Only where the value was put: no match across a piece's edge.
    tc.assume(text.replace(&value, "\u{0}") == pieces.join("\u{0}"));
    let secrets = BTreeMap::from([("key".to_owned(), value.clone())]);
    let scrubbed = Scrub::new(&secrets).text(&text);
    assert!(!scrubbed.contains(&value), "{scrubbed:?}");
    assert_eq!(scrubbed, pieces.join("[secret:key]"));
    let bytes = Scrub::new(&secrets).bytes(text.as_bytes());
    assert_eq!(bytes, pieces.join("[secret:key]").into_bytes());
}

/// Each agent's keys are a file of its own: one agent's store never
/// holds another's (ADR 0037).
#[test]
fn each_agent_s_keys_are_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config");
    let mine = SecretStore::of_agent(&config, "triage");
    let theirs = SecretStore::of_agent(&config, "digest");
    block_on(mine.set("github", "mine-value-1234")).unwrap();
    assert_eq!(mine.path(), config.join("secrets").join("triage.json"));
    assert_eq!(block_on(mine.names()).unwrap(), ["github"]);
    assert!(block_on(theirs.names()).unwrap().is_empty());
}
