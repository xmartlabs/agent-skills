//! Port of the SPENT-COUNTER region: folds readings of a per-turn counter,
//! which restarts at each turn, into a run total that only grows.

/// `spentCounter(read)`. The first reading is taken on construction.
pub struct SpentCounter<F: FnMut() -> f64> {
    read: F,
    last: f64,
    total: f64,
    resets: u64,
}

impl<F: FnMut() -> f64> SpentCounter<F> {
    pub fn new(mut read: F) -> Self {
        let last = read();
        Self {
            read,
            last,
            total: 0.0,
            resets: 0,
        }
    }

    /// `now()`: a drop is a reset, and the new reading is added in full.
    pub fn now(&mut self) -> f64 {
        let cur = (self.read)();
        if cur < self.last {
            self.resets += 1;
            self.total += cur;
        } else {
            self.total += cur - self.last;
        }
        self.last = cur;
        self.total
    }

    /// `resets()`.
    pub fn resets(&self) -> u64 {
        self.resets
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(readings: &[f64]) -> (Vec<f64>, u64) {
        let mut q = readings.iter().copied();
        let mut c = SpentCounter::new(|| q.next().unwrap());
        let seen = (1..readings.len()).map(|_| c.now()).collect();
        (seen, c.resets())
    }

    #[test]
    fn a_drop_is_a_reset_never_a_negative_phase() {
        assert_eq!(
            feed(&[1_700_000.0, 1_710_000.0, 90_000.0, 120_000.0]),
            (vec![10_000.0, 100_000.0, 130_000.0], 1)
        );
        assert_eq!(
            feed(&[10.0, 50.0, 5.0, 30.0, 2.0]),
            (vec![40.0, 45.0, 70.0, 72.0], 2)
        );
    }
}
