//! Anomaly report generation as AiSignalV1.
//!
//! PURPOSE: Convert detected anomalies into the standard AiSignalV1 format
//! for on-chain publishing.
//!
//! INVARIANTS:
//! - Reports are signed with validator's key
//! - Payload is stored off-chain, only hash committed on-chain
//! - Confidence is preserved from detection
//!
//! FAILURE MODES:
//! - Returns error if signing fails
//! - Returns error if payload storage fails

use crate::detector::{AnomalyKind, DetectedAnomaly};
use ed25519_dalek::SigningKey;
use novai_ai_entities::{AiSignalType, AiSignalV1, SignalPayload};
use novai_crypto::sign_bytes;
use novai_types::Address;

/// Reporter that converts anomalies to AiSignalV1.
pub struct AnomalyReporter {
    /// Validator's signing key.
    signing_key: SigningKey,

    /// Validator's address (issuer ID).
    issuer: Address,
}

impl AnomalyReporter {
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

    /// Create an AiSignalV1 from a detected anomaly.
    ///
    /// # Arguments
    /// - `anomaly`: The detected anomaly
    /// - `payload_hash`: Hash of the off-chain payload (from artifact store)
    ///
    /// # Returns
    /// A signed AiSignalV1 ready for on-chain submission.
    #[must_use]
    pub fn create_signal(&self, anomaly: &DetectedAnomaly, payload_hash: [u8; 32]) -> AiSignalV1 {
        let mut signal = AiSignalV1 {
            signal_type: AiSignalType::Anomaly,
            height: anomaly.height,
            issuer: self.issuer,
            confidence: anomaly.confidence,
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

    /// Create a SignalPayload from a detected anomaly.
    ///
    /// This is the off-chain payload that will be stored and referenced
    /// by hash in the on-chain signal.
    #[must_use]
    pub fn create_payload(&self, anomaly: &DetectedAnomaly) -> SignalPayload {
        let (model_id, input_summary, output_data, explanation) = match &anomaly.kind {
            AnomalyKind::MissedBlocks {
                validator,
                missed_count,
                average_missed,
            } => (
                "novai-copilot-missed-blocks".to_string(),
                format!("Validator {:?} block proposal tracking", &validator[..4]),
                encode_missed_blocks_data(validator, *missed_count, *average_missed),
                format!(
                    "Validator {:?} missed {} blocks, significantly above average of {}",
                    &validator[..4],
                    missed_count,
                    average_missed
                ),
            ),

            AnomalyKind::VoteDelay {
                delay_ms,
                p95_delay_ms,
            } => (
                "novai-copilot-vote-delay".to_string(),
                "Vote latency monitoring".to_string(),
                encode_vote_delay_data(*delay_ms, *p95_delay_ms),
                format!(
                    "Vote delay of {delay_ms}ms detected, exceeds p95 baseline of {p95_delay_ms}ms"
                ),
            ),

            AnomalyKind::PeerChurn {
                current_peers,
                baseline_peers,
            } => (
                "novai-copilot-peer-churn".to_string(),
                "Network peer connectivity monitoring".to_string(),
                encode_peer_churn_data(*current_peers, *baseline_peers),
                format!("Peer count changed to {current_peers} from baseline of {baseline_peers}"),
            ),

            AnomalyKind::MempoolCongestion {
                current_size,
                baseline_size,
            } => (
                "novai-copilot-mempool-congestion".to_string(),
                "Mempool size monitoring".to_string(),
                encode_mempool_data(*current_size, *baseline_size),
                format!(
                    "Mempool size {current_size} significantly exceeds baseline of {baseline_size}"
                ),
            ),
        };

        SignalPayload::new(
            model_id,
            "1.0.0".to_string(), // Copilot version
            input_summary,
            output_data,
            explanation,
        )
    }

    /// Create both payload and signal for an anomaly.
    ///
    /// # Returns
    /// Tuple of (SignalPayload, AiSignalV1) where the signal references
    /// the payload by its content hash.
    #[must_use]
    pub fn create_report(&self, anomaly: &DetectedAnomaly) -> (SignalPayload, AiSignalV1) {
        let payload = self.create_payload(anomaly);
        let payload_hash = payload.compute_hash();
        let signal = self.create_signal(anomaly, payload_hash);

        (payload, signal)
    }
}

/// Encode missed blocks anomaly data as binary.
///
/// Format: validator (32 bytes) + missed_count (8 bytes LE) + average (8 bytes LE)
fn encode_missed_blocks_data(validator: &Address, missed: u64, average: u64) -> Vec<u8> {
    let mut data = Vec::with_capacity(48);
    data.extend_from_slice(validator);
    data.extend_from_slice(&missed.to_le_bytes());
    data.extend_from_slice(&average.to_le_bytes());
    data
}

/// Encode vote delay anomaly data as binary.
///
/// Format: delay_ms (8 bytes LE) + p95_delay_ms (8 bytes LE)
fn encode_vote_delay_data(delay_ms: u64, p95_delay_ms: u64) -> Vec<u8> {
    let mut data = Vec::with_capacity(16);
    data.extend_from_slice(&delay_ms.to_le_bytes());
    data.extend_from_slice(&p95_delay_ms.to_le_bytes());
    data
}

/// Encode peer churn anomaly data as binary.
///
/// Format: current_peers (8 bytes LE) + baseline_peers (8 bytes LE)
fn encode_peer_churn_data(current: u64, baseline: u64) -> Vec<u8> {
    let mut data = Vec::with_capacity(16);
    data.extend_from_slice(&current.to_le_bytes());
    data.extend_from_slice(&baseline.to_le_bytes());
    data
}

/// Encode mempool congestion data as binary.
///
/// Format: current_size (8 bytes LE) + baseline_size (8 bytes LE)
fn encode_mempool_data(current: u64, baseline: u64) -> Vec<u8> {
    let mut data = Vec::with_capacity(16);
    data.extend_from_slice(&current.to_le_bytes());
    data.extend_from_slice(&baseline.to_le_bytes());
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_signing_key() -> SigningKey {
        SigningKey::from_bytes(&[0x42u8; 32])
    }

    fn test_anomaly() -> DetectedAnomaly {
        DetectedAnomaly {
            kind: AnomalyKind::MissedBlocks {
                validator: [0x01u8; 32],
                missed_count: 10,
                average_missed: 2,
            },
            confidence: 180,
            height: 1000,
        }
    }

    #[test]
    fn reporter_creates_valid_signal() {
        let reporter = AnomalyReporter::new(test_signing_key());
        let anomaly = test_anomaly();

        let payload = reporter.create_payload(&anomaly);
        let payload_hash = payload.compute_hash();
        let signal = reporter.create_signal(&anomaly, payload_hash);

        assert_eq!(signal.signal_type, AiSignalType::Anomaly);
        assert_eq!(signal.height, 1000);
        assert_eq!(signal.confidence, 180);
        assert_eq!(signal.payload_hash, payload_hash);
        assert_eq!(signal.issuer, *reporter.issuer());
    }

    #[test]
    fn signal_signature_is_valid() {
        let signing_key = test_signing_key();
        let reporter = AnomalyReporter::new(signing_key.clone());
        let anomaly = test_anomaly();

        let (_, signal) = reporter.create_report(&anomaly);

        // Verify signature
        let commitment = signal.compute_commitment_hash();
        let verifying_key = signing_key.verifying_key();

        assert!(
            novai_crypto::verify_bytes(&verifying_key, &commitment, &signal.signature),
            "Signal signature should be valid"
        );
    }

    #[test]
    fn payload_contains_anomaly_details() {
        let reporter = AnomalyReporter::new(test_signing_key());
        let anomaly = DetectedAnomaly {
            kind: AnomalyKind::VoteDelay {
                delay_ms: 500,
                p95_delay_ms: 100,
            },
            confidence: 200,
            height: 2000,
        };

        let payload = reporter.create_payload(&anomaly);

        assert_eq!(payload.model_id, "novai-copilot-vote-delay");
        assert!(payload.explanation.contains("500ms"));
        assert!(payload.explanation.contains("100ms"));
    }

    #[test]
    fn all_anomaly_types_create_valid_payloads() {
        let reporter = AnomalyReporter::new(test_signing_key());

        let anomalies = vec![
            DetectedAnomaly {
                kind: AnomalyKind::MissedBlocks {
                    validator: [0x01; 32],
                    missed_count: 10,
                    average_missed: 2,
                },
                confidence: 150,
                height: 100,
            },
            DetectedAnomaly {
                kind: AnomalyKind::VoteDelay {
                    delay_ms: 500,
                    p95_delay_ms: 100,
                },
                confidence: 200,
                height: 100,
            },
            DetectedAnomaly {
                kind: AnomalyKind::PeerChurn {
                    current_peers: 1,
                    baseline_peers: 4,
                },
                confidence: 180,
                height: 100,
            },
            DetectedAnomaly {
                kind: AnomalyKind::MempoolCongestion {
                    current_size: 500,
                    baseline_size: 50,
                },
                confidence: 220,
                height: 100,
            },
        ];

        for anomaly in anomalies {
            let (payload, signal) = reporter.create_report(&anomaly);

            // Payload should be non-empty
            assert!(!payload.model_id.is_empty());
            assert!(!payload.explanation.is_empty());

            // Signal should reference payload
            assert_eq!(signal.payload_hash, payload.compute_hash());

            // Signal should be properly typed
            assert_eq!(signal.signal_type, AiSignalType::Anomaly);
        }
    }

    #[test]
    fn output_data_encoding_is_deterministic() {
        let validator = [0x42u8; 32];

        let data1 = encode_missed_blocks_data(&validator, 100, 20);
        let data2 = encode_missed_blocks_data(&validator, 100, 20);

        assert_eq!(data1, data2);
        assert_eq!(data1.len(), 48); // 32 + 8 + 8
    }

    #[test]
    fn create_report_returns_matching_pair() {
        let reporter = AnomalyReporter::new(test_signing_key());
        let anomaly = test_anomaly();

        let (payload, signal) = reporter.create_report(&anomaly);

        // Verify the signal's payload_hash matches the actual payload hash
        assert_eq!(signal.payload_hash, payload.compute_hash());
    }
}
