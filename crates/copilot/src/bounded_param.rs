//! PURPOSE: Integer-only bounded parameter with clamped percentage adjustments.
//!
//! INVARIANTS:
//! - `current` is always within `[min, max]`
//! - All arithmetic is integer-only (no floats, deterministic)
//! - `min <= default <= max` enforced at construction
//!
//! FAILURE MODES:
//! - Construction panics if `min > max` or `default` is out of `[min, max]`
//! - Percentage adjustments saturate to bounds (never overflow/underflow)

#![forbid(unsafe_code)]

/// A parameter value clamped within `[min, max]` with integer-only arithmetic.
///
/// Used by autonomous responders to adjust protocol parameters within
/// governance-defined bounds. All operations are deterministic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedParam {
    current: u64,
    min: u64,
    max: u64,
    default: u64,
}

impl BoundedParam {
    /// Create a new bounded parameter.
    ///
    /// # Panics
    /// Panics if `min > max` or `default` is outside `[min, max]`.
    pub fn new(min: u64, max: u64, default: u64) -> Self {
        assert!(min <= max, "BoundedParam: min ({min}) > max ({max})");
        assert!(
            default >= min && default <= max,
            "BoundedParam: default ({default}) outside [{min}, {max}]"
        );
        Self {
            current: default,
            min,
            max,
            default,
        }
    }

    /// Current value, always within `[min, max]`.
    pub fn current(&self) -> u64 {
        self.current
    }

    /// Minimum bound.
    pub fn min(&self) -> u64 {
        self.min
    }

    /// Maximum bound.
    pub fn max(&self) -> u64 {
        self.max
    }

    /// Default value.
    pub fn default_value(&self) -> u64 {
        self.default
    }

    /// Adjust current value by a signed percentage.
    ///
    /// Formula: `new = current * (100 + pct) / 100`, clamped to `[min, max]`.
    /// Uses integer-only arithmetic. Negative `pct` decreases the value.
    ///
    /// Examples:
    /// - `adjust_pct(25)`: increase by 25% → `current * 125 / 100`
    /// - `adjust_pct(-10)`: decrease by 10% → `current * 90 / 100`
    /// - `adjust_pct(0)`: no change
    pub fn adjust_pct(&mut self, pct: i32) {
        if pct == 0 {
            return;
        }

        let factor = (100i64 + pct as i64).max(0) as u64;
        // Use u128 to prevent overflow on large values
        let new_val = ((self.current as u128) * (factor as u128) / 100) as u64;

        self.current = new_val.clamp(self.min, self.max);
    }

    /// Reset to default value.
    pub fn reset(&mut self) {
        self.current = self.default;
    }

    /// Set to a specific value, clamped to bounds.
    pub fn set(&mut self, value: u64) {
        self.current = value.clamp(self.min, self.max);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_starts_at_default() {
        let p = BoundedParam::new(1, 100, 50);
        assert_eq!(p.current(), 50);
        assert_eq!(p.min(), 1);
        assert_eq!(p.max(), 100);
        assert_eq!(p.default_value(), 50);
    }

    #[test]
    fn adjust_pct_increases() {
        let mut p = BoundedParam::new(1, 10_000, 100);
        p.adjust_pct(25); // 100 * 125 / 100 = 125
        assert_eq!(p.current(), 125);
    }

    #[test]
    fn adjust_pct_decreases() {
        let mut p = BoundedParam::new(1, 10_000, 100);
        p.adjust_pct(-10); // 100 * 90 / 100 = 90
        assert_eq!(p.current(), 90);
    }

    #[test]
    fn adjust_pct_clamps_to_max() {
        let mut p = BoundedParam::new(1, 150, 100);
        p.adjust_pct(100); // 100 * 200 / 100 = 200, clamped to 150
        assert_eq!(p.current(), 150);
    }

    #[test]
    fn adjust_pct_clamps_to_min() {
        let mut p = BoundedParam::new(10, 1000, 50);
        p.adjust_pct(-90); // 50 * 10 / 100 = 5, clamped to 10
        assert_eq!(p.current(), 10);
    }

    #[test]
    fn adjust_pct_zero_is_noop() {
        let mut p = BoundedParam::new(1, 1000, 42);
        p.adjust_pct(0);
        assert_eq!(p.current(), 42);
    }

    #[test]
    fn adjust_pct_negative_100_clamps_to_min() {
        let mut p = BoundedParam::new(1, 1000, 100);
        p.adjust_pct(-100); // 100 * 0 / 100 = 0, clamped to 1
        assert_eq!(p.current(), 1);
    }

    #[test]
    fn adjust_pct_beyond_negative_100_clamps_to_min() {
        let mut p = BoundedParam::new(5, 1000, 100);
        p.adjust_pct(-200); // factor = max(0, -100) = 0 → 0, clamped to 5
        assert_eq!(p.current(), 5);
    }

    #[test]
    fn reset_returns_to_default() {
        let mut p = BoundedParam::new(1, 10_000, 100);
        p.adjust_pct(50);
        assert_eq!(p.current(), 150);
        p.reset();
        assert_eq!(p.current(), 100);
    }

    #[test]
    fn set_clamps_to_bounds() {
        let mut p = BoundedParam::new(10, 500, 100);
        p.set(1000);
        assert_eq!(p.current(), 500);
        p.set(1);
        assert_eq!(p.current(), 10);
        p.set(250);
        assert_eq!(p.current(), 250);
    }

    #[test]
    fn large_value_no_overflow() {
        let mut p = BoundedParam::new(1, u64::MAX, u64::MAX / 2);
        p.adjust_pct(50); // uses u128 internally
                          // (u64::MAX/2) * 150 / 100 should not overflow
        let expected = ((u64::MAX as u128 / 2) * 150 / 100) as u64;
        assert_eq!(p.current(), expected);
    }

    #[test]
    fn deterministic_repeated_adjustments() {
        let mut p1 = BoundedParam::new(1, 10_000, 100);
        let mut p2 = BoundedParam::new(1, 10_000, 100);

        for pct in [25, -10, 50, -30, 100, -50] {
            p1.adjust_pct(pct);
            p2.adjust_pct(pct);
        }
        assert_eq!(p1.current(), p2.current());
    }

    #[test]
    #[should_panic(expected = "min")]
    fn panics_on_invalid_min_max() {
        BoundedParam::new(100, 10, 50);
    }

    #[test]
    #[should_panic(expected = "default")]
    fn panics_on_default_out_of_range() {
        BoundedParam::new(10, 100, 5);
    }

    #[test]
    fn min_equals_max_works() {
        let mut p = BoundedParam::new(42, 42, 42);
        p.adjust_pct(100); // clamped to 42
        assert_eq!(p.current(), 42);
        p.adjust_pct(-50); // clamped to 42
        assert_eq!(p.current(), 42);
    }
}
