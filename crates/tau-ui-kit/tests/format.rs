//! Numbers as the interface writes them (`format.rs`), each against a
//! reading of its text back into the number it stands for.

use std::time::Duration;

use hegel::generators as gs;
use tau_ui_kit::format::{clock, grouped, tokens};

/// A count reads back as itself once its commas go, and the commas cut
/// it into threes from the right: the first group holds one to three
/// digits, every other group three.
#[hegel::test(test_cases = 300)]
fn grouped_counts_read_back_in_threes(tc: hegel::TestCase) {
    let count: usize = tc.draw(hegel::one_of!(
        gs::integers::<usize>().max_value(10_000),
        gs::integers::<usize>(),
    ));
    let text = grouped(count);
    assert_eq!(text.replace(',', ""), count.to_string(), "{text}");
    let groups: Vec<&str> = text.split(',').collect();
    assert!((1..=3).contains(&groups[0].len()), "{text}");
    assert!(groups[1..].iter().all(|group| group.len() == 3), "{text}");
}

/// A duration reads as whole minutes, a colon and two digits of
/// seconds, which together are its whole seconds.
#[hegel::test(test_cases = 300)]
fn a_clock_reads_back_as_its_whole_seconds(tc: hegel::TestCase) {
    let millis: u64 = tc.draw(gs::integers::<u64>().max_value(1 << 40));
    let duration = Duration::from_millis(millis);
    let text = clock(duration);
    let (minutes, seconds) = text.split_once(':').expect("a colon");
    assert_eq!(seconds.len(), 2, "{text}");
    let (minutes, seconds): (u64, u64) =
        (minutes.parse().unwrap(), seconds.parse().unwrap());
    assert!(seconds < 60, "{text}");
    assert_eq!(minutes * 60 + seconds, duration.as_secs(), "{text}");
}

/// A token count under a thousand is itself; under a million it is the
/// thousands it holds, cut down, and a `k`; past that, millions to a
/// tenth, rounded, and an `M`.
#[hegel::test(test_cases = 300)]
fn token_counts_read_back_to_their_unit(tc: hegel::TestCase) {
    let count: u64 = tc.draw(hegel::one_of!(
        gs::integers::<u64>().max_value(2_000),
        gs::integers::<u64>().max_value(2_000_000),
        gs::integers::<u64>().max_value(1 << 40),
    ));
    let text = tokens(count);
    if count < 1_000 {
        assert_eq!(text, count.to_string());
    } else if let Some(thousands) = text.strip_suffix('k') {
        let thousands: u64 = thousands.parse().unwrap();
        assert!(count < 1_000_000, "{count} as {text}");
        assert!(
            (thousands * 1_000..(thousands + 1) * 1_000).contains(&count),
            "{count} as {text}"
        );
    } else {
        let millions = text.strip_suffix('M').expect("a unit");
        let (whole, tenth) = millions.split_once('.').expect("a tenth");
        assert_eq!(tenth.len(), 1, "{text}");
        let millions: f64 = millions.parse().unwrap();
        assert!(count >= 1_000_000, "{count} as {text}");
        let off = (millions * 1e6 - count as f64).abs();
        assert!(off <= 50_000.0 + 1.0, "{count} as {text}");
        assert!(!whole.is_empty());
    }
}
