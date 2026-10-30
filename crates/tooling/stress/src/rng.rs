//! A small random source for the choices of simulated people: which page, how long they read, what they type.
//!
//! xorshift64*: fast, good enough for spreading load, and no dependency. Not for anything secret.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A random source owned by one simulated person or thread.
pub struct Rng(u64);

impl Rng {
    /// A source seeded by the clock and a number that tells its owner apart from the others started at once.
    pub fn new(salt: u64) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        // A zero state would stay zero; the odd constant keeps it away from that.
        Self((nanos ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15)) | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A number in `0..n` (0 when `n` is 0).
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }

    /// A number in `[0, 1)`.
    pub fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// True with probability `p`.
    pub fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }

    /// One of `items`, or `None` when there are none.
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() {
            None
        } else {
            items.get(self.below(items.len()))
        }
    }

    /// A duration spread evenly from half to one and a half times `mean`: people do not act on a beat.
    pub fn around(&mut self, mean: Duration) -> Duration {
        mean.mul_f64(0.5 + self.unit())
    }
}
