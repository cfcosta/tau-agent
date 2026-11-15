//! | Property | Kind |
//! |---|---|
//! | a row counts, sums and keeps the longest of what it is given | example |
//! | each measurement lands in the bucket of its bound | example |
//! | a span records under its name and restores what the thread was doing | example |

use std::time::Duration;

use heiko_perf::{BOUNDS, doing, record, report, span};

fn row(group: &str, name: &str) -> heiko_perf::Row {
    report().groups[group][name].clone()
}

#[test]
fn a_row_counts_sums_and_keeps_the_longest() {
    for ms in [2, 40, 7] {
        record("test-sum", "a", Duration::from_millis(ms));
    }
    let row = row("test-sum", "a");
    assert_eq!(row.count, 3);
    assert_eq!(row.total_ms, 49.);
    assert_eq!(row.max_ms, 40.);
}

#[test]
fn each_measurement_lands_in_the_bucket_of_its_bound() {
    // One under the first bound, one at each bound: the bucket after it.
    record("test-buckets", "a", Duration::from_micros(500));
    for bound in BOUNDS {
        record("test-buckets", "a", Duration::from_millis(bound));
    }
    let row = row("test-buckets", "a");
    assert_eq!(row.buckets, [1; BOUNDS.len() + 1]);
    assert_eq!(row.over(16), 5);
    assert_eq!(row.over(1000), 1);
}

#[test]
fn a_span_records_under_its_name_and_restores_what_was_doing() {
    doing("before");
    {
        let _span = span("test-span", "inside");
        assert_eq!(doing("inside"), "inside");
    }
    assert_eq!(doing(""), "before");
    assert_eq!(row("test-span", "inside").count, 1);
}
