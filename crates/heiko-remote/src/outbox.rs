//! What a phone asks of the computer, sent so each request takes effect
//! once, with no sockets in it: the client keeps an [`Outbox`] over the
//! network, and tests keep one in process.
//!
//! The phone numbers each request and keeps it until the computer says
//! it took it ([`Down::Ack`](crate::Down::Ack)). Whenever the phone
//! connects, it sends again what was not taken: a request made while
//! away, or one lost with a connection. The computer remembers the last
//! number it took from each phone and takes nothing at or below it, so
//! a request it took before its answer was lost is not taken twice.
//! Numbers only grow: a phone sends its requests in order, and each new
//! connection sends the ones left in order too.

use std::collections::VecDeque;

use serde_json::Value;

use crate::wire::Up;

/// The phone's requests the computer has not said it took.
#[derive(Debug, Clone)]
pub struct Outbox {
    /// The last request's number.
    last: u64,
    pending: VecDeque<(u64, Value)>,
}

impl Outbox {
    /// An outbox whose first request is numbered `after + 1`. A phone
    /// starts from the clock, so numbers grow across its restarts too.
    pub fn new(after: u64) -> Self {
        Self {
            last: after,
            pending: VecDeque::new(),
        }
    }

    /// Numbers `body` and keeps it; returns it, to send now if connected.
    pub fn push(&mut self, body: Value) -> Up {
        self.last += 1;
        self.pending.push_back((self.last, body.clone()));
        Up::Up {
            id: self.last,
            body,
        }
    }

    /// The computer took every request up to `id`.
    pub fn taken(&mut self, id: u64) {
        while self.pending.front().is_some_and(|(first, _)| *first <= id) {
            self.pending.pop_front();
        }
    }

    /// What to send again on a new connection, in order.
    pub fn pending(&self) -> impl Iterator<Item = Up> + '_ {
        self.pending.iter().map(|(id, body)| Up::Up {
            id: *id,
            body: body.clone(),
        })
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

/// Whether the computer takes request `id` from a phone whose last
/// request taken was `last`: one it took already comes again only
/// because its answer was lost.
pub fn is_new(last: Option<u64>, id: u64) -> bool {
    last.is_none_or(|last| id > last)
}
