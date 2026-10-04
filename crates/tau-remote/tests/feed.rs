//! The feed against a model of what each phone must end up with: the
//! host's whole log, with no message lost, repeated or out of order,
//! whether the phone stays, comes back in time for a replay, or comes
//! back too late and needs a snapshot. Messages go on while a snapshot
//! is asked for and before it is answered.

use std::collections::VecDeque;

use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_remote::{
    Down,
    feed::{Feed, Joined},
};

/// Numbers start here, as they start from the clock on a computer.
const FIRST: u64 = 1_000;
/// A small buffer, so phones fall behind it.
const KEEP: usize = 4;

#[derive(Debug, Default)]
struct Phone {
    /// Its connection, while it has one.
    conn: Option<u32>,
    /// What was sent to it and has not arrived.
    inbox: VecDeque<Down>,
    /// What it applied: the host's log, as far as it got.
    seen: Vec<i64>,
    /// The last message's number it applied.
    last: Option<u64>,
    /// It asked for a snapshot that has not been answered.
    waiting: bool,
}

struct Machine {
    feed: Feed<u32>,
    /// What the host sent, in order.
    log: Vec<i64>,
    phones: Vec<Phone>,
    next_conn: u32,
}

impl Machine {
    fn phone(&self, tc: &TestCase) -> usize {
        tc.draw(gs::integers().min_value(0).max_value(self.phones.len() - 1))
    }

    fn arrive(phone: &mut Phone, down: Down) {
        match down {
            Down::Message { seq, body } => {
                assert_eq!(
                    Some(seq),
                    phone.last.map(|last| last + 1),
                    "a message out of order, repeated, or after a gap"
                );
                phone.seen.push(body.as_i64().unwrap());
                phone.last = Some(seq);
            }
            Down::Snapshot { seq, bodies } => {
                let [log]: [Value; 1] = bodies.try_into().unwrap();
                phone.seen = serde_json::from_value(log).unwrap();
                phone.last = Some(seq);
            }
            Down::Ack { .. } => unreachable!("the feed answers no request"),
        }
    }
}

#[hegel::state_machine]
impl Machine {
    #[rule(weight = 3)]
    fn host_sends(&mut self, tc: TestCase) {
        let value: i64 = tc.draw(gs::integers().min_value(0).max_value(99));
        self.log.push(value);
        let (down, to) = self.feed.broadcast(json!(value));
        for phone in &mut self.phones {
            if phone.conn.is_some_and(|conn| to.contains(&conn)) {
                phone.inbox.push_back(down.clone());
            }
        }
    }

    #[rule(weight = 3)]
    fn one_arrives(&mut self, tc: TestCase) {
        let i = self.phone(&tc);
        let phone = &mut self.phones[i];
        let Some(down) = phone.inbox.pop_front() else {
            tc.reject()
        };
        Self::arrive(phone, down);
    }

    #[rule]
    fn host_answers_snapshot(&mut self, tc: TestCase) {
        let i = self.phone(&tc);
        let phone = &mut self.phones[i];
        tc.assume(phone.waiting);
        phone.waiting = false;
        let conn = phone.conn.unwrap();
        let down = self.feed.snapshot(conn, vec![json!(self.log)]).unwrap();
        phone.inbox.push_back(down);
    }

    #[rule]
    fn phone_drops(&mut self, tc: TestCase) {
        let i = self.phone(&tc);
        let phone = &mut self.phones[i];
        let Some(conn) = phone.conn.take() else {
            tc.reject()
        };
        // What was on its way is lost with the connection.
        phone.inbox.clear();
        phone.waiting = false;
        self.feed.leave(conn);
    }

    #[rule]
    fn phone_connects(&mut self, tc: TestCase) {
        let i = self.phone(&tc);
        let phone = &mut self.phones[i];
        tc.assume(phone.conn.is_none());
        let conn = self.next_conn;
        self.next_conn += 1;
        phone.conn = Some(conn);
        match self.feed.join(conn, phone.last) {
            Joined::Replay(missed) => phone.inbox.extend(missed),
            Joined::NeedSnapshot => phone.waiting = true,
        }
    }

    /// Whatever a phone applied is the start of the host's log, and its
    /// number says how far into it.
    #[invariant(always_run)]
    fn phones_follow_the_log(&self, _tc: TestCase) {
        for phone in &self.phones {
            assert_eq!(phone.seen[..], self.log[..phone.seen.len()]);
            if let Some(last) = phone.last {
                assert_eq!(last, FIRST + phone.seen.len() as u64);
            }
        }
    }

    /// A connected phone with nothing on its way and no snapshot owed
    /// has all of it.
    #[invariant(always_run)]
    fn settled_phones_have_everything(&self, _tc: TestCase) {
        for phone in &self.phones {
            if phone.conn.is_some() && phone.inbox.is_empty() && !phone.waiting
            {
                assert_eq!(phone.seen, self.log);
            }
        }
    }
}

#[hegel::test(test_cases = 300)]
fn every_phone_ends_with_the_whole_log(tc: TestCase) {
    let phones: usize = tc.draw(gs::integers().min_value(1).max_value(3));
    let machine = Machine {
        feed: Feed::keeping(FIRST, KEEP),
        log: Vec::new(),
        phones: (0..phones).map(|_| Phone::default()).collect(),
        next_conn: 0,
    };
    hegel::stateful::machine(machine).steps(60).run(tc);
}
