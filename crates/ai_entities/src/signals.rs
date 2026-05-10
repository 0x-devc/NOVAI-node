//! AI advisory signal primitives.
//!
//! Signals are compact on-chain commitments to richer off-chain payloads.

use blake3::Hasher;

/// Domain separator for commitment hashing.
const SIGNAL_COMMIT_DOMAIN_V1: &[u8] = b"NOVAI_SIGNAL_COMMIT_V1";

/// Signal types emitted by AI entities in advisory mode.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AiSignalType {
    Anomaly = 0,
    Optimization = 1,
    Prediction = 2,
    RiskScore = 3,
    AuditReport = 4,
    SpamRisk = 5,
    CongestionForecast = 6,
    /// Reputation update emitted by an oracle entity (requires
    /// `submit_reputation_updates` capability).
    ReputationUpdate = 7,
    /// Purchase of a priced signal listed in the seller's `SignalCatalog`
    /// memory object. Carries an inline 41-byte purchase tail
    /// (seller_entity_id || purchased_signal_type || max_price_be).
    SignalPurchase = 8,
    /// Stake-deposit signal moving funds from the issuer's `economic_balance`
    /// to its `stake_balance`. Locks the deposited amount until
    /// `current_height + STAKE_LOCK_PERIOD`. Carries a 16-byte amount tail.
    StakeDeposit = 9,
    /// Stake-withdraw signal moving funds from the issuer's `stake_balance`
    /// back to its `economic_balance`. Rejected unless
    /// `stake_locked_until <= current_height`. Carries a 16-byte amount tail.
    StakeWithdraw = 10,
    /// Slash signal emitted by an oracle entity (requires
    /// `submit_reputation_updates` capability). Deducts from a target
    /// entity's `stake_balance`, credits the slashed amount to
    /// `KEY_SLASH_TREASURY`, and applies a reputation update atomically.
    /// Carries a 51-byte tail: target_id:32 | slash_amount_be:16 |
    /// rep_event_type:1 | points_delta_be:2.
    StakeSlash = 11,
    /// Composition health check emitted by an oracle entity (requires
    /// `submit_reputation_updates` capability). Validates a target entity's
    /// declared `CompositionGraph` dependency and, if the failure is
    /// corroborated by chain state and the dependency is marked required,
    /// auto-pauses the target by setting `is_active = false`. Always emits a
    /// `REP_EVENT_COMPOSITION_FAILURE` reputation event with delta -1.
    /// Carries a 34-byte tail: target_id:32 | failed_dependency_idx:1 |
    /// failure_reason:1.
    CompositionCheck = 12,
    /// AI entity submits a ZK proof attesting to off-chain computation
    /// integrity. Carries a 65-byte tail: proof_type:1 | code_hash:32 |
    /// computation_hash:32. The proof bytes themselves are NOT carried
    /// inline (the SignalCommitment tail is fixed-size); a real verifier
    /// would resolve them via the off-chain payload referenced by
    /// `signal_hash`. The execution handler verifies the proof via the
    /// `ZkVerifier` trait, and on success creates a `VerificationRecord`
    /// memory object owned by the issuer plus a `REP_EVENT_PROOF_VERIFIED`
    /// reputation event with delta +3.
    ProofSubmission = 13,
    /// Recurring payment subscription create: the issuing entity (the
    /// subscriber) locks `rate_per_block * duration_blocks` of its
    /// `economic_balance` to a producer for a fixed covered signal type.
    /// On success the handler creates a `MemoryObjectType::Subscription`
    /// memory object owned by the subscriber, recording the active
    /// agreement. Settlement is lazy and is performed when the
    /// subscriber emits a matching `SubscriptionCancel` signal. Carries
    /// a 49-byte tail: producer_entity_id:32 | covered_signal_type:1 |
    /// rate_per_block_be:8 | duration_blocks_be:8.
    SubscriptionCreate = 14,
    /// Recurring payment subscription cancel: the original subscriber
    /// terminates an active subscription early. The handler settles
    /// accrued payment to the producer (less the standard 2% marketplace
    /// fee), applies a 5% cancel fee on the remaining locked funds (paid
    /// to the producer with no marketplace cut as compensation for early
    /// termination), refunds the rest to the subscriber, and marks the
    /// subscription record `is_active = false`. Only the original
    /// subscriber may emit this signal. Carries a 32-byte tail:
    /// subscription_id:32 (the memory object id of the `Subscription`
    /// record being cancelled).
    SubscriptionCancel = 15,
}

impl AiSignalType {
    /// Encode to canonical byte representation.
    pub fn to_byte(self) -> u8 {
        self as u8
    }

    /// Decode from byte, returning None for invalid values.
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            0 => Some(AiSignalType::Anomaly),
            1 => Some(AiSignalType::Optimization),
            2 => Some(AiSignalType::Prediction),
            3 => Some(AiSignalType::RiskScore),
            4 => Some(AiSignalType::AuditReport),
            5 => Some(AiSignalType::SpamRisk),
            6 => Some(AiSignalType::CongestionForecast),
            7 => Some(AiSignalType::ReputationUpdate),
            8 => Some(AiSignalType::SignalPurchase),
            9 => Some(AiSignalType::StakeDeposit),
            10 => Some(AiSignalType::StakeWithdraw),
            11 => Some(AiSignalType::StakeSlash),
            12 => Some(AiSignalType::CompositionCheck),
            13 => Some(AiSignalType::ProofSubmission),
            14 => Some(AiSignalType::SubscriptionCreate),
            15 => Some(AiSignalType::SubscriptionCancel),
            _ => None,
        }
    }
}

/// Canonical v1 signal structure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiSignalV1 {
    /// What kind of advisory output this is.
    pub signal_type: AiSignalType,
    /// Block height when the signal was generated.
    pub height: u64,
    /// AI entity ID (32-byte) that issued it.
    pub issuer: [u8; 32],
    /// 0-255 confidence level.
    pub confidence: u8,
    /// Hash of full payload stored off-chain.
    pub payload_hash: [u8; 32],
    /// Optional verifiable proof (e.g., ZK proof).
    pub zk_proof: Option<Vec<u8>>,
    /// Issuer's signature (protocol-defined scheme).
    pub signature: [u8; 64],
}

/// Compact commitment view for indexing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalCommitment {
    pub commitment_hash: [u8; 32],
    pub signal_type: AiSignalType,
    pub height: u64,
    pub issuer: [u8; 32],
}

impl AiSignalV1 {
    /// Compute domain-separated commitment hash for this signal.
    ///
    /// Commitment binds to all fields except the signature.
    pub fn compute_commitment_hash(&self) -> [u8; 32] {
        let mut hasher = Hasher::new();
        hasher.update(SIGNAL_COMMIT_DOMAIN_V1);

        hasher.update(&[self.signal_type.to_byte()]);
        hasher.update(&self.height.to_le_bytes());
        hasher.update(&self.issuer);
        hasher.update(&[self.confidence]);
        hasher.update(&self.payload_hash);

        // Bind optional proof deterministically
        match &self.zk_proof {
            Some(p) => {
                let len_u32: u32 = p.len().try_into().unwrap_or(u32::MAX);
                hasher.update(&len_u32.to_le_bytes());
                hasher.update(p);
            }
            None => {
                hasher.update(&0u32.to_le_bytes());
            }
        }

        *hasher.finalize().as_bytes()
    }

    /// Convert this signal into its compact commitment.
    pub fn to_commitment(&self) -> SignalCommitment {
        SignalCommitment {
            commitment_hash: self.compute_commitment_hash(),
            signal_type: self.signal_type,
            height: self.height,
            issuer: self.issuer,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ai_signal_type_from_byte_boundary() {
        assert_eq!(
            AiSignalType::from_byte(11),
            Some(AiSignalType::StakeSlash),
            "11 must decode to StakeSlash"
        );
        assert_eq!(
            AiSignalType::from_byte(12),
            Some(AiSignalType::CompositionCheck),
            "12 must decode to CompositionCheck"
        );
        assert_eq!(
            AiSignalType::from_byte(13),
            Some(AiSignalType::ProofSubmission),
            "13 must decode to ProofSubmission"
        );
        assert_eq!(
            AiSignalType::from_byte(14),
            Some(AiSignalType::SubscriptionCreate),
            "14 must decode to SubscriptionCreate"
        );
        assert_eq!(
            AiSignalType::from_byte(15),
            Some(AiSignalType::SubscriptionCancel),
            "15 must decode to SubscriptionCancel"
        );
        assert_eq!(
            AiSignalType::from_byte(16),
            None,
            "16 must be rejected as unknown signal type"
        );
        assert_eq!(AiSignalType::CompositionCheck.to_byte(), 12);
        assert_eq!(AiSignalType::ProofSubmission.to_byte(), 13);
        assert_eq!(AiSignalType::SubscriptionCreate.to_byte(), 14);
        assert_eq!(AiSignalType::SubscriptionCancel.to_byte(), 15);
    }

    #[test]
    fn commitment_hash_is_deterministic() {
        let s = AiSignalV1 {
            signal_type: AiSignalType::Prediction,
            height: 123,
            issuer: [0x11u8; 32],
            confidence: 200,
            payload_hash: [0x22u8; 32],
            zk_proof: None,
            signature: [0x33u8; 64],
        };

        let h1 = s.compute_commitment_hash();
        let h2 = s.compute_commitment_hash();
        assert_eq!(h1, h2);
    }
}
