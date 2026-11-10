//! tau-constitution against the real Jev: a rule tells a call that
//! breaks it from one that keeps it. Its Luau tests and fixtures are
//! `tau-plugin test`'s (ADR 0053).

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::sync::Arc;

use serde_json::{Value, json};
use tau_jev::Jev;
use tau_luau_plugins_host::{
    runtime::{Context, Hook, Loaded, NoReach, Reach, StoreRef, load},
    store::{Fuse, Kv},
};
use tau_testing::block_on_io;
use tokio_util::sync::CancellationToken;

fn loaded() -> Loaded {
    let files = tau_luau_plugins_host::built_in::files(
        tau_luau_plugins::names::CONSTITUTION,
    )
    .unwrap();
    block_on_io(load(tau_luau_plugins::names::CONSTITUTION, files)).unwrap()
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
        job: None,
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
