//! An agent's polls (ADR 0037): a tool or an HTTP request, made every
//! interval, that fires once per item whose key it has not seen.

use crate::schedule::{self, SHORTEST};

/// `every` as seconds, or why a poll cannot go by it: a poll costs a
/// request each time, and at least a minute goes between two.
pub fn every(every: &str) -> Result<i64, String> {
    let seconds = schedule::duration(every)
        .ok_or_else(|| format!("`{every}` is not a duration, like \"15m\""))?;
    if seconds < SHORTEST {
        return Err(format!(
            "`{every}` is shorter than a minute, the shortest a poll may be"
        ));
    }
    Ok(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_poll_goes_by_at_least_a_minute() {
        assert_eq!(every("10m"), Ok(600));
        assert!(every("30s").unwrap_err().contains("shorter than a minute"));
        assert!(every("soon").unwrap_err().contains("not a duration"));
    }
}
