/// SplitMix64: tiny, fast and the same on every machine, which a fixture generator needs more
/// than it needs quality.
#[derive(Debug, Clone)]
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) const fn seeded(seed: u64) -> Self {
        Self(seed ^ 0x9E37_79B9_7F4A_7C15)
    }

    pub(crate) const fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `low..=high`.
    pub(crate) fn range(&mut self, low: i64, high: i64) -> i64 {
        let span = u64::try_from(high.saturating_sub(low))
            .unwrap_or(0)
            .saturating_add(1);
        let offset = i64::try_from(self.next_u64() % span).unwrap_or(0);
        low.saturating_add(offset)
    }

    pub(crate) const fn chance(&mut self, percent: u64) -> bool {
        self.next_u64() % 100 < percent
    }

    pub(crate) fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() {
            return None;
        }
        let index = usize::try_from(self.next_u64() % items.len() as u64).unwrap_or(0);
        items.get(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_stream() {
        let mut a = Rng::seeded(7);
        let mut b = Rng::seeded(7);
        assert_eq!(a.next_u64(), b.next_u64());
        assert_eq!(a.range(3, 9), b.range(3, 9));
        assert!((3..=9).contains(&a.range(3, 9)));
    }
}
