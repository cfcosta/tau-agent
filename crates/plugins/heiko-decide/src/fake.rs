//! A [`Decider`] that answers from a function, for tests.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;

use crate::{Answer, Decider, DecideError, DecideUsage, Request, Response};

type Answerer =
    dyn Fn(&Request) -> Result<Response, DecideError> + Send + Sync + 'static;

/// Answers every request with a function, and records the requests.
/// Clones share the record.
#[derive(Clone)]
pub struct FakeDecider {
    answer: Arc<Answerer>,
    requests: Arc<Mutex<Vec<Request>>>,
}

impl FakeDecider {
    pub fn new(
        answer: impl Fn(&Request) -> Result<Response, DecideError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            answer: Arc::new(answer),
            requests: Arc::default(),
        }
    }

    /// Answers every Predicate question with `predicate(id)`, and any other
    /// question with an error.
    pub fn predicates(predicate: impl Fn(&str) -> f64 + Send + Sync + 'static) -> Self {
        Self::new(move |request| {
            let answers = request
                .questions
                .keys()
                .map(|id| (id.clone(), Answer::Predicate { predicate: predicate(id) }))
                .collect();
            Ok(response(answers, request))
        })
    }

    /// Every request asked so far, in order.
    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().expect("not poisoned").clone()
    }
}

/// A response with `answers`, billed one input token per byte of the
/// request's JSON, so a bigger request costs more.
pub fn response(
    answers: BTreeMap<String, Answer>,
    request: &Request,
) -> Response {
    let input_tokens = serde_json::to_string(request)
        .expect("requests serialize")
        .len() as u64;
    Response {
        model: "decide-fake".to_owned(),
        answers,
        usage: DecideUsage {
            input_tokens,
            output_tokens: 0,
        },
    }
}

#[async_trait]
impl Decider for FakeDecider {
    async fn ask(&self, request: &Request) -> Result<Response, DecideError> {
        self.requests
            .lock()
            .expect("not poisoned")
            .push(request.clone());
        (self.answer)(request)
    }
}
