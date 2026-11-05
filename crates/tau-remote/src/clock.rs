//! The time the computer and the phone go by: the wall clock, which
//! messages and requests are numbered from, and a clock that only moves
//! forward, which a pairing secret's age is told by. Tests set both.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub trait Clock: Send + Sync {
    /// The wall clock: a person or the network may move it either way.
    fn now(&self) -> SystemTime;

    /// A clock that only moves forward.
    fn instant(&self) -> Instant;
}

/// The machine's clocks.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }

    fn instant(&self) -> Instant {
        Instant::now()
    }
}

/// The first number counted from `at`: its milliseconds since 1970,
/// with room for 1024 numbers in each, so numbers counted from a later
/// time are higher.
pub fn count_from(at: SystemTime) -> u64 {
    let millis = at
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    u64::try_from(millis).unwrap_or(u64::MAX >> 12) << 10
}
