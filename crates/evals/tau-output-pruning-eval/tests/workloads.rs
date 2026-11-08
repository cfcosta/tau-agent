//! Workloads: the same for the same seed, and every needle where it is
//! claimed, exactly once, over the gate.

use std::sync::OnceLock;

use hegel::{TestCase, generators as gs, generators::Generator as _};
use serde_json::Value;
use tau_output_pruning_eval::{
    measure::{self, estimate_tokens},
    runner::{self, exec_result},
    workload::{Kind, generate},
};
use tau_testing::block_on_io;
use tau_tools_host::truncate::MAX_LINES;

/// The plugin's settings `key`, by its default, as it declares them.
fn default(key: &str) -> usize {
    static DEFAULTS: OnceLock<Value> = OnceLock::new();
    let defaults =
        DEFAULTS.get_or_init(|| block_on_io(runner::defaults()).unwrap());
    defaults[key].as_u64().unwrap() as usize
}

/// The same kind and seed make the same workload; another seed, another.
#[hegel::test(test_cases = 20)]
fn generation_is_deterministic(tc: TestCase) {
    let kind = tc.draw(gs::sampled_from(Kind::ALL.to_vec()).print_as_debug());
    let seed = tc.draw(gs::integers::<u64>());
    let workload = generate(kind, seed);
    assert_eq!(workload, generate(kind, seed));
    assert_ne!(workload.output, generate(kind, seed.wrapping_add(1)).output);
}

/// Every needle is the line it claims to be, and appears in the output
/// exactly once; the output passes the gate; only all-noise needs
/// nothing.
#[hegel::test(test_cases = 20)]
fn needles_are_where_claimed_exactly_once(tc: TestCase) {
    let kind = tc.draw(gs::sampled_from(Kind::ALL.to_vec()).print_as_debug());
    let seed = tc.draw(gs::integers::<u64>());
    let workload = generate(kind, seed);
    let lines: Vec<&str> = workload.output.split('\n').collect();
    for needle in &workload.needles {
        assert_eq!(lines[needle.line], needle.text);
        assert_eq!(
            workload.output.matches(needle.text.as_str()).count(),
            1,
            "{}",
            needle.text
        );
    }
    assert_eq!(workload.needles.is_empty(), kind == Kind::AllNoise);
    assert!(estimate_tokens(&workload.output) > default("output_min_tokens"));
}

/// The kinds that promise a place keep it: cut-middle's needle is
/// above the tail `exec_command` keeps; multi-needle's three are far apart;
/// needle-error's error is in neither end chunk;
/// earlier-requirement's needle is named only by its earlier read,
/// which is larger than the half of a state the history gets beside a
/// large output.
#[hegel::test(test_cases = 20)]
fn kinds_keep_their_promises(tc: TestCase) {
    let seed = tc.draw(gs::integers::<u64>());
    let cut = generate(Kind::CutMiddle, seed);
    let lines = cut.output.split('\n').count();
    assert!(cut.needles[0].line + MAX_LINES < lines);
    let (tail, truncated) = exec_result(&cut.output);
    assert!(truncated);
    assert!(!tail.contains(&cut.needles[0].text));

    let multi = generate(Kind::MultiNeedle, seed);
    assert_eq!(multi.needles.len(), 3);
    let lines = multi.output.split('\n').count();
    for pair in multi.needles.windows(2) {
        assert!(pair[1].line - pair[0].line > lines / 5);
    }

    // needle-error's error is in neither end chunk, which pruning keeps
    // whatever Jev says, so only Jev's answer can keep it.
    let error = generate(Kind::NeedleError, seed);
    let lines = measure::lines(&error.output).len();
    let chunks = measure::chunks(lines, default("output_chunk_lines"));
    let (first, last) = (&chunks[0], &chunks[chunks.len() - 1]);
    for needle in &error.needles {
        assert!(
            !first.contains(&needle.line) && !last.contains(&needle.line),
            "line {} of {lines}",
            needle.line
        );
    }

    let earlier = generate(Kind::EarlierRequirement, seed);
    let needle = &earlier.needles[0].text;
    let name = needle.rsplit_once("dist/").unwrap().1;
    assert!(!earlier.prompt.contains(name));
    assert!(earlier.earlier[0].result.contains(name));
    assert!(
        estimate_tokens(&earlier.earlier[0].result)
            > default("output_max_state_tokens") / 2
    );
}
