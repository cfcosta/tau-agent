//! Workloads: the same for the same seed, and every needle where it is
//! claimed, exactly once, over the gate.

use hegel::{TestCase, generators as gs, generators::Generator as _};
use tau_fast_compaction::{OutputPruning, state::estimate_tokens};
use tau_output_pruning_eval::{
    runner::bash_result,
    workload::{Kind, generate},
};
use tau_tools_host::truncate::MAX_LINES;

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
    assert!(
        estimate_tokens(&workload.output)
            > OutputPruning::default().min_output_tokens
    );
}

/// The kinds that promise a place keep it: spilled-middle's needle is
/// above the tail `bash` keeps; multi-needle's three are far apart;
/// earlier-requirement's needle is named only by its earlier read,
/// which is larger than the half of a state the history gets beside a
/// large output.
#[hegel::test(test_cases = 20)]
fn kinds_keep_their_promises(tc: TestCase) {
    let seed = tc.draw(gs::integers::<u64>());
    let spilled = generate(Kind::SpilledMiddle, seed);
    let lines = spilled.output.split('\n').count();
    assert!(spilled.needles[0].line + MAX_LINES < lines);
    let (tail, truncated) =
        bash_result(&spilled.output, std::path::Path::new("/x/tau-bash-0.log"));
    assert!(truncated);
    assert!(!tail.contains(&spilled.needles[0].text));

    let multi = generate(Kind::MultiNeedle, seed);
    assert_eq!(multi.needles.len(), 3);
    let lines = multi.output.split('\n').count();
    for pair in multi.needles.windows(2) {
        assert!(pair[1].line - pair[0].line > lines / 5);
    }

    let earlier = generate(Kind::EarlierRequirement, seed);
    let needle = &earlier.needles[0].text;
    let name = needle.rsplit_once("dist/").unwrap().1;
    assert!(!earlier.prompt.contains(name));
    assert!(earlier.earlier[0].result.contains(name));
    assert!(
        estimate_tokens(&earlier.earlier[0].result)
            > OutputPruning::default().max_state_tokens / 2
    );
}
