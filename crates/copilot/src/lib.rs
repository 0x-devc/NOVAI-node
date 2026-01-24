//! Validator Co-Pilot: Statistics-based anomaly detection for NOVAI.
//!
//! PURPOSE: First real AI feature - a sidecar observer that monitors chain
//! health and publishes anomaly signals when statistical thresholds are exceeded.
//!
//! # Architecture
//!
//! - `stats` - Rolling window statistics collection (ChainStats, RingBuffer)
//! - `detector` - Threshold-based anomaly detection (integer math for determinism)
//! - `reporter` - Convert anomalies to AiSignalV1 format
//! - `observer` - Main observation loop that ties everything together
//!
//! # Design Principles
//!
//! - **No ML**: Pure statistics-based detection using simple thresholds
//! - **Deterministic**: All threshold comparisons use integer math
//! - **Non-blocking**: Observer runs in background, doesn't affect consensus
//! - **Signal-only**: Publishes advisory signals, never affects consensus directly
//!
//! # Thresholds (D16.2)
//!
//! - Missed blocks: `missed > 3 * average` → flag anomaly
//! - Vote delay: `delay > 5 * p95_delay` → flag anomaly
//! - Peer churn: `churn > 2 * baseline` → flag anomaly
//! - Mempool growth: `growth > 3 * normal` → flag congestion

#![forbid(unsafe_code)]

pub mod congestion_forecaster;
pub mod congestion_reporter;
pub mod congestion_stats;
pub mod detector;
pub mod observer;
pub mod reporter;
pub mod resource_budget;
pub mod spam_detector;
pub mod spam_observer;
pub mod spam_reporter;
pub mod spam_stats;
pub mod stats;

pub use detector::{AnomalyDetector, AnomalyKind, DetectedAnomaly};
pub use observer::{ChainObserver, ObservableState};
pub use reporter::AnomalyReporter;
pub use spam_detector::{DetectedSpamPattern, SpamDetector, SpamPatternKind, SpamThresholds};
pub use spam_observer::{SpamCallback, SpamObserver, SpamObserverConfig, SpamObserverMetrics};
pub use spam_reporter::SpamReporter;
pub use spam_stats::{SenderStats, SpamStats, TxRejectionReason};
pub use congestion_forecaster::{
    BlockSizeRecommendation, CongestionForecast, CongestionForecaster, CongestionLevel,
    CongestionThresholds, FeeRecommendation, ForecastEvidence,
};
pub use congestion_reporter::CongestionReporter;
pub use congestion_stats::CongestionStats;
pub use resource_budget::{ResourceBudget, ResourceTracker, ResourceUsage};
pub use stats::{ChainStats, RingBuffer};

#[cfg(test)]
mod non_censorship_tests;
