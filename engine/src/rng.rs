fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e3779b97f4a7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}

/// Weights are published out of this, and drawn against it without a modulo.
pub const TOTAL: u64 = 1 << 32;

/// xoroshiro128++, seeded through splitmix64 — the deterministic stream both the program and the
/// client derive a card from. The draws below read high bits; the low bits are the weak ones.
pub struct Rng(u64, u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        let a = splitmix64(seed);
        let b = splitmix64(a);
        Self(a.max(1), b)
    }

    /// Folds all 32 seed bytes into 128 bits of state; using only 8 would cap the card space at 2^64.
    pub fn from_bytes(seed: &[u8; 32]) -> Self {
        let mut a = 0u64;
        let mut b = 0u64;
        for (i, chunk) in seed.chunks_exact(8).enumerate() {
            let w = splitmix64(u64::from_le_bytes(chunk.try_into().unwrap()));
            if i % 2 == 0 {
                a = splitmix64(a ^ w);
            } else {
                b = splitmix64(b ^ w);
            }
        }
        // all-zero is xoroshiro's one forbidden state: it emits zeros forever
        if a == 0 && b == 0 {
            a = 1;
        }
        Self(a, b)
    }

    #[inline]
    pub fn next(&mut self) -> u64 {
        let s0 = self.0;
        let mut s1 = self.1;
        let result = s0.wrapping_add(s1).rotate_left(17).wrapping_add(s0);
        s1 ^= s0;
        self.0 = s0.rotate_left(49) ^ s1 ^ (s1 << 21);
        self.1 = s1.rotate_left(28);
        result
    }

    /// A weight in [0, 2^32) from the high half — exact, since 2^64 is a multiple of 2^32.
    #[inline]
    pub fn weight(&mut self) -> u32 {
        (self.next() >> 32) as u32
    }

    /// Uniform-ish in `[0, n)` by multiply-shift on the top bits; bias ≤ n/2^64.
    #[inline]
    pub fn below(&mut self, n: u64) -> u64 {
        if n <= 1 {
            return 0;
        }
        ((self.next() as u128 * n as u128) >> 64) as u64
    }

    /// Inclusive `[min, max]`.
    #[inline]
    pub fn range(&mut self, min: i32, max: i32) -> i32 {
        if max <= min {
            return min;
        }
        min + self.below((max as i64 - min as i64 + 1) as u64) as i32
    }

    /// Walks a weight table, drawing against the full 2^32 so a short table leaves the rest as a miss.
    #[inline]
    pub fn pick(&mut self, weights: &[u32]) -> Option<usize> {
        let mut r = self.weight();
        for (i, &w) in weights.iter().enumerate() {
            if r < w {
                return Some(i);
            }
            r -= w;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_respects_the_shortfall() {
        let full = [u32::MAX / 2 + 1, u32::MAX / 2]; // sums to exactly 2^32 - 1 + 1
        let half = [1u32 << 31];
        let mut rng = Rng::from_bytes(&[7u8; 32]);
        let mut misses_full = 0;
        let mut misses_half = 0;
        for _ in 0..200_000 {
            if rng.pick(&full).is_none() {
                misses_full += 1;
            }
            if rng.pick(&half).is_none() {
                misses_half += 1;
            }
        }
        assert_eq!(misses_full, 0, "a full table must always land");
        let rate = misses_half as f64 / 200_000.0;
        assert!((rate - 0.5).abs() < 0.01, "half a table should miss half the time, got {rate}");
    }

    #[test]
    fn every_seed_byte_changes_the_stream() {
        let base = [0u8; 32];
        let first = Rng::from_bytes(&base).next();
        for i in 0..32 {
            let mut seed = base;
            seed[i] = 1;
            assert_ne!(Rng::from_bytes(&seed).next(), first, "byte {i} is ignored");
        }
    }

    #[test]
    fn all_zero_state_still_generates() {
        let mut rng = Rng::from_bytes(&[0u8; 32]);
        assert!((0..8).any(|_| rng.next() != 0), "state collapsed to zero");
    }

    #[test]
    fn below_is_even() {
        let mut rng = Rng::from_bytes(&[3u8; 32]);
        let mut counts = [0u32; 9];
        for _ in 0..90_000 {
            counts[rng.below(9) as usize] += 1;
        }
        for (i, &c) in counts.iter().enumerate() {
            assert!((c as f64 - 10_000.0).abs() < 500.0, "slot {i} got {c}");
        }
    }
}
