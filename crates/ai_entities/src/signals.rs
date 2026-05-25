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
    /// Native x402-style per-request payment. The issuing entity (the
    /// payer) debits its `economic_balance` by `amount + fee` and credits
    /// the payee's `economic_balance` by `amount`; the fee is routed to
    /// `KEY_MARKETPLACE_TREASURY`. The handler also writes a
    /// `PaymentRecord` to `b"ai/payments/by_hash/" || signal_hash` plus
    /// two scan indexes (by-payer, by-payee). Replay protection is
    /// enforced by rejecting any payment whose `signal_hash` already has
    /// a `by_hash` record. Carries a 112-byte tail: payee_entity_id:32 |
    /// amount_be:8 | service_descriptor_hash:32 | request_hash:32 |
    /// max_block_height_be:8.
    PaymentRequest = 16,
    /// Service-delivery attestation issued by the payer of a prior
    /// `PaymentRequest`. The handler loads the referenced `PaymentRecord`
    /// from `b"ai/payments/by_hash/" || payment_signal_hash`, verifies
    /// the issuer of this signal equals the recorded payer, then applies
    /// either `REP_EVENT_PAYMENT_DELIVERED` or `REP_EVENT_PAYMENT_FAILED`
    /// to the payee depending on `status`. The payment record is rewritten
    /// in place with `attested_status` and `attested_height`; the record
    /// can be attested at most once. Carries a 65-byte tail:
    /// payment_signal_hash:32 | payee_entity_id:32 | status:1
    /// (0 = Delivered, 1 = Failed).
    ServiceAttestation = 17,
    /// SLA acceptance (Week 31): the issuing entity (the seller named in
    /// a previously proposed `SlaAgreement` memory object) accepts the
    /// agreement. The handler loads the SLA memory object via the
    /// embedded buyer entity id and SLA object id, verifies the signal
    /// issuer equals the SLA's seller, gates on the seller's current
    /// `stake_balance >= sla.slash_amount`, then transitions the SLA
    /// from `SLA_STATUS_PROPOSED` to `SLA_STATUS_ACTIVE` and records the
    /// acceptance height. The acceptance must land BEFORE the SLA's
    /// `start_height`; the active-pair singleton index entry stays in
    /// place (it was written at create time). Carries a 64-byte tail:
    /// sla_object_id:32 | buyer_entity_id:32.
    SlaAccept = 18,
    /// Payment channel accept (Week 32): the issuing entity (party B,
    /// the counterparty named in a previously proposed `PaymentChannel`
    /// memory object) accepts the channel. The handler loads the
    /// channel memory object via the embedded party A entity id and
    /// channel object id, verifies the signal issuer equals the
    /// channel's `party_b_entity_id`, debits B's `economic_balance` by
    /// the channel's `deposit_b`, transitions the channel from
    /// `PAYMENT_CHANNEL_STATUS_PROPOSED` to `_OPEN`, sets
    /// `accepted_at_height`, and bumps `balance_b` to `deposit_b`.
    /// Carries a 64-byte tail: channel_object_id:32 | party_a_entity_id:32.
    ChannelAccept = 19,
    /// Payment channel close (Week 32): handles both cooperative
    /// settle (instant) and unilateral close (opens a dispute window).
    /// Carries a 233-byte tail: channel_object_id:32 |
    /// party_a_entity_id:32 | nonce_be:8 | balance_a_be:16 |
    /// balance_b_be:16 | is_final:1 | sig_a:64 | sig_b:64. Both party
    /// signatures are always required (even on unilateral close); the
    /// `is_final` flag distinguishes cooperative settle (instant
    /// distribution + memory object delete) from unilateral close
    /// (status -> CLOSING, dispute window opens). Inside the dispute
    /// window a strictly larger nonce overrides the recorded state.
    /// The submitter signs the enclosing `TxV1` and pays the fee;
    /// either participant may submit.
    ChannelClose = 20,
    /// Payment channel finalize (Week 32): credits the recorded
    /// `balance_a` / `balance_b` back to the parties'
    /// `economic_balance` and deletes the channel memory object plus
    /// its secondary indexes. Valid only when `status ==
    /// PAYMENT_CHANNEL_STATUS_CLOSING` and `current_height >
    /// dispute_deadline_height`. Permissionless: any active AI entity
    /// may submit (the parties have aligned incentives to submit
    /// themselves; allowing third parties means a finalize is never
    /// gated on a specific participant's liveness). Carries a 64-byte
    /// tail: channel_object_id:32 | party_a_entity_id:32.
    ChannelFinalize = 21,
    /// Oracle data anchor (Week 35): an entity holding the
    /// `post_oracle_anchors` capability publishes a commitment to
    /// external off-chain data (a price feed, an API response, an
    /// external timestamp) so other agents and future protocol
    /// mechanisms can reference it from a registered, reputation-bearing
    /// source. The handler writes an `OracleAnchorRecord` KV aux row
    /// under `ai/oracle_anchors/by_hash/<signal_hash>` (which doubles as
    /// the replay guard) plus by-entity and by-tag scan indexes, and
    /// bumps the issuer's `total_transactions` (reputation-neutral on
    /// post; accuracy challenges are deferred to a future week). Carries
    /// a variable-length tail: data_hash:32 | external_timestamp_be:8 |
    /// source_hash:32 | expiry_height_be:8 | data_tag_len:1 |
    /// data_tag:[1..=32].
    OracleAnchor = 22,
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
            16 => Some(AiSignalType::PaymentRequest),
            17 => Some(AiSignalType::ServiceAttestation),
            18 => Some(AiSignalType::SlaAccept),
            19 => Some(AiSignalType::ChannelAccept),
            20 => Some(AiSignalType::ChannelClose),
            21 => Some(AiSignalType::ChannelFinalize),
            22 => Some(AiSignalType::OracleAnchor),
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
            Some(AiSignalType::PaymentRequest),
            "16 must decode to PaymentRequest"
        );
        assert_eq!(
            AiSignalType::from_byte(17),
            Some(AiSignalType::ServiceAttestation),
            "17 must decode to ServiceAttestation"
        );
        assert_eq!(
            AiSignalType::from_byte(18),
            Some(AiSignalType::SlaAccept),
            "18 must decode to SlaAccept (Week 31)"
        );
        assert_eq!(
            AiSignalType::from_byte(19),
            Some(AiSignalType::ChannelAccept),
            "19 must decode to ChannelAccept (Week 32)"
        );
        assert_eq!(
            AiSignalType::from_byte(20),
            Some(AiSignalType::ChannelClose),
            "20 must decode to ChannelClose (Week 32)"
        );
        assert_eq!(
            AiSignalType::from_byte(21),
            Some(AiSignalType::ChannelFinalize),
            "21 must decode to ChannelFinalize (Week 32)"
        );
        assert_eq!(
            AiSignalType::from_byte(22),
            Some(AiSignalType::OracleAnchor),
            "22 must decode to OracleAnchor (Week 35)"
        );
        assert_eq!(
            AiSignalType::from_byte(23),
            None,
            "23 must be rejected as unknown signal type"
        );
        assert_eq!(AiSignalType::CompositionCheck.to_byte(), 12);
        assert_eq!(AiSignalType::ProofSubmission.to_byte(), 13);
        assert_eq!(AiSignalType::SubscriptionCreate.to_byte(), 14);
        assert_eq!(AiSignalType::SubscriptionCancel.to_byte(), 15);
        assert_eq!(AiSignalType::PaymentRequest.to_byte(), 16);
        assert_eq!(AiSignalType::ServiceAttestation.to_byte(), 17);
        assert_eq!(AiSignalType::SlaAccept.to_byte(), 18);
        assert_eq!(AiSignalType::ChannelAccept.to_byte(), 19);
        assert_eq!(AiSignalType::ChannelClose.to_byte(), 20);
        assert_eq!(AiSignalType::ChannelFinalize.to_byte(), 21);
        assert_eq!(AiSignalType::OracleAnchor.to_byte(), 22);
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
