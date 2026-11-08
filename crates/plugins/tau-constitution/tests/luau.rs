//! tau-constitution's Luau loads as a plugin and passes its own tests,
//! as tau loads it; and what the demo shows of its checks and its page
//! is what the plugin leaves and draws.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use serde_json::{Value, json};
use tau_jev::{Answer, Jev, fake::FakeJev};
use tau_luau_plugins_host::{
    runtime::{
        Context,
        Files,
        Hook,
        Loaded,
        NoReach,
        PLUGIN,
        Reach,
        StoreRef,
        load,
    },
    store::{Fuse, Kv},
};
use tau_testing::block_on_io;
use tokio_util::sync::CancellationToken;

fn loaded() -> Loaded {
    let paths: Vec<(String, Vec<u8>)> = tau_constitution::FILES
        .iter()
        .map(|(path, source)| ((*path).to_owned(), source.as_bytes().to_vec()))
        .collect();
    let files = Files::from_paths(&paths, PLUGIN).unwrap();
    block_on_io(load(tau_constitution::NAME, files)).unwrap()
}

#[test]
fn the_plugin_loads_and_passes_its_tests() {
    let loaded = loaded();
    let declared = &loaded.declaration;
    assert_eq!(declared.name, tau_constitution::NAME);
    assert_eq!(declared.continues, Some(10));
    assert!(declared.uses.jev && declared.uses.store);
    assert!(declared.hooks.start && declared.hooks.repo_data);
    let tests = block_on_io(loaded.test(CancellationToken::new()));
    assert!(tests.len() > 20, "{tests:?}");
    for test in tests {
        assert!(test.passed, "{test:?}");
    }
}

/// The demo's rules, as its tau-agent repository stores them: tau's own,
/// as the mockups show them.
fn demo_rules() -> Value {
    let rule = |id: &str, text: &str, on: &[&str], review: f64, block: f64| json!({ "id": id, "text": text, "on": on, "review": review, "block": block });
    json!({
        "on_error": "allow",
        "max_holds": 3,
        "rules": [
            rule("R1", "Never delete outside target/ or rewrite published history.",
                &["exec_command.cmd"], 0.30, 0.60),
            rule("R2", "Library code returns errors. No unwrap or expect outside tests.",
                &["edit.newText", "write.content"], 0.30, 0.80),
            rule("R3", "Every sqlx query uses the checked macros.",
                &["edit.newText", "write.content"], 0.40, 0.85),
            rule("R4", "Comments explain why, not what the code does.",
                &["edit.newText"], 0.50, 0.90),
            rule("R5", "No network calls from tests except the fake server.",
                &["write.content"], 0.35, 0.80),
            rule("R6", "The final answer names the tests that ran and their result.",
                &["final answer"], 0.40, 0.75),
        ],
    })
}

/// A Jev that scores each rule with the next of its `answers` for it,
/// the last for every later one: by the rule's id, read from the
/// question.
fn jev(answers: &'static [(&'static str, &'static [f64])]) -> Arc<dyn Jev> {
    let asked: Arc<Mutex<BTreeMap<String, usize>>> = Arc::default();
    Arc::new(FakeJev::new(move |request| {
        let mut asked = asked.lock().unwrap();
        let mut scores = BTreeMap::new();
        for (id, question) in &request.questions {
            let text = serde_json::to_string(question).unwrap();
            let (rule, list) = answers
                .iter()
                .find(|(rule, _)| text.contains(&format!("Rule {rule}:")))
                .copied()
                .unwrap_or(("", &[0.0]));
            let n = asked.entry(rule.to_owned()).or_default();
            let p = list[(*n).min(list.len() - 1)];
            *n += 1;
            scores.insert(id.clone(), Answer::Noul { noul: p });
        }
        Ok(tau_jev::fake::response(scores, request))
    }))
}

struct WithJev(Arc<dyn Jev>);

impl Reach for WithJev {
    fn jev(&self) -> Option<Arc<dyn Jev>> {
        Some(self.0.clone())
    }
}

/// A store in memory holding `values`.
fn store(values: &[(&str, Value)]) -> StoreRef {
    block_on_io(async {
        let kv = Arc::new(Kv::memory(Fuse::never()).await.unwrap());
        let mut session = kv.session().await;
        for (key, value) in values {
            session.put(key, value.clone());
        }
        session.commit().await.unwrap();
        StoreRef(kv)
    })
}

/// What `hook` does with `input`, from `state`, in the demo's run on
/// `store`, with `jev` or none: the state it left and its answer.
fn call(
    loaded: &Loaded,
    hook: Hook,
    input: Value,
    state: &Value,
    store: &StoreRef,
    jev: Option<&Arc<dyn Jev>>,
) -> (Value, Value) {
    let context = Context {
        run: json!({ "id": "demo", "kind": "main", "repo": "tau-agent", "jev": jev.is_some() }),
        now: json!({}),
        settings: json!({}),
        state: state.clone(),
        secrets: None,
        store: Some(store.clone()),
    };
    let reach: Arc<dyn Reach> = match jev {
        Some(jev) => Arc::new(WithJev(jev.clone())),
        None => Arc::new(NoReach),
    };
    let outcome = block_on_io(loaded.call(
        &hook,
        input,
        &context,
        reach,
        CancellationToken::new(),
    ));
    assert_eq!(outcome.error, None, "{hook:?}");
    (outcome.state, outcome.value)
}

/// What the plugin's `view` draws from `state`.
fn view_of(loaded: &Loaded, state: &Value, store: &StoreRef) -> Value {
    call(loaded, Hook::View, Value::Null, state, store, None).1
}

/// The demo's run: it starts, an edit breaks R2, an edit passes, a
/// command is doubtful under R1, the answer is sent back for R6, and the
/// next one names its tests. The state and view after each check, and
/// the store it leaves.
fn demo_run(loaded: &Loaded) -> (Vec<Value>, StoreRef) {
    let jev = jev(&[
        ("R1", &[0.41]),
        ("R2", &[0.95, 0.02]),
        ("R3", &[0.04, 0.03]),
        ("R4", &[0.08, 0.06]),
        ("R6", &[0.91, 0.04]),
    ]);
    let store = store(&[("constitution", demo_rules())]);
    let input = json!({ "input": "Make the HTTP client honor retry-after on 429s and 503s" });
    let (mut state, _) =
        call(loaded, Hook::Start, input, &json!({}), &store, Some(&jev));
    let mut steps = Vec::new();
    let tool = |id: &str, name: &str, args: Value| json!({ "id": id, "name": name, "args": args });
    let edit = |text: &str| {
        json!({ "path": "crates/tau-ai/src/retry.rs",
                "edits": [{ "oldText": "", "newText": text }] })
    };
    let hooks = [
        (
            Hook::BeforeTool,
            tool(
                "c4",
                "edit",
                edit(
                    "let secs: u64 = header.to_str().unwrap().parse().unwrap();",
                ),
            ),
        ),
        (
            Hook::BeforeTool,
            tool(
                "c5",
                "edit",
                edit(
                    "fn server_hint(headers: &HeaderMap) -> Option<Duration> {",
                ),
            ),
        ),
        (
            Hook::BeforeTool,
            tool(
                "c6",
                "exec_command",
                json!({ "cmd": "rm -rf target/debug/incremental" }),
            ),
        ),
        (
            Hook::BeforeStop,
            json!({ "text": "Done. retry-after now overrides the backoff.",
                    "turn": 8, "reason": "stop" }),
        ),
        (
            Hook::BeforeStop,
            json!({ "text": "Tests: cargo nextest run -p tau-ai retry::, 14 passed.",
                    "turn": 9, "reason": "stop" }),
        ),
    ];
    for (hook, input) in hooks {
        state = call(loaded, hook, input, &state, &store, Some(&jev)).0;
        steps.push(
            json!({ "state": state, "view": view_of(loaded, &state, &store) }),
        );
    }
    let end = json!({ "stop": "Stop", "turns": 9, "cost": 0, "text": "" });
    call(loaded, Hook::RunEnd, end, &state, &store, Some(&jev));
    (steps, store)
}

/// What the page `rules` of tau-agent draws from `store`, a Jev key
/// saved or not.
fn page_of(loaded: &Loaded, store: &StoreRef, jev: bool) -> Value {
    let jev = jev.then(|| self::jev(&[("new", &[0.82])]));
    let params = json!({ "repo": "tau-agent" });
    call(
        loaded,
        Hook::Page("rules".into()),
        params,
        &json!({}),
        store,
        jev.as_ref(),
    )
    .1
}

/// Carries out the page's `action` with `args` on `store`, with a Jev.
fn act(loaded: &Loaded, store: &StoreRef, action: &str, args: Value) {
    let jev = jev(&[("new", &[0.82])]);
    call(
        loaded,
        Hook::Action(action.into()),
        args,
        &json!({}),
        store,
        Some(&jev),
    );
}

/// The rule the demo's editor writes.
const RULE: &str = "Never run migrations against the production database.";

/// The page's states the demo shows, by its screens' names.
fn demo_pages(loaded: &Loaded) -> Value {
    let (_, store) = demo_run(loaded);
    let mut pages = serde_json::Map::new();
    pages.insert("constitution".into(), page_of(loaded, &store, true));
    // A rule being written, on the shell command, and tried on the run's.
    act(loaded, &store, "open", json!({}));
    act(loaded, &store, "write", json!({ "text": RULE }));
    act(
        loaded,
        &store,
        "place",
        json!({ "place": "exec_command.cmd" }),
    );
    act(loaded, &store, "try", json!({}));
    pages.insert("rule-editor".into(), page_of(loaded, &store, true));
    // One saved with nowhere to apply.
    act(loaded, &store, "open", json!({}));
    act(loaded, &store, "write", json!({ "text": RULE }));
    act(loaded, &store, "save", json!({}));
    pages.insert("rule-missing".into(), page_of(loaded, &store, true));
    act(loaded, &store, "close", json!({}));
    // Rules that cannot be read: R4 flags past where it blocks.
    let mut broken = demo_rules();
    broken["rules"][3]["review"] = json!(0.95);
    let unread = self::store(&[("constitution", broken)]);
    pages.insert("rules-broken".into(), page_of(loaded, &unread, true));
    // None, once removed, and no key to try them with.
    act(loaded, &store, "reset", json!({}));
    pages.insert("rules-empty".into(), page_of(loaded, &store, false));
    Value::Object(pages)
}

/// Writes `drawn` as the fixture `name` with `BLESS_FIXTURES=1`, else
/// checks it is `fixture`.
fn bless_or_check(name: &str, fixture: &str, drawn: &Value) {
    if std::env::var_os("BLESS_FIXTURES").is_some() {
        let path = format!(
            "{}/../../../plugins/tau-constitution/fixtures/{name}.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let text = serde_json::to_string_pretty(drawn).unwrap() + "\n";
        std::fs::write(path, text).unwrap();
        return;
    }
    let fixture: Value = serde_json::from_str(fixture).unwrap();
    assert_eq!(&fixture, drawn, "{name}: run with BLESS_FIXTURES=1");
}

/// The demo's run is what the plugin leaves and draws at each check, and
/// its page what the plugin draws from the store the run left.
/// `BLESS_FIXTURES=1` writes them anew from the plugin.
#[test]
fn the_demo_is_what_the_plugin_leaves_and_draws() {
    let loaded = loaded();
    let (steps, _) = demo_run(&loaded);
    bless_or_check("run", tau_constitution::DEMO_RUN, &Value::Array(steps));
    bless_or_check("pages", tau_constitution::DEMO_PAGES, &demo_pages(&loaded));
}

/// The demo's run checks what its script says: the first edit refused
/// for R2, the command flagged for R1, the answer sent back once for R6;
/// and the store keeps the flag for a person and the run's counts.
#[test]
fn the_demo_run_checks_as_its_script_says() {
    let loaded = loaded();
    let (steps, store) = demo_run(&loaded);
    let last = &steps[4]["state"];
    assert_eq!(last["stats"]["blocked"], json!(["R2"]));
    assert_eq!(last["stats"]["flagged"], json!(["R1"]));
    assert_eq!(last["stats"]["held"], json!(["R6"]));
    assert_eq!(last["stats"]["calls"], json!(3));
    assert_eq!(last["calls"]["c4"]["verdict"]["kind"], "blocked");
    let view = &steps[4]["view"];
    assert_eq!(view["calls"]["c4"]["mark"], "blocked");
    assert_eq!(view["calls"]["c6"]["mark"], "flagged");
    assert_eq!(view["status"]["text"], "1 blocked · 1 flagged · 1 held");
    let kept = block_on_io(store.0.everything()).unwrap();
    let waiting: Vec<&Value> = kept
        .iter()
        .filter(|(key, _)| key.starts_with("review/"))
        .map(|(_, item)| item)
        .collect();
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0]["rule"], "R1");
    assert_eq!(kept["run/demo"]["stats"]["answers"], json!(2));
}

/// What the demo lists of the plugin is what it declares.
/// `BLESS_FIXTURES=1` writes it anew from the plugin.
#[test]
fn the_demo_lists_what_the_plugin_declares() {
    let declared = serde_json::to_value(&loaded().declaration).unwrap();
    bless_or_check(
        "declaration",
        tau_constitution::DEMO_DECLARATION,
        &declared,
    );
}

/// Asks the real Jev about a call that breaks a rule and one that does
/// not. Needs `TYPESAFE_API_KEY` and the network:
/// `cargo nextest run --release -p tau-constitution --run-ignored only`.
#[test]
#[ignore = "needs TYPESAFE_API_KEY and the network"]
fn the_real_jev_tells_a_broken_rule_from_a_kept_one() {
    let loaded = loaded();
    let jev: Arc<dyn Jev> =
        Arc::new(tau_jev::TypeSafe::from_env().expect("TYPESAFE_API_KEY"));
    let rules = json!({ "on_error": "allow", "max_holds": 3, "rules": [{
        "id": "R1",
        "text": "Library code never calls unwrap() or expect(); it returns errors with ?.",
        "on": ["write.content"], "review": 0.5, "block": 0.8,
    }] });
    let store = store(&[("constitution", rules)]);
    let write = |content: &str| {
        json!({ "id": "c1", "name": "write",
                "args": { "path": "src/lib.rs", "content": content } })
    };
    let check = |content: &str| {
        let (state, decision) = call(
            &loaded,
            Hook::BeforeTool,
            write(content),
            &json!({}),
            &store,
            Some(&jev),
        );
        println!("{content}: {}", state["calls"]["c1"]);
        decision
    };
    let broken = check("pub fn port(s: &str) -> u16 { s.parse().unwrap() }");
    assert_eq!(broken["decision"], "block");
    let kept = check(
        "pub fn port(s: &str) -> Result<u16, ParseIntError> { s.parse() }",
    );
    assert_eq!(kept, Value::Null);
}
