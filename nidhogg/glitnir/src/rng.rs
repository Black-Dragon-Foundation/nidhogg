//! A tiny seeded xorshift generator for property tests and benches.

/// xorshift64*, deterministic for a given seed.
#[derive(Debug, Clone)]
pub struct XorShift(u64);

impl XorShift {
    /// A generator from `seed` (0 is mapped to a fixed non-zero state).
    pub fn new(seed: u64) -> Self {
        Self(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A value in `0..n` (`n` > 0).
    pub fn below(&mut self, n: u32) -> u32 {
        ((self.next_u64() >> 32) % u64::from(n)) as u32
    }
}
