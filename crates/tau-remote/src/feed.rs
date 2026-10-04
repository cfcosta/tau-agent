//! The numbered messages every phone gets, with no sockets in it: the
//! server drives a [`Feed`] over the network, and tests drive it in
//! process.
//!
//! Each message the host sends is numbered. The feed keeps the last
//! [`REPLAY`] of them, so a phone that comes back with the number of
//! the last one it had gets what it missed. A phone that has none, or
//! missed more than the feed kept, needs a snapshot: until it has one
//! it takes no messages, since the snapshot holds them.

use std::{
    collections::{HashMap, VecDeque},
    hash::Hash,
};

use serde_json::Value;

use crate::wire::Down;

/// How many messages the host keeps for phones that come back.
pub const REPLAY: usize = 4096;

/// What a phone that joins gets.
#[derive(Debug, Clone, PartialEq)]
pub enum Joined {
    /// What it missed, in order (none when it missed nothing). It takes
    /// new messages from now on.
    Replay(Vec<Down>),
    /// Everything: answer with [`Feed::snapshot`].
    NeedSnapshot,
}

/// The messages sent and the phones that take them, by connection `C`.
#[derive(Debug, Clone)]
pub struct Feed<C> {
    /// The last message's number.
    seq: u64,
    /// The last messages, oldest first.
    buffer: VecDeque<(u64, Value)>,
    /// The connections, and whether each takes new messages: it has had
    /// its replay or snapshot.
    conns: HashMap<C, bool>,
    keep: usize,
}

impl<C: Copy + Eq + Hash> Feed<C> {
    /// A feed whose first message is numbered `after + 1`, keeping
    /// [`REPLAY`] messages.
    pub fn new(after: u64) -> Self {
        Self::keeping(after, REPLAY)
    }

    /// [`Feed::new`], keeping only `keep` messages.
    pub fn keeping(after: u64, keep: usize) -> Self {
        Self {
            seq: after,
            buffer: VecDeque::new(),
            conns: HashMap::new(),
            keep,
        }
    }

    /// The last message's number.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Numbers `body`; returns it, and the connections to send it to.
    pub fn broadcast(&mut self, body: Value) -> (Down, Vec<C>) {
        self.seq += 1;
        self.buffer.push_back((self.seq, body.clone()));
        if self.buffer.len() > self.keep {
            self.buffer.pop_front();
        }
        let to = self
            .conns
            .iter()
            .filter(|(_, live)| **live)
            .map(|(conn, _)| *conn)
            .collect();
        (
            Down::Message {
                seq: self.seq,
                body,
            },
            to,
        )
    }

    /// `conn` joins, having had every message up to `last`, if any.
    pub fn join(&mut self, conn: C, last: Option<u64>) -> Joined {
        let missed = last.and_then(|last| self.missed(last));
        self.conns.insert(conn, missed.is_some());
        missed.map_or(Joined::NeedSnapshot, Joined::Replay)
    }

    /// Answers [`Joined::NeedSnapshot`] with `bodies`, as of the last
    /// message; `conn` takes new messages from now on. None if `conn`
    /// left.
    pub fn snapshot(&mut self, conn: C, bodies: Vec<Value>) -> Option<Down> {
        let live = self.conns.get_mut(&conn)?;
        *live = true;
        Some(Down::Snapshot {
            seq: self.seq,
            bodies,
        })
    }

    pub fn leave(&mut self, conn: C) {
        self.conns.remove(&conn);
    }

    /// The connections, for closing some.
    pub fn conns(&self) -> impl Iterator<Item = C> + '_ {
        self.conns.keys().copied()
    }

    /// The messages after `last`, if the buffer still has them all.
    fn missed(&self, last: u64) -> Option<Vec<Down>> {
        if last == self.seq {
            return Some(Vec::new());
        }
        let oldest = self.buffer.front()?.0;
        (last < self.seq && oldest <= last + 1).then(|| {
            self.buffer
                .iter()
                .filter(|(seq, _)| *seq > last)
                .map(|(seq, body)| Down::Message {
                    seq: *seq,
                    body: body.clone(),
                })
                .collect()
        })
    }
}
