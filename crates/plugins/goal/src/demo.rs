//! The goals tau-ui's demo shows: what a run published as it went.

use serde_json::Value;

use crate::{Check, Exhausted, Record};

/// What a run working on `condition` published, as it stands at `state`:
/// `working` after two checks that did not hold, `met` when the third
/// did, `stopped` when it ran out of continuations.
pub fn records(condition: &str, state: &str) -> Vec<Value> {
    let check = |n: u32, met: bool, p: f64, turn: u32, continuation| {
        Record::Check(Check {
            n,
            met,
            p,
            turn,
            continuation,
            cost: 0.00003,
            spent: 0.12 * f64::from(n),
        })
    };
    let mut records = vec![Record::Set {
        goal: condition.into(),
        continuations: 10,
        budget: 2.0,
    }];
    match state {
        "stopped" => {
            records.push(Record::Set {
                goal: condition.into(),
                continuations: 2,
                budget: 2.0,
            });
            records.push(check(1, false, 0.12, 9, Some(1)));
            records.push(check(2, false, 0.11, 11, Some(2)));
            records.push(check(3, false, 0.11, 13, None));
            records.push(Record::Stopped {
                why: Exhausted::Continuations,
            });
        }
        _ => {
            records.push(check(1, false, 0.03, 8, Some(1)));
            records.push(check(2, false, 0.08, 10, Some(2)));
            if state == "met" {
                records.push(check(3, true, 0.96, 11, None));
            }
        }
    }
    records
        .iter()
        .map(|record| {
            serde_json::to_value(record).expect("a record serializes")
        })
        .collect()
}
