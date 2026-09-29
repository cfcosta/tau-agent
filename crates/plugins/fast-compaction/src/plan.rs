//! Which requests to send when the history comes in segments, and what
//! their answers add up to.
//!
//! Ported from `tamaratran/jev-pruner` (`src/output.ts`,
//! `scoringRequests`; see `THIRD_PARTY_NOTICES.md`). Every item (an
//! output chunk, or a tool call) is asked about against every segment:
//! the items go into each segment's state in groups that fit the state
//! budget, and their questions in batches that fit the request budget
//! beside it. Counted in tenths of a token, so what adds up here is
//! exactly [`crate::state::estimate_state_tokens`] of the JSON sent, or
//! more.

/// Tokens a request takes besides its state and questions.
pub const REQUEST_OVERHEAD_TOKENS: usize = 20;

/// Something to ask about: what it adds to a state (its JSON and a
/// comma), and what its questions add to a request, in tenths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Item {
    pub state_tenths: usize,
    pub question_tenths: usize,
}

/// The tokens of a state kept free of history, given its `fixed` part
/// (all but the history and the items), `all` of it with every item,
/// and its `largest` item that fits it at all: room for every item when
/// they take half the state or less, else half; and room for the
/// largest item beside any segment, unless that leaves the history less
/// than half of what the fixed part leaves. An item bigger than that
/// is asked about only beside the segments it fits beside.
pub fn reserve(
    fixed: usize,
    all: usize,
    largest: usize,
    max_state: usize,
) -> usize {
    let half_the_rest = fixed + max_state.saturating_sub(fixed).div_ceil(2);
    all.min(max_state.div_ceil(2))
        .max((fixed + largest).min(half_the_rest))
        .max(fixed)
}

/// One request: the segment its state holds, the items in its state,
/// and the items it asks about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub segment: usize,
    pub group: Vec<usize>,
    pub batch: Vec<usize>,
}

/// The requests that ask about every item against every segment, given
/// each segment's state without items (`bases`, in tenths): items in
/// groups whose state fits `max_state` tokens, and questions in batches
/// that fit `max_request` tokens with the state. An item that does not
/// fit a segment's state alone, or whose questions do not fit beside
/// its group, is not asked about in that segment. Requests come round
/// robin over the segments, first requests first, so cutting the list
/// short spreads what is left out.
pub fn plan(
    bases: &[usize],
    items: &[Item],
    max_state: usize,
    max_request: usize,
) -> Vec<Planned> {
    let max_state_tenths = max_state * 10;
    let by_segment: Vec<Vec<Planned>> = bases
        .iter()
        .enumerate()
        .map(|(segment, &base)| {
            let mut groups: Vec<(Vec<usize>, usize)> = Vec::new();
            let mut group = Vec::new();
            let mut tenths = base;
            for (index, item) in items.iter().enumerate() {
                if base + item.state_tenths > max_state_tenths {
                    continue;
                }
                if !group.is_empty()
                    && tenths + item.state_tenths > max_state_tenths
                {
                    groups.push((std::mem::take(&mut group), tenths));
                    tenths = base;
                }
                group.push(index);
                tenths += item.state_tenths;
            }
            if !group.is_empty() {
                groups.push((group, tenths));
            }
            groups
                .into_iter()
                .flat_map(|(group, tenths)| {
                    let budget = max_request
                        .saturating_sub(tenths.div_ceil(10))
                        .saturating_sub(REQUEST_OVERHEAD_TOKENS)
                        * 10;
                    let mut batches: Vec<Vec<usize>> = Vec::new();
                    let mut batch = Vec::new();
                    let mut used = 0;
                    for &index in &group {
                        let cost = items[index].question_tenths;
                        if cost > budget {
                            continue;
                        }
                        if !batch.is_empty() && used + cost > budget {
                            batches.push(std::mem::take(&mut batch));
                            used = 0;
                        }
                        batch.push(index);
                        used += cost;
                    }
                    if !batch.is_empty() {
                        batches.push(batch);
                    }
                    batches.into_iter().map(move |batch| Planned {
                        segment,
                        group: group.clone(),
                        batch,
                    })
                })
                .collect()
        })
        .collect();
    let rounds = by_segment.iter().map(Vec::len).max().unwrap_or(0);
    (0..rounds)
        .flat_map(|round| {
            by_segment
                .iter()
                .filter_map(move |requests| requests.get(round).cloned())
        })
        .collect()
}

/// The answers about each item, per segment.
#[derive(Debug, Clone, PartialEq)]
pub struct Tally {
    /// By item, then segment: the answers, when that segment's were
    /// given.
    answers: Vec<Vec<Option<Vec<f64>>>>,
}

impl Tally {
    pub fn new(items: usize, segments: usize) -> Self {
        Self {
            answers: vec![vec![None; segments]; items],
        }
    }

    /// Records the answers about `item` against `segment`, keeping the
    /// larger of each when it was answered before.
    pub fn record(&mut self, item: usize, segment: usize, answers: Vec<f64>) {
        let slot = &mut self.answers[item][segment];
        *slot = Some(match slot.take() {
            Some(before) => before
                .iter()
                .zip(&answers)
                .map(|(a, b)| a.max(*b))
                .collect(),
            None => answers,
        });
    }

    /// Each answer's largest value over the segments, once every segment
    /// answered about `item`; `None` while any has not.
    pub fn complete(&self, item: usize) -> Option<Vec<f64>> {
        let mut most: Option<Vec<f64>> = None;
        for answers in &self.answers[item] {
            let answers = answers.as_ref()?;
            most = Some(match most {
                Some(most) => {
                    most.iter().zip(answers).map(|(a, b)| a.max(*b)).collect()
                }
                None => answers.clone(),
            });
        }
        most
    }
}
