//! Jev, counted: every request the plugins make this session, what it
//! cost and how long it took, for the Plugins screen.

use std::{
    sync::{Arc, Mutex},
    time::Instant,
};

use async_trait::async_trait;
use tau_jev::{Jev, JevError, Request, Response};

/// What Jev did this session.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Meter {
    pub requests: u64,
    pub input_tokens: u64,
    pub spent: f64,
    pub failed: u32,
    /// The model that answered last, such as `jev-1.13.0`.
    pub model: Option<String>,
    latencies_ms: Vec<u32>,
}

impl Meter {
    /// The median latency of the answered requests, in milliseconds.
    pub fn latency_p50_ms(&self) -> u32 {
        let mut sorted = self.latencies_ms.clone();
        sorted.sort_unstable();
        sorted.get(sorted.len() / 2).copied().unwrap_or_default()
    }
}

/// A Jev that counts what goes through it into a shared [`Meter`].
pub struct Metered {
    inner: Arc<dyn Jev>,
    meter: Arc<Mutex<Meter>>,
}

impl Metered {
    pub fn new(inner: Arc<dyn Jev>, meter: Arc<Mutex<Meter>>) -> Self {
        Self { inner, meter }
    }
}

#[async_trait]
impl Jev for Metered {
    async fn ask(&self, request: &Request) -> Result<Response, JevError> {
        let started = Instant::now();
        let answer = self.inner.ask(request).await;
        let mut meter = self.meter.lock().expect("not poisoned");
        meter.requests += 1;
        match &answer {
            Ok(response) => {
                let usage = response.usage();
                meter.input_tokens += usage.input;
                meter.spent += usage.cost.total;
                meter.model = Some(response.model.clone());
                meter.latencies_ms.push(
                    u32::try_from(started.elapsed().as_millis())
                        .unwrap_or(u32::MAX),
                );
            }
            Err(_) => meter.failed += 1,
        }
        answer
    }
}

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0027)"
)]
mod tests {
    use tau_jev::{JevError, fake::FakeJev};

    use super::*;

    #[test]
    fn requests_are_counted_answered_or_not() {
        let meter = Arc::new(Mutex::new(Meter::default()));
        let answering =
            Metered::new(Arc::new(FakeJev::nouls(|_| 0.5)), meter.clone());
        let failing = Metered::new(
            Arc::new(FakeJev::new(|_| Err(JevError::Status(503)))),
            meter.clone(),
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        runtime.block_on(async {
            let request = Request::new("state")
                .question("q", tau_jev::Question::noul("?"));
            answering.ask(&request).await.unwrap();
            answering.ask(&request).await.unwrap();
            failing.ask(&request).await.unwrap_err();
        });
        let meter = meter.lock().unwrap();
        assert_eq!((meter.requests, meter.failed), (3, 1));
        assert!(meter.input_tokens > 0 && meter.spent > 0.0);
        assert_eq!(meter.model.as_deref(), Some("jev-fake"));
    }
}
