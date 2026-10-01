//! Numbers as the interface writes them: counts, tokens, dollars and
//! durations.

use std::time::Duration;

/// `4810` as `4,810`.
pub fn grouped(count: usize) -> String {
    let digits = count.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// `184000` as `184k`.
pub fn tokens(count: u64) -> String {
    match count {
        0..1_000 => count.to_string(),
        1_000..1_000_000 => format!("{}k", count / 1_000),
        _ => format!("{:.1}M", count as f64 / 1_000_000.0),
    }
}

/// Dollars to the cent, or to a tenth of one under a dollar: `$0.042`,
/// `$1.00`, `$12.34`. The choice is made on the rounded amount, so
/// `0.9996` reads `$1.00`, never `$1.000`.
pub fn usd(amount: f64) -> String {
    let fine = format!("{amount:.3}");
    if fine.parse::<f64>().is_ok_and(|rounded| rounded < 1.0) {
        format!("${fine}")
    } else {
        format!("${amount:.2}")
    }
}

/// A Jev cost, which is often a fraction of a cent: to four places
/// under a cent, else as [`usd`].
pub fn fine_usd(amount: f64) -> String {
    if amount < 0.01 {
        format!("${amount:.4}")
    } else {
        usd(amount)
    }
}

pub fn clock(duration: Duration) -> String {
    let secs = duration.as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
}
