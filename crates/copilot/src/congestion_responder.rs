//! PURPOSE: Converts congestion forecasts into fee floor adjustments within governance bounds.
//!
//! INVARIANTS:
//! - Fee floor adjustments are always within BoundedParam bounds
//! - All arithmetic is integer-only (deterministic)
//! - Low congestion gradually decreases toward default (not zero)
//! - Non-censorship: fee floor adjustment NEVER rejects already-accepted transactions
//!
//! FAILURE MODES:
//! - If AtomicU64 store races with mempool read, mempool sees a stale value
//!   (acceptable: next cycle corrects it)

#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::bounded_param::BoundedParam;
use crate::congestion_forecaster::{CongestionForecast, CongestionLevel};

/// Default bounds for the dynamic fee floor.
///
/// - `min = 1`: fee floor can go as low as 1 (effectively disabled)
/// - `max = 10_000`: fee floor cannot exceed 10,000 (governance override possible)
/// - `default = 1`: starts at 1 (same as no dynamic floor)
pub const DEFAULT_FEE_FLOOR_MIN: u64 = 1;
pub const DEFAULT_FEE_FLOOR_MAX: u64 = 10_000;
pub const DEFAULT_FEE_FLOOR_DEFAULT: u64 = 1;

/// Converts congestion forecasts into dynamic fee floor adjustments.
///
/// Adjustment rubric by congestion level:
/// - **Low**: decrease toward default by 10% (gradual cooldown)
/// - **Moderate**: increase by 25%
/// - **High**: increase by 50%
/// - **Critical**: increase by 100%
///
/// The fee floor is stored in a shared `Arc<AtomicU64>` that the mempool reads
/// during `insert()` to compute `effective_min_fee = max(base_min_fee, dynamic_floor)`.
pub struct CongestionResponder {
    fee_floor: BoundedParam,
    shared_floor: Arc<AtomicU64>,
}

impl CongestionResponder {
    /// Create a new responder with default bounds, publishing to the given atomic.
    pub fn new(shared_floor: Arc<AtomicU64>) -> Self {
        let fee_floor = BoundedParam::new(
            DEFAULT_FEE_FLOOR_MIN,
            DEFAULT_FEE_FLOOR_MAX,
            DEFAULT_FEE_FLOOR_DEFAULT,
        );
        shared_floor.store(fee_floor.current(), Ordering::Relaxed);
        Self {
            fee_floor,
            shared_floor,
        }
    }

    /// Create with custom bounds.
    pub fn with_bounds(min: u64, max: u64, default: u64, shared_floor: Arc<AtomicU64>) -> Self {
        let fee_floor = BoundedParam::new(min, max, default);
        shared_floor.store(fee_floor.current(), Ordering::Relaxed);
        Self {
            fee_floor,
            shared_floor,
        }
    }

    /// Process a congestion forecast and adjust the dynamic fee floor.
    ///
    /// Returns the new fee floor value after adjustment.
    pub fn respond(&mut self, forecast: &CongestionForecast) -> u64 {
        let pct = match forecast.level {
            CongestionLevel::Low => -10,
            CongestionLevel::Moderate => 25,
            CongestionLevel::High => 50,
            CongestionLevel::Critical => 100,
        };

        let old = self.fee_floor.current();
        self.fee_floor.adjust_pct(pct);
        let new = self.fee_floor.current();

        self.shared_floor.store(new, Ordering::Relaxed);

        tracing::debug!(
            level = ?forecast.level,
            old_fee_floor = old,
            new_fee_floor = new,
            adjustment_pct = pct,
            "Congestion response: adjusted dynamic fee floor"
        );

        new
    }

    /// Current fee floor value.
    pub fn current_fee_floor(&self) -> u64 {
        self.fee_floor.current()
    }

    /// Reset fee floor to default.
    pub fn reset(&mut self) {
        self.fee_floor.reset();
        self.shared_floor
            .store(self.fee_floor.current(), Ordering::Relaxed);
        tracing::debug!(
            fee_floor = self.fee_floor.current(),
            "Congestion response: fee floor reset to default"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::congestion_forecaster::ForecastEvidence;

    fn make_forecast(level: CongestionLevel) -> CongestionForecast {
        CongestionForecast {
            level,
            confidence: 200,
            height: 100,
            fee_recommendation: None,
            block_size_recommendation: None,
            evidence: ForecastEvidence {
                mempool_size: 100,
                avg_mempool_size: 50,
                mempool_growth_pct: 200,
                block_fullness_pct: 80,
                avg_block_fullness_pct: 60,
                avg_fee: 10,
                fee_p95: 50,
            },
        }
    }

    #[test]
    fn low_congestion_decreases_floor() {
        let shared = Arc::new(AtomicU64::new(0));
        let mut resp = CongestionResponder::with_bounds(1, 10_000, 100, Arc::clone(&shared));

        let new = resp.respond(&make_forecast(CongestionLevel::Low));
        assert_eq!(new, 90); // 100 * 90/100 = 90
        assert_eq!(shared.load(Ordering::Relaxed), 90);
    }

    #[test]
    fn moderate_congestion_increases_25pct() {
        let shared = Arc::new(AtomicU64::new(0));
        let mut resp = CongestionResponder::with_bounds(1, 10_000, 100, Arc::clone(&shared));

        let new = resp.respond(&make_forecast(CongestionLevel::Moderate));
        assert_eq!(new, 125); // 100 * 125/100 = 125
    }

    #[test]
    fn high_congestion_increases_50pct() {
        let shared = Arc::new(AtomicU64::new(0));
        let mut resp = CongestionResponder::with_bounds(1, 10_000, 100, Arc::clone(&shared));

        let new = resp.respond(&make_forecast(CongestionLevel::High));
        assert_eq!(new, 150); // 100 * 150/100 = 150
    }

    #[test]
    fn critical_congestion_doubles() {
        let shared = Arc::new(AtomicU64::new(0));
        let mut resp = CongestionResponder::with_bounds(1, 10_000, 100, Arc::clone(&shared));

        let new = resp.respond(&make_forecast(CongestionLevel::Critical));
        assert_eq!(new, 200); // 100 * 200/100 = 200
    }

    #[test]
    fn fee_floor_respects_max_bound() {
        let shared = Arc::new(AtomicU64::new(0));
        let mut resp = CongestionResponder::with_bounds(1, 150, 100, Arc::clone(&shared));

        // Critical doubles: 100 → 200, clamped to 150
        let new = resp.respond(&make_forecast(CongestionLevel::Critical));
        assert_eq!(new, 150);
    }

    #[test]
    fn fee_floor_respects_min_bound() {
        let shared = Arc::new(AtomicU64::new(0));
        let mut resp = CongestionResponder::with_bounds(10, 10_000, 10, Arc::clone(&shared));

        // Low decreases 10%: 10 * 90/100 = 9, clamped to 10
        let new = resp.respond(&make_forecast(CongestionLevel::Low));
        assert_eq!(new, 10);
    }

    #[test]
    fn reset_returns_to_default() {
        let shared = Arc::new(AtomicU64::new(0));
        let mut resp = CongestionResponder::with_bounds(1, 10_000, 100, Arc::clone(&shared));

        resp.respond(&make_forecast(CongestionLevel::Critical));
        assert_eq!(resp.current_fee_floor(), 200);

        resp.reset();
        assert_eq!(resp.current_fee_floor(), 100);
        assert_eq!(shared.load(Ordering::Relaxed), 100);
    }

    #[test]
    fn default_bounds_start_at_1() {
        let shared = Arc::new(AtomicU64::new(0));
        let resp = CongestionResponder::new(Arc::clone(&shared));
        assert_eq!(resp.current_fee_floor(), 1);
        assert_eq!(shared.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn sequential_levels_compound() {
        let shared = Arc::new(AtomicU64::new(0));
        let mut resp = CongestionResponder::with_bounds(1, 10_000, 100, Arc::clone(&shared));

        // Moderate: 100 → 125
        resp.respond(&make_forecast(CongestionLevel::Moderate));
        assert_eq!(resp.current_fee_floor(), 125);

        // High: 125 → 187 (125 * 150 / 100 = 187)
        resp.respond(&make_forecast(CongestionLevel::High));
        assert_eq!(resp.current_fee_floor(), 187);

        // Low: 187 → 168 (187 * 90 / 100 = 168)
        resp.respond(&make_forecast(CongestionLevel::Low));
        assert_eq!(resp.current_fee_floor(), 168);
    }

    #[test]
    fn deterministic_across_instances() {
        let levels = [
            CongestionLevel::Moderate,
            CongestionLevel::High,
            CongestionLevel::Critical,
            CongestionLevel::Low,
            CongestionLevel::Low,
        ];

        let shared1 = Arc::new(AtomicU64::new(0));
        let mut resp1 = CongestionResponder::with_bounds(1, 10_000, 100, shared1);

        let shared2 = Arc::new(AtomicU64::new(0));
        let mut resp2 = CongestionResponder::with_bounds(1, 10_000, 100, shared2);

        for level in &levels {
            resp1.respond(&make_forecast(*level));
            resp2.respond(&make_forecast(*level));
        }

        assert_eq!(resp1.current_fee_floor(), resp2.current_fee_floor());
    }
}
