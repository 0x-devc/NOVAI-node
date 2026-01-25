//! Congestion forecaster - predicts congestion and generates recommendations.
//!
//! PURPOSE: Analyze congestion trends and suggest parameter adjustments.
//! This module is PURELY ADVISORY - it recommends but NEVER auto-applies changes.
//!
//! INVARIANTS:
//! - All computations use integer math for determinism
//! - Recommendations are suggestions only, never enforced
//! - No side effects on chain parameters
//!
//! FAILURE MODES:
//! - Insufficient data returns no forecast
//! - Edge cases return conservative recommendations

use crate::congestion_stats::CongestionStats;

/// Congestion severity level.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CongestionLevel {
    /// Normal operation, no action needed.
    Low = 0,
    /// Moderate congestion, may need attention soon.
    Moderate = 1,
    /// High congestion, action recommended.
    High = 2,
    /// Critical congestion, urgent action recommended.
    Critical = 3,
}

impl CongestionLevel {
    /// Convert to byte representation.
    #[must_use]
    pub fn to_byte(self) -> u8 {
        self as u8
    }

    /// Convert from byte representation.
    #[must_use]
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            0 => Some(CongestionLevel::Low),
            1 => Some(CongestionLevel::Moderate),
            2 => Some(CongestionLevel::High),
            3 => Some(CongestionLevel::Critical),
            _ => None,
        }
    }

    /// Human-readable description.
    #[must_use]
    pub fn description(&self) -> &'static str {
        match self {
            CongestionLevel::Low => "Low - normal operation",
            CongestionLevel::Moderate => "Moderate - elevated load",
            CongestionLevel::High => "High - significant congestion",
            CongestionLevel::Critical => "Critical - urgent action recommended",
        }
    }
}

/// Recommendation for MIN_FEE adjustment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeeRecommendation {
    /// Suggested adjustment as signed percentage.
    /// e.g., +10 means increase by 10%, -5 means decrease by 5%.
    pub adjustment_pct: i32,

    /// Rationale for the recommendation.
    pub rationale: String,
}

/// Recommendation for BLOCK_SIZE_LIMIT adjustment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockSizeRecommendation {
    /// Suggested adjustment as signed percentage.
    pub adjustment_pct: i32,

    /// Rationale for the recommendation.
    pub rationale: String,
}

/// Complete congestion forecast with recommendations.
#[derive(Debug, Clone)]
pub struct CongestionForecast {
    /// Predicted congestion level.
    pub level: CongestionLevel,

    /// Confidence in the prediction (0-255).
    pub confidence: u8,

    /// Block height when forecast was generated.
    pub height: u64,

    /// Fee adjustment recommendation (if any).
    pub fee_recommendation: Option<FeeRecommendation>,

    /// Block size adjustment recommendation (if any).
    pub block_size_recommendation: Option<BlockSizeRecommendation>,

    /// Supporting data summary.
    pub evidence: ForecastEvidence,
}

/// Evidence supporting the forecast.
#[derive(Debug, Clone)]
pub struct ForecastEvidence {
    /// Current mempool size.
    pub mempool_size: u64,
    /// Average mempool size.
    pub avg_mempool_size: u64,
    /// Mempool growth percentage.
    pub mempool_growth_pct: u64,
    /// Current block fullness percentage.
    pub block_fullness_pct: u64,
    /// Average block fullness percentage.
    pub avg_block_fullness_pct: u64,
    /// Average fee.
    pub avg_fee: u64,
    /// P95 fee.
    pub fee_p95: u64,
}

/// Thresholds for congestion detection.
#[derive(Debug, Clone)]
pub struct CongestionThresholds {
    /// Mempool growth % above which moderate congestion is flagged.
    pub moderate_mempool_growth_pct: u64,
    /// Mempool growth % above which high congestion is flagged.
    pub high_mempool_growth_pct: u64,
    /// Mempool growth % above which critical congestion is flagged.
    pub critical_mempool_growth_pct: u64,

    /// Block fullness % above which moderate congestion is flagged.
    pub moderate_block_fullness_pct: u64,
    /// Block fullness % above which high congestion is flagged.
    pub high_block_fullness_pct: u64,
    /// Block fullness % above which critical congestion is flagged.
    pub critical_block_fullness_pct: u64,
}

impl Default for CongestionThresholds {
    fn default() -> Self {
        Self {
            moderate_mempool_growth_pct: 150, // 1.5x baseline
            high_mempool_growth_pct: 200,     // 2x baseline
            critical_mempool_growth_pct: 300, // 3x baseline

            moderate_block_fullness_pct: 70,
            high_block_fullness_pct: 85,
            critical_block_fullness_pct: 95,
        }
    }
}

/// Congestion forecaster that analyzes trends and generates recommendations.
///
/// This forecaster is PURELY ADVISORY:
/// - It analyzes statistics and generates recommendations
/// - It NEVER modifies chain parameters
/// - It NEVER enforces any changes
/// - All recommendations are suggestions for governance review
#[derive(Debug, Clone)]
pub struct CongestionForecaster {
    thresholds: CongestionThresholds,
}

impl CongestionForecaster {
    /// Create a new forecaster with default thresholds.
    #[must_use]
    pub fn new() -> Self {
        Self {
            thresholds: CongestionThresholds::default(),
        }
    }

    /// Create a forecaster with custom thresholds.
    #[must_use]
    pub fn with_thresholds(thresholds: CongestionThresholds) -> Self {
        Self { thresholds }
    }

    /// Generate a congestion forecast from current statistics.
    ///
    /// Returns None if insufficient data is available.
    ///
    /// This method is PURELY ADVISORY - it generates recommendations
    /// but NEVER applies any changes to chain parameters.
    #[must_use]
    pub fn forecast(&self, stats: &CongestionStats) -> Option<CongestionForecast> {
        if !stats.has_sufficient_data() {
            return None;
        }

        let evidence = ForecastEvidence {
            mempool_size: stats.current_mempool_size(),
            avg_mempool_size: stats.avg_mempool_size(),
            mempool_growth_pct: stats.mempool_growth_pct(),
            block_fullness_pct: stats.current_block_fullness(),
            avg_block_fullness_pct: stats.avg_block_fullness(),
            avg_fee: stats.avg_fee(),
            fee_p95: stats.fee_p95(),
        };

        let level = self.determine_level(&evidence);
        let confidence = self.compute_confidence(&evidence, level);
        let fee_recommendation = self.recommend_fee_adjustment(&evidence, level);
        let block_size_recommendation = self.recommend_block_size_adjustment(&evidence, level);

        Some(CongestionForecast {
            level,
            confidence,
            height: stats.current_height(),
            fee_recommendation,
            block_size_recommendation,
            evidence,
        })
    }

    /// Determine congestion level from evidence.
    fn determine_level(&self, evidence: &ForecastEvidence) -> CongestionLevel {
        let mempool_growth = evidence.mempool_growth_pct;
        let fullness = evidence.block_fullness_pct;

        // Check critical first
        if mempool_growth >= self.thresholds.critical_mempool_growth_pct
            || fullness >= self.thresholds.critical_block_fullness_pct
        {
            return CongestionLevel::Critical;
        }

        // Check high
        if mempool_growth >= self.thresholds.high_mempool_growth_pct
            || fullness >= self.thresholds.high_block_fullness_pct
        {
            return CongestionLevel::High;
        }

        // Check moderate
        if mempool_growth >= self.thresholds.moderate_mempool_growth_pct
            || fullness >= self.thresholds.moderate_block_fullness_pct
        {
            return CongestionLevel::Moderate;
        }

        CongestionLevel::Low
    }

    /// Compute confidence based on data quality and signal strength.
    fn compute_confidence(&self, evidence: &ForecastEvidence, level: CongestionLevel) -> u8 {
        let mut confidence: u64 = 100; // Base confidence

        // Increase confidence if multiple indicators agree
        let mempool_signals_high =
            evidence.mempool_growth_pct >= self.thresholds.high_mempool_growth_pct;
        let fullness_signals_high =
            evidence.block_fullness_pct >= self.thresholds.high_block_fullness_pct;

        if mempool_signals_high && fullness_signals_high {
            confidence += 50; // Strong agreement
        } else if mempool_signals_high || fullness_signals_high {
            confidence += 20; // Single strong signal
        }

        // Increase confidence for extreme values
        if evidence.mempool_growth_pct >= self.thresholds.critical_mempool_growth_pct {
            confidence += 30;
        }
        if evidence.block_fullness_pct >= self.thresholds.critical_block_fullness_pct {
            confidence += 30;
        }

        // Lower confidence for low congestion (less certain when things are normal)
        if level == CongestionLevel::Low {
            confidence = confidence.saturating_sub(20);
        }

        confidence.min(255) as u8
    }

    /// Generate fee adjustment recommendation.
    fn recommend_fee_adjustment(
        &self,
        evidence: &ForecastEvidence,
        level: CongestionLevel,
    ) -> Option<FeeRecommendation> {
        match level {
            CongestionLevel::Low => {
                // Consider lowering fees if consistently underutilized
                if evidence.avg_block_fullness_pct < 30 {
                    Some(FeeRecommendation {
                        adjustment_pct: -5,
                        rationale: format!(
                            "Block utilization low ({}% avg). Consider modest fee reduction to encourage usage.",
                            evidence.avg_block_fullness_pct
                        ),
                    })
                } else {
                    None // No change needed
                }
            }
            CongestionLevel::Moderate => Some(FeeRecommendation {
                adjustment_pct: 10,
                rationale: format!(
                    "Moderate congestion detected. Mempool at {}% of baseline, blocks {}% full.",
                    evidence.mempool_growth_pct, evidence.block_fullness_pct
                ),
            }),
            CongestionLevel::High => Some(FeeRecommendation {
                adjustment_pct: 25,
                rationale: format!(
                    "High congestion. Mempool growth {}%, block fullness {}%. Recommend fee increase.",
                    evidence.mempool_growth_pct, evidence.block_fullness_pct
                ),
            }),
            CongestionLevel::Critical => Some(FeeRecommendation {
                adjustment_pct: 50,
                rationale: format!(
                    "CRITICAL congestion. Mempool {}x baseline, blocks {}% full. Urgent fee increase recommended.",
                    evidence.mempool_growth_pct / 100, evidence.block_fullness_pct
                ),
            }),
        }
    }

    /// Generate block size adjustment recommendation.
    fn recommend_block_size_adjustment(
        &self,
        evidence: &ForecastEvidence,
        level: CongestionLevel,
    ) -> Option<BlockSizeRecommendation> {
        match level {
            CongestionLevel::Low | CongestionLevel::Moderate => None,
            CongestionLevel::High => {
                // Only recommend if blocks are consistently full
                if evidence.avg_block_fullness_pct >= 80 {
                    Some(BlockSizeRecommendation {
                        adjustment_pct: 10,
                        rationale: format!(
                            "Blocks consistently full ({}% avg). Consider modest size increase.",
                            evidence.avg_block_fullness_pct
                        ),
                    })
                } else {
                    None
                }
            }
            CongestionLevel::Critical => Some(BlockSizeRecommendation {
                adjustment_pct: 20,
                rationale: format!(
                    "Critical congestion with {}% block fullness. Block size increase recommended.",
                    evidence.block_fullness_pct
                ),
            }),
        }
    }
}

impl Default for CongestionForecaster {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_stats_with_data(
        mempool_baseline: u64,
        mempool_current: u64,
        fullness: u64,
    ) -> CongestionStats {
        let mut stats = CongestionStats::new(10);

        // Build baseline
        for i in 0..5 {
            stats.record_block(i, mempool_baseline, fullness, 100, 1000, 10);
        }

        // Set current
        stats.record_block(6, mempool_current, fullness, 100, 1000, 10);

        stats
    }

    #[test]
    fn forecast_returns_none_without_data() {
        let forecaster = CongestionForecaster::new();
        let stats = CongestionStats::new(10);

        assert!(forecaster.forecast(&stats).is_none());
    }

    #[test]
    fn forecast_low_congestion() {
        let forecaster = CongestionForecaster::new();
        let stats = make_stats_with_data(50, 50, 30); // No growth, 30% full

        let forecast = forecaster.forecast(&stats).expect("should have data");

        assert_eq!(forecast.level, CongestionLevel::Low);
    }

    #[test]
    fn forecast_moderate_congestion() {
        let forecaster = CongestionForecaster::new();
        let stats = make_stats_with_data(50, 80, 75); // 160% growth, 75% full

        let forecast = forecaster.forecast(&stats).expect("should have data");

        assert_eq!(forecast.level, CongestionLevel::Moderate);
        assert!(forecast.fee_recommendation.is_some());
        assert_eq!(forecast.fee_recommendation.unwrap().adjustment_pct, 10);
    }

    #[test]
    fn forecast_high_congestion() {
        let forecaster = CongestionForecaster::new();
        let stats = make_stats_with_data(50, 110, 90); // 220% growth, 90% full

        let forecast = forecaster.forecast(&stats).expect("should have data");

        assert_eq!(forecast.level, CongestionLevel::High);
        assert!(forecast.fee_recommendation.is_some());
        assert_eq!(forecast.fee_recommendation.unwrap().adjustment_pct, 25);
    }

    #[test]
    fn forecast_critical_congestion() {
        let forecaster = CongestionForecaster::new();
        let stats = make_stats_with_data(50, 160, 98); // 320% growth, 98% full

        let forecast = forecaster.forecast(&stats).expect("should have data");

        assert_eq!(forecast.level, CongestionLevel::Critical);
        assert!(forecast.fee_recommendation.is_some());
        assert_eq!(forecast.fee_recommendation.unwrap().adjustment_pct, 50);
        assert!(forecast.block_size_recommendation.is_some());
    }

    #[test]
    fn congestion_level_roundtrip() {
        for level in [
            CongestionLevel::Low,
            CongestionLevel::Moderate,
            CongestionLevel::High,
            CongestionLevel::Critical,
        ] {
            let byte = level.to_byte();
            let decoded = CongestionLevel::from_byte(byte).unwrap();
            assert_eq!(level, decoded);
        }
    }

    #[test]
    fn invalid_congestion_level_returns_none() {
        assert!(CongestionLevel::from_byte(255).is_none());
        assert!(CongestionLevel::from_byte(4).is_none());
    }

    #[test]
    fn confidence_increases_with_severity() {
        let forecaster = CongestionForecaster::new();

        let low_stats = make_stats_with_data(50, 50, 30);
        let high_stats = make_stats_with_data(50, 160, 98);

        let low_forecast = forecaster.forecast(&low_stats).unwrap();
        let high_forecast = forecaster.forecast(&high_stats).unwrap();

        assert!(
            high_forecast.confidence > low_forecast.confidence,
            "Critical should have higher confidence"
        );
    }

    #[test]
    fn evidence_is_populated() {
        let forecaster = CongestionForecaster::new();
        let stats = make_stats_with_data(50, 100, 75);

        let forecast = forecaster.forecast(&stats).unwrap();

        assert_eq!(forecast.evidence.mempool_size, 100);
        assert!(forecast.evidence.mempool_growth_pct > 100);
        assert_eq!(forecast.evidence.block_fullness_pct, 75);
    }

    #[test]
    fn low_utilization_recommends_fee_decrease() {
        let forecaster = CongestionForecaster::new();
        let stats = make_stats_with_data(50, 50, 20); // Very low fullness

        let forecast = forecaster.forecast(&stats).unwrap();

        assert_eq!(forecast.level, CongestionLevel::Low);
        if let Some(rec) = forecast.fee_recommendation {
            assert!(rec.adjustment_pct < 0, "Should recommend decrease");
        }
    }

    #[test]
    fn forecast_is_advisory_only() {
        // This test documents the advisory nature.
        // The forecaster:
        // - Returns CongestionForecast (pure data)
        // - Contains recommendations (suggestions only)
        // - Has NO side effects
        // - Cannot modify any chain parameters

        let forecaster = CongestionForecaster::new();
        let stats = make_stats_with_data(50, 160, 98);

        let forecast = forecaster.forecast(&stats).unwrap();

        // Forecast is pure data - no enforcement capability
        assert!(forecast.fee_recommendation.is_some());
        assert!(forecast.block_size_recommendation.is_some());

        // The recommendation contains rationale for human review
        let fee_rec = forecast.fee_recommendation.unwrap();
        assert!(!fee_rec.rationale.is_empty());

        // INVARIANT: Forecast is advisory only by design.
        // No automatic application - this is documented behavior.
    }
}
