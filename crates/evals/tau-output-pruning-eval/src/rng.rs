//! A small deterministic generator, so a workload is the same for the
//! same seed on every machine and every build.

/// SplitMix64.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number in `low..high`; `low` when the range is empty.
    pub fn range(&mut self, low: usize, high: usize) -> usize {
        if high <= low {
            return low;
        }
        low + (self.next_u64() % (high - low) as u64) as usize
    }

    /// True with probability `1 / n`.
    pub fn one_in(&mut self, n: usize) -> bool {
        self.range(0, n.max(1)) == 0
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.range(0, items.len())]
    }

    /// `len` lowercase hex digits.
    pub fn hex(&mut self, len: usize) -> String {
        (0..len)
            .map(|_| {
                char::from_digit((self.next_u64() % 16) as u32, 16)
                    .expect("under 16")
            })
            .collect()
    }
}
