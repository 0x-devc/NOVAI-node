//! Congestion forecast report generation as AiSignalV1.
//!
//! PURPOSE: Convert congestion forecasts and recommendations into the standard
//! AiSignalV1 format for on-chain publishing.
//!
//! INVARIANTS:
//! - Reports are signed with validator's key
//! - Payload is stored off-chain, only hash committed on-chain
//! - Recommendations are advisory only - NO automatic parameter changes
//!
//! FAILURE MODES:
//! - Returns error if signing fails
//!
//! NON-ACTIONS (this module does NOT):
//! - Modify chain parameters (MIN_FEE, BLOCK_SIZE_LIMIT)
//! - Execute any governance actions
//! - Enforce any recommendations

use crate::congestion_forecaster::CongestionForecast;
#[cfg(test)]
use crate::congestion_forecaster::CongestionLevel;
use ed25519_dalek::SigningKey;
use novai_ai_entities::{AiSignalType, AiSignalV1, SignalPayload};
use novai_crypto::sign_bytes;
use novai_types::Address;

/// Reporter that converts congestion forecasts to AiSignalV1.
///
/// This reporter ONLY produces data structures. It does NOT:
/// - Modify chain parameters
/// - Execute governance actions
/// - Enforce any recommendations
pub struct CongestionReporter {
    /// Validator's signing key.
    signing_key: SigningKey,

    /// Validator's address (issuer ID).
    issuer: Address,
}

impl CongestionReporter {
    /// Create a new reporter with the given signing key.
    #[must_use]
    pub fn new(signing_key: SigningKey) -> Self {
        let verifying_key = signing_key.verifying_key();
        let issuer = novai_crypto::address_from_pubkey(&verifying_key);

        Self {
            signing_key,
            issuer,
        }
    }

    /// Get the issuer address.
    #[must_use]
    pub fn issuer(&self) -> &Address {
        &self.issuer
    }

    /// Create an AiSignalV1 from a congestion forecast.
    ///
    /// Uses `AiSignalType::CongestionForecast` (value 6).
    ///
    /// This is purely data generation - NO parameter changes are made.
    #[must_use]
    pub fn create_signal(
        &self,
        forecast: &CongestionForecast,
        payload_hash: [u8; 32],
    ) -> AiSignalV1 {
        let mut signal = AiSignalV1 {
            signal_type: AiSignalType::CongestionForecast,
            height: forecast.height,
            issuer: self.issuer,
            confidence: forecast.confidence,
            payload_hash,
            zk_proof: None,
            signature: [0u8; 64],
        };

        // Sign the commitment hash
        let commitment = signal.compute_commitment_hash();
        let signature = sign_bytes(&self.signing_key, &commitment);
        signal.signature = signature;

        signal
    }

    /// Create a SignalPayload from a congestion forecast.
    ///
    /// The payload includes:
    /// - Congestion level and evidence
    /// - Fee adjustment recommendation (if any)
    /// - Block size recommendation (if any)
    /// - Rationale for each recommendation
    ///
    /// All recommendations are ADVISORY ONLY.
    #[must_use]
    pub fn create_payload(&self, forecast: &CongestionForecast) -> SignalPayload {
        let model_id = "novai-congestion-forecaster".to_string();
        let model_version = "1.0.0".to_string();

        let input_summary = format!(
            "Mempool: {} txs ({}% growth), Block fullness: {}%",
            forecast.evidence.mempool_size,
            forecast.evidence.mempool_growth_pct,
            forecast.evidence.block_fullness_pct
        );

        let output_data = encode_forecast_data(forecast);

        let explanation = build_explanation(forecast);

        SignalPayload::new(
            model_id,
            model_version,
            input_summary,
            output_data,
            explanation,
        )
    }

    /// Create both payload and signal for a forecast.
    ///
    /// # Returns
    /// Tuple of (SignalPayload, AiSignalV1) where the signal references
    /// the payload by its content hash.
    ///
    /// This is purely data generation - NO parameter changes are made.
    #[must_use]
    pub fn create_report(
        &self,
        forecast: &CongestionForecast,
    ) -> (SignalPayload, AiSignalV1) {
        let payload = self.create_payload(forecast);
        let payload_hash = payload.compute_hash();
        let signal = self.create_signal(forecast, payload_hash);

        (payload, signal)
    }
}

/// Build human-readable explanation for the forecast.
fn build_explanation(forecast: &CongestionForecast) -> String {
    let mut parts = Vec::new();

    // Level description
    parts.push(format!(
        "Congestion Level: {} (confidence: {}%)",
        forecast.level.description(),
        (forecast.confidence as u32 * 100) / 255
    ));

    // Evidence summary
    parts.push(format!(
        "Evidence: Mempool at {}% of baseline ({} txs), blocks {}% full (avg {}%)",
        forecast.evidence.mempool_growth_pct,
        forecast.evidence.mempool_size,
        forecast.evidence.block_fullness_pct,
        forecast.evidence.avg_block_fullness_pct
    ));

    // Fee recommendation
    if let Some(ref fee_rec) = forecast.fee_recommendation {
        let direction = if fee_rec.adjustment_pct >= 0 { "+" } else { "" };
        parts.push(format!(
            "FEE RECOMMENDATION (ADVISORY): {}{}% - {}",
            direction, fee_rec.adjustment_pct, fee_rec.rationale
        ));
    }

    // Block size recommendation
    if let Some(ref size_rec) = forecast.block_size_recommendation {
        let direction = if size_rec.adjustment_pct >= 0 { "+" } else { "" };
        parts.push(format!(
            "BLOCK SIZE RECOMMENDATION (ADVISORY): {}{}% - {}",
            direction, size_rec.adjustment_pct, size_rec.rationale
        ));
    }

    // Advisory notice
    parts.push(
        "NOTE: All recommendations are advisory only. No automatic changes are made. \
         Governance review required before any parameter modifications."
            .to_string(),
    );

    parts.join("\n\n")
}

/// Encode forecast data as binary.
///
/// Format:
/// - congestion_level: u8 (0-3)
/// - confidence: u8 (0-255)
/// - height: u64 LE
/// - mempool_size: u64 LE
/// - avg_mempool_size: u64 LE
/// - mempool_growth_pct: u64 LE
/// - block_fullness_pct: u64 LE
/// - avg_block_fullness_pct: u64 LE
/// - avg_fee: u64 LE
/// - fee_p95: u64 LE
/// - has_fee_rec: u8 (0 or 1)
/// - fee_adjustment_pct: i32 LE (if has_fee_rec)
/// - has_size_rec: u8 (0 or 1)
/// - size_adjustment_pct: i32 LE (if has_size_rec)
fn encode_forecast_data(forecast: &CongestionForecast) -> Vec<u8> {
    let mut data = Vec::with_capacity(128);

    // Header
    data.push(forecast.level.to_byte());
    data.push(forecast.confidence);
    data.extend_from_slice(&forecast.height.to_le_bytes());

    // Evidence
    data.extend_from_slice(&forecast.evidence.mempool_size.to_le_bytes());
    data.extend_from_slice(&forecast.evidence.avg_mempool_size.to_le_bytes());
    data.extend_from_slice(&forecast.evidence.mempool_growth_pct.to_le_bytes());
    data.extend_from_slice(&forecast.evidence.block_fullness_pct.to_le_bytes());
    data.extend_from_slice(&forecast.evidence.avg_block_fullness_pct.to_le_bytes());
    data.extend_from_slice(&forecast.evidence.avg_fee.to_le_bytes());
    data.extend_from_slice(&forecast.evidence.fee_p95.to_le_bytes());

    // Fee recommendation
    if let Some(ref fee_rec) = forecast.fee_recommendation {
        data.push(1);
        data.extend_from_slice(&fee_rec.adjustment_pct.to_le_bytes());
    } else {
        data.push(0);
    }

    // Block size recommendation
    if let Some(ref size_rec) = forecast.block_size_recommendation {
        data.push(1);
        data.extend_from_slice(&size_rec.adjustment_pct.to_le_bytes());
    } else {
        data.push(0);
    }

    data
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::congestion_forecaster::{
        BlockSizeRecommendation, FeeRecommendation, ForecastEvidence,
    };

    fn test_signing_key() -> SigningKey {
        SigningKey::from_bytes(&[0x42u8; 32])
    }

    fn test_forecast(level: CongestionLevel) -> CongestionForecast {
        CongestionForecast {
            level,
            confidence: 180,
            height: 1000,
            fee_recommendation: Some(FeeRecommendation {
                adjustment_pct: 10,
                rationale: "Test rationale".to_string(),
            }),
            block_size_recommendation: None,
            evidence: ForecastEvidence {
                mempool_size: 100,
                avg_mempool_size: 50,
                mempool_growth_pct: 200,
                block_fullness_pct: 80,
                avg_block_fullness_pct: 60,
                avg_fee: 10,
                fee_p95: 25,
            },
        }
    }

    #[test]
    fn reporter_creates_valid_signal() {
        let reporter = CongestionReporter::new(test_signing_key());
        let forecast = test_forecast(CongestionLevel::High);

        let payload = reporter.create_payload(&forecast);
        let payload_hash = payload.compute_hash();
        let signal = reporter.create_signal(&forecast, payload_hash);

        assert_eq!(signal.signal_type, AiSignalType::CongestionForecast);
        assert_eq!(signal.height, 1000);
        assert_eq!(signal.confidence, 180);
        assert_eq!(signal.payload_hash, payload_hash);
        assert_eq!(signal.issuer, *reporter.issuer());
    }

    #[test]
    fn signal_uses_congestion_forecast_type() {
        let reporter = CongestionReporter::new(test_signing_key());
        let forecast = test_forecast(CongestionLevel::Moderate);

        let (_, signal) = reporter.create_report(&forecast);

        assert_eq!(signal.signal_type, AiSignalType::CongestionForecast);
        assert_eq!(signal.signal_type.to_byte(), 6);
    }

    #[test]
    fn signal_signature_is_valid() {
        let signing_key = test_signing_key();
        let reporter = CongestionReporter::new(signing_key.clone());
        let forecast = test_forecast(CongestionLevel::High);

        let (_, signal) = reporter.create_report(&forecast);

        let commitment = signal.compute_commitment_hash();
        let verifying_key = signing_key.verifying_key();

        assert!(
            novai_crypto::verify_bytes(&verifying_key, &commitment, &signal.signature),
            "Signal signature should be valid"
        );
    }

    #[test]
    fn payload_contains_advisory_notice() {
        let reporter = CongestionReporter::new(test_signing_key());
        let forecast = test_forecast(CongestionLevel::Critical);

        let payload = reporter.create_payload(&forecast);

        assert!(payload.explanation.contains("advisory"));
        assert!(payload.explanation.contains("Governance review"));
    }

    #[test]
    fn payload_contains_recommendations() {
        let reporter = CongestionReporter::new(test_signing_key());
        let mut forecast = test_forecast(CongestionLevel::Critical);
        forecast.block_size_recommendation = Some(BlockSizeRecommendation {
            adjustment_pct: 20,
            rationale: "Block size rationale".to_string(),
        });

        let payload = reporter.create_payload(&forecast);

        assert!(payload.explanation.contains("FEE RECOMMENDATION"));
        assert!(payload.explanation.contains("+10%"));
        assert!(payload.explanation.contains("BLOCK SIZE RECOMMENDATION"));
        assert!(payload.explanation.contains("+20%"));
    }

    #[test]
    fn output_data_encoding_is_deterministic() {
        let forecast1 = test_forecast(CongestionLevel::High);
        let forecast2 = test_forecast(CongestionLevel::High);

        let data1 = encode_forecast_data(&forecast1);
        let data2 = encode_forecast_data(&forecast2);

        assert_eq!(data1, data2);
    }

    #[test]
    fn output_data_encodes_level_correctly() {
        for level in [
            CongestionLevel::Low,
            CongestionLevel::Moderate,
            CongestionLevel::High,
            CongestionLevel::Critical,
        ] {
            let forecast = test_forecast(level);
            let data = encode_forecast_data(&forecast);

            assert_eq!(data[0], level.to_byte());
        }
    }

    #[test]
    fn create_report_returns_matching_pair() {
        let reporter = CongestionReporter::new(test_signing_key());
        let forecast = test_forecast(CongestionLevel::High);

        let (payload, signal) = reporter.create_report(&forecast);

        assert_eq!(signal.payload_hash, payload.compute_hash());
    }

    #[test]
    fn model_id_is_consistent() {
        let reporter = CongestionReporter::new(test_signing_key());
        let forecast = test_forecast(CongestionLevel::Low);

        let payload = reporter.create_payload(&forecast);

        assert_eq!(payload.model_id, "novai-congestion-forecaster");
    }

    #[test]
    fn no_recommendations_handled_gracefully() {
        let reporter = CongestionReporter::new(test_signing_key());
        let mut forecast = test_forecast(CongestionLevel::Low);
        forecast.fee_recommendation = None;
        forecast.block_size_recommendation = None;

        let (payload, signal) = reporter.create_report(&forecast);

        // Should still work without recommendations
        assert!(!payload.explanation.is_empty());
        assert_eq!(signal.signal_type, AiSignalType::CongestionForecast);
    }

    #[test]
    fn reporter_is_advisory_only() {
        // This test documents that the reporter only produces data.
        // It has NO capability to:
        // - Modify chain parameters
        // - Execute governance actions
        // - Enforce recommendations

        let reporter = CongestionReporter::new(test_signing_key());
        let forecast = test_forecast(CongestionLevel::Critical);

        let (payload, signal) = reporter.create_report(&forecast);

        // Reporter only produces data structures
        assert!(payload.explanation.contains("advisory"));
        assert!(payload.explanation.contains("No automatic changes"));

        // Signal is just a commitment - no enforcement capability
        assert_eq!(signal.signal_type, AiSignalType::CongestionForecast);

        // INVARIANT: Reporter is advisory only by design.
    }

    #[test]
    fn integration_full_pipeline() {
        // Integration test: Full pipeline from stats -> forecast -> report
        use crate::congestion_stats::CongestionStats;
        use crate::congestion_forecaster::CongestionForecaster;

        // Step 1: Create CongestionStats with window size 10
        let mut stats = CongestionStats::new(10);

        // Step 2: Record blocks to simulate HIGH congestion
        // Build baseline first (mempool ~50, block fullness ~50%)
        for i in 0..5 {
            stats.record_block(i, 50, 50, 100, 5000, 10);
        }

        // Now simulate congestion: mempool 110 (220% of baseline 50), block fullness 90%
        stats.record_block(6, 110, 90, 100, 10000, 20);

        // Step 3: Create CongestionForecaster and forecast
        let forecaster = CongestionForecaster::new();
        let forecast = forecaster.forecast(&stats).expect("Should have sufficient data");

        // Step 4: Create CongestionReporter and create_report
        let signing_key = test_signing_key();
        let reporter = CongestionReporter::new(signing_key.clone());
        let (payload, signal) = reporter.create_report(&forecast);

        // Step 5: Assertions

        // Signal type is CongestionForecast
        assert_eq!(
            signal.signal_type,
            AiSignalType::CongestionForecast,
            "Signal type must be CongestionForecast"
        );

        // Payload hash matches
        assert_eq!(
            signal.payload_hash,
            payload.compute_hash(),
            "Signal payload_hash must match payload.compute_hash()"
        );

        // Forecast level is High (mempool 220% > 200% threshold OR fullness 90% > 85%)
        assert_eq!(
            forecast.level,
            CongestionLevel::High,
            "Forecast level should be High for 220% mempool growth and 90% block fullness"
        );

        // Fee recommendation is +25% for High level
        let fee_rec = forecast
            .fee_recommendation
            .as_ref()
            .expect("High congestion should have fee recommendation");
        assert_eq!(
            fee_rec.adjustment_pct, 25,
            "High congestion should recommend +25% fee adjustment"
        );

        // Signature is valid
        let commitment = signal.compute_commitment_hash();
        let verifying_key = signing_key.verifying_key();
        assert!(
            novai_crypto::verify_bytes(&verifying_key, &commitment, &signal.signature),
            "Signal signature must be valid"
        );
    }
}
