//! Shared admission and accounting for inference attempts in one agent run.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use tau_ai::message::Usage;
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore},
    time::{Instant, sleep_until},
};
use tokio_util::sync::CancellationToken;

/// Limits shared by every inference attempt in an agent run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    pub max_calls: usize,
    pub max_concurrency: usize,
    pub timeout: Duration,
    pub max_tokens: Option<u64>,
    pub max_cost_usd: Option<f64>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_calls: 16,
            max_concurrency: 4,
            timeout: Duration::from_secs(60),
            max_tokens: Some(100_000),
            max_cost_usd: Some(1.0),
        }
    }
}

/// A point-in-time view of admitted attempts and reported usage.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub calls: usize,
    pub in_flight: usize,
    pub usage: Usage,
}

#[derive(Debug, Default)]
struct State {
    calls: usize,
    in_flight: usize,
    usage: Usage,
}

/// One budget shared across scripts and retries in an agent run.
#[derive(Debug)]
pub struct Budget {
    limits: Limits,
    deadline: Instant,
    slots: Arc<Semaphore>,
    state: Mutex<State>,
}

impl Budget {
    /// Creates a budget. A zero call limit is valid and disables admissions.
    pub fn new(limits: Limits) -> Result<Arc<Self>, String> {
        if limits.max_concurrency == 0 {
            return Err("max_concurrency must be positive".into());
        }
        if limits.max_concurrency > Semaphore::MAX_PERMITS {
            return Err("max_concurrency exceeds semaphore capacity".into());
        }
        if limits
            .max_cost_usd
            .is_some_and(|cost| !cost.is_finite() || cost <= 0.0)
        {
            return Err("max_cost_usd must be finite and positive".into());
        }
        let deadline = Instant::now()
            .checked_add(limits.timeout)
            .ok_or("timeout cannot be represented as a deadline")?;
        Ok(Arc::new(Self {
            limits,
            deadline,
            slots: Arc::new(Semaphore::new(limits.max_concurrency)),
            state: Mutex::new(State::default()),
        }))
    }

    /// Waits for a slot, then atomically checks limits and counts one attempt.
    pub async fn admit(
        self: &Arc<Self>,
        cancel: &CancellationToken,
    ) -> Result<Permit, String> {
        let slot = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err("inference cancelled".into()),
            _ = sleep_until(self.deadline) => return Err("inference deadline reached".into()),
            slot = self.slots.clone().acquire_owned() =>
                slot.map_err(|_| "inference semaphore closed")?,
        };
        let mut state = self.state.lock().unwrap();
        // Recheck after acquiring: a waiter must not consume a call when a
        // preceding in-flight attempt exhausts usage or the deadline passes.
        if cancel.is_cancelled() {
            return Err("inference cancelled".into());
        }
        if Instant::now() >= self.deadline {
            return Err("inference deadline reached".into());
        }
        if state.calls >= self.limits.max_calls {
            return Err("inference call limit reached".into());
        }
        let tokens = state
            .usage
            .input
            .saturating_add(state.usage.output)
            .saturating_add(state.usage.cache_read)
            .saturating_add(state.usage.cache_write);
        if self.limits.max_tokens.is_some_and(|max| tokens >= max) {
            return Err("inference token limit reached".into());
        }
        if self
            .limits
            .max_cost_usd
            .is_some_and(|max| state.usage.cost.total >= max)
        {
            return Err("inference cost limit reached".into());
        }
        state.calls += 1;
        state.in_flight += 1;
        Ok(Permit {
            budget: self.clone(),
            _slot: slot,
            reported: false,
        })
    }

    /// Reads the counters and reported usage together.
    pub fn snapshot(&self) -> Snapshot {
        let state = self.state.lock().unwrap();
        Snapshot {
            calls: state.calls,
            in_flight: state.in_flight,
            usage: state.usage.clone(),
        }
    }

    /// Shared deadline for admission, provider work, and answer validation.
    pub fn deadline(&self) -> Instant {
        self.deadline
    }
}

/// One admitted attempt; dropping it frees its concurrency slot.
#[derive(Debug)]
pub struct Permit {
    budget: Arc<Budget>,
    _slot: OwnedSemaphorePermit,
    reported: bool,
}

impl Permit {
    /// Adds the attempt's reported usage at most once, even when called again.
    pub fn report(&mut self, usage: &Usage) {
        if !self.reported {
            self.budget.state.lock().unwrap().usage += usage;
            self.reported = true;
        }
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.budget.state.lock().unwrap().in_flight -= 1;
    }
}
