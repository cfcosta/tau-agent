//! Per-run limits (`docs/reference/api.md`, "Limits"), checked after
//! every turn against actual usage.

use std::time::Duration;

use tau_ai::message::Usage;

use crate::event::LimitKind;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Limits {
    pub max_turns: Option<u32>,
    /// Input, output and cached tokens together.
    pub max_tokens: Option<u64>,
    pub max_usd: Option<f64>,
    pub timeout: Option<Duration>,
}

impl Limits {
    pub fn max_turns(mut self, turns: u32) -> Self {
        self.max_turns = Some(turns);
        self
    }

    pub fn max_tokens(mut self, tokens: u64) -> Self {
        self.max_tokens = Some(tokens);
        self
    }

    pub fn max_usd(mut self, usd: f64) -> Self {
        self.max_usd = Some(usd);
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// The first limit reached after `turns` turns with `usage` in
    /// `elapsed`, checked in the order turns, tokens, cost, time.
    pub fn reached(
        &self,
        turns: u32,
        usage: &Usage,
        elapsed: Duration,
    ) -> Option<LimitKind> {
        if self.max_turns.is_some_and(|max| turns >= max) {
            return Some(LimitKind::Turns);
        }
        let tokens =
            usage.input + usage.output + usage.cache_read + usage.cache_write;
        if self.max_tokens.is_some_and(|max| tokens >= max) {
            return Some(LimitKind::Tokens);
        }
        if self.max_usd.is_some_and(|max| usage.cost.total >= max) {
            return Some(LimitKind::Usd);
        }
        if self.timeout.is_some_and(|max| elapsed >= max) {
            return Some(LimitKind::Time);
        }
        None
    }
}
