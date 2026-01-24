//! Spam pattern report generation as AiSignalV1.
//!
//! PURPOSE: Convert detected spam patterns into the standard AiSignalV1 format
//! for on-chain publishing. Uses existing AiSignalType::SpamRisk (value 5).
//!
//! INVARIANTS:
//! - Reports are signed with validator's key
//! - Payload is stored off-chain, only hash committed on-chain
//! - Confidence is preserved from detection
//! - This module ONLY produces data - NO enforcement actions
//!
//! FAILURE MODES:
//! - Returns error if signing fails
//! - Returns error if payload encoding fails
//!
//! NON-ACTIONS (this module does NOT):
//! - Modify mempool state
//! - Ban or disconnect peers
//! - Reject transactions
//! - Take any enforcement action

use crate::spam_detector::{DetectedSpamPattern, SpamPatternKind};
use ed25519_dalek::SigningKey;
use novai_ai_entities::{AiSignalType, AiSignalV1, SignalPayload};
use novai_crypto::sign_bytes;
use novai_types::Address;

/// Reporter that converts spam patterns to AiSignalV1.
///
/// This reporter ONLY produces data structures. It does NOT:
/// - Modify mempool behavior
/// - Ban peers
/// - Reject transactions
/// - Take any enforcement action
pub struct SpamReporter {
    /// Validator's signing key.
    signing_key: SigningKey,

    /// Validator's address (issuer ID).
    issuer: Address,
}

impl SpamReporter {
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

    /// Create an AiSignalV1 from a detected spam pattern.
    ///
    /// Uses `AiSignalType::SpamRisk` (value 5) as defined in ai_entities.
    ///
    /// # Arguments
    /// - `pattern`: The detected spam pattern
    /// - `payload_hash`: Hash of the off-chain payload (from artifact store)
    ///
    /// # Returns
    /// A signed AiSignalV1 ready for on-chain submission.
    /// This is purely data - no enforcement action is taken.
    #[must_use]
    pub fn create_signal(
        &self,
        pattern: &DetectedSpamPattern,
        payload_hash: [u8; 32],
    ) -> AiSignalV1 {
        let mut signal = AiSignalV1 {
            signal_type: AiSignalType::SpamRisk, // Uses existing type (value 5)
            height: pattern.height,
            issuer: self.issuer,
            confidence: pattern.confidence,
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

    /// Create a SignalPayload from a detected spam pattern.
    ///
    /// The payload includes:
    /// - Affected sender address (if applicable)
    /// - Pattern type identifier
    /// - Evidence counts (rejection rates, tx counts, etc.)
    ///
    /// This is the off-chain payload that will be stored and referenced
    /// by hash in the on-chain signal.
    #[must_use]
    pub fn create_payload(&self, pattern: &DetectedSpamPattern) -> SignalPayload {
        let (model_id, input_summary, output_data, explanation) = match &pattern.kind {
            SpamPatternKind::HighInvalidRate {
                sender,
                invalid_count,
                total_count,
                rejection_pct,
            } => (
                "novai-spam-high-invalid-rate".to_string(),
                format!("Sender {:?} transaction rejection monitoring", &sender[..4]),
                encode_high_invalid_rate_data(sender, *invalid_count, *total_count, *rejection_pct),
                format!(
                    "Sender {:?} has {}% rejection rate ({}/{} transactions rejected). \
                     This is an advisory signal only - no action taken.",
                    &sender[..4],
                    rejection_pct,
                    invalid_count,
                    total_count
                ),
            ),

            SpamPatternKind::HighTxRate {
                sender,
                tx_count,
                threshold,
            } => (
                "novai-spam-high-tx-rate".to_string(),
                format!("Sender {:?} submission rate monitoring", &sender[..4]),
                encode_high_tx_rate_data(sender, *tx_count, *threshold),
                format!(
                    "Sender {:?} submitted {} transactions in observation window \
                     (threshold: {}). This is an advisory signal only - no action taken.",
                    &sender[..4],
                    tx_count,
                    threshold
                ),
            ),

            SpamPatternKind::MempoolSpike {
                current_size,
                baseline_size,
            } => (
                "novai-spam-mempool-spike".to_string(),
                "Mempool size anomaly monitoring".to_string(),
                encode_mempool_spike_data(*current_size, *baseline_size),
                format!(
                    "Mempool size {} significantly exceeds baseline of {} \
                     ({}x increase). This is an advisory signal only - no action taken.",
                    current_size,
                    baseline_size,
                    if *baseline_size > 0 {
                        current_size / baseline_size
                    } else {
                        0
                    }
                ),
            ),

            SpamPatternKind::LowFeeFlood {
                sender,
                low_fee_count,
                threshold_fee,
            } => (
                "novai-spam-low-fee-flood".to_string(),
                format!("Sender {:?} low-fee transaction monitoring", &sender[..4]),
                encode_low_fee_flood_data(sender, *low_fee_count, *threshold_fee),
                format!(
                    "Sender {:?} submitted {} transactions with fees below \
                     threshold {}. This is an advisory signal only - no action taken.",
                    &sender[..4],
                    low_fee_count,
                    threshold_fee
                ),
            ),
        };

        SignalPayload::new(
            model_id,
            "1.0.0".to_string(), // Spam detector version
            input_summary,
            output_data,
            explanation,
        )
    }

    /// Create both payload and signal for a spam pattern.
    ///
    /// # Returns
    /// Tuple of (SignalPayload, AiSignalV1) where the signal references
    /// the payload by its content hash.
    ///
    /// This is purely data generation - NO enforcement action is taken.
    #[must_use]
    pub fn create_report(&self, pattern: &DetectedSpamPattern) -> (SignalPayload, AiSignalV1) {
        let payload = self.create_payload(pattern);
        let payload_hash = payload.compute_hash();
        let signal = self.create_signal(pattern, payload_hash);

        (payload, signal)
    }
}

// =============================================================================
// Binary encoding functions for payload output_data
// All encodings are deterministic and use little-endian byte order.
// =============================================================================

/// Encode high invalid rate pattern data as binary.
///
/// Format:
/// - pattern_type: u8 (0 = HighInvalidRate)
/// - sender: [u8; 32]
/// - invalid_count: u64 LE
/// - total_count: u64 LE
/// - rejection_pct: u64 LE
///
/// Total: 57 bytes
fn encode_high_invalid_rate_data(
    sender: &Address,
    invalid_count: u64,
    total_count: u64,
    rejection_pct: u64,
) -> Vec<u8> {
    let mut data = Vec::with_capacity(57);
    data.push(0u8); // pattern_type = HighInvalidRate
    data.extend_from_slice(sender);
    data.extend_from_slice(&invalid_count.to_le_bytes());
    data.extend_from_slice(&total_count.to_le_bytes());
    data.extend_from_slice(&rejection_pct.to_le_bytes());
    data
}

/// Encode high tx rate pattern data as binary.
///
/// Format:
/// - pattern_type: u8 (1 = HighTxRate)
/// - sender: [u8; 32]
/// - tx_count: u64 LE
/// - threshold: u64 LE
///
/// Total: 49 bytes
fn encode_high_tx_rate_data(sender: &Address, tx_count: u64, threshold: u64) -> Vec<u8> {
    let mut data = Vec::with_capacity(49);
    data.push(1u8); // pattern_type = HighTxRate
    data.extend_from_slice(sender);
    data.extend_from_slice(&tx_count.to_le_bytes());
    data.extend_from_slice(&threshold.to_le_bytes());
    data
}

/// Encode mempool spike pattern data as binary.
///
/// Format:
/// - pattern_type: u8 (2 = MempoolSpike)
/// - current_size: u64 LE
/// - baseline_size: u64 LE
///
/// Total: 17 bytes
fn encode_mempool_spike_data(current_size: u64, baseline_size: u64) -> Vec<u8> {
    let mut data = Vec::with_capacity(17);
    data.push(2u8); // pattern_type = MempoolSpike
    data.extend_from_slice(&current_size.to_le_bytes());
    data.extend_from_slice(&baseline_size.to_le_bytes());
    data
}

/// Encode low fee flood pattern data as binary.
///
/// Format:
/// - pattern_type: u8 (3 = LowFeeFlood)
/// - sender: [u8; 32]
/// - low_fee_count: u64 LE
/// - threshold_fee: u64 LE
///
/// Total: 49 bytes
fn encode_low_fee_flood_data(sender: &Address, low_fee_count: u64, threshold_fee: u64) -> Vec<u8> {
    let mut data = Vec::with_capacity(49);
    data.push(3u8); // pattern_type = LowFeeFlood
    data.extend_from_slice(sender);
    data.extend_from_slice(&low_fee_count.to_le_bytes());
    data.extend_from_slice(&threshold_fee.to_le_bytes());
    data
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spam_detector::SpamPatternKind;

    fn test_signing_key() -> SigningKey {
        SigningKey::from_bytes(&[0x42u8; 32])
    }

    fn test_pattern_high_invalid() -> DetectedSpamPattern {
        DetectedSpamPattern {
            kind: SpamPatternKind::HighInvalidRate {
                sender: [0x01u8; 32],
                invalid_count: 80,
                total_count: 100,
                rejection_pct: 80,
            },
            confidence: 180,
            height: 1000,
        }
    }

    fn test_pattern_high_tx_rate() -> DetectedSpamPattern {
        DetectedSpamPattern {
            kind: SpamPatternKind::HighTxRate {
                sender: [0x02u8; 32],
                tx_count: 100,
                threshold: 50,
            },
            confidence: 200,
            height: 2000,
        }
    }

    fn test_pattern_mempool_spike() -> DetectedSpamPattern {
        DetectedSpamPattern {
            kind: SpamPatternKind::MempoolSpike {
                current_size: 500,
                baseline_size: 100,
            },
            confidence: 150,
            height: 3000,
        }
    }

    fn test_pattern_low_fee_flood() -> DetectedSpamPattern {
        DetectedSpamPattern {
            kind: SpamPatternKind::LowFeeFlood {
                sender: [0x03u8; 32],
                low_fee_count: 50,
                threshold_fee: 10,
            },
            confidence: 170,
            height: 4000,
        }
    }

    #[test]
    fn reporter_creates_valid_signal() {
        let reporter = SpamReporter::new(test_signing_key());
        let pattern = test_pattern_high_invalid();

        let payload = reporter.create_payload(&pattern);
        let payload_hash = payload.compute_hash();
        let signal = reporter.create_signal(&pattern, payload_hash);

        assert_eq!(signal.signal_type, AiSignalType::SpamRisk);
        assert_eq!(signal.height, 1000);
        assert_eq!(signal.confidence, 180);
        assert_eq!(signal.payload_hash, payload_hash);
        assert_eq!(signal.issuer, *reporter.issuer());
    }

    #[test]
    fn signal_uses_spam_risk_type() {
        let reporter = SpamReporter::new(test_signing_key());
        let pattern = test_pattern_high_invalid();

        let (_, signal) = reporter.create_report(&pattern);

        // Verify it uses SpamRisk (value 5), not Anomaly
        assert_eq!(signal.signal_type, AiSignalType::SpamRisk);
        assert_eq!(signal.signal_type.to_byte(), 5);
    }

    #[test]
    fn signal_signature_is_valid() {
        let signing_key = test_signing_key();
        let reporter = SpamReporter::new(signing_key.clone());
        let pattern = test_pattern_high_invalid();

        let (_, signal) = reporter.create_report(&pattern);

        // Verify signature
        let commitment = signal.compute_commitment_hash();
        let verifying_key = signing_key.verifying_key();

        assert!(
            novai_crypto::verify_bytes(&verifying_key, &commitment, &signal.signature),
            "Signal signature should be valid"
        );
    }

    #[test]
    fn payload_contains_pattern_details() {
        let reporter = SpamReporter::new(test_signing_key());
        let pattern = test_pattern_high_invalid();

        let payload = reporter.create_payload(&pattern);

        assert_eq!(payload.model_id, "novai-spam-high-invalid-rate");
        assert!(payload.explanation.contains("80%"));
        assert!(payload.explanation.contains("advisory"));
        assert!(payload.explanation.contains("no action taken"));
    }

    #[test]
    fn all_pattern_types_create_valid_payloads() {
        let reporter = SpamReporter::new(test_signing_key());

        let patterns = vec![
            test_pattern_high_invalid(),
            test_pattern_high_tx_rate(),
            test_pattern_mempool_spike(),
            test_pattern_low_fee_flood(),
        ];

        for pattern in patterns {
            let (payload, signal) = reporter.create_report(&pattern);

            // Payload should be non-empty
            assert!(!payload.model_id.is_empty());
            assert!(!payload.explanation.is_empty());

            // All explanations should note advisory nature
            assert!(
                payload.explanation.contains("advisory"),
                "Payload should mention advisory: {}",
                payload.explanation
            );

            // Signal should reference payload
            assert_eq!(signal.payload_hash, payload.compute_hash());

            // Signal should use SpamRisk type
            assert_eq!(signal.signal_type, AiSignalType::SpamRisk);
        }
    }

    #[test]
    fn output_data_encoding_is_deterministic() {
        let sender = [0x42u8; 32];

        let data1 = encode_high_invalid_rate_data(&sender, 80, 100, 80);
        let data2 = encode_high_invalid_rate_data(&sender, 80, 100, 80);

        assert_eq!(data1, data2);
        assert_eq!(data1.len(), 57); // 1 + 32 + 8 + 8 + 8
    }

    #[test]
    fn output_data_has_correct_lengths() {
        let sender = [0x42u8; 32];

        let high_invalid = encode_high_invalid_rate_data(&sender, 80, 100, 80);
        assert_eq!(high_invalid.len(), 57);
        assert_eq!(high_invalid[0], 0); // pattern_type

        let high_rate = encode_high_tx_rate_data(&sender, 100, 50);
        assert_eq!(high_rate.len(), 49);
        assert_eq!(high_rate[0], 1); // pattern_type

        let spike = encode_mempool_spike_data(500, 100);
        assert_eq!(spike.len(), 17);
        assert_eq!(spike[0], 2); // pattern_type

        let low_fee = encode_low_fee_flood_data(&sender, 50, 10);
        assert_eq!(low_fee.len(), 49);
        assert_eq!(low_fee[0], 3); // pattern_type
    }

    #[test]
    fn create_report_returns_matching_pair() {
        let reporter = SpamReporter::new(test_signing_key());
        let pattern = test_pattern_high_tx_rate();

        let (payload, signal) = reporter.create_report(&pattern);

        // Verify the signal's payload_hash matches the actual payload hash
        assert_eq!(signal.payload_hash, payload.compute_hash());
    }

    #[test]
    fn mempool_spike_payload_includes_ratio() {
        let reporter = SpamReporter::new(test_signing_key());
        let pattern = test_pattern_mempool_spike();

        let payload = reporter.create_payload(&pattern);

        // Should show 5x increase (500/100)
        assert!(payload.explanation.contains("5x"));
    }

    #[test]
    fn payload_model_ids_are_distinct() {
        let reporter = SpamReporter::new(test_signing_key());

        let patterns = [
            test_pattern_high_invalid(),
            test_pattern_high_tx_rate(),
            test_pattern_mempool_spike(),
            test_pattern_low_fee_flood(),
        ];

        let model_ids: Vec<String> = patterns
            .iter()
            .map(|p| reporter.create_payload(p).model_id)
            .collect();

        // All model IDs should be unique
        let mut unique = model_ids.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            model_ids.len(),
            "All pattern types should have distinct model IDs"
        );
    }
}
