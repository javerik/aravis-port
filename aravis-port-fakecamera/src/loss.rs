use std::sync::atomic::{AtomicU64, Ordering};

/// A simple, deterministic-given-a-seed Bernoulli packet-drop injector: each call independently
/// drops with probability `ratio`, used to drive resend-logic tests against the fake camera.
/// Uses a tiny xorshift PRNG (not `rand`, to avoid a mutex/thread-safety wrapper around
/// `rand::thread_rng` for what's a single atomically-updated `u64` of state) rather than
/// depending on external randomness quality — this only needs to be "spread out enough" to
/// exercise resend logic, not cryptographically sound.
pub struct LossInjector {
    ratio: f64,
    state: AtomicU64,
}

impl LossInjector {
    pub fn new(ratio: f64, seed: u64) -> Self {
        Self {
            ratio: ratio.clamp(0.0, 1.0),
            state: AtomicU64::new(seed.max(1)),
        }
    }

    /// `true` if this packet should be dropped.
    pub fn should_drop(&self) -> bool {
        if self.ratio <= 0.0 {
            return false;
        }
        let mut x = self.state.load(Ordering::Relaxed);
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state.store(x, Ordering::Relaxed);
        let normalized = (x >> 11) as f64 / (1u64 << 53) as f64;
        normalized < self.ratio
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_ratio_never_drops() {
        let injector = LossInjector::new(0.0, 42);
        for _ in 0..1000 {
            assert!(!injector.should_drop());
        }
    }

    #[test]
    fn one_ratio_always_drops() {
        let injector = LossInjector::new(1.0, 42);
        for _ in 0..1000 {
            assert!(injector.should_drop());
        }
    }

    #[test]
    fn mid_ratio_drops_roughly_that_fraction() {
        let injector = LossInjector::new(0.3, 12345);
        let dropped = (0..10_000).filter(|_| injector.should_drop()).count();
        let fraction = dropped as f64 / 10_000.0;
        assert!((fraction - 0.3).abs() < 0.05, "expected ~30% drop rate, got {fraction}");
    }
}
