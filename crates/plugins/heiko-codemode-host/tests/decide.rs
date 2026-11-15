//! The `decide` global against fake deciders.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use common::{FakeHost, beside, error, script, texts};
use hegel::generators as gs;
use heiko_codemode_host::{CallStatus, decide::MAX_IN_FLIGHT};
use heiko_decide::{
    Answer,
    Decider,
    DecideError,
    Question,
    Request,
    Response,
    fake::{FakeDecider, response},
};
use heiko_testing::block_on_io;
use tokio::sync::{mpsc, oneshot};

use crate::common;

#[tokio::test]
async fn predicate_returns_a_probability_and_charges_its_usage() {
    let decide = FakeDecider::predicates(|_| 0.75);
    let host = Arc::new(FakeHost::with_decide(decide.clone()));
    let outcome = script(
        &host,
        "return decide.predicate({ state = { n = 1 }, question = 'Is n odd?', yes = 'odd' }).probability",
    )
    .await;
    assert_eq!(texts(&outcome), ["0.75"]);
    let request = &decide.requests()[0];
    assert_eq!(request.state, serde_json::json!({ "n": 1 }));
    assert!(matches!(
        &request.questions["answer"],
        Question::Predicate { criteria: Some(c), .. } if c.yes == Some("odd".into())
    ));
    assert!(outcome.usage.input > 0);
    let row = &outcome.calls[0];
    assert_eq!(
        (row.name.as_str(), row.status),
        ("decide.predicate", CallStatus::Ok)
    );
    assert_eq!(row.id, "call_1/decide/1");
    assert!(row.cost.unwrap() > 0.0);
}

/// A call also gives what it cost, as its second value: what its row
/// says it cost.
#[tokio::test]
async fn a_call_gives_its_cost_too() {
    let decide = FakeDecider::predicates(|_| 0.75);
    let host = Arc::new(FakeHost::with_decide(decide));
    let outcome = script(
        &host,
        "local answer, cost = decide.predicate({ state = 1, question = 'q?' })\n\
         return cost",
    )
    .await;
    let cost: f64 = texts(&outcome)[0].parse().unwrap();
    assert!(cost > 0.0);
    assert_eq!(Some(cost), outcome.calls[0].cost);
}

fn chooser() -> FakeDecider {
    FakeDecider::new(|request: &Request| {
        let answers = request
            .questions
            .iter()
            .map(|(id, question)| {
                let answer = match question {
                    Question::Predicate { .. } => Answer::Predicate { predicate: 0.5 },
                    Question::Choice { criteria, .. } => Answer::Choice {
                        choice: criteria.keys().next().unwrap().clone(),
                        probabilities: BTreeMap::new(),
                        confidence: 0.8,
                    },
                    Question::Score { .. } => Answer::Score {
                        score: 1.0,
                        probabilities: BTreeMap::new(),
                        confidence: 0.6,
                    },
                };
                (id.clone(), answer)
            })
            .collect();
        Ok(response(answers, request))
    })
}

#[tokio::test]
async fn choice_score_and_ask() {
    let host = Arc::new(FakeHost::with_decide(chooser()));
    let outcome = script(
        &host,
        "local c = decide.choice({ state = 's', question = 'kind?', options = { bug = 'a bug', feature = 'a feature' } })\n\
         local s = decide.score({ state = 's', question = 'how bad?', levels = { 'low', 'high' } })\n\
         local all = decide.ask({ state = 's', questions = {\n\
             a = { kind = 'predicate', question = 'q' },\n\
             b = { kind = 'score', question = 'q', levels = { 'x', 'y', 'z' } },\n\
         } })\n\
         return c.choice, c.confidence, s.score, all.a.probability, all.b.confidence",
    )
    .await;
    assert_eq!(texts(&outcome), ["bug", "0.8", "1", "0.5", "0.6"]);
    assert_eq!(outcome.calls.len(), 3);
}

#[tokio::test]
async fn bad_arguments_fail_before_a_request() {
    let decide = chooser();
    let host = Arc::new(FakeHost::with_decide(decide.clone()));
    let outcome = script(
        &host,
        "decide.score({ state = 's', question = 'q', levels = { 'only' } })",
    )
    .await;
    assert_eq!(
        error(&outcome),
        "codemode:1: decide.score: `levels` must have 2 to 10 levels; it has 1"
    );
    assert!(decide.requests().is_empty());
    assert!(outcome.calls.is_empty());
}

#[tokio::test]
async fn a_bad_answer_raises() {
    let host = Arc::new(FakeHost::with_decide(FakeDecider::predicates(|_| 1.5)));
    let outcome = script(
        &host,
        "local ok, e = pcall(decide.predicate, { state = 1, question = 'q' })\nreturn e",
    )
    .await;
    assert_eq!(
        texts(&outcome),
        ["The decider's answer for answer is out of range: 1.5"]
    );
    assert_eq!(outcome.calls[0].status, CallStatus::Error);
}

/// Answers once the test lets it, counting requests in flight; each
/// request says, as it starts, how many were in flight with it.
#[derive(Clone)]
struct GatedDecider {
    running: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    started: mpsc::UnboundedSender<(usize, oneshot::Sender<()>)>,
}

#[async_trait]
impl Decider for GatedDecider {
    async fn ask(&self, request: &Request) -> Result<Response, DecideError> {
        let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        let (gate, opened) = oneshot::channel();
        self.started.send((now, gate)).expect("the test listens");
        let _ = opened.await;
        self.running.fetch_sub(1, Ordering::SeqCst);
        let answers = request
            .questions
            .keys()
            .map(|id| (id.clone(), Answer::Predicate { predicate: 0.1 }))
            .collect();
        Ok(response(answers, request))
    }
}

/// However many requests a script makes in `parallel`, and whichever
/// the test lets end first, as many run at once as the limit allows and
/// the requests left can fill: a request waits only when the limit's
/// worth are in flight. Each holds until the test lets it go, so what
/// runs beside what is set by what the test let go, not by timing.
#[hegel::test(test_cases = 100)]
fn at_most_four_requests_run_at_once(tc: hegel::TestCase) {
    let asked: usize =
        tc.draw(gs::integers::<usize>().min_value(1).max_value(12));
    let (started, mut heard) = mpsc::unbounded_channel();
    let decide = GatedDecider {
        running: Arc::default(),
        peak: Arc::default(),
        started,
    };
    let host = Arc::new(FakeHost::with_decide(decide.clone()));
    let code = format!(
        "local f = function() return decide.predicate({{ state = 1, question = 'q' }}).probability end\n\
         return parallel({})",
        vec!["f"; asked].join(", ")
    );
    let test = async {
        let mut in_flight = Vec::new();
        let mut ended = 0;
        while ended < asked {
            while in_flight.len() < MAX_IN_FLIGHT.min(asked - ended) {
                let (running, gate) = heard.recv().await.expect("a request");
                assert!(running <= MAX_IN_FLIGHT, "{running} in flight");
                in_flight.push(gate);
            }
            let at =
                tc.draw(gs::integers::<usize>().max_value(in_flight.len() - 1));
            let _ = in_flight.swap_remove(at).send(());
            ended += 1;
        }
    };
    let outcome = block_on_io(beside(script(&host, &code), test));
    assert_eq!(outcome.failure, None, "{outcome:?}");
    assert_eq!(outcome.calls.len(), asked);
    assert_eq!(texts(&outcome), vec!["0.1"; asked]);
    assert_eq!(decide.peak.load(Ordering::SeqCst), asked.min(MAX_IN_FLIGHT));
}

/// `state_json`, JSON text, stands for `state`, its keys in the order it
/// writes them, which a table cannot keep; one that is not JSON fails
/// the call.
#[tokio::test]
async fn a_state_given_as_json_keeps_its_order() {
    let decide = FakeDecider::predicates(|_| 0.5);
    let host = Arc::new(FakeHost::with_decide(decide.clone()));
    let outcome = script(
        &host,
        r#"return decide.predicate({ state_json = '{"z":1,"a":[2,{"y":3,"b":4}]}', question = 'q?' }).probability"#,
    )
    .await;
    assert_eq!(texts(&outcome), ["0.5"]);
    assert_eq!(
        serde_json::to_string(&decide.requests()[0].state).unwrap(),
        r#"{"z":1,"a":[2,{"y":3,"b":4}]}"#
    );
    let broken = script(
        &host,
        "return decide.predicate({ state_json = '{', question = 'q?' })",
    )
    .await;
    assert!(
        error(&broken).contains("`state_json` is not JSON"),
        "{}",
        error(&broken)
    );
}
