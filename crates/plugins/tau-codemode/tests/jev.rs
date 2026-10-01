//! The `jev` global against fake Jevs.

mod common;

use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use common::{FakeHost, error, script, texts};
use tau_codemode::CallStatus;
use tau_jev::{
    Answer,
    Jev,
    JevError,
    Question,
    Request,
    Response,
    fake::{FakeJev, response},
};

#[tokio::test]
async fn noul_returns_a_probability_and_charges_its_usage() {
    let jev = FakeJev::nouls(|_| 0.75);
    let host = Arc::new(FakeHost::with_jev(jev.clone()));
    let outcome = script(
        &host,
        "return jev.noul({ state = { n = 1 }, question = 'Is n odd?', yes = 'odd' }).probability",
    )
    .await;
    assert_eq!(texts(&outcome), ["0.75"]);
    let request = &jev.requests()[0];
    assert_eq!(request.state, serde_json::json!({ "n": 1 }));
    assert!(matches!(
        &request.questions["answer"],
        Question::Noul { criteria: Some(c), .. } if c.yes == Some("odd".into())
    ));
    assert!(outcome.usage.input > 0);
    let row = &outcome.calls[0];
    assert_eq!(
        (row.name.as_str(), row.status),
        ("jev.noul", CallStatus::Ok)
    );
    assert_eq!(row.id, "call_1/jev/1");
    assert!(row.cost.unwrap() > 0.0);
}

fn chooser() -> FakeJev {
    FakeJev::new(|request: &Request| {
        let answers = request
            .questions
            .iter()
            .map(|(id, question)| {
                let answer = match question {
                    Question::Noul { .. } => Answer::Noul { noul: 0.5 },
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
    let host = Arc::new(FakeHost::with_jev(chooser()));
    let outcome = script(
        &host,
        "local c = jev.choice({ state = 's', question = 'kind?', options = { bug = 'a bug', feature = 'a feature' } })\n\
         local s = jev.score({ state = 's', question = 'how bad?', levels = { 'low', 'high' } })\n\
         local all = jev.ask({ state = 's', questions = {\n\
             a = { kind = 'noul', question = 'q' },\n\
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
    let jev = chooser();
    let host = Arc::new(FakeHost::with_jev(jev.clone()));
    let outcome = script(
        &host,
        "jev.score({ state = 's', question = 'q', levels = { 'only' } })",
    )
    .await;
    assert_eq!(
        error(&outcome),
        "codemode:1: jev.score: `levels` must have 2 to 10 levels; it has 1"
    );
    assert!(jev.requests().is_empty());
    assert!(outcome.calls.is_empty());
}

#[tokio::test]
async fn a_bad_answer_raises() {
    let host = Arc::new(FakeHost::with_jev(FakeJev::nouls(|_| 1.5)));
    let outcome = script(
        &host,
        "local ok, e = pcall(jev.noul, { state = 1, question = 'q' })\nreturn e",
    )
    .await;
    assert_eq!(
        texts(&outcome),
        ["Jev's answer for answer is out of range: 1.5"]
    );
    assert_eq!(outcome.calls[0].status, CallStatus::Error);
}

/// Answers after a while, counting requests in flight.
#[derive(Clone, Default)]
struct SlowJev {
    running: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
}

#[async_trait]
impl Jev for SlowJev {
    async fn ask(&self, request: &Request) -> Result<Response, JevError> {
        let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(50)).await;
        self.running.fetch_sub(1, Ordering::SeqCst);
        let answers = request
            .questions
            .keys()
            .map(|id| (id.clone(), Answer::Noul { noul: 0.1 }))
            .collect();
        Ok(response(answers, request))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn at_most_four_requests_run_at_once() {
    let jev = SlowJev::default();
    let host = Arc::new(FakeHost::with_jev(jev.clone()));
    let outcome = script(
        &host,
        "local f = function() return jev.noul({ state = 1, question = 'q' }).probability end\n\
         return parallel(f, f, f, f, f, f, f, f)",
    )
    .await;
    assert_eq!(outcome.failure, None, "{outcome:?}");
    assert_eq!(outcome.calls.len(), 8);
    assert_eq!(jev.peak.load(Ordering::SeqCst), 4);
}
